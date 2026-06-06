mod support;
mod support_evolve;

use support::marrow;
use support_evolve::{
    REQUIRED_NO_DEFAULT_SOURCE, commit_catalog, member_catalog_id, native_books_project,
    open_native_store, root_place, seed_title_only,
};

#[test]
fn check_data_reports_repair_required_from_attached_store() {
    let root = native_books_project("check-data-repair", REQUIRED_NO_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&[
        "check",
        "--data",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["code"], "evolve.repair_required");
    assert_eq!(
        record["data"]["catalog_id"],
        serde_json::json!(member_catalog_id(&place, "pages"))
    );
}
