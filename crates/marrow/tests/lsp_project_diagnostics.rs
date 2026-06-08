//! Opening a file under a configured project root publishes the whole-project
//! checker's diagnostics, located in the opened file: an existing source file, a
//! brand-new source file, a brand-new test file, and a test file even when other
//! sources carry errors. These pin that the LSP runs the project checker, not the
//! single-file parser, for files inside the project.

use std::fs;

use serde_json::json;

mod support;
mod support_lsp;

use support::{temp_dir as temp_project, write};
use support_lsp::{
    file_uri, frame, initialize_with_root, published_diagnostics, run_lsp, shutdown_exit,
};

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
    let diagnostics = published_diagnostics(&frames);
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == json!("check.assignment_type"))
        .expect("project checker diagnostic should be published");
    assert_eq!(
        diagnostic["range"],
        json!({
            "start": { "line": 2, "character": 4 },
            "end": { "line": 2, "character": 22 },
        })
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "new project source should get checker diagnostics: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "new project test should get checker diagnostics: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("check.assignment_type")),
        "project test should get checker diagnostics despite source errors: {frames:#?}",
    );
}
