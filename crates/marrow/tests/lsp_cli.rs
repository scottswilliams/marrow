use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

mod support;

use support::{temp_dir as temp_project, write};

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

fn initialize_with_root(root_uri: &str) -> Vec<u8> {
    frame(&json!({
        "jsonrpc":"2.0",
        "id":1,
        "method":"initialize",
        "params":{"capabilities":{}, "rootUri": root_uri}
    }))
}

fn shutdown_exit() -> Vec<u8> {
    let mut bytes = frame(&json!({"jsonrpc":"2.0","id":99,"method":"shutdown"}));
    bytes.extend(frame(&json!({"jsonrpc":"2.0","method":"exit"})));
    bytes
}

fn file_uri(path: impl AsRef<Path>) -> String {
    format!("file://{}", path.as_ref().display())
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
fn did_open_in_project_publishes_checker_diagnostics() {
    let root = temp_project("lsp-check-diagnostics");
    write(&root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
    write(
        &root,
        "src/app.mw",
        "module app\nfn f()\n    var x: int = 1\n",
    );
    let file = root.join("src/app.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"module app\nfn f()\n    var x: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "project checker diagnostic should be published: {publish}",
    );
}

#[test]
fn did_open_new_project_source_publishes_checker_diagnostics() {
    let root = temp_project("lsp-new-source");
    write(&root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
    fs::create_dir_all(root.join("src")).expect("create src");
    let file = root.join("src/new_file.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"module new_file\nfn f()\n    var x: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "new project source should get checker diagnostics: {publish}",
    );
}

#[test]
fn did_open_new_project_test_publishes_checker_diagnostics() {
    let root = temp_project("lsp-new-test");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(&root, "src/app.mw", "module app\n");
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/new_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"fn smoke()\n    var x: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "new project test should get checker diagnostics: {publish}",
    );
}

#[test]
fn did_open_project_test_gets_checker_diagnostics_when_sources_have_errors() {
    let root = temp_project("lsp-test-with-source-error");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(
        &root,
        "src/app.mw",
        "module app\nfn f()\n    var x: int = \"str\"\n",
    );
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/new_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"fn smoke()\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "project test should get checker diagnostics despite source errors: {publish}",
    );
}

#[test]
fn did_open_project_test_suppresses_resolution_noise_when_source_parse_fails() {
    let root = temp_project("lsp-test-incomplete-source");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    // The tab is a lexical error, so this file contributes no `app` module,
    // even though the parser saw its resource and function declarations.
    write(
        &root,
        "src/app.mw",
        "module app\n\
         resource Book at ^books(id: int)\n\
         \x20   title: string\n\
         fn f()\n\
         \x20   return\n\
         \tconst BAD = 1\n",
    );
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/new_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"use app\nfn smoke()\n    app::f()\n    var b: Book\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "test-local checker diagnostics should remain: {publish}",
    );
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic["code"]
            == json!("check.unresolved_import")
            || diagnostic["code"] == json!("check.unresolved_call")
            || diagnostic["code"] == json!("check.unknown_type")),
        "resolution against incomplete source modules should be suppressed: {publish}",
    );
}

#[test]
fn did_open_project_test_keeps_local_resolution_diagnostics_when_source_parse_fails() {
    let root = temp_project("lsp-test-local-errors-incomplete-source");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(&root, "src/app.mw", "module app\n\tfn f()\n");
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/new_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"use std::definitely_missing\nfn smoke()\n    tests::helper::missing()\n    missing_local()\n    var n: NotAType\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    for code in [
        "check.unresolved_import",
        "check.unresolved_call",
        "check.unknown_type",
        "check.assignment_type",
    ] {
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic["code"] == json!(code)),
            "{code} should remain for test-local errors: {publish}",
        );
    }
}

#[test]
fn did_open_project_test_keeps_bare_call_matching_hidden_source_module() {
    let root = temp_project("lsp-test-local-bare-call-incomplete-source");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(&root, "src/app.mw", "module app\n\tfn f()\n");
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/new_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"fn smoke()\n    app()\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.unresolved_call")),
        "bare test-local calls should remain: {publish}",
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "other test-local checker diagnostics should remain: {publish}",
    );
}

#[test]
fn did_open_project_test_keeps_submodule_import_matching_hidden_source_prefix() {
    let root = temp_project("lsp-test-local-submodule-import-incomplete-source");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(&root, "src/app.mw", "module app\n\tfn f()\n");
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/new_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"use app::missing\nfn smoke()\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.unresolved_import")),
        "submodule imports should remain exact-module errors: {publish}",
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "other test-local checker diagnostics should remain: {publish}",
    );
}

#[test]
fn did_open_project_test_keeps_unresolved_call_when_another_test_has_parse_error() {
    let root = temp_project("lsp-test-local-call-with-broken-sibling-test");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(
        &root,
        "src/app.mw",
        "module app\npub fn main()\n    return\n",
    );
    write(&root, "tests/a_bad_test.mw", "fn broken()\n\treturn\n");
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/b_smoke_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"fn smoke()\n    missing_local()\n    var n: NotAType\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    for code in [
        "check.unresolved_call",
        "check.unknown_type",
        "check.assignment_type",
    ] {
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic["code"] == json!(code)),
            "{code} should remain for the clean configured test: {publish}",
        );
    }
}

#[test]
fn did_open_project_test_suppresses_unresolved_import_when_broken_configured_test_is_imported() {
    let root = temp_project("lsp-test-import-broken-sibling-test");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(
        &root,
        "src/app.mw",
        "module app\npub fn main()\n    return\n",
    );
    write(&root, "tests/helper.mw", "fn helper()\n\treturn\n");
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/smoke_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"use tests::helper\nfn smoke()\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.unresolved_import")),
        "imports of incomplete configured tests should not become resolution noise: {publish}",
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "other test-local checker diagnostics should remain: {publish}",
    );
}

#[test]
fn did_open_project_test_ignores_declared_modules_in_broken_configured_tests_for_call_suppression()
{
    let root = temp_project("lsp-test-local-call-with-broken-declared-module-test");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(
        &root,
        "src/app.mw",
        "module app\npub fn main()\n    return\n",
    );
    write(
        &root,
        "tests/a_bad_test.mw",
        "module app\nfn broken()\n\treturn\n",
    );
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/b_smoke_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"fn smoke()\n    app::missing()\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.unresolved_call")),
        "declared modules in configured tests must not suppress source-module calls: {publish}",
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "other test-local checker diagnostics should remain: {publish}",
    );
}

#[test]
fn did_open_project_test_keeps_source_module_calls_when_broken_test_path_collides() {
    let root = temp_project("lsp-test-path-collides-with-source-module");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(
        &root,
        "src/tests/app.mw",
        "module tests::app\npub fn main()\n    return\n",
    );
    write(&root, "tests/app.mw", "fn broken()\n\treturn\n");
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/b_smoke_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"fn smoke()\n    tests::app::missing()\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.unresolved_call")),
        "broken test paths must not suppress calls into complete source modules: {publish}",
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "other test-local checker diagnostics should remain: {publish}",
    );
}

#[test]
fn did_open_project_test_keeps_test_module_calls_when_broken_source_path_collides() {
    let root = temp_project("lsp-source-path-collides-with-test-module");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(&root, "src/tests/app.mw", "module tests::app\n\tfn f()\n");
    write(&root, "tests/app.mw", "fn existing()\n    return\n");
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/b_smoke_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"fn smoke()\n    tests::app::missing()\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.unresolved_call")),
        "broken source paths must not suppress calls into complete test modules: {publish}",
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "other test-local checker diagnostics should remain: {publish}",
    );
}

#[test]
fn did_open_project_test_suppresses_unresolved_call_when_test_module_duplicates_source_module() {
    let root = temp_project("lsp-test-module-duplicates-source-module");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(
        &root,
        "src/tests/app.mw",
        "module tests::app\npub fn sourceOnly()\n    return\n",
    );
    write(&root, "tests/app.mw", "pub fn testOnly()\n    return\n");
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/b_smoke_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"fn smoke()\n    tests::app::testOnly()\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.unresolved_call")),
        "duplicate source/test modules should not look like a missing function: {publish}",
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "other test-local checker diagnostics should remain: {publish}",
    );
}

#[test]
fn did_open_project_test_suppresses_unknown_types_from_broken_configured_test_declarations() {
    let root = temp_project("lsp-test-type-from-broken-sibling-test");
    write(
        &root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#,
    );
    write(
        &root,
        "src/app.mw",
        "module app\npub fn main()\n    return\n",
    );
    write(
        &root,
        "tests/a_bad_test.mw",
        "resource Fixture\n    title: string\n\tconst BAD = 1\n",
    );
    fs::create_dir_all(root.join("tests")).expect("create tests");
    let file = root.join("tests/b_smoke_test.mw");
    let root_uri = file_uri(&root);
    let file_uri = file_uri(&file);

    let mut input = initialize_with_root(&root_uri);
    input.extend(frame(&json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri": file_uri, "languageId":"marrow","version":1,
            "text":"fn smoke()\n    var f: Fixture\n    var y: int = \"str\"\n"}}
    })));
    input.extend(shutdown_exit());
    let (output, frames) = run_lsp(&input);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.unknown_type")),
        "types declared in broken configured tests should not become sibling unknown-type noise: {publish}",
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "other test-local checker diagnostics should remain: {publish}",
    );
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
    let publish = frames
        .iter()
        .find(|m| m["method"] == json!("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics");
    let diagnostics = publish["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("parse.syntax")),
        "non-project files should still get parse diagnostics: {publish}",
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
