//! The Marrow language server: a basic LSP over stdio.
//!
//! It speaks JSON-RPC 2.0 with `Content-Length` framing, handles the
//! `initialize`/`shutdown`/`exit` lifecycle, and tracks open documents with full
//! text sync. On every `didOpen`/`didChange` it parses the buffer and publishes
//! diagnostics (`textDocument/publishDiagnostics`). This first slice reports parse
//! diagnostics from [`marrow_syntax::parse_source`]; hover, definition, and
//! project-level (checked-fact) diagnostics are later slices.
//!
//! This is the editor language server, distinct from `marrow serve` (a data/IPC
//! server with different framing and purpose).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;

use marrow_syntax::{Severity, parse_source};
use serde_json::{Value, json};

pub fn run(args: &[String]) -> ExitCode {
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

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    match serve(&mut reader, &mut writer) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) | Err(_) => ExitCode::FAILURE,
    }
}

/// The message loop. Returns `Ok(true)` when `exit` arrived after `shutdown` (a
/// clean stop), `Ok(false)` when `exit` (or EOF) arrived without `shutdown`, and
/// `Err` on an I/O failure.
fn serve(reader: &mut impl BufRead, writer: &mut impl Write) -> io::Result<bool> {
    let mut documents: HashMap<String, String> = HashMap::new();
    let mut shutdown = false;
    while let Some(message) = read_message(reader)? {
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();
        match method {
            "initialize" => {
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
                    write_message(writer, &diagnostics_notification(&uri, &text))?;
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

/// The `(uri, full text)` of a `didOpen`/`didChange`. Full text sync means a
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

/// Parse `text` and build a `publishDiagnostics` notification for `uri`.
fn diagnostics_notification(uri: &str, text: &str) -> Value {
    let parsed = parse_source(text);
    let diagnostics = parsed
        .diagnostics
        .iter()
        .map(|diagnostic| lsp_diagnostic(diagnostic, text))
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

/// Map a Marrow diagnostic to the LSP shape: a `range` from the span's byte
/// offsets, a numeric `severity`, the dotted `code`, `source: "marrow"`, and the
/// message (with any `help` appended, as `marrow check` does).
fn lsp_diagnostic(diagnostic: &marrow_syntax::Diagnostic, text: &str) -> Value {
    let severity = match diagnostic.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
    };
    let mut message = diagnostic.message.clone();
    if let Some(help) = &diagnostic.help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }
    json!({
        "range": {
            "start": position(diagnostic.span.start_byte, text),
            "end": position(diagnostic.span.end_byte, text),
        },
        "severity": severity,
        "code": diagnostic.code,
        "source": "marrow",
        "message": message,
    })
}

/// Convert a byte offset into a 0-based LSP `{line, character}`. `character`
/// counts Unicode scalar values on the line; this matches UTF-16 code units for
/// the basic multilingual plane (and exactly for ASCII source), which is correct
/// for `.mw` in practice. Translating to UTF-16 for astral characters is a
/// later refinement.
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
            character += 1;
        }
        offset += ch.len_utf8();
    }
    json!({ "line": line, "character": character })
}

fn response(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Read one LSP message: parse the `Content-Length` header block, then read
/// exactly that many body bytes and parse them as JSON. `Ok(None)` on clean EOF.
fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
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
    // Bound the body so a corrupt header cannot force a huge allocation.
    const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
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
