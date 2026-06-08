// Each LSP test binary includes this whole module but uses only the helpers its
// cases need, so a helper unused by one binary is not dead across the split.
#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// Frame one JSON-RPC body the way LSP expects: a `Content-Length` header (byte
/// length), a blank line, then the JSON.
pub(crate) fn frame(body: &Value) -> Vec<u8> {
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
pub(crate) fn run_lsp(input: &[u8]) -> (std::process::Output, Vec<Value>) {
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

pub(crate) fn initialize() -> Vec<u8> {
    frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}))
}

pub(crate) fn initialize_with_root(root_uri: &str) -> Vec<u8> {
    frame(&json!({
        "jsonrpc":"2.0",
        "id":1,
        "method":"initialize",
        "params":{"capabilities":{}, "rootUri": root_uri}
    }))
}

pub(crate) fn shutdown_exit() -> Vec<u8> {
    let mut bytes = frame(&json!({"jsonrpc":"2.0","id":99,"method":"shutdown"}));
    bytes.extend(frame(&json!({"jsonrpc":"2.0","method":"exit"})));
    bytes
}

pub(crate) fn file_uri(path: impl AsRef<Path>) -> String {
    format!("file://{}", path.as_ref().display())
}

/// The single `textDocument/publishDiagnostics` notification's diagnostics array,
/// the structured oracle every project-aware LSP case asserts against.
pub(crate) fn published_diagnostics(frames: &[Value]) -> &Vec<Value> {
    frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics")["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array")
}

/// Whether any published diagnostic carries `code`.
pub(crate) fn has_code(diagnostics: &[Value], code: &str) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic["code"] == json!(code))
}
