//! End-to-end Graph Report dogfood tests (P02a): a storeless `.mw` program over the
//! final procedural surface (records/enums, the text/collection/generic floor, nested
//! loops, in-source `test`s) travels the real production path through the built binary.
//! The `graph_report` conformance fixture's in-source `test`s run under `marrow test`,
//! and the frozen `report(Text)` export runs under `marrow run` with a multiline input.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

/// The frozen `report` output for the canonical rooted chain input `-> a / a -> b /
/// b -> c`, as one multiline UTF-8 text. `marrow run` prints this followed by a
/// newline in text form and carries it verbatim as the `data` field in JSONL.
const CHAIN_REPORT: &str = "Graph Report\n\
     nodes=3 edges=2 root=a malformed=0\n\
     -- degrees --\n\
     \x20 a out=1 in=0 role=source\n\
     \x20 b out=1 in=1 role=internal\n\
     \x20 c out=0 in=1 role=sink\n\
     -- reachable --\n\
     \x20 from a: a, b, c (3/3)\n\
     -- order --\n\
     \x20 a\n  b\n  c\n\
     -- cycle --\n\
     \x20 none";

fn conformance_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance")
        .join(name)
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run marrow binary")
}

/// The Graph Report fixture's in-source `test`s pass end to end: parsing a line-based
/// directed-graph encoding, per-node degree/role classification, a bounded
/// reachability fixpoint, a layered topological order, and cycle detection all report
/// `passed` through the production `marrow test` path.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn graph_report_conformance_fixture_passes_on_the_production_path() {
    let output = run_in(
        &conformance_dir("graph_report"),
        &["test", "--format", "jsonl"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "graph_report fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""errored":0"#), "{summary}");
    assert!(summary.contains(r#""total":13"#), "{summary}");
}

/// The frozen `report(Text)` export travels the full production path under `marrow
/// run` and renders one deterministic multiline UTF-8 report. Text form prints the
/// report and a trailing newline; JSONL carries the identical text as the `data`
/// field of a `value` outcome, so the two renderings agree on the same bytes.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn report_renders_a_deterministic_multiline_report_through_run() {
    let dir = conformance_dir("graph_report");
    let input = "-> a\na -> b\nb -> c";

    let text = run_in(&dir, &["run", "graph_report.report", "--", input]);
    assert!(
        text.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&text.stdout),
        format!("{CHAIN_REPORT}\n"),
    );

    let jsonl = run_in(
        &dir,
        &[
            "run",
            "graph_report.report",
            "--format",
            "jsonl",
            "--",
            input,
        ],
    );
    assert!(jsonl.status.success(), "{jsonl:?}");
    // The report's only JSON-significant character is the newline, so escaping it is
    // exactly the `\n` substitution; the canonical run record keys are in byte order.
    let escaped = CHAIN_REPORT.replace('\n', "\\n");
    let expected = format!("{{\"data\":\"{escaped}\",\"kind\":\"run\",\"outcome\":\"value\"}}\n");
    assert_eq!(String::from_utf8_lossy(&jsonl.stdout), expected);
}

/// A graph with a cycle reaches the `report` export's cycle-detection path through
/// `marrow run`: the acyclic prefix appears in the topological order and the cyclic
/// nodes are named on the `-- cycle --` line, evidencing the bounded Kahn traversal
/// runs on the real VM, not only under `marrow test`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn report_detects_a_cycle_through_run() {
    let dir = conformance_dir("graph_report");
    let input = "a -> b\nb -> c\nc -> a\nd -> a";

    let jsonl = run_in(
        &dir,
        &[
            "run",
            "graph_report.report",
            "--format",
            "jsonl",
            "--",
            input,
        ],
    );
    let stdout = String::from_utf8_lossy(&jsonl.stdout);
    assert!(jsonl.status.success(), "{stdout}");
    assert!(stdout.contains(r#""outcome":"value""#), "{stdout}");
    // Only `d` (in-degree 0) is emittable; a, b, c form the reported cycle.
    assert!(
        stdout.contains(r"-- order --\n  d\n-- cycle --\n  a, b, c"),
        "{stdout}"
    );
}
