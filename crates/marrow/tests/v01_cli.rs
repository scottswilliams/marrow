mod support;

use support::{marrow, temp_project, write};

const LIBRARY_SOURCE: &str = include_str!("../../../fixtures/v01/library.mw");

#[test]
fn v01_library_fixture_checks_and_runs_through_cli() {
    let root = temp_project("v01-library-cli", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(root, "src/v01/library.mw", LIBRARY_SOURCE);
    });
    let dir = root.path().to_str().unwrap().to_string();

    let check = marrow(&["check", &dir]);
    let seed = marrow(&["run", "--entry", "v01::library::seed", &dir]);
    let print_author = marrow(&["run", "--entry", "v01::library::printSeededAuthor", &dir]);
    let print_stdout = std::str::from_utf8(&print_author.stdout).expect("stdout utf8");

    assert_eq!(check.status.code(), Some(0), "check: {check:?}");
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    assert_eq!(
        print_author.status.code(),
        Some(0),
        "print author: {print_author:?}"
    );
    assert_eq!(print_stdout, "Ursula K. Le Guin\n");
}
