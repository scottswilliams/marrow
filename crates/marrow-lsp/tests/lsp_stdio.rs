//! The stdio boundary of the real `marrow-lsp` binary: header framing, the
//! initialize/initialized handshake, the not-initialized refusal, clean shutdown/exit, and
//! nonzero termination on EOF. Payload semantics belong to the in-process coordinator and
//! fact tests; the queries kept here are the ones those tests cannot reach.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

/// A framed JSON-RPC connection to a spawned `marrow-lsp`.
struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// One bounded inbound wait. A diagnostic wait must name the client-owned document version
/// the publication analyzed; waiting for an arbitrary publication by URI is deliberately not
/// representable.
enum ExpectedMessage<'a> {
    Response(i64),
    Diagnostics {
        uri: &'a str,
        /// `None` matches a frame the server publishes with no version — a file no open
        /// document names, which is every dependency file.
        version: Option<i64>,
    },
}

impl ExpectedMessage<'_> {
    fn matches(&self, message: &Value) -> bool {
        match self {
            Self::Response(id) => message.get("id").and_then(Value::as_i64) == Some(*id),
            Self::Diagnostics { uri, version } => {
                message.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
                    && message["params"]["uri"].as_str() == Some(*uri)
                    && message["params"]["version"].as_i64() == *version
            }
        }
    }
}

impl Connection {
    /// The server takes no arguments and never reads the working directory: it selects its
    /// project from the `rootUri` of `initialize`.
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_marrow-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn marrow-lsp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, message: &Value) {
        let body = serde_json::to_string(message).unwrap();
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        self.stdin.write_all(body.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, id: i64, method: &str, params: Value) {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    /// Read one framed message, or `None` at end of stream.
    fn recv(&mut self) -> Option<Value> {
        let mut content_length = None;
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).ok()?;
            if read == 0 {
                return None;
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(value) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let length = content_length?;
        let mut body = vec![0u8; length];
        self.stdout.read_exact(&mut body).ok()?;
        serde_json::from_slice(&body).ok()
    }

    /// Read messages until the expected typed boundary arrives, up to a bound.
    fn recv_until(&mut self, expected: ExpectedMessage<'_>) -> Value {
        for _ in 0..64 {
            let message = self.recv().expect("a framed message");
            if expected.matches(&message) {
                return message;
            }
        }
        panic!("no matching message within bound");
    }

    fn recv_response(&mut self, id: i64) -> Value {
        self.recv_until(ExpectedMessage::Response(id))
    }

    fn recv_diagnostics(&mut self, uri: &str, version: i64) -> Value {
        self.recv_until(ExpectedMessage::Diagnostics {
            uri,
            version: Some(version),
        })
    }

    /// The diagnostics frame for a file no open document names.
    fn recv_unversioned_diagnostics(&mut self, uri: &str) -> Value {
        self.recv_until(ExpectedMessage::Diagnostics { uri, version: None })
    }

    fn wait(mut self) -> i32 {
        // Close stdin so the reader observes EOF if exit was not sent.
        drop(self.stdin);
        let status = self.child.wait().expect("wait for child");
        status.code().unwrap_or(-1)
    }
}

/// The Graph Report conformance fixture source: one module of structs, an enum with
/// members, monomorphic helpers, and tests — the earning caller for completion, signature
/// help, and document symbols.
const GRAPH_REPORT: &str =
    include_str!("../../../fixtures/v01/conformance/graph_report/src/graph_report.mw");

/// The library the Graph Report reaches through its `[dependencies]` alias: the generic
/// helper and the struct that crosses the boundary live here now, and the file checks on
/// its own, so it is the earning caller for the declarations the application no longer
/// declares.
const GRAPH_TEXT: &str =
    include_str!("../../../fixtures/v01/conformance/graph_report_lib/src/text.mw");

/// The zero-based LSP position (line, UTF-16 character) of a UTF-8 byte offset in a
/// source string. Mirrors the server's own UTF-16 owner so the probe addresses the exact
/// position the checker classifies.
fn lsp_position(source: &str, byte: usize) -> (i64, i64) {
    let clamped = byte.min(source.len());
    let mut line = 0i64;
    let mut line_start = 0usize;
    for (index, ch) in source.as_bytes()[..clamped].iter().enumerate() {
        if *ch == b'\n' {
            line += 1;
            line_start = index + 1;
        }
    }
    let mut character = 0i64;
    for (index, ch) in source[line_start..].char_indices() {
        if line_start + index + ch.len_utf8() > clamped {
            break;
        }
        character += ch.len_utf16() as i64;
    }
    (line, character)
}

/// The byte offset immediately after `needle`'s first occurrence in `source`.
fn after(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle present") + needle.len()
}

fn root_uri(dir: &Path) -> String {
    let mut uri = String::from("file://");
    for component in dir.components() {
        if let std::path::Component::Normal(part) = component {
            uri.push('/');
            uri.push_str(part.to_str().unwrap());
        }
    }
    uri
}

/// A workspace whose project declares one local dependency nested inside it, so both
/// trees sit under the one selected root and the server must tell them apart by origin
/// rather than by containment.
fn temp_dependency_project(tag: &str, main: &str) -> PathBuf {
    let base = temp_project(tag, main);
    let lib = base.join("lib/graphtext");
    std::fs::create_dir_all(lib.join("src")).unwrap();
    std::fs::write(lib.join("marrow.toml"), "edition = \"2026\"\n").unwrap();
    std::fs::write(lib.join("src/text.mw"), GRAPH_TEXT).unwrap();
    std::fs::write(
        base.join("marrow.toml"),
        "edition = \"2026\"\n\n[dependencies]\ngraphtext = { path = \"lib/graphtext\" }\n",
    )
    .unwrap();
    base
}

fn temp_project(tag: &str, main: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "marrow-lsp-stdio-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("marrow.toml"), "edition = \"2026\"\n").unwrap();
    std::fs::write(base.join("src/main.mw"), main).unwrap();
    base
}

fn initialize(conn: &mut Connection, dir: &Path) {
    conn.request(
        1,
        "initialize",
        serde_json::json!({
            "processId": Value::Null,
            "rootUri": root_uri(dir),
            "capabilities": {},
        }),
    );
    let reply = conn.recv_response(1);
    assert!(reply.get("result").is_some(), "initialize returns a result");
    assert!(
        reply["result"]["capabilities"]["hoverProvider"]
            .as_bool()
            .unwrap_or(false),
        "advertises hover"
    );
    conn.notify("initialized", serde_json::json!({}));
}

fn did_open(conn: &mut Connection, dir: &Path, text: &str, version: i64) {
    let uri = format!("{}/src/main.mw", root_uri(dir));
    conn.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": { "uri": uri, "languageId": "marrow", "version": version, "text": text }
        }),
    );
}

fn document_uri(dir: &Path) -> String {
    format!("{}/src/main.mw", root_uri(dir))
}

#[test]
fn handshake_and_clean_shutdown() {
    let dir = temp_project(
        "handshake",
        "module main\n\npub fn f(): int {\n    return 1\n}\n",
    );
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    conn.request(9, "shutdown", Value::Null);
    let reply = conn.recv_response(9);
    assert!(reply.get("result").is_some());
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0, "clean shutdown then exit is zero");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn eof_without_exit_is_nonzero() {
    let dir = temp_project("eof", "module main\n");
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    // Close stdin without sending exit: the server must terminate promptly, nonzero.
    assert_eq!(conn.wait(), 1, "EOF without exit is nonzero");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn request_before_initialize_is_server_not_initialized() {
    let dir = temp_project("preinit", "module main\n");
    let mut conn = Connection::spawn();
    conn.request(
        2,
        "textDocument/formatting",
        serde_json::json!({
            "textDocument": { "uri": document_uri(&dir) },
            "options": { "tabSize": 4, "insertSpaces": true },
        }),
    );
    let reply = conn.recv_response(2);
    assert_eq!(reply["error"]["code"].as_i64(), Some(-32002));
    // Now initialize and exit cleanly.
    initialize(&mut conn, &dir);
    conn.request(9, "shutdown", Value::Null);
    conn.recv_response(9);
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

/// Open the Graph Report fixture as the project's `main.mw` and drain its initial
/// diagnostic publication, leaving a ready snapshot for a follow-up semantic query.
fn open_graph_report(conn: &mut Connection, dir: &Path) {
    did_open(conn, dir, GRAPH_REPORT, 1);
    conn.recv_diagnostics(&document_uri(dir), 1);
}

#[test]
fn completion_at_enum_path_returns_members() {
    // The in-progress edit state the feature serves: the developer has typed `Role::` in
    // `classifyRole` and not yet the member. The incomplete path does not parse; the
    // bounded parser recovery still classifies the enum-path position.
    let editing = GRAPH_REPORT.replacen("return Role::isolated", "return Role::", 1);
    let dir = temp_project("completion", &editing);
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    did_open(&mut conn, &dir, &editing, 1);
    conn.recv_diagnostics(&document_uri(&dir), 1);
    // Just past the typed `Role::` — an enum-path position whose namespace is the enum's
    // members.
    let (line, character) = lsp_position(&editing, after(&editing, "return Role::"));
    conn.request(
        30,
        "textDocument/completion",
        serde_json::json!({
            "textDocument": { "uri": document_uri(&dir) },
            "position": { "line": line, "character": character },
        }),
    );
    let reply = conn.recv_response(30);
    // A `CompletionList` object or a bare items array; normalize to the items array.
    let items = reply["result"]
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| reply["result"].as_array())
        .expect("completion returns items");
    let labels: Vec<&str> = items
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    for member in ["source", "sink", "internal", "isolated"] {
        assert!(labels.contains(&member), "enum member {member} offered");
    }
    conn.request(9, "shutdown", Value::Null);
    conn.recv_response(9);
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn signature_help_inside_call_marks_active_parameter() {
    let dir = temp_project("sighelp", GRAPH_REPORT);
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    open_graph_report(&mut conn, &dir);
    // Inside `classifyRole(o, i)` at the second argument slot.
    let (line, character) = lsp_position(GRAPH_REPORT, after(GRAPH_REPORT, "classifyRole(o, "));
    conn.request(
        31,
        "textDocument/signatureHelp",
        serde_json::json!({
            "textDocument": { "uri": document_uri(&dir) },
            "position": { "line": line, "character": character },
        }),
    );
    let reply = conn.recv_response(31);
    let signatures = reply["result"]["signatures"]
        .as_array()
        .expect("signature help returns signatures");
    assert_eq!(signatures.len(), 1, "one active signature");
    assert!(
        signatures[0]["label"]
            .as_str()
            .unwrap_or("")
            .contains("classifyRole"),
        "the callee signature is `classifyRole`"
    );
    assert_eq!(
        reply["result"]["activeParameter"].as_i64(),
        Some(1),
        "the cursor sits at the second parameter"
    );
    conn.request(9, "shutdown", Value::Null);
    conn.recv_response(9);
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn document_symbol_returns_declaration_outline() {
    let dir = temp_project("symbols", GRAPH_REPORT);
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    open_graph_report(&mut conn, &dir);
    conn.request(
        32,
        "textDocument/documentSymbol",
        serde_json::json!({
            "textDocument": { "uri": document_uri(&dir) },
        }),
    );
    let reply = conn.recv_response(32);
    let symbols = reply["result"].as_array().expect("a symbol array");
    let names: Vec<&str> = symbols
        .iter()
        .filter_map(|symbol| symbol["name"].as_str())
        .collect();
    for name in ["Edge", "Role", "classifyRole", "topoOrder", "report"] {
        assert!(
            names.contains(&name),
            "top-level declaration {name} present"
        );
    }
    // The enum carries its members as nested children.
    let role = symbols
        .iter()
        .find(|symbol| symbol["name"].as_str() == Some("Role"))
        .expect("Role symbol");
    let members: Vec<&str> = role["children"]
        .as_array()
        .expect("enum children")
        .iter()
        .filter_map(|child| child["name"].as_str())
        .collect();
    for member in ["source", "sink", "internal", "isolated"] {
        assert!(members.contains(&member), "enum member {member} nested");
    }
    conn.request(9, "shutdown", Value::Null);
    conn.recv_response(9);
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

/// A dependency's file is reported at its own location. The library sits at
/// `lib/graphtext`, so its `src/text.mw` publishes under that directory rather than under
/// the consuming project's `src`, where no such file exists. Identities are relative to
/// their own tree; the origin is what places them.
#[test]
fn a_dependency_file_publishes_under_its_own_root() {
    let main = "module main\n\nuse graphtext::text\n\npub fn f(): int {\n    return 1\n}\n";
    let dir = temp_dependency_project("dependency-uri", main);
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    did_open(&mut conn, &dir, main, 1);
    conn.recv_diagnostics(&document_uri(&dir), 1);
    let published =
        conn.recv_unversioned_diagnostics(&format!("{}/lib/graphtext/src/text.mw", root_uri(&dir)));
    assert_eq!(
        published["params"]["version"],
        Value::Null,
        "a dependency file names no open document, so it publishes unversioned"
    );
    conn.request(9, "shutdown", Value::Null);
    conn.recv_response(9);
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

/// A dependency file is read-only: opening one never places its body in the capture
/// overlay, so the workspace keeps analysing. An overlay entry naming a file the root
/// project does not declare refuses the whole capture, which is exactly what must not
/// happen when a developer opens a library file to read it.
#[test]
fn opening_a_dependency_file_leaves_the_workspace_analysing() {
    let main = "module main\n\nuse graphtext::text\n\npub fn f(): int {\n    return 1\n}\n";
    let dir = temp_dependency_project("dependency-readonly", main);
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    did_open(&mut conn, &dir, main, 1);
    conn.recv_diagnostics(&document_uri(&dir), 1);

    let library_uri = format!("{}/lib/graphtext/src/text.mw", root_uri(&dir));
    conn.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": library_uri,
                "languageId": "marrow",
                "version": 1,
                "text": "module text\n\nthis is not Marrow source\n",
            }
        }),
    );

    // The project still analyses, and against the library's committed bytes: an overlay
    // entry naming a file the root project does not declare refuses the whole capture,
    // and no diagnostics for this edit would ever arrive.
    let edited = main.replace("return 1", "return 2");
    conn.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": { "uri": document_uri(&dir), "version": 2 },
            "contentChanges": [{ "text": edited }],
        }),
    );
    conn.recv_diagnostics(&document_uri(&dir), 2);

    conn.request(9, "shutdown", Value::Null);
    conn.recv_response(9);
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

/// Definition from the application into a library function returns the library's own
/// file URI. The target resolves through the existing definition fact: the boundary needs
/// no new canonical fact, only the origin the snapshot will carry.
#[test]
#[ignore = "needs the compiler half of local dependencies"]
fn definition_across_a_dependency_boundary_names_the_library_file() {
    let main = "module main\n\nuse graphtext::text\n\npub fn f(): bool {\n    return text::startsWith(\"ab\", \"a\")\n}\n";
    let dir = temp_dependency_project("dependency-definition", main);
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    did_open(&mut conn, &dir, main, 1);
    conn.recv_diagnostics(&document_uri(&dir), 1);
    let (line, character) = lsp_position(main, after(main, "text::start"));
    conn.request(
        40,
        "textDocument/definition",
        serde_json::json!({
            "textDocument": { "uri": document_uri(&dir) },
            "position": { "line": line, "character": character },
        }),
    );
    let reply = conn.recv_response(40);
    assert_eq!(
        reply["result"]["uri"].as_str(),
        Some(format!("{}/lib/graphtext/src/text.mw", root_uri(&dir)).as_str()),
        "the definition names the library's own file: {reply}"
    );
    conn.request(9, "shutdown", Value::Null);
    conn.recv_response(9);
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}
