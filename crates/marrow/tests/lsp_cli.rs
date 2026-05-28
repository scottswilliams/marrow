use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// Frame one JSON-RPC body the way LSP expects: a `Content-Length` header (byte
/// length), a blank line, then the JSON.
fn frame(body: &Value) -> Vec<u8> {
    let json = serde_json::to_vec(body).expect("serialize");
    let mut bytes = format!("Content-Length: {}\r\n\r\n", json.len()).into_bytes();
    bytes.extend_from_slice(&json);
    bytes
}

/// Parse every `Content-Length`-framed message out of a captured byte stream.
fn parse_frames(mut bytes: &[u8]) -> Vec<Value> {
    let mut messages = Vec::new();
    while let Some(header_end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
        let header = std::str::from_utf8(&bytes[..header_end]).expect("header utf8");
        let len: usize = header
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|n| n.trim().to_string())
            })
            .expect("content-length header")
            .parse()
            .expect("length");
        let body_start = header_end + 4;
        let body = &bytes[body_start..body_start + len];
        messages.push(serde_json::from_slice(body).expect("body json"));
        bytes = &bytes[body_start + len..];
    }
    messages
}

/// Run `marrow lsp`, feeding `input` on stdin and returning the exit status and
/// the framed messages it wrote to stdout. `input` must stay small: it is written
/// in full before stdout is drained, so a buffer-filling input could deadlock.
fn run_lsp(input: &[u8]) -> (std::process::Output, Vec<Value>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn marrow lsp");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input)
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    let frames = parse_frames(&output.stdout);
    (output, frames)
}

fn initialize() -> Vec<u8> {
    frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}))
}

fn shutdown_exit() -> Vec<u8> {
    let mut bytes = frame(&json!({"jsonrpc":"2.0","id":99,"method":"shutdown"}));
    bytes.extend(frame(&json!({"jsonrpc":"2.0","method":"exit"})));
    bytes
}

#[test]
fn initialize_advertises_sync_and_shuts_down_cleanly() {
    let mut input = initialize();
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let init = frames
        .iter()
        .find(|m| m["id"] == json!(1))
        .expect("initialize response");
    assert_eq!(init["result"]["capabilities"]["textDocumentSync"], json!(1));
}

#[test]
fn did_open_with_an_error_publishes_a_located_diagnostic() {
    let mut input = initialize();
    // A tab on line 2 is a lexical error (`parse.syntax`).
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri":"file:///t.mw","languageId":"marrow","version":1,
            "text":"module app\n\tpub fn main()\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    assert_eq!(publish["params"]["uri"], json!("file:///t.mw"));
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(!diagnostics.is_empty(), "{publish}");
    assert_eq!(diagnostics[0]["code"], json!("parse.syntax"));
    assert_eq!(diagnostics[0]["severity"], json!(1));
    assert_eq!(diagnostics[0]["source"], json!("marrow"));
    // The tab is on the second line (0-based line 1).
    assert_eq!(diagnostics[0]["range"]["start"]["line"], json!(1));
}

#[test]
fn an_unknown_request_gets_method_not_found() {
    let mut input = initialize();
    input.extend(frame(&json!({
        "jsonrpc":"2.0","id":7,"method":"textDocument/nonsense","params":{}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let reply = frames
        .iter()
        .find(|m| m["id"] == json!(7))
        .expect("a response to the unknown request");
    assert_eq!(reply["error"]["code"], json!(-32601), "{reply}");
}

#[test]
fn did_change_republishes_diagnostics() {
    let mut input = initialize();
    // Open a clean document, then change it to introduce a tab error.
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri":"file:///t.mw","languageId":"marrow","version":1,
            "text":"module app\n"}}
    })));
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didChange",
        "params":{"textDocument":{"uri":"file:///t.mw","version":2},
            "contentChanges":[{"text":"module app\n\tpub fn main()\n"}]}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publishes: Vec<&Value> = frames
        .iter()
        .filter(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .collect();
    assert_eq!(publishes.len(), 2, "{frames:#?}");
    assert!(
        publishes[0]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "opening a clean document reports no diagnostics: {}",
        publishes[0]
    );
    assert!(
        !publishes[1]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the change introduces a diagnostic: {}",
        publishes[1]
    );
}
