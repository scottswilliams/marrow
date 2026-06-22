use crate::support;
use marrow_catalog::CatalogEntryKind;
use marrow_store::tree::TreeStore;
use serde_json::{Value, json};
use support::{marrow_sub, temp_project, temp_project_uncommitted, write};

fn catalog_id(root: &support::TempProject, kind: CatalogEntryKind, path: &str) -> String {
    let store_path = root.join(".data").join("marrow.redb");
    let store = TreeStore::open_read_only(&store_path).expect("open store read-only");
    let catalog = store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
        .expect("project has an accepted catalog");
    catalog
        .entries
        .iter()
        .find(|entry| entry.kind == kind && entry.path == path)
        .unwrap_or_else(|| panic!("catalog entry {kind:?} {path}"))
        .stable_id
        .clone()
}

fn footprint_identities(report: &Value) -> Vec<&Value> {
    report["entry_footprints"]
        .as_array()
        .expect("entry footprints array")
        .iter()
        .flat_map(|entry| {
            ["stores_read", "stores_written", "indexes_touched"]
                .into_iter()
                .flat_map(move |field| entry[field].as_array().expect("identity array"))
        })
        .collect()
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

const FOOTPRINT_APP: &str = "module app\n\
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
     \x20   return count(^books.byShelf(shelf))\n";

fn write_footprint_project(root: &std::path::Path) {
    write(
        root,
        "marrow.json",
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
    );
    write(root, "src/app.mw", FOOTPRINT_APP);
}

#[test]
fn check_json_reports_entry_footprints_with_structural_paths() {
    let root = temp_project("check-json-entry-footprints", write_footprint_project);
    let store_path = "app::^books";
    let index_path = "app::^books::byShelf";

    let output = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = support::json(output.stdout);
    let save = entry(&report, "app::save");
    assert_eq!(save["write_effects_reachable"], true);
    assert_eq!(save["stores_read"], serde_json::json!([]));
    assert_eq!(save["stores_written"], serde_json::json!([store_path]));
    assert_eq!(save["indexes_touched"], serde_json::json!([index_path]));
    assert_eq!(save["work_shape"], "writes_saved_data");

    let read = entry(&report, "app::countOnShelf");
    assert_eq!(read["write_effects_reachable"], false);
    assert_eq!(read["stores_read"], serde_json::json!([store_path]));
    assert_eq!(read["stores_written"], serde_json::json!([]));
    assert_eq!(read["indexes_touched"], serde_json::json!([index_path]));
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
fn check_json_entry_footprint_identities_are_deterministic_when_unfrozen() {
    let root = temp_project_uncommitted(
        "check-json-entry-footprints-unfrozen-deterministic",
        write_footprint_project,
    );

    let first = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let second = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(0), "{second:?}");

    let first_report = support::json(first.stdout);
    let second_report = support::json(second.stdout);

    assert_eq!(
        footprint_identities(&first_report),
        footprint_identities(&second_report),
        "two identical unfrozen checks must emit byte-identical footprint identities"
    );
}

#[test]
fn check_json_unfrozen_footprint_identities_are_structural_paths() {
    let root = temp_project_uncommitted(
        "check-json-entry-footprints-unfrozen-paths",
        write_footprint_project,
    );

    let output = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = support::json(output.stdout);

    let save = entry(&report, "app::save");
    assert_eq!(save["stores_written"], serde_json::json!(["app::^books"]));
    assert_eq!(
        save["indexes_touched"],
        serde_json::json!(["app::^books::byShelf"])
    );

    let read = entry(&report, "app::countOnShelf");
    assert_eq!(read["stores_read"], serde_json::json!(["app::^books"]));
    assert_eq!(
        read["indexes_touched"],
        serde_json::json!(["app::^books::byShelf"])
    );
}

#[test]
fn check_json_footprint_access_graph_matches_across_frozen_and_unfrozen() {
    let frozen = temp_project(
        "check-json-entry-footprints-frozen-graph",
        write_footprint_project,
    );
    let unfrozen = temp_project_uncommitted(
        "check-json-entry-footprints-unfrozen-graph",
        write_footprint_project,
    );

    let frozen_out = marrow_sub("check", &["--format", "json", frozen.to_str().unwrap()]);
    assert_eq!(frozen_out.status.code(), Some(0), "{frozen_out:?}");
    let unfrozen_out = marrow_sub("check", &["--format", "json", unfrozen.to_str().unwrap()]);
    assert_eq!(unfrozen_out.status.code(), Some(0), "{unfrozen_out:?}");

    let frozen_report = support::json(frozen_out.stdout);
    let unfrozen_report = support::json(unfrozen_out.stdout);

    assert_eq!(
        frozen_report["entry_footprints"], unfrozen_report["entry_footprints"],
        "structural-path footprints describe the same access graph before and after freeze"
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
                    "render_name": "Status",
                    "enum_catalog_id": status_id,
                    "members": [
                        { "render_label": "draft", "catalog_id": draft_id },
                        { "render_label": "published", "catalog_id": published_id }
                    ]
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
fn check_json_surface_abi_exports_create_descriptor() {
    let root = temp_project("check-json-surface-abi-create-descriptor", |root| {
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
    assert_eq!(
        books["create"]["profile_version"], "surface.create.v1",
        "{books:#?}"
    );
    assert_eq!(
        books["create"]["kind"],
        json!({ "kind": "point_create" }),
        "{books:#?}"
    );
    assert_eq!(
        books["create"]["body_semantics"], "exact_declared_body",
        "{books:#?}"
    );
    assert_eq!(
        books["create"]["existence_semantics"], "reject_existing_no_replace",
        "{books:#?}"
    );
    assert_eq!(
        books["create"]["fields"][0]["render_label"], "title",
        "{books:#?}"
    );
    assert!(books.get("update").is_some(), "{books:#?}");
}

#[test]
fn check_json_reports_surface_route_manifest() {
    let root = temp_project("check-json-surface-routes", |root| {
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
             \x20   shelf: string\n\
             store ^books(id: int): Book\n\
             \x20   index byShelf(shelf, id)\n\
             surface Books from ^books\n\
             \x20   fields title, shelf\n\
             \x20   create title, shelf\n\
             \x20   update shelf\n\
             \x20   collection ^books.byShelf as byShelf\n",
        );
    });

    let output = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = support::json(output.stdout);
    let routes = report["surface_routes"]["routes"]
        .as_array()
        .expect("surface route manifest routes");
    assert_eq!(
        report["surface_routes"]["profile_version"],
        "surface.route.v1"
    );
    assert_eq!(
        report["surface_routes"]["operation_profile_version"],
        "surface.operation.v1"
    );
    assert_eq!(
        routes
            .iter()
            .map(|route| route["alias"].as_str().expect("route alias"))
            .collect::<Vec<_>>(),
        vec!["get", "byShelf", "create", "update"]
    );
    assert_eq!(
        routes
            .iter()
            .map(|route| route["request"]["kind"].as_str().expect("request kind"))
            .collect::<Vec<_>>(),
        vec!["point_read", "page", "point_create", "point_update"]
    );
    assert!(
        routes.iter().all(|route| route.get("delete").is_none()
            && route["method"] == "POST"
            && route["path"]
                .as_str()
                .expect("route path")
                .contains(route["operation_tag"].as_str().expect("operation tag"))),
        "routes stay descriptor-derived and omit absent delete operations: {routes:#?}"
    );
    assert!(
        routes.iter().any(|route| route["path"]
            .as_str()
            .expect("route path")
            .starts_with("/surface/v1/create/")),
        "create route uses its operation-family prefix: {routes:#?}"
    );
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
    assert!(
        report.get("surface_routes").is_none(),
        "failed checks must not publish partial surface routes: {report:#?}"
    );
}
