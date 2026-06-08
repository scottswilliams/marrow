//! When a configured test file is opened but a *source* (or sibling test) file
//! fails to parse, the hidden module's declarations must not leak resolution
//! noise into the opened test: cross-module imports, calls, and type references
//! that would only resolve against the incomplete module are suppressed, while
//! the opened test's own local errors are always kept. The opened test's
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        has_code(diagnostics, "check.assignment_type"),
        "test-local checker diagnostics should remain: {frames:#?}",
    );
    assert!(
        !has_code(diagnostics, "check.unresolved_import")
            && !has_code(diagnostics, "check.unresolved_call")
            && !has_code(diagnostics, "check.unknown_type"),
        "resolution against incomplete source modules should be suppressed: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    for code in [
        "check.unresolved_import",
        "check.unresolved_call",
        "check.unknown_type",
        "check.assignment_type",
    ] {
        assert!(
            has_code(diagnostics, code),
            "{code} should remain for test-local errors: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        has_code(diagnostics, "check.unresolved_call"),
        "bare test-local calls should remain: {frames:#?}",
    );
    assert!(
        has_code(diagnostics, "check.assignment_type"),
        "other test-local checker diagnostics should remain: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        has_code(diagnostics, "check.unresolved_import"),
        "submodule imports should remain exact-module errors: {frames:#?}",
    );
    assert!(
        has_code(diagnostics, "check.assignment_type"),
        "other test-local checker diagnostics should remain: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    for code in [
        "check.unresolved_call",
        "check.unknown_type",
        "check.assignment_type",
    ] {
        assert!(
            has_code(diagnostics, code),
            "{code} should remain for the clean configured test: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        !has_code(diagnostics, "check.unresolved_import"),
        "imports of incomplete configured tests should not become resolution noise: {frames:#?}",
    );
    assert!(
        has_code(diagnostics, "check.assignment_type"),
        "other test-local checker diagnostics should remain: {frames:#?}",
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
    let diagnostics = published_diagnostics(&frames);
    assert!(
        !has_code(diagnostics, "check.unknown_type"),
        "types declared in broken configured tests should not become sibling unknown-type noise: {frames:#?}",
    );
    assert!(
        has_code(diagnostics, "check.assignment_type"),
        "other test-local checker diagnostics should remain: {frames:#?}",
    );
}
