mod support;
mod support_evolve;

use support::marrow;
use support_evolve::{REQUIRED_DEFAULT_SOURCE, native_books_project};

#[test]
fn legacy_evolution_subcommands_are_absent() {
    let root = native_books_project("evolve-legacy", REQUIRED_DEFAULT_SOURCE);

    let output = marrow(&["evolve", "migrate", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown evolve subcommand"), "{stderr}");
    assert!(
        stderr.contains("preview") && stderr.contains("apply"),
        "{stderr}"
    );
}
