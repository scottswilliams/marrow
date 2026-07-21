//! `marrow check`: the minimal check surface and the per-export demand sentence.
//!
//! A project travels the real production path through the built binary — capture,
//! the resilient analysis floor, then compile and verify — and each exported function
//! is described by its verifier-reconstructed durable demand in source spelling. The
//! sentence bytes are frozen here: the demand describes which durable places an export
//! reads and writes, and never grants that access.
//!
//! The two-export bookstore is the shared harness's `fixtures/v01/bookstore` fixture;
//! the storeless and broken projects are inline, exercising both authoring styles.

mod common;

use common::Project;

/// The frozen demand report for the bookstore fixture. One line per export, in
/// `module.item` order, each naming its durable places in source spelling: the
/// read-only `lookup` reads the whole entry and the index; the transactional `put`
/// reads the entry (unique-index maintenance) and writes it. An index read renders
/// under `reads`, and a place a writer both reads and writes appears in both clauses.
const BOOKSTORE_REPORT: &str = "bookstore.lookup reads ^books and ^books.byIsbn\n\
     bookstore.put reads ^books; writes ^books\n";

/// The check surface prints the exact per-export demand sentences and exits 0 for a
/// project that checks clean.
#[test]
fn check_describes_each_export_demand_in_source_spelling() {
    let output = Project::from_fixture("bookstore").run_cli("bookstore", &["check"]);
    assert!(
        output.status.success(),
        "check must succeed on a clean project: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), BOOKSTORE_REPORT);
}

/// An explicit project-directory argument checks the same project as the working
/// directory, so the demand report is identical.
#[test]
fn check_accepts_an_explicit_project_directory() {
    let output = Project::from_fixture("bookstore").run_cli("bookstore-arg", &["check", "."]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), BOOKSTORE_REPORT);
}

/// A storeless export touches no durable data, and the sentence says exactly that.
#[test]
fn check_describes_a_storeless_export_as_touching_no_durable_data() {
    let output = Project::single("pub fn answer(): int {\n    return 42\n}\n")
        .run_cli("storeless", &["check"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "main.answer reads or writes no durable data\n"
    );
}

/// A project with a diagnostic reports it with its span and typed code on standard
/// error, writes no demand line, and exits nonzero — the code and span are the
/// contract, not the message prose.
#[test]
fn check_reports_a_diagnostic_with_its_span_and_exits_nonzero() {
    let output = Project::single("pub fn oops(): int {\n    return \"nope\"\n}\n")
        .run_cli("broken", &["check"]);
    assert!(!output.status.success(), "a broken project must fail check");
    assert!(
        output.stdout.is_empty(),
        "a failing check prints no demand report: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("src/main.mw:2:12"), "{stderr}");
    assert!(stderr.contains("check.type"), "{stderr}");
}

/// `check` is a live command, not a refounding stub: it never reports
/// `cli.command_unsupported`.
#[test]
fn check_is_not_a_refounding_command() {
    let output =
        Project::single("pub fn answer(): int {\n    return 1\n}\n").run_cli("live", &["check"]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("cli.command_unsupported"),
        "check must be live: {combined}"
    );
}
