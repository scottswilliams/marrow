//! When a configured test's module path collides with, or duplicates, a source
//! module, suppression of resolution noise must not over-reach: calls into a
//! *complete* source or test module stay reported even when a broken sibling
//! shares its path, while a genuine source/test module duplication is recognized
//! so a present function is not mistaken for a missing one. The opened test's
//! `check.assignment_type` is the positive control present in every case.

use std::fs;

use serde_json::json;

mod support;
mod support_lsp;

use support::{temp_dir as temp_project, write};
use support_lsp::{
    file_uri, frame, has_code, initialize_with_root, published_diagnostics, run_lsp, shutdown_exit,
};

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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        has_code(diagnostics, "check.unresolved_call"),
        "declared modules in configured tests must not suppress source-module calls: {frames:#?}",
    );
    assert!(
        has_code(diagnostics, "check.assignment_type"),
        "other test-local checker diagnostics should remain: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        has_code(diagnostics, "check.unresolved_call"),
        "broken test paths must not suppress calls into complete source modules: {frames:#?}",
    );
    assert!(
        has_code(diagnostics, "check.assignment_type"),
        "other test-local checker diagnostics should remain: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        has_code(diagnostics, "check.unresolved_call"),
        "broken source paths must not suppress calls into complete test modules: {frames:#?}",
    );
    assert!(
        has_code(diagnostics, "check.assignment_type"),
        "other test-local checker diagnostics should remain: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        !has_code(diagnostics, "check.unresolved_call"),
        "duplicate source/test modules should not look like a missing function: {frames:#?}",
    );
    assert!(
        has_code(diagnostics, "check.assignment_type"),
        "other test-local checker diagnostics should remain: {frames:#?}",
    );
}
