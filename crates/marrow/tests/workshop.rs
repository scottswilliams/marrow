//! The Workshop tool-crib catalog fixture drives `marrow test` end to end: its
//! `test "example: ..."` declarations travel the real production path (capture ->
//! compile-with-tests -> verify -> VM) through the built binary and report typed
//! JSONL. Each `test` body reaches durable data only by driving the application's
//! exports, so every call is its own invocation boundary against the test's fresh
//! ephemeral attachment — no raw seeder mints state.
//!
//! This is the source-test half of the E06 evidence: it proves single-invocation
//! invariants (add-then-read, exact replace, descendant-preserving erase, guarded
//! and unguarded sparse sets, unique-index lookup, bounded traversal, the three
//! sparse-presence journeys, and the invocation-boundary isolation of a rejected
//! mutation). The `workshop_e2e` sibling drives the same source in process over one
//! ephemeral attachment for the runtime-fault rollback and the demand facts.

use std::path::PathBuf;
use std::process::Command;

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/v01/conformance/workshop")
}

/// Every `test` block in the catalog fixture passes through `marrow test`, and the
/// run ends with a typed summary that selects and passes all of them with none
/// failed or errored.
#[test]
fn workshop_source_tests_pass_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "marrow test must exit zero when every test passes: {output:?}\n{stdout}"
    );

    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""errored":0"#), "{summary}");
    assert!(summary.contains(r#""total":11"#), "{summary}");
    assert!(summary.contains(r#""passed":11"#), "{summary}");
}
