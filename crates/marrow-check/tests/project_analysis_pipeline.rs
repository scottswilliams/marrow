mod support;

use marrow_check::check_project;

use support::{assert_clean, config, temp_project, write};

/// The `.mw` code block from the canonical reference sample.
fn sample_source() -> String {
    let doc = include_str!("../../../docs/language/sample.md");
    doc.split("```mw")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("the sample document has an mw code block")
        .to_string()
}

#[test]
fn the_reference_sample_checks_clean() {
    // The canonical sample (`module shelf::sample`) must check with no diagnostics
    // — in particular no false `check.unresolved_call` on its builtins
    // (keys/append/exists/nextId/...), which would mean `is_builtin_call` no
    // longer recognizes the full set of builtins.
    let root = temp_project("sample-check", |root| {
        write(root, "src/shelf/sample.mw", &sample_source());
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn surfaces_resource_body_index_errors() {
    let root = temp_project("schema-error", |root| {
        // Resource bodies no longer own index declarations; indexes belong to the
        // store body, so a nested resource-body index is rejected by the parser.
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             \x20   notes(noteId: string)\n\
             \x20       index bad(noteId)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "parse.syntax" && diagnostic.span.line == 5),
        "{:#?}",
        report.diagnostics
    );
}
