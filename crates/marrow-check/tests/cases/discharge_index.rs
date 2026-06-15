use crate::support;
use crate::support_discharge;
use marrow_catalog::CatalogEntryKind;
use marrow_check::evolution::{RepairReason, Verdict, preview};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

use support::catalog::write_catalog;
use support::{check_with_accepted, temp_project, write};
use support_discharge::*;

/// Retiring a member whose source is gone, with populated records, is a destructive
/// decision. The verdict names the exact catalog id and the populated count.
#[test]
fn retire_of_populated_member_requires_scoped_approval() -> Result<(), Box<dyn std::error::Error>> {
    let subtitle_id = hex_id(4);
    let root = temp_project("discharge-retire", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire Book.subtitle\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            6,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::subtitle",
                    &subtitle_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    for (id, value) in [(1, "A"), (2, "B")] {
        seed.record(id);
        seed.member_by_id(id, &subtitle_id, Scalar::Str(value.into()));
    }

    let result = witness(&program, &store);

    match verdict_for(&result, &subtitle_id) {
        Verdict::DestructiveDecisionRequired { populated } => assert_eq!(*populated, 2),
        other => panic!("expected destructive decision, got {other:#?}"),
    }

    Ok(())
}

/// Dropping a sparse source field that nothing else depends on is a legal no-op. The
/// accepted entry lingers as data under its stable id, so the verdict is a no-op, not
/// an error and not a distinct deprecation outcome.
#[test]
fn dropped_sparse_field_is_no_op_not_error() {
    let subtitle_id = hex_id(4);
    let root = temp_project("discharge-dropped-sparse-field", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            11,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::subtitle",
                    &subtitle_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let store = TreeStore::memory();

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        matches!(verdict_for(&result, &subtitle_id), Verdict::NoOp),
        "{:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == subtitle_id),
        "{diagnostics:#?}"
    );
}

/// Dropping a source field that still holds stored data, with no `evolve retire` intent
/// and no dependent index, fails closed rather than silently orphaning the data. The
/// accepted entry lingers, but its cells are populated, so the bare drop is repair-required
/// and names `evolve retire`; this is the only difference from the empty-store no-op above.
#[test]
fn dropped_field_with_populated_data_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let subtitle_id = hex_id(4);
    let root = temp_project("discharge-dropped-field-populated", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            11,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::subtitle",
                    &subtitle_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    // Seed `subtitle` so the dropped member has stored data to orphan.
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &subtitle_id, Scalar::Str("Appendix".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &subtitle_id,
        RepairReason::PopulatedDropRequiresRetire,
    );

    Ok(())
}

#[test]
fn populated_drop_with_same_resource_same_type_addition_suggests_rename_before_retire()
-> Result<(), Box<dyn std::error::Error>> {
    let subtitle_id = hex_id(4);
    let root = temp_project("discharge-dropped-field-rename-plausible", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   summary: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string, summary: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            11,
            "books::Book",
            "books::^books",
            None,
            vec![
                member_entry("books::Book::title", &hex_id(3), "string"),
                member_entry("books::Book::subtitle", &subtitle_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.member_by_id(1, &subtitle_id, Scalar::Str("Appendix".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &subtitle_id,
        RepairReason::PopulatedDropRequiresRetire,
    );
    let diagnostic = diagnostic_for(&diagnostics, &subtitle_id);
    assert_repair_guidance_order(&diagnostic.message, "evolve rename", "evolve retire");

    Ok(())
}

#[test]
fn populated_drop_with_ambiguous_same_type_dropped_members_does_not_suggest_rename()
-> Result<(), Box<dyn std::error::Error>> {
    let subtitle_id = hex_id(4);
    let summary_id = hex_id(5);
    let root = temp_project("discharge-dropped-field-ambiguous-dropped-side", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   blurb: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string, blurb: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            11,
            "books::Book",
            "books::^books",
            None,
            vec![
                member_entry("books::Book::title", &hex_id(3), "string"),
                member_entry("books::Book::subtitle", &subtitle_id, "string"),
                member_entry("books::Book::summary", &summary_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.member_by_id(1, &subtitle_id, Scalar::Str("Appendix".into()));
    seed.member_by_id(1, &summary_id, Scalar::Str("Short".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    for dropped_id in [&subtitle_id, &summary_id] {
        assert_fails_closed(
            &result,
            &diagnostics,
            dropped_id,
            RepairReason::PopulatedDropRequiresRetire,
        );
        assert!(
            !diagnostic_for(&diagnostics, dropped_id)
                .message
                .contains("evolve rename"),
            "{diagnostics:#?}"
        );
    }

    Ok(())
}

#[test]
fn populated_drop_ignores_same_type_addition_in_another_resource_for_rename_hint()
-> Result<(), Box<dyn std::error::Error>> {
    let subtitle_id = hex_id(6);
    let root = temp_project("discharge-dropped-field-cross-resource-addition", |root| {
        write(
            root,
            "src/library.mw",
            "module library\n\
             resource Book\n\
             \x20   required title: string\n\
             resource Author\n\
             \x20   required name: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             store ^authors(id: int): Author\n\
             pub fn addBook(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = marrow_catalog::CatalogMetadata::new(
            20,
            vec![
                entry(CatalogEntryKind::Resource, "library::Book", &hex_id(1)),
                store_entry("library::^books", &hex_id(2), "int"),
                member_entry("library::Book::title", &hex_id(3), "string"),
                entry(CatalogEntryKind::Resource, "library::Author", &hex_id(4)),
                store_entry("library::^authors", &hex_id(5), "int"),
                member_entry("library::Book::subtitle", &subtitle_id, "string"),
            ],
        )
        .expect("catalog builds");
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.member_by_id(1, &subtitle_id, Scalar::Str("Appendix".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &subtitle_id,
        RepairReason::PopulatedDropRequiresRetire,
    );
    assert!(
        !diagnostic_for(&diagnostics, &subtitle_id)
            .message
            .contains("evolve rename"),
        "{diagnostics:#?}"
    );

    Ok(())
}

#[test]
fn populated_drop_ignores_same_resource_different_type_addition_for_rename_hint()
-> Result<(), Box<dyn std::error::Error>> {
    let subtitle_id = hex_id(4);
    let root = temp_project("discharge-dropped-field-different-type-addition", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   pages: int\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string, pages: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            11,
            "books::Book",
            "books::^books",
            None,
            vec![
                member_entry("books::Book::title", &hex_id(3), "string"),
                member_entry("books::Book::subtitle", &subtitle_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.member_by_id(1, &subtitle_id, Scalar::Str("Appendix".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &subtitle_id,
        RepairReason::PopulatedDropRequiresRetire,
    );
    assert!(
        !diagnostic_for(&diagnostics, &subtitle_id)
            .message
            .contains("evolve rename"),
        "{diagnostics:#?}"
    );

    Ok(())
}

fn diagnostic_for<'a>(
    diagnostics: &'a [marrow_check::evolution::RepairDiagnostic],
    catalog_id: &str,
) -> &'a marrow_check::evolution::RepairDiagnostic {
    diagnostics
        .iter()
        .find(|diagnostic| diagnostic.catalog_id.as_str() == catalog_id)
        .unwrap_or_else(|| panic!("diagnostic for `{catalog_id}` among {diagnostics:#?}"))
}

fn assert_repair_guidance_order(message: &str, first: &str, second: &str) {
    let first_index = message
        .find(first)
        .unwrap_or_else(|| panic!("missing `{first}` in diagnostic: {message}"));
    let second_index = message
        .find(second)
        .unwrap_or_else(|| panic!("missing `{second}` in diagnostic: {message}"));
    assert!(
        first_index < second_index,
        "`{first}` must appear before `{second}` in diagnostic: {message}"
    );
}

/// Dropping a whole resource (its `resource` block, its `store`, and its members) whose
/// store still holds records would orphan every record under the gone root. The store entry
/// the accepted catalog still records is no longer declared in source, so the discharge
/// fences the dropped store with the same `PopulatedDropRequiresRetire` reason a populated
/// member drop uses, naming the root once. The now-also-absent member entry stays a no-op,
/// covered by the store fence, so a dropped populated root yields exactly one fence.
#[test]
fn dropped_populated_store_fails_closed() {
    let book_store_id = hex_id(5);
    let book_member_id = hex_id(6);
    let root = temp_project("discharge-drop-populated-store", |root| {
        // Source keeps only `Author`; the `Book` resource, its store, and its member are gone.
        write(
            root,
            "src/library.mw",
            "module library\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             pub fn add(name: string): Id(^authors)\n\
             \x20   return nextId(^authors)\n",
        );
        let accepted = marrow_catalog::CatalogMetadata::new(
            20,
            vec![
                entry(CatalogEntryKind::Resource, "library::Author", &hex_id(1)),
                store_entry("library::^authors", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Author::name",
                    &hex_id(3),
                ),
                entry(CatalogEntryKind::Resource, "library::Book", &hex_id(4)),
                store_entry("library::^books", &book_store_id, "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Book::title",
                    &book_member_id,
                ),
            ],
        )
        .expect("catalog builds");
        write_catalog(root, &accepted);
    });
    let (_report, program) = check_with_accepted(&root);
    let store = TreeStore::memory();
    seed_catalog_member(
        &store,
        &book_store_id,
        &[SavedKey::Int(1)],
        &book_member_id,
        Scalar::Str("Dune".into()),
    );

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    // The store entry naming the dropped root is the single fence.
    assert_fails_closed(
        &result,
        &diagnostics,
        &book_store_id,
        RepairReason::PopulatedDropRequiresRetire,
    );
    // The dropped member is covered by the store fence, not double-fenced.
    assert!(
        matches!(verdict_for(&result, &book_member_id), Verdict::NoOp),
        "the dropped member is covered by the store fence: {:#?}",
        result.verdicts
    );
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == book_member_id),
        "exactly one diagnostic per dropped root: {diagnostics:#?}"
    );
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.catalog_id.as_str() == book_store_id)
        .expect("store diagnostic");
    assert!(
        diagnostic.message.contains("^books") && diagnostic.message.contains("evolve retire"),
        "the fence names the dropped root and points at retire: {}",
        diagnostic.message
    );
}

#[test]
fn dropped_populated_singleton_store_fails_closed() {
    let settings_store_id = hex_id(5);
    let settings_member_id = hex_id(6);
    let root = temp_project("discharge-drop-populated-singleton-store", |root| {
        write(
            root,
            "src/library.mw",
            "module library\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             pub fn add(name: string): Id(^authors)\n\
             \x20   return nextId(^authors)\n",
        );
        let accepted = marrow_catalog::CatalogMetadata::new(
            20,
            vec![
                entry(CatalogEntryKind::Resource, "library::Author", &hex_id(1)),
                store_entry("library::^authors", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Author::name",
                    &hex_id(3),
                ),
                entry(CatalogEntryKind::Resource, "library::Settings", &hex_id(4)),
                store_entry("library::^settings", &settings_store_id, ""),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Settings::theme",
                    &settings_member_id,
                ),
            ],
        )
        .expect("catalog builds");
        write_catalog(root, &accepted);
    });
    let (_report, program) = check_with_accepted(&root);
    let store = TreeStore::memory();
    seed_catalog_member(
        &store,
        &settings_store_id,
        &[],
        &settings_member_id,
        Scalar::Str("dark".into()),
    );

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &settings_store_id,
        RepairReason::PopulatedDropRequiresRetire,
    );
}

#[test]
fn retired_populated_singleton_store_requires_scoped_approval() {
    let settings_store_id = hex_id(5);
    let settings_member_id = hex_id(6);
    let root = temp_project("discharge-retire-populated-singleton-store", |root| {
        write(
            root,
            "src/library.mw",
            "module library\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             evolve\n\
             \x20   retire ^settings\n\
             pub fn add(name: string): Id(^authors)\n\
             \x20   return nextId(^authors)\n",
        );
        let accepted = marrow_catalog::CatalogMetadata::new(
            20,
            vec![
                entry(CatalogEntryKind::Resource, "library::Author", &hex_id(1)),
                store_entry("library::^authors", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Author::name",
                    &hex_id(3),
                ),
                entry(CatalogEntryKind::Resource, "library::Settings", &hex_id(4)),
                store_entry("library::^settings", &settings_store_id, ""),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Settings::theme",
                    &settings_member_id,
                ),
            ],
        )
        .expect("catalog builds");
        write_catalog(root, &accepted);
    });
    let (_report, program) = check_with_accepted(&root);
    let store = TreeStore::memory();
    seed_catalog_member(
        &store,
        &settings_store_id,
        &[],
        &settings_member_id,
        Scalar::Str("dark".into()),
    );

    let result = witness(&program, &store);

    match verdict_for(&result, &settings_store_id) {
        Verdict::DestructiveDecisionRequired { populated } => assert_eq!(*populated, 1),
        other => panic!("expected destructive decision for store retire, got {other:#?}"),
    }
    assert!(
        matches!(verdict_for(&result, &settings_member_id), Verdict::NoOp),
        "the retired store owns the single destructive fence: {:#?}",
        result.verdicts
    );
}

#[test]
fn dropped_populated_store_without_accepted_key_shape_fails_closed() {
    let settings_store_id = hex_id(5);
    let settings_member_id = hex_id(6);
    let root = temp_project("discharge-drop-populated-unknown-shape-store", |root| {
        write(
            root,
            "src/library.mw",
            "module library\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             pub fn add(name: string): Id(^authors)\n\
             \x20   return nextId(^authors)\n",
        );
        let accepted = marrow_catalog::CatalogMetadata::new(
            20,
            vec![
                entry(CatalogEntryKind::Resource, "library::Author", &hex_id(1)),
                store_entry("library::^authors", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Author::name",
                    &hex_id(3),
                ),
                entry(CatalogEntryKind::Resource, "library::Settings", &hex_id(4)),
                entry(
                    CatalogEntryKind::Store,
                    "library::^settings",
                    &settings_store_id,
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Settings::theme",
                    &settings_member_id,
                ),
            ],
        )
        .expect("catalog builds");
        write_catalog(root, &accepted);
    });
    let (_report, program) = check_with_accepted(&root);
    let store = TreeStore::memory();
    seed_catalog_record(&store, &settings_store_id, &[]);

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &settings_store_id,
        RepairReason::PopulatedDropRequiresRetire,
    );
}

/// Dropping a whole resource whose store holds no records is a free no-op: there is no data
/// to orphan, so the carve-out keeps the empty store activatable, consistent with the
/// empty-member-drop case.
#[test]
fn dropped_empty_store_is_a_free_no_op() {
    let book_store_id = hex_id(5);
    let book_member_id = hex_id(6);
    let root = temp_project("discharge-drop-empty-store", |root| {
        write(
            root,
            "src/library.mw",
            "module library\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             pub fn add(name: string): Id(^authors)\n\
             \x20   return nextId(^authors)\n",
        );
        let accepted = marrow_catalog::CatalogMetadata::new(
            20,
            vec![
                entry(CatalogEntryKind::Resource, "library::Author", &hex_id(1)),
                store_entry("library::^authors", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Author::name",
                    &hex_id(3),
                ),
                entry(CatalogEntryKind::Resource, "library::Book", &hex_id(4)),
                store_entry("library::^books", &book_store_id, "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "library::Book::title",
                    &book_member_id,
                ),
            ],
        )
        .expect("catalog builds");
        write_catalog(root, &accepted);
    });
    let (_report, program) = check_with_accepted(&root);
    // The `Book` store was never seeded, so it holds no records to orphan.
    let store = TreeStore::memory();

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        matches!(verdict_for(&result, &book_store_id), Verdict::NoOp),
        "an empty dropped store stays a no-op: {:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == book_store_id),
        "no fence on a drop with no data to lose: {diagnostics:#?}"
    );
}

/// A dropped source field a unique index still reads is not a silent deprecation;
/// discharge requires a retire intent. The accepted catalog keeps a member `isbn`
/// and an index `byIsbn(isbn)`; current source drops the member but keeps the index,
/// so the proposal carries the lingering member with the index still reading it.
#[test]
fn dropped_field_an_index_needs_requires_retire() {
    let isbn_id = hex_id(4);
    let root = temp_project("discharge-dropped-field-needs-retire", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required isbn: string\n\
             store ^books(id: int): Book\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(title: string, isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            12,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::isbn",
                    &isbn_id,
                ),
                store_index_entry(
                    "books::^books::byIsbn",
                    &hex_id(5),
                    &format!("unique=true;keys=[member:{isbn_id}:string]"),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    // The accepted catalog already matches source, so this first check is clean and
    // binds `isbn`. Now drop the `isbn` member from source while keeping the index,
    // so the index reads a member current source no longer declares.
    checked(&root).expect("checked fixture");
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         \x20   index byIsbn(isbn) unique\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let (_report, program) = check_with_accepted(&root);
    let store = TreeStore::memory();
    let (result, diagnostics) = preview(&program, &store).expect("preview");

    // The dropped member an index still reads is its own typed failure: a retire is
    // required, distinct from a plain missing-required-member repair. The verdict
    // carries the index's catalog identity (its accepted stable id), prose-free; the
    // developer-facing name surfaces only in the diagnostic.
    match verdict_for(&result, &isbn_id) {
        Verdict::RepairRequired {
            reason: RepairReason::RetireRequired { index },
        } => assert_eq!(index.as_str(), hex_id(5)),
        other => panic!("expected RetireRequired, got {other:#?}"),
    }
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == isbn_id),
        "{diagnostics:#?}"
    );
}

#[test]
fn dropped_field_ignores_same_named_index_on_another_resource() {
    let book_subtitle_id = hex_id(5);
    let root = temp_project("discharge-dropped-field-index-owner", |root| {
        write(
            root,
            "src/media.mw",
            "module media\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             resource Movie\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             store ^movies(id: int): Movie\n\
             \x20   index bySubtitle(subtitle) unique\n",
        );
        let accepted = accepted_catalog(
            12,
            "media::Book",
            "media::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Book::subtitle",
                    &book_subtitle_id,
                ),
                entry(CatalogEntryKind::Resource, "media::Movie", &hex_id(6)),
                entry(CatalogEntryKind::Store, "media::^movies", &hex_id(7)),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Movie::title",
                    &hex_id(8),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Movie::subtitle",
                    &hex_id(9),
                ),
                store_index_entry(
                    "media::^movies::bySubtitle",
                    &hex_id(10),
                    &format!("unique=true;keys=[member:{}:string]", hex_id(9)),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    checked(&root).expect("checked fixture");
    write(
        &root,
        "src/media.mw",
        "module media\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         resource Movie\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^movies(id: int): Movie\n\
         \x20   index bySubtitle(subtitle) unique\n",
    );
    let (_report, program) = check_with_accepted(&root);
    let store = TreeStore::memory();
    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        matches!(verdict_for(&result, &book_subtitle_id), Verdict::NoOp),
        "{result:#?}"
    );
    // Dropping `Book.subtitle` must not be blamed on `Movie`'s `bySubtitle` index: no
    // diagnostic carries that index's catalog id (`hex_id(10)`).
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == hex_id(10)),
        "{diagnostics:#?}"
    );
}

/// Dropping a source index while keeping the member it covered is an index-subtree
/// deletion, not a silent no-op: the index binding is gone but its cells would linger.
/// The accepted catalog carries a member `isbn` and an index `byIsbn(isbn)`; current
/// source keeps `isbn` and drops the index, so discharge classifies the dropped index
/// id as `IndexDropped` and tags it as a changed index id apply deletes.
#[test]
fn dropped_index_is_index_dropped() {
    let index_id = hex_id(5);
    let root = temp_project("discharge-drop-index", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required isbn: string\n\
             store ^books(id: int): Book\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(title: string, isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            13,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::isbn",
                    &hex_id(4),
                ),
                store_index_entry(
                    "books::^books::byIsbn",
                    &index_id,
                    &format!("unique=true;keys=[member:{}:string]", hex_id(4)),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    // The accepted catalog matches source, so this first check is clean. Then drop the
    // index from source while keeping `isbn`, so only the index binding disappears.
    checked(&root).expect("checked fixture");
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required isbn: string\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string, isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let (_report, program) = check_with_accepted(&root);
    let store = TreeStore::memory();
    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        matches!(verdict_for(&result, &index_id), Verdict::IndexDropped),
        "{:#?}",
        result.verdicts
    );
    assert!(
        result
            .changed_index_catalog_ids
            .iter()
            .any(|id| id.as_str() == index_id),
        "dropped index id is tagged as a changed index id: {:#?}",
        result.changed_index_catalog_ids
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    // A source-only index drop must build a proposal that advances the epoch and drops the
    // index catalog entry; without it apply would stamp the same epoch and leave the index
    // active in the catalog forever, the heart of the silent index no-op.
    let proposal = program
        .catalog
        .proposal
        .as_ref()
        .expect("a source-dropped index builds a proposal");
    assert_eq!(proposal.epoch, 14, "the dropped index advances the epoch");
    assert!(
        !proposal
            .entries
            .iter()
            .any(|entry| entry.stable_id == index_id),
        "the dropped index entry is gone from the published catalog: {:#?}",
        proposal.entries
    );
}
