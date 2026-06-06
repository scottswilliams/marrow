use marrow_store::value::Scalar;

mod support;
mod support_evolve;

use support::{marrow, write};
use support_evolve::{
    REQUIRED_DEFAULT_SOURCE, commit_catalog, member_catalog_id, native_books_project,
    open_native_store, root_place, seed_member, seed_title_only,
};

#[test]
fn evolve_preview_reports_the_exact_witness_counts() {
    let root = native_books_project("evolve-preview-default", REQUIRED_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&["evolve", "preview", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("status: activatable"), "{stdout}");
    assert!(stdout.contains("records to backfill: 1"), "{stdout}");
    assert!(stdout.contains("source digest:"), "{stdout}");
    assert!(stdout.contains("accepted epoch:"), "{stdout}");
}

#[test]
fn evolve_preview_reports_destructive_approval_requirement() {
    let root = native_books_project(
        "evolve-preview-retire",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("Appendix".into()),
        );
    }
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    let text = marrow(&["evolve", "preview", root.to_str().unwrap()]);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    let stderr = String::from_utf8(text.stderr).expect("stderr");
    assert!(stderr.contains("evolve.approval_required"), "{stderr}");
    assert!(stderr.contains("--approve-retire"), "{stderr}");

    let json = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);

    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let value = support::json(json.stdout);
    assert_eq!(value["status"], "blocked");
    let blocking = value["blocking"].as_array().expect("blocking reports");
    let report = blocking
        .iter()
        .find(|report| report["code"] == serde_json::json!("evolve.approval_required"))
        .unwrap_or_else(|| panic!("{value:#?}"));
    assert_eq!(
        report["data"]["catalog_id"],
        serde_json::json!(member_catalog_id(&accepted_place, "subtitle"))
    );
    assert_eq!(report["data"]["populated"], serde_json::json!(1));
}
