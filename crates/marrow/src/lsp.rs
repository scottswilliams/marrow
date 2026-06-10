//! The Marrow editor language server: JSON-RPC 2.0 over stdio with
//! `Content-Length` framing, full text sync, publishing
//! `textDocument/publishDiagnostics` on every `didOpen`/`didChange`. Distinct from
//! `marrow serve`, which is a data/IPC server with different framing and purpose.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use marrow_syntax::{Severity, SourceSpan, parse_source};
use serde_json::{Value, json};

pub(crate) fn run(args: &[String]) -> ExitCode {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!(
            "\
Usage:
  marrow lsp

Run the Marrow language server over stdio (JSON-RPC, Content-Length framed). It
reports diagnostics for open `.mw` documents; point an LSP-capable editor at it.
"
        );
        return ExitCode::SUCCESS;
    }
    if let Some(option) = args.iter().find(|arg| arg.starts_with('-')) {
        eprintln!("unknown lsp option: {option}");
        return ExitCode::from(2);
    }
    if let Some(argument) = args.first() {
        eprintln!("unknown lsp argument: {argument}");
        return ExitCode::from(2);
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    match serve(&mut reader, &mut writer) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) | Err(_) => ExitCode::FAILURE,
    }
}

/// The message loop. `Ok(true)` is a clean stop (`exit` after `shutdown`);
/// `Ok(false)` is `exit` or EOF without `shutdown`; `Err` on I/O failure.
fn serve(reader: &mut impl BufRead, writer: &mut impl Write) -> io::Result<bool> {
    let mut documents: HashMap<String, String> = HashMap::new();
    let mut project: Option<ProjectContext> = None;
    let mut shutdown = false;
    while let Some(message) = read_message(reader)? {
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();
        match method {
            "initialize" => {
                project = project_context(&message);
                let result = json!({
                    "capabilities": { "textDocumentSync": 1 },
                    "serverInfo": {
                        "name": "marrow-lsp",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                });
                write_message(writer, &response(id, result))?;
            }
            "textDocument/didOpen" | "textDocument/didChange" => {
                if let Some((uri, text)) = document_params(method, &message) {
                    documents.insert(uri.clone(), text.clone());
                    write_message(
                        writer,
                        &diagnostics_notification(&uri, &text, project.as_ref(), &documents),
                    )?;
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = message["params"]["textDocument"]["uri"].as_str() {
                    documents.remove(uri);
                    // Clear any diagnostics the editor is showing for the file.
                    write_message(writer, &publish_notification(uri, Vec::new()))?;
                }
            }
            "shutdown" => {
                shutdown = true;
                write_message(writer, &response(id, Value::Null))?;
            }
            "exit" => return Ok(shutdown),
            // `initialized` and other notifications are ignored; an unknown
            // request (one with an `id`) gets a method-not-found reply.
            _ => {
                if let Some(id) = id {
                    write_message(writer, &error_response(id, -32601, "method not found"))?;
                }
            }
        }
    }
    Ok(shutdown)
}

struct ProjectContext {
    root: PathBuf,
    config: marrow_project::ProjectConfig,
}

/// The `(uri, full text)` of a `didOpen`/`didChange`. Under full text sync a
/// change carries the whole new document as its last content change.
fn document_params(method: &str, message: &Value) -> Option<(String, String)> {
    let document = &message["params"]["textDocument"];
    let uri = document["uri"].as_str()?.to_string();
    let text = if method == "textDocument/didOpen" {
        document["text"].as_str()?.to_string()
    } else {
        message["params"]["contentChanges"].as_array()?.last()?["text"]
            .as_str()?
            .to_string()
    };
    Some((uri, text))
}

fn project_context(message: &Value) -> Option<ProjectContext> {
    let root = uri_to_path(message["params"]["rootUri"].as_str()?)?;
    let config_text = std::fs::read_to_string(root.join("marrow.json")).ok()?;
    let config = marrow_project::parse_config(&config_text).ok()?;
    Some(ProjectContext { root, config })
}

/// Build a `publishDiagnostics` notification, using project checking when a valid
/// project root was supplied and falling back to parse-only diagnostics otherwise.
fn diagnostics_notification(
    uri: &str,
    text: &str,
    project: Option<&ProjectContext>,
    documents: &HashMap<String, String>,
) -> Value {
    if let Some(project) = project
        && let Some(notification) = checked_diagnostics_notification(uri, text, project, documents)
    {
        return notification;
    }
    parse_diagnostics_notification(uri, text)
}

fn checked_diagnostics_notification(
    uri: &str,
    text: &str,
    project: &ProjectContext,
    documents: &HashMap<String, String>,
) -> Option<Value> {
    let path = uri_to_path(uri)?;
    let mut sources = marrow_check::ProjectSources::new();
    for (uri, text) in documents {
        if let Some(path) = uri_to_path(uri) {
            sources.insert(path, text);
        }
    }
    // Editor analysis is best-effort and read-only: it binds whatever accepted catalog
    // the engine-resident store already holds, and a missing or unreadable store simply
    // binds none rather than blocking diagnostics.
    let accepted = project
        .root
        .to_str()
        .and_then(|root| {
            crate::read_accepted_store_catalog(root, &project.config, crate::CheckFormat::Text).ok()
        })
        .flatten();
    let snapshot =
        marrow_check::analyze_project(&project.root, &project.config, &sources, accepted.as_ref())
            .ok()?;
    if !snapshot.files.iter().any(|file| file.path == path) {
        return None;
    }
    let diagnostics = snapshot
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.file == path)
        .map(|diagnostic| {
            lsp_diagnostic(
                diagnostic.code,
                diagnostic.severity,
                &diagnostic.message,
                None,
                diagnostic.span,
                text,
            )
        })
        .collect();
    Some(publish_notification(uri, diagnostics))
}

fn parse_diagnostics_notification(uri: &str, text: &str) -> Value {
    let parsed = parse_source(text);
    let diagnostics = parsed
        .diagnostics
        .iter()
        .map(|diagnostic| {
            lsp_diagnostic(
                diagnostic.code,
                diagnostic.severity,
                &diagnostic.message,
                diagnostic.help.as_deref(),
                diagnostic.span,
                text,
            )
        })
        .collect();
    publish_notification(uri, diagnostics)
}

fn publish_notification(uri: &str, diagnostics: Vec<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": diagnostics },
    })
}

/// Map a Marrow diagnostic to the LSP shape. Any `help` is appended to the
/// message, matching how `marrow check` renders it.
fn lsp_diagnostic(
    code: &str,
    severity: Severity,
    message: &str,
    help: Option<&str>,
    span: SourceSpan,
    text: &str,
) -> Value {
    let severity = match severity {
        Severity::Error => 1,
        Severity::Warning => 2,
    };
    let mut message = message.to_string();
    if let Some(help) = help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }
    json!({
        "range": {
            "start": position(span.start_byte, text),
            "end": position(span.end_byte, text),
        },
        "severity": severity,
        "code": code,
        "source": "marrow",
        "message": message,
    })
}

/// Convert a byte offset into a 0-based LSP `{line, character}` in the default
/// UTF-16 code-unit coordinate space.
fn position(byte: usize, text: &str) -> Value {
    let byte = byte.min(text.len());
    let mut line = 0u32;
    let mut character = 0u32;
    let mut offset = 0usize;
    for ch in text.chars() {
        if offset >= byte {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
        offset += ch.len_utf8();
    }
    json!({ "line": line, "character": character })
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    uri.strip_prefix("file://")
        .map(percent_decode_path)
        .map(PathBuf::from)
}

fn percent_decode_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn response(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Read one LSP message: parse the `Content-Length` header, read that many body
/// bytes, parse them as JSON. `Ok(None)` on clean EOF.
fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    let mut header_bytes = 0usize;
    loop {
        let Some(line) = read_header_line(reader, &mut header_bytes)? else {
            return Ok(None);
        };
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = strip_header(line, "content-length:") {
            content_length = value.trim().parse().ok();
        }
    }
    let Some(length) = content_length else {
        return Ok(None);
    };
    if length > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message exceeds the size limit",
        ));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some(value))
}

fn read_header_line(
    reader: &mut impl BufRead,
    header_bytes: &mut usize,
) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() && *header_bytes == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unterminated LSP header",
            ));
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len() + take > MAX_HEADER_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP header line exceeds the size limit",
            ));
        }
        if *header_bytes + take > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP header block exceeds the size limit",
            ));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        *header_bytes += take;
        if line.ends_with(b"\n") {
            return String::from_utf8(line)
                .map(Some)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        }
    }
}

/// Match an LSP header name case-insensitively, returning its value.
fn strip_header<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let prefix = line.get(..name.len())?;
    prefix
        .eq_ignore_ascii_case(name)
        .then(|| &line[name.len()..])
}

fn write_message(writer: &mut impl Write, body: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(body).expect("an LSP message serializes");
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::io::{self, BufReader, Cursor};

    use serde_json::json;

    use super::{position, read_message};

    #[test]
    fn positions_count_utf16_code_units() {
        let text = "a😀b\nz";
        let after_astral = "a😀".len();

        assert_eq!(
            position(after_astral, text),
            json!({ "line": 0, "character": 3 })
        );
        assert_eq!(
            position(text.len(), text),
            json!({ "line": 1, "character": 1 })
        );
    }

    #[test]
    fn rejects_an_oversized_header_line_before_reading_a_body() {
        let input = vec![b'x'; 9 * 1024];
        let mut reader = BufReader::new(Cursor::new(input));

        let error = read_message(&mut reader).expect_err("oversized header rejected");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_an_oversized_header_block_before_reading_a_body() {
        let mut input = Vec::new();
        for _ in 0..20 {
            input.extend_from_slice(b"X-Marrow-Test: ");
            input.extend(vec![b'x'; 900]);
            input.extend_from_slice(b"\r\n");
        }
        let mut reader = BufReader::new(Cursor::new(input));

        let error = read_message(&mut reader).expect_err("oversized header rejected");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
