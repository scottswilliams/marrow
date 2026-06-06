use std::fs;

mod support;

use support::{TempProject, marrow, marrow_sub, write};

/// A native-store project with a saved root but no committed catalog: checking it
/// proposes durable identity that no flow has frozen yet. Built without committing
/// so the catalog stays pending until a run commits it.
fn pending_native_project(name: &str) -> TempProject {
    support::temp_project_uncommitted(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn main()\n\
             \x20   print(\"ran\")\n",
        );
    })
}

#[test]
fn check_on_a_pending_project_exits_zero_and_writes_no_catalog() {
    let root = pending_native_project("run-pending-check");
    let catalog = root.join("marrow.catalog.json");

    let output = marrow(&["check", root.to_str().unwrap()]);
    let catalog_written = catalog.exists();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        !catalog_written,
        "check is read-only and must not write the catalog"
    );
}

#[test]
fn run_commits_the_pending_catalog_transparently() {
    let root = pending_native_project("run-pending-commit");
    let catalog = root.join("marrow.catalog.json");
    assert!(
        !catalog.exists(),
        "fixture starts with no committed catalog"
    );

    let output = marrow_sub("run", &[root.to_str().unwrap()]);
    let committed = fs::read_to_string(&catalog).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "ran\n");
    let committed = committed.expect("run must commit the pending catalog");
    assert!(committed.contains("\"epoch\": 1"), "{committed}");
}

#[test]
fn a_second_run_on_an_accepted_catalog_does_not_churn_it() {
    let root = pending_native_project("run-accepted-noop");
    let catalog = root.join("marrow.catalog.json");

    let first = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let after_first = fs::read_to_string(&catalog).expect("first run commits the catalog");

    let second = marrow_sub("run", &[root.to_str().unwrap()]);
    let after_second = fs::read_to_string(&catalog).expect("catalog still present");

    assert_eq!(second.status.code(), Some(0), "{second:?}");
    assert_eq!(
        after_first, after_second,
        "a clean accepted catalog must not be rewritten"
    );
}
