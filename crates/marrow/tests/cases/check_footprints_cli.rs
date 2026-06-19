use crate::support;
use marrow_catalog::CatalogEntryKind;
use serde_json::{Value, json};
use support::{marrow_sub, temp_project, temp_project_uncommitted, write};

fn catalog_id(root: &support::TempProject, kind: CatalogEntryKind, path: &str) -> String {
    let catalog = marrow_catalog::CatalogMetadata::from_json(
        &std::fs::read_to_string(root.join("marrow.catalog.json")).expect("read catalog"),
    )
    .expect("catalog parses");
    catalog
        .entries
        .iter()
        .find(|entry| entry.kind == kind && entry.path == path)
        .unwrap_or_else(|| panic!("catalog entry {kind:?} {path}"))
        .stable_id
        .clone()
}

fn entry<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["entry_footprints"]
        .as_array()
        .expect("entry footprints array")
        .iter()
        .find(|entry| entry["entry"] == name)
        .unwrap_or_else(|| panic!("entry footprint {name} in {report:#?}"))
}

fn surface<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["surface_abi"]["surfaces"]
        .as_array()
        .expect("surface ABI surface array")
        .iter()
        .find(|surface| surface["name"] == name)
        .unwrap_or_else(|| panic!("surface ABI descriptor {name} in {report:#?}"))
}

#[test]
fn check_json_reports_entry_footprints_with_catalog_ids() {
    let root = temp_project("check-json-entry-footprints", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   shelf: string\n\
             store ^books(id: int): Book\n\
             \x20   index byTitle(title, id)\n\
             \x20   index byShelf(shelf, id)\n\
             fn writeShelf(id: int, shelf: string)\n\
             \x20   ^books(id).shelf = shelf\n\
             pub fn save(id: int, shelf: string)\n\
             \x20   writeShelf(id, shelf)\n\
             pub fn countOnShelf(shelf: string): int\n\
             \x20   return count(^books.byShelf(shelf))\n",
        );
    });
    let store_id = catalog_id(&root, CatalogEntryKind::Store, "app::^books");
    let index_id = catalog_id(&root, CatalogEntryKind::StoreIndex, "app::^books::byShelf");

    let output = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = support::json(output.stdout);
    let save = entry(&report, "app::save");
    assert_eq!(save["write_effects_reachable"], true);
    assert_eq!(save["stores_read"], serde_json::json!([]));
    assert_eq!(save["stores_written"], serde_json::json!([store_id]));
    assert_eq!(save["indexes_touched"], serde_json::json!([index_id]));
    assert_eq!(save["work_shape"], "writes_saved_data");

    let read = entry(&report, "app::countOnShelf");
    assert_eq!(read["write_effects_reachable"], false);
    assert_eq!(read["stores_read"], serde_json::json!([store_id]));
    assert_eq!(read["stores_written"], serde_json::json!([]));
    assert_eq!(read["indexes_touched"], serde_json::json!([index_id]));
    assert_eq!(read["work_shape"], "read_only");

    assert!(
        report["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .is_empty(),
        "{report:#?}"
    );
}

#[test]
fn check_json_reports_surface_abi_read_and_update_descriptors() {
    let root = temp_project("check-json-surface-abi", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             enum Status\n\
             \x20   draft\n\
             \x20   published\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   status: Status\n\
             \x20   author: Id(^authors)\n\
             store ^books(shelf: string, id: int): Book\n\
             \x20   index byStatus(status, shelf, id)\n\
             surface Books from ^books\n\
             \x20   fields title, status, author\n\
             \x20   update status, author\n\
             \x20   collection ^books.byStatus as byStatus\n",
        );
    });
    let book_id = catalog_id(&root, CatalogEntryKind::Resource, "app::Book");
    let books_id = catalog_id(&root, CatalogEntryKind::Store, "app::^books");
    let authors_id = catalog_id(&root, CatalogEntryKind::Store, "app::^authors");
    let status_id = catalog_id(&root, CatalogEntryKind::Enum, "app::Status");
    let draft_id = catalog_id(&root, CatalogEntryKind::EnumMember, "app::Status::draft");
    let published_id = catalog_id(
        &root,
        CatalogEntryKind::EnumMember,
        "app::Status::published",
    );
    let status_member_id = catalog_id(&root, CatalogEntryKind::ResourceMember, "app::Book::status");
    let author_member_id = catalog_id(&root, CatalogEntryKind::ResourceMember, "app::Book::author");
    let by_status_id = catalog_id(&root, CatalogEntryKind::StoreIndex, "app::^books::byStatus");

    let output = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = support::json(output.stdout);
    let books = surface(&report, "Books");
    assert_eq!(books["module"], "app");
    assert_eq!(books["catalog_status"], json!({ "kind": "stable" }));

    let reads = books["read"].as_array().expect("read descriptors");
    let point = reads
        .iter()
        .find(|descriptor| descriptor["kind"]["kind"] == "point_read")
        .expect("point read descriptor");
    assert_eq!(point["profile_version"], "surface.read.v1");
    assert!(
        point["operation_tag"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(point["store_catalog_id"], books_id);
    assert_eq!(point["resource_catalog_id"], book_id);
    assert_eq!(
        point["identity_keys"],
        json!([
            { "render_label": "shelf", "value": { "kind": "scalar", "scalar": "string" } },
            { "render_label": "id", "value": { "kind": "scalar", "scalar": "int" } }
        ])
    );
    let paged = reads
        .iter()
        .find(|descriptor| descriptor["kind"]["kind"] == "paged_index_collection")
        .expect("paged index descriptor");
    assert_eq!(
        paged["kind"],
        json!({
            "kind": "paged_index_collection",
            "index_catalog_id": by_status_id,
            "exact_key_count": 1,
            "identity_key_count": 2
        })
    );

    let update = books["update"].as_object().expect("update descriptor");
    assert_eq!(update["profile_version"], "surface.update.v1");
    assert!(
        update["operation_tag"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(update["kind"], json!({ "kind": "point_update" }));
    assert_eq!(update["patch_semantics"], "non_empty_patch");
    assert_eq!(update["store_catalog_id"], books_id);
    assert_eq!(update["resource_catalog_id"], book_id);
    let mut expected_update_fields = vec![
        json!(
            {
                "render_label": "status",
                "member_catalog_id": status_member_id,
                "backing_required": false,
                "value": {
                    "kind": "enum",
                    "enum_catalog_id": status_id,
                    "member_catalog_ids": [draft_id, published_id]
                }
            }
        ),
        json!(
            {
                "render_label": "author",
                "member_catalog_id": author_member_id,
                "backing_required": false,
                "value": {
                    "kind": "identity",
                    "store_catalog_id": authors_id,
                    "arity": 1,
                    "key_scalars": ["int"]
                }
            }
        ),
    ];
    expected_update_fields.sort_by(|left, right| {
        left["member_catalog_id"]
            .as_str()
            .expect("left member id")
            .cmp(
                right["member_catalog_id"]
                    .as_str()
                    .expect("right member id"),
            )
    });
    assert_eq!(update["fields"], json!(expected_update_fields));
}

#[test]
fn check_json_reports_source_only_surface_blockers_without_descriptors() {
    let root = temp_project_uncommitted("check-json-surface-abi-source-only", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             surface Books from ^books\n\
             \x20   fields title\n\
             \x20   update title\n",
        );
    });

    let output = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = support::json(output.stdout);
    let books = surface(&report, "Books");
    assert_eq!(
        books["catalog_status"],
        json!({
            "kind": "source_only",
            "blockers": ["pending_catalog_proposal", "missing_accepted_catalog_ids"]
        })
    );
    assert!(
        books["read"]
            .as_array()
            .expect("read descriptors")
            .is_empty(),
        "{books:#?}"
    );
    assert!(
        books.get("update").is_none(),
        "source-only update descriptor must be suppressed: {books:#?}"
    );
}

#[test]
fn check_jsonl_summary_reports_surface_abi() {
    let root = temp_project("check-jsonl-surface-abi", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Settings\n\
             \x20   required theme: string\n\
             \x20   mode: string\n\
             store ^settings: Settings\n\
             surface SettingsSurface from ^settings\n\
             \x20   fields theme, mode\n\
             \x20   update mode\n",
        );
    });

    let output = marrow_sub("check", &["--format", "jsonl", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let records = support::jsonl(output.stdout);
    let summary = records.last().expect("summary record");
    assert_eq!(summary["kind"], "summary");
    let surface = surface(summary, "SettingsSurface");
    assert_eq!(surface["module"], "app");
    assert_eq!(
        surface["update"]["kind"],
        json!({ "kind": "singleton_update" })
    );
}

#[test]
fn check_json_surface_abi_ordering_and_descriptor_presence_are_deterministic() {
    let root = temp_project("check-json-surface-abi-ordering", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource BetaResource\n\
             \x20   name: string\n\
             store ^beta(id: int): BetaResource\n\
             surface Beta from ^beta\n\
             \x20   fields name\n\
             \x20   update name\n\
             resource AlphaResource\n\
             \x20   name: string\n\
             store ^alpha(id: int): AlphaResource\n\
             surface Alpha from ^alpha\n\
             \x20   fields name\n",
        );
    });

    let output = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = support::json(output.stdout);
    let surfaces = report["surface_abi"]["surfaces"]
        .as_array()
        .expect("surfaces array");
    assert_eq!(
        surfaces
            .iter()
            .map(|surface| surface["name"].as_str().expect("surface name"))
            .collect::<Vec<_>>(),
        vec!["Alpha", "Beta"]
    );
    assert!(
        surfaces[0].get("update").is_none(),
        "stable surface without update fields has no update descriptor: {surfaces:#?}"
    );
    assert_eq!(
        surfaces[1]["update"]["kind"],
        json!({ "kind": "point_update" })
    );
}

#[test]
fn check_json_surface_abi_excludes_reserved_create_metadata() {
    let root = temp_project("check-json-surface-abi-create-reserved", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             surface Books from ^books\n\
             \x20   fields title\n\
             \x20   create title\n\
             \x20   update title\n",
        );
    });

    let output = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = support::json(output.stdout);
    let books = surface(&report, "Books");
    assert!(
        books.get("create").is_none(),
        "create is parsed/resolved reserved metadata, not a serialized ABI descriptor: {books:#?}"
    );
    assert!(books.get("update").is_some(), "{books:#?}");
}

#[test]
fn failed_check_json_suppresses_entry_footprints() {
    let root = temp_project("check-json-failed-no-entry-footprints", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn writeTitle(id: int, title: string)\n\
             \x20   ^books(id).title = title\n\
             pub fn save(id: int, title: string)\n\
             \x20   missing()\n\
             \x20   writeTitle(id, title)\n",
        );
    });

    let output = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = support::json(output.stdout);
    assert_eq!(report["status"], "failed");
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["code"] == "check.unresolved_call"),
        "{report:#?}"
    );
    assert!(
        report.get("entry_footprints").is_none(),
        "failed checks must not publish partial static footprints: {report:#?}"
    );
    assert!(
        report.get("surface_abi").is_none(),
        "failed checks must not publish partial surface ABI descriptors: {report:#?}"
    );
}
