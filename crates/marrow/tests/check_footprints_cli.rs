use marrow_catalog::CatalogEntryKind;
use serde_json::Value;

mod support;

use support::{marrow_sub, temp_project, write};

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
fn failed_check_json_suppresses_entry_footprints() {
    let root = temp_project("check-json-failed-no-entry-footprints", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
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
}
