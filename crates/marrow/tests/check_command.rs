//! `marrow check`: the human-shaped default demand summary and the full per-export
//! `--demand` form.
//!
//! A project travels the real production path through the built binary — capture, the
//! resilient analysis floor, then compile and verify. On a clean check the default
//! output summarizes each export's verifier-reconstructed durable demand grouped by
//! module: exports that share an identical demand are listed once, each demand names its
//! roots with a child-place count, and storeless exports collapse to one note per
//! module. `--demand` prints the full per-export sentence form instead — the exact
//! bytes downstream consumers and the deployment-ceiling review read — so both surfaces
//! are frozen here.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::{MARROW_BIN, Project};

/// The full per-export demand form for the bookstore fixture, printed by
/// `marrow check --demand`: one line per export in `module.item` order, each naming every
/// durable place it reads and writes. This is the frozen surface downstream consumers and
/// `marrow image`'s ceiling review depend on; the default summary must never change it.
const BOOKSTORE_DEMAND: &str = "bookstore.lookup reads ^books and ^books.byIsbn\n\
     bookstore.put reads ^books; writes ^books\n";

/// The default summary for the bookstore fixture: the module header, one entry per
/// export, and each demand rolled up to its roots. The read-only `lookup` reads the
/// entry and its one index (rolled to `^books (+1 place)`); `put` reads and writes the
/// whole entry.
const BOOKSTORE_SUMMARY: &str = "2 exports across 1 module\n\
     \n\
     bookstore: 2 exports\n\
     \x20 lookup\n\
     \x20   reads ^books (+1 place)\n\
     \x20 put\n\
     \x20   reads ^books\n\
     \x20   writes ^books\n";

/// The default summary for the `demand_summary` fixture, which exercises every collapse:
/// an all-storeless module folds to its header line, two exports that share a demand are
/// listed once, a root with several touched child places rolls up to a child-place count,
/// and a storeless export in a durable module collapses to one note.
const DEMAND_SUMMARY_REPORT: &str = "6 exports across 2 modules\n\
     \n\
     checks: 2 exports, all storeless\n\
     \n\
     ledger: 4 exports\n\
     \x20 accountBalance\n\
     \x20   reads ^accounts (+1 place)\n\
     \x20 alpha, beta (2 exports, one shared demand)\n\
     \x20   reads ^accounts (+2 places), ^events\n\
     \x20   writes ^accounts (+2 places), ^events\n\
     \x20 storeless: double\n";

/// The default `check` prints the module-grouped demand summary and exits 0 on a clean
/// project.
#[test]
fn check_default_summarizes_demand_grouped_by_module() {
    let output = Project::from_fixture("bookstore").run_cli("bookstore", &["check"]);
    assert!(
        output.success(),
        "check must succeed on a clean project: {}",
        output.stderr_text()
    );
    assert_eq!(output.stdout_text(), BOOKSTORE_SUMMARY);
}

/// An explicit project-directory argument summarizes the same project as the working
/// directory.
#[test]
fn check_accepts_an_explicit_project_directory() {
    let output = Project::from_fixture("bookstore").run_cli("bookstore-arg", &["check", "."]);
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout_text(), BOOKSTORE_SUMMARY);
}

/// `marrow check --demand` prints the full per-export sentence form, byte-for-byte the
/// frozen surface downstream consumers read. Pinning it here guards the default summary
/// from disturbing that surface.
#[test]
fn check_demand_prints_the_full_per_export_sentences() {
    let output =
        Project::from_fixture("bookstore").run_cli("bookstore-demand", &["check", "--demand"]);
    assert!(output.success(), "{}", output.stderr_text());
    assert_eq!(output.stdout_text(), BOOKSTORE_DEMAND);
}

/// The summary collapses an all-storeless module, de-duplicates a shared demand, rolls
/// children up to their root, and collapses a storeless export — the three DX fixes, on
/// one fixture. The frozen snapshot pairs with typed assertions on each collapse so the
/// contract, not only the bytes, is enforced.
#[test]
fn check_summary_dedups_collapses_and_rolls_up() {
    let output = Project::from_fixture("demand_summary").run_cli("demand-summary", &["check"]);
    assert!(output.success(), "{}", output.stderr_text());
    let report = output.stdout_text();
    assert_eq!(report, DEMAND_SUMMARY_REPORT);

    // All-storeless module: one folded header line, and no per-export storeless line —
    // the collapsed export names do not appear at all.
    assert!(report.contains("checks: 2 exports, all storeless"));
    assert!(!report.contains("isPositive"), "collapsed: {report}");
    assert!(!report.contains("isEven"), "collapsed: {report}");

    // Shared demand: the two exports share one header, and neither prints its own entry.
    assert!(report.contains("alpha, beta (2 exports, one shared demand)"));
    assert!(
        !report.contains("\n  alpha\n"),
        "not listed alone: {report}"
    );
    assert!(!report.contains("\n  beta\n"), "not listed alone: {report}");

    // Root rollup: the summary names roots with a child-place count, never the child
    // atoms — those stay behind `--demand`.
    assert!(report.contains("^accounts (+2 places)"));
    assert!(
        !report.contains("^accounts.balance"),
        "atoms hidden: {report}"
    );

    // A storeless export inside a durable module collapses to one note.
    assert!(report.contains("  storeless: double"));

    // The `--demand` form of the same project keeps every atom the summary rolled away.
    let full = Project::from_fixture("demand_summary")
        .run_cli("demand-summary-full", &["check", "--demand"]);
    assert!(full.success(), "{}", full.stderr_text());
    let atoms = full.stdout_text();
    assert!(
        atoms.contains("ledger.alpha reads ^accounts.balance"),
        "{atoms}"
    );
    assert!(atoms.contains("^accounts.name"), "{atoms}");
}

/// The summary is a pure function of the demand facts: two runs of the same project
/// produce byte-identical output.
#[test]
fn check_summary_is_byte_stable_across_runs() {
    let project = Project::from_fixture("demand_summary");
    let first = project.run_cli("stable-a", &["check"]);
    let second = project.run_cli("stable-b", &["check"]);
    assert!(first.success() && second.success());
    assert_eq!(first.stdout_text(), second.stdout_text());
}

/// A project whose only export touches no durable data folds to one all-storeless module
/// line rather than a per-export note.
#[test]
fn check_describes_a_storeless_project_as_an_all_storeless_module() {
    let output = Project::single("pub fn answer(): int {\n    return 42\n}\n")
        .run_cli("storeless", &["check"]);
    assert!(output.success(), "{output:?}");
    assert_eq!(
        output.stdout_text(),
        "1 export across 1 module\n\nmain: 1 export, all storeless\n"
    );
}

/// A project with a diagnostic reports it with its span and typed code on standard
/// error, writes no demand report, and exits nonzero — the code and span are the
/// contract, not the message prose.
#[test]
fn check_reports_a_diagnostic_with_its_span_and_exits_nonzero() {
    let output = Project::single("pub fn oops(): int {\n    return \"nope\"\n}\n")
        .run_cli("broken", &["check"]);
    assert!(!output.success(), "a broken project must fail check");
    assert!(
        output.stdout.is_empty(),
        "a failing check prints no demand report: {}",
        output.stdout_text()
    );
    let stderr = output.stderr_text();
    assert!(stderr.contains("src/main.mw:2:12"), "{stderr}");
    assert!(stderr.contains("check.type"), "{stderr}");
}

/// A duplicate `--demand` flag is a usage error and exits 2.
#[test]
fn check_rejects_a_duplicate_demand_flag() {
    let output = Project::single("pub fn answer(): int {\n    return 1\n}\n")
        .run_cli("dup-demand", &["check", "--demand", "--demand"]);
    assert_eq!(output.code(), Some(2), "{}", output.stderr_text());
}

/// `check` is a live command, not a refounding stub: it never reports
/// `cli.command_unsupported`.
#[test]
fn check_is_not_a_refounding_command() {
    let output =
        Project::single("pub fn answer(): int {\n    return 1\n}\n").run_cli("live", &["check"]);
    let combined = format!("{}{}", output.stdout_text(), output.stderr_text());
    assert!(
        !combined.contains("cli.command_unsupported"),
        "check must be live: {combined}"
    );
}

/// The acceptance case: the EMR application. Its default check must read as a human-shaped
/// summary — the all-storeless `status` module collapses to one line, the six writers
/// that share a transaction helper's demand list once, and no line is the former
/// atom-by-atom wall. These are shape invariants robust to ordinary EMR evolution, not a
/// byte snapshot of a concurrently edited app.
#[test]
fn check_emr_acceptance_summarizes_without_the_wall() {
    let Some(emr) = emr_dir() else {
        eprintln!("skipping EMR acceptance: apps/emr not present");
        return;
    };
    let output = Command::new(MARROW_BIN)
        .arg("check")
        .arg(&emr)
        .env("NO_COLOR", "1")
        .output()
        .expect("run the marrow binary against apps/emr");
    assert!(
        output.status.success(),
        "EMR must check clean: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout);

    // The storeless `status` module collapses, and a shared transaction demand groups its
    // writers — the two headline collapses.
    assert!(
        report.contains("all storeless"),
        "no storeless collapse: {report}"
    );
    assert!(
        report.contains("one shared demand"),
        "no shared-demand dedup: {report}"
    );

    // The wall is gone: the summary rolls children up to roots, so no line approaches the
    // former per-atom length (the widest atom sentence ran past 600 columns).
    let widest = report.lines().map(str::len).max().unwrap_or(0);
    assert!(
        widest < 300,
        "a demand line is still a wall ({widest} cols): {report}"
    );
}

/// The EMR application directory, resolved from the crate manifest, or `None` when the
/// app tree is not checked out beside the crate.
fn emr_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/emr")
        .canonicalize()
        .ok()?;
    dir.join("marrow.toml").is_file().then_some(dir)
}
