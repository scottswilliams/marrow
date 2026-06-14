use crate::support;
use marrow_check::{StoreIndexKeySource, check_project};
use marrow_schema::NodeKind;

use support::{config, temp_project, write};

const LIBRARY_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/library.mw"
));

#[test]
fn v01_library_fixture_checks_clean_and_exposes_store_identity_refs() {
    let root = temp_project("v01-library-check", |root| {
        write(root, "src/v01/library.mw", LIBRARY_SOURCE);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

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

    let module_id = program
        .facts
        .module_id("v01::library")
        .expect("library facts module");
    let books_store = program
        .facts
        .store_id(module_id, "books")
        .expect("books store fact");
    let by_author = program
        .facts
        .store_indexes()
        .iter()
        .find(|index| index.store == books_store && index.name == "byAuthor")
        .expect("byAuthor index");

    assert_eq!(by_author.keys.len(), 2);
    assert_eq!(by_author.keys[0].name, "author");
    assert!(matches!(
        by_author.keys[0].source,
        StoreIndexKeySource::ResourceMember(member_id)
            if program
                .facts
                .resource_members()
                .get(member_id.0 as usize)
                .is_some_and(|member| member.name == "author")
    ));
    assert_eq!(by_author.keys[1].name, "id");
    assert_eq!(by_author.keys[1].source, StoreIndexKeySource::IdentityKey);
}
