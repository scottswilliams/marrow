//! Production probes for `marrow lsp`: spawn the real binary and drive the JSON-RPC
//! protocol over stdio.
//!
//! These are boundary tests. They establish protocol framing, the initialize/initialized
//! handshake, diagnostic publication over an opened document, hover/formatting responses,
//! clean shutdown/exit, and prompt nonzero termination on EOF without a test-only
//! production entry point. The client side uses `serde_json` generically — a dev-only,
//! std-only edge that does not weaken the server's closed production boundary.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

/// A framed JSON-RPC connection to a spawned `marrow lsp`.
struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Connection {
    fn spawn(root: &Path) -> Self {
        let _ = root;
        let mut child = Command::new(env!("CARGO_BIN_EXE_marrow"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn marrow lsp");
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

    /// Read messages until one matching `predicate` arrives, up to a bound.
    fn recv_until(&mut self, mut predicate: impl FnMut(&Value) -> bool) -> Value {
        for _ in 0..64 {
            let message = self.recv().expect("a framed message");
            if predicate(&message) {
                return message;
            }
        }
        panic!("no matching message within bound");
    }

    fn wait(mut self) -> i32 {
        // Close stdin so the reader observes EOF if exit was not sent.
        drop(self.stdin);
        let status = self.child.wait().expect("wait for child");
        status.code().unwrap_or(-1)
    }
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
    let reply = conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(1));
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
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    conn.request(9, "shutdown", Value::Null);
    let reply = conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    assert!(reply.get("result").is_some());
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0, "clean shutdown then exit is zero");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_invalid_document_publishes_diagnostics() {
    let dir = temp_project(
        "diag",
        "module main\n\npub fn f(): int {\n    return 1\n}\n",
    );
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    // Open with an invalid overlay body: expect a nonempty diagnostic publication for it.
    did_open(
        &mut conn,
        &dir,
        "module main\n\npub fn f(): int {\n    return \n}\n",
        1,
    );
    let target = document_uri(&dir);
    let publish = conn.recv_until(|m| {
        m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && m["params"]["uri"].as_str() == Some(target.as_str())
            && m["params"]["diagnostics"]
                .as_array()
                .map(|d| !d.is_empty())
                .unwrap_or(false)
    });
    let diagnostic = &publish["params"]["diagnostics"][0];
    assert!(
        diagnostic.get("range").is_some(),
        "diagnostic carries a range"
    );
    assert!(diagnostic["code"].is_string(), "diagnostic carries a code");
    conn.request(9, "shutdown", Value::Null);
    conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn clean_project_publishes_empty_diagnostics() {
    let dir = temp_project(
        "clean",
        "module main\n\npub fn f(): int {\n    return 1\n}\n",
    );
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    did_open(
        &mut conn,
        &dir,
        "module main\n\npub fn f(): int {\n    return 1\n}\n",
        1,
    );
    let target = document_uri(&dir);
    let publish = conn.recv_until(|m| {
        m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && m["params"]["uri"].as_str() == Some(target.as_str())
    });
    assert_eq!(
        publish["params"]["diagnostics"].as_array().unwrap().len(),
        0,
        "a clean file publishes an empty diagnostic list"
    );
    conn.request(9, "shutdown", Value::Null);
    conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn formatting_returns_edits() {
    let dir = temp_project("fmt", "module main\n\npub fn f(): int {\n    return 1\n}\n");
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    let unformatted = "module main\n\npub fn f():int{\n return 1\n}\n";
    did_open(&mut conn, &dir, unformatted, 1);
    // Drain the initial diagnostic publication.
    let target = document_uri(&dir);
    conn.recv_until(|m| {
        m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && m["params"]["uri"].as_str() == Some(target.as_str())
    });
    conn.request(
        5,
        "textDocument/formatting",
        serde_json::json!({
            "textDocument": { "uri": target },
            "options": { "tabSize": 4, "insertSpaces": true },
        }),
    );
    let reply = conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(5));
    let edits = reply["result"]
        .as_array()
        .expect("formatting returns edits");
    assert_eq!(edits.len(), 1, "one whole-document edit");
    conn.request(9, "shutdown", Value::Null);
    conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn eof_without_exit_is_nonzero() {
    let dir = temp_project("eof", "module main\n");
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    // Close stdin without sending exit: the server must terminate promptly, nonzero.
    assert_eq!(conn.wait(), 1, "EOF without exit is nonzero");
    std::fs::remove_dir_all(&dir).ok();
}

/// Measured (not a default gate): edit-to-diagnostic latency over repeated full-document
/// changes. Run with `--ignored --nocapture`. The instant-response requirement is met
/// when the median stays well under the A02a diagnostics budget.
#[test]
#[ignore = "measured latency probe; run explicitly with --ignored --nocapture"]
fn measure_edit_to_diagnostic_latency() {
    let dir = temp_project(
        "latency",
        "module main\n\npub fn f(): int {\n    return 1\n}\n",
    );
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    did_open(
        &mut conn,
        &dir,
        "module main\n\npub fn f(): int {\n    return 1\n}\n",
        1,
    );
    let target = document_uri(&dir);
    conn.recv_until(|m| {
        m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && m["params"]["uri"].as_str() == Some(target.as_str())
    });
    let mut samples = Vec::new();
    for version in 2..22 {
        let body = format!("module main\n\npub fn f(): int {{\n    return {version}\n}}\n");
        let start = std::time::Instant::now();
        conn.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": target, "version": version },
                "contentChanges": [ { "text": body } ],
            }),
        );
        conn.recv_until(|m| {
            m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
                && m["params"]["uri"].as_str() == Some(target.as_str())
        });
        samples.push(start.elapsed());
    }
    samples.sort();
    let median = samples[samples.len() / 2];
    let max = samples.last().copied().unwrap();
    println!(
        "edit-to-diagnostic: median={median:?} max={max:?} over {} edits",
        samples.len()
    );
    assert!(
        median.as_millis() < 200,
        "median edit-to-diagnostic under 200ms"
    );
    conn.request(9, "shutdown", Value::Null);
    conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn request_before_initialize_is_server_not_initialized() {
    let dir = temp_project("preinit", "module main\n");
    let mut conn = Connection::spawn(&dir);
    conn.request(
        2,
        "textDocument/formatting",
        serde_json::json!({
            "textDocument": { "uri": document_uri(&dir) },
            "options": { "tabSize": 4, "insertSpaces": true },
        }),
    );
    let reply = conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(2));
    assert_eq!(reply["error"]["code"].as_i64(), Some(-32002));
    // Now initialize and exit cleanly.
    initialize(&mut conn, &dir);
    conn.request(9, "shutdown", Value::Null);
    conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}
