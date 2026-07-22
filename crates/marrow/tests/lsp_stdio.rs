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

/// The Graph Report conformance fixture source: a single 393-line module (structs, an
/// enum with members, a generic helper, monomorphic helpers, and tests) — the earning
/// caller for completion, signature help, and document symbols.
const GRAPH_REPORT: &str =
    include_str!("../../../fixtures/v01/conformance/graph_report/src/graph_report.mw");

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

/// Open the Graph Report fixture as the project's `main.mw` and drain its initial
/// diagnostic publication, leaving a ready snapshot for a follow-up semantic query.
fn open_graph_report(conn: &mut Connection, dir: &Path) {
    did_open(conn, dir, GRAPH_REPORT, 1);
    let target = document_uri(dir);
    conn.recv_until(|m| {
        m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && m["params"]["uri"].as_str() == Some(target.as_str())
    });
}

#[test]
fn completion_at_enum_path_returns_members() {
    // The in-progress edit state the feature serves: the developer has typed `Role::` in
    // `classifyRole` and not yet the member. The incomplete path does not parse; the
    // bounded parser recovery still classifies the enum-path position.
    let editing = GRAPH_REPORT.replacen("return Role::isolated", "return Role::", 1);
    let dir = temp_project("completion", &editing);
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    did_open(&mut conn, &dir, &editing, 1);
    let target = document_uri(&dir);
    conn.recv_until(|m| {
        m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && m["params"]["uri"].as_str() == Some(target.as_str())
    });
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
    let reply = conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(30));
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
    conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn signature_help_inside_call_marks_active_parameter() {
    let dir = temp_project("sighelp", GRAPH_REPORT);
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    open_graph_report(&mut conn, &dir);
    // Inside `getOr(reached, e.src, false)` at the second argument slot.
    let (line, character) = lsp_position(GRAPH_REPORT, after(GRAPH_REPORT, "getOr(reached, "));
    conn.request(
        31,
        "textDocument/signatureHelp",
        serde_json::json!({
            "textDocument": { "uri": document_uri(&dir) },
            "position": { "line": line, "character": character },
        }),
    );
    let reply = conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(31));
    let signatures = reply["result"]["signatures"]
        .as_array()
        .expect("signature help returns signatures");
    assert_eq!(signatures.len(), 1, "one active signature");
    assert!(
        signatures[0]["label"]
            .as_str()
            .unwrap_or("")
            .contains("getOr"),
        "the callee signature is `getOr`"
    );
    assert_eq!(
        reply["result"]["activeParameter"].as_i64(),
        Some(1),
        "the cursor sits at the second parameter"
    );
    conn.request(9, "shutdown", Value::Null);
    conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn document_symbol_returns_declaration_outline() {
    let dir = temp_project("symbols", GRAPH_REPORT);
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    open_graph_report(&mut conn, &dir);
    conn.request(
        32,
        "textDocument/documentSymbol",
        serde_json::json!({
            "textDocument": { "uri": document_uri(&dir) },
        }),
    );
    let reply = conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(32));
    let symbols = reply["result"].as_array().expect("a symbol array");
    let names: Vec<&str> = symbols
        .iter()
        .filter_map(|symbol| symbol["name"].as_str())
        .collect();
    for name in ["Pair", "Edge", "Role", "getOr", "classifyRole", "report"] {
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
    conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn advertises_completion_signature_and_symbol() {
    let dir = temp_project("caps", GRAPH_REPORT);
    let mut conn = Connection::spawn(&dir);
    conn.request(
        1,
        "initialize",
        serde_json::json!({
            "processId": Value::Null,
            "rootUri": root_uri(&dir),
            "capabilities": {},
        }),
    );
    let reply = conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(1));
    let caps = &reply["result"]["capabilities"];
    assert!(
        caps["completionProvider"].is_object(),
        "advertises completion"
    );
    assert!(
        caps["signatureHelpProvider"].is_object(),
        "advertises signature help"
    );
    assert_eq!(
        caps["documentSymbolProvider"].as_bool(),
        Some(true),
        "advertises document symbols"
    );
    // The refused surface is never advertised.
    assert!(
        caps["completionProvider"]["resolveProvider"]
            .as_bool()
            .unwrap_or(false)
            == false,
        "no completionItem/resolve"
    );
    conn.notify("initialized", serde_json::json!({}));
    conn.request(9, "shutdown", Value::Null);
    conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(9));
    conn.notify("exit", Value::Null);
    assert_eq!(conn.wait(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

/// Measured (not a default gate): ready-snapshot latency for the three H00c methods over
/// the Graph Report probe positions. Run with `--ignored --nocapture`. The frozen budgets
/// are median <= 5 ms and p95 <= 25 ms per query on an already-ready snapshot.
#[test]
#[ignore = "measured latency probe; run explicitly with --ignored --nocapture"]
fn measure_earned_facts_latency() {
    // The incomplete-edit overlay so completion returns the enum members; document symbols
    // and signature help are unaffected by the single recovered path.
    let editing = GRAPH_REPORT.replacen("return Role::isolated", "return Role::", 1);
    let dir = temp_project("latency-facts", &editing);
    let mut conn = Connection::spawn(&dir);
    initialize(&mut conn, &dir);
    did_open(&mut conn, &dir, &editing, 1);
    let target = document_uri(&dir);
    conn.recv_until(|m| {
        m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && m["params"]["uri"].as_str() == Some(target.as_str())
    });

    let (comp_line, comp_char) = lsp_position(&editing, after(&editing, "return Role::"));
    let (sig_line, sig_char) = lsp_position(&editing, after(&editing, "getOr(reached, "));

    let cases: [(&str, &str, serde_json::Value); 3] = [
        (
            "completion",
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": target },
                "position": { "line": comp_line, "character": comp_char },
            }),
        ),
        (
            "signatureHelp",
            "textDocument/signatureHelp",
            serde_json::json!({
                "textDocument": { "uri": target },
                "position": { "line": sig_line, "character": sig_char },
            }),
        ),
        (
            "documentSymbol",
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": target } }),
        ),
    ];

    for (name, method, params) in cases {
        let mut samples = Vec::new();
        for iteration in 0..50 {
            let id = 1000 + iteration;
            let start = std::time::Instant::now();
            conn.request(id, method, params.clone());
            conn.recv_until(|m| m.get("id").and_then(Value::as_i64) == Some(id));
            samples.push(start.elapsed());
        }
        samples.sort();
        let median = samples[samples.len() / 2];
        let p95 = samples[(samples.len() * 95) / 100];
        println!(
            "{name}: median={median:?} p95={p95:?} over {} queries",
            samples.len()
        );
        assert!(
            median.as_millis() <= 5,
            "{name} median {median:?} exceeds the 5ms budget"
        );
        assert!(
            p95.as_millis() <= 25,
            "{name} p95 {p95:?} exceeds the 25ms budget"
        );
    }

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
