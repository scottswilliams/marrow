//! LSP request lifecycle and single-document sync: initialize/shutdown, the
//! document-sync capability, parse diagnostics for an opened or changed file, the
//! method-not-found reply for an unknown request, and the non-project parse-only
//! fallback. These cases never depend on a configured project's resolution.

use serde_json::{Value, json};

mod support;
mod support_lsp;

use support::{temp_dir as temp_project, write};
use support_lsp::{
    file_uri, frame, initialize, initialize_with_root, published_diagnostics, run_lsp,
    shutdown_exit,
};

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
fn did_open_outside_project_sources_falls_back_to_parse_diagnostics() {
    let root = temp_project("lsp-outside-source");
    write(&root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
    write(&root, "src/app.mw", "module app\n");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(root.join("scratch.mw"));

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"module scratch\n\tfn f()\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let diagnostics = published_diagnostics(&frames);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("parse.syntax")),
        "non-project files should still get parse diagnostics: {frames:#?}",
    );
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
