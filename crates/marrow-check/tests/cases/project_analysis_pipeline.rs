use crate::support;

use marrow_check::check_project;

use support::{config, temp_project, write};

#[test]
fn surfaces_resource_body_index_errors() {
    let root = temp_project("schema-error", |root| {
        // Resource bodies no longer own index declarations; indexes belong to the
        // store body, so a nested resource-body index is rejected by the parser.
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             resource Book\n\
             \x20   title: string\n\
             \x20   notes(noteId: string)\n\
             \x20       index bad(noteId)\n\
             store ^books(id: int): Book\n",
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
