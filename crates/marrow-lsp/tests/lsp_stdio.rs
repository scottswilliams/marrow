//! The stdio boundary of the real `marrow-lsp` binary: header framing, the
//! initialize/initialized handshake, the not-initialized refusal, clean shutdown/exit, and
//! nonzero termination on EOF. Payload semantics belong to the in-process coordinator and
//! fact tests.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

use marrow_test_support::{Scratch, file_uri};

/// A framed JSON-RPC connection to a spawned `marrow-lsp`.
struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
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

    /// Read messages until the response to request `id` arrives, up to a bound.
    fn recv_response(&mut self, id: i64) -> Value {
        for _ in 0..64 {
            let message = self.recv().expect("a framed message");
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                return message;
            }
        }
        panic!("no response to request {id} within bound");
    }

    fn wait(mut self) -> i32 {
        // Close stdin so the reader observes EOF if exit was not sent.
        drop(self.stdin);
        let status = self.child.wait().expect("wait for child");
        status.code().unwrap_or(-1)
    }

    /// Wait for the process with stdin still open, as an editor that sent `exit` and
    /// simply waits would.
    fn wait_keeping_stdin_open(mut self) -> i32 {
        let status = self.child.wait().expect("wait for child");
        drop(self.stdin);
        status.code().unwrap_or(-1)
    }
}

fn root_uri(dir: &Path) -> String {
    file_uri(dir)
}

fn temp_project(tag: &str, main: &str) -> Scratch {
    Scratch::project(&format!("stdio-{tag}"), main)
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
}

#[test]
fn exit_ends_the_process_while_stdin_stays_open() {
    let dir = temp_project("exit-open-stdin", "module main\n");
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    conn.request(9, "shutdown", Value::Null);
    conn.recv_response(9);
    conn.notify("exit", Value::Null);
    assert_eq!(
        conn.wait_keeping_stdin_open(),
        0,
        "exit alone ends the process; no stdin close is needed"
    );
}

/// Responses already admitted when an immediate `exit` arrives all reach the client:
/// the terminal drain writes everything queued before the process ends.
#[test]
fn immediate_exit_drains_every_admitted_response() {
    let dir = temp_project("exit-drain", "module main\n");
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    conn.request(9, "shutdown", Value::Null);
    let ids: Vec<i64> = (10..24).collect();
    for id in &ids {
        conn.request(*id, "noSuchMethod", Value::Null);
    }
    conn.notify("exit", Value::Null);
    let mut answered = Vec::new();
    while let Some(message) = conn.recv() {
        if let Some(id) = message.get("id").and_then(Value::as_i64) {
            answered.push(id);
        }
    }
    let expected: Vec<i64> = std::iter::once(9).chain(ids).collect();
    assert_eq!(
        answered, expected,
        "every admitted response arrives, in order"
    );
    assert_eq!(
        conn.wait_keeping_stdin_open(),
        0,
        "exit after shutdown is zero"
    );
}

#[test]
fn eof_without_exit_is_nonzero() {
    let dir = temp_project("eof", "module main\n");
    let mut conn = Connection::spawn();
    initialize(&mut conn, &dir);
    // Close stdin without sending exit: the server must terminate promptly, nonzero.
    assert_eq!(conn.wait(), 1, "EOF without exit is nonzero");
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
}
