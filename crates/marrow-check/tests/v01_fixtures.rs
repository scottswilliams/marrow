mod support;

use marrow_check::check_project;
use marrow_schema::NodeKind;

use support::{config, temp_project, write};

const LIBRARY_SOURCE: &str = include_str!("../../../fixtures/v01/library.mw");

#[test]
fn v01_library_fixture_checks_clean_and_exposes_store_identity_refs() {
    let root = temp_project("v01-library-check", |root| {
        write(root, "src/v01/library.mw", LIBRARY_SOURCE);
    });
    let (report, program) = check_project(root.path(), &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let module = program
        .modules
        .iter()
        .find(|module| module.name == "v01::library")
        .expect("v01 library module");
    assert!(
        module
            .resources
            .iter()
            .any(|resource| resource.name == "Author"),
        "Author resource"
    );
    let book = module
        .resources
        .iter()
        .find(|resource| resource.name == "Book")
        .expect("Book resource");

    assert_eq!(
        book.field_type(&["author"])
            .expect("author field")
            .to_string(),
        "Id(^authors)"
    );
    let author = book
        .members
        .iter()
        .find(|member| member.name == "author")
        .expect("author member");
    assert!(
        matches!(author.kind, NodeKind::Slot { required: true, .. }),
        "author is the required relationship field"
    );
    assert_eq!(
        book.field_type(&["title"])
            .expect("title field")
            .to_string(),
        "string"
    );
    assert_eq!(
        book.leaf_type(&["tags"]).expect("tags leaf").to_string(),
        "string"
    );
}
