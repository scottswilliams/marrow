mod support;

use marrow_catalog::{CatalogEntry, CatalogEntryKind};
use marrow_check::{ScalarType, StoreIndexKeySource, StoredValueMeaning};

use support::catalog::{catalog, derived_id, entry as literal_entry, write_catalog};
use support::{check_with_accepted, temp_project, write};

/// A catalog entry whose stable id is minted deterministically from `label`, so a
/// fixture refers to a member by a readable name and still gets a `cat_`-shaped id the
/// catalog parser accepts.
fn entry(
    kind: CatalogEntryKind,
    canonical_path: &str,
    label: &str,
    aliases: &[&str],
) -> CatalogEntry {
    literal_entry(kind, canonical_path, &derived_id(label), aliases)
}

fn sorted_enum_member_catalog_ids(
    facts: &marrow_check::CheckedFacts,
    members: &[marrow_check::EnumMemberId],
) -> Vec<String> {
    let mut ids: Vec<String> = members
        .iter()
        .map(|id| {
            facts
                .enum_members()
                .iter()
                .find(|member| member.id == *id)
                .expect("enum member")
                .catalog_id
                .clone()
                .expect("accepted enum member catalog id")
        })
        .collect();
    ids.sort();
    ids
}

#[test]
fn enum_member_facts_use_catalog_ids_independent_of_source_order() {
    let root = temp_project("catalog-enum-order", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   archived\n\
             \x20   active\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Enum, "books::Status", "enum-status", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::active",
                "enum-member-active",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::archived",
                "enum-member-archived",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("books").expect("module");
    let status = program.facts.enum_id(module, "Status").expect("enum");
    let active = program
        .facts
        .enum_members()
        .iter()
        .find(|member| member.enum_id == status && member.name == "active")
        .expect("active");
    let archived = program
        .facts
        .enum_members()
        .iter()
        .find(|member| member.enum_id == status && member.name == "archived")
        .expect("archived");
    assert_eq!(
        active.catalog_id.as_deref(),
        Some(derived_id("enum-member-active").as_str())
    );
    assert_eq!(
        archived.catalog_id.as_deref(),
        Some(derived_id("enum-member-archived").as_str())
    );
}

#[test]
fn enum_field_value_meaning_uses_catalog_member_identity_after_source_reorder() {
    let root = temp_project("catalog-enum-value-meaning", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   archived\n\
             \x20   active\n\
             resource Order\n\
             \x20   state: Status\n\
             store ^orders(id: int): Order\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Enum, "books::Status", "enum-status", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::active",
                "enum-member-active",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::archived",
                "enum-member-archived",
                &[],
            ),
            entry(CatalogEntryKind::Resource, "books::Order", "res-order", &[]),
            entry(
                CatalogEntryKind::Store,
                "books::^orders",
                "store-orders",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Order::state",
                "member-state",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("books").expect("module");
    let status = program.facts.enum_id(module, "Status").expect("enum");
    let order = program
        .facts
        .resource_id(module, "Order")
        .expect("resource");
    let state = program
        .facts
        .resource_members()
        .iter()
        .find(|member| member.resource == order && member.name == "state")
        .expect("state member");

    let Some(StoredValueMeaning::Enum { enum_id, members }) = &state.value_meaning else {
        panic!("state should store by enum member identity: {state:#?}");
    };
    assert_eq!(*enum_id, status);
    let catalog_ids = sorted_enum_member_catalog_ids(&program.facts, members);
    assert_eq!(
        catalog_ids,
        [
            derived_id("enum-member-active"),
            derived_id("enum-member-archived")
        ]
    );
}

#[test]
fn enum_index_key_meaning_uses_catalog_member_identity_after_source_reorder() {
    let root = temp_project("catalog-enum-index-meaning", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   archived\n\
             \x20   active\n\
             resource Order\n\
             \x20   state: Status\n\
             store ^orders(id: int): Order\n\
             \x20   index byState(state, id)\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Enum, "books::Status", "enum-status", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::active",
                "enum-member-active",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::archived",
                "enum-member-archived",
                &[],
            ),
            entry(CatalogEntryKind::Resource, "books::Order", "res-order", &[]),
            entry(
                CatalogEntryKind::Store,
                "books::^orders",
                "store-orders",
                &[],
            ),
            entry(
                CatalogEntryKind::StoreIndex,
                "books::^orders::byState",
                "index-by-state",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Order::state",
                "member-state",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("books").expect("module");
    let status = program.facts.enum_id(module, "Status").expect("enum");
    let order = program
        .facts
        .resource_id(module, "Order")
        .expect("resource");
    let state = program
        .facts
        .resource_members()
        .iter()
        .find(|member| member.resource == order && member.name == "state")
        .expect("state member");
    let index = program
        .facts
        .store_indexes()
        .iter()
        .find(|index| index.name == "byState")
        .expect("byState index");
    let key = index
        .keys
        .iter()
        .find(|key| key.name == "state")
        .expect("state key");

    assert_eq!(key.source, StoreIndexKeySource::ResourceMember(state.id));
    let StoredValueMeaning::Enum { enum_id, members } = &key.value_meaning else {
        panic!("state key should store by enum member identity: {key:#?}");
    };
    assert_eq!(*enum_id, status);
    let catalog_ids = sorted_enum_member_catalog_ids(&program.facts, members);
    assert_eq!(
        catalog_ids,
        [
            derived_id("enum-member-active"),
            derived_id("enum-member-archived")
        ]
    );
}

#[test]
fn enum_field_value_meaning_fails_closed_for_unresolved_bare_enum_names() {
    let root = temp_project("catalog-enum-value-meaning-fail-closed", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             enum Status\n\
             \x20   active\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             resource Order\n\
             \x20   label: string\n\
             \x20   state: Status\n\
             store ^orders(id: int): Order\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Enum, "a::Status", "enum-status", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "a::Status::active",
                "enum-member-active",
                &[],
            ),
            entry(CatalogEntryKind::Resource, "b::Order", "res-order", &[]),
            entry(CatalogEntryKind::Store, "b::^orders", "store-orders", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "b::Order::label",
                "member-label",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "b::Order::state",
                "member-state",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (_report, program) = check_with_accepted(&root);

    let module = program.facts.module_id("b").expect("module");
    let order = program
        .facts
        .resource_id(module, "Order")
        .expect("resource");
    let member = |name: &str| {
        program
            .facts
            .resource_members()
            .iter()
            .find(|member| member.resource == order && member.name == name)
            .unwrap_or_else(|| panic!("{name} member"))
    };

    // A resolvable scalar field in the same resource still records its value
    // meaning, so the unresolved enum field below is failing closed in isolation
    // rather than the checker blanket-dropping the module's value meanings.
    assert_eq!(
        member("label").value_meaning,
        Some(StoredValueMeaning::Scalar(ScalarType::Str)),
        "{:#?}",
        member("label")
    );
    assert_eq!(
        member("state").value_meaning,
        None,
        "{:#?}",
        member("state")
    );
}
