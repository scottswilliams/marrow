//! The two-tree journey: an application that reaches a separately located library of
//! Marrow source by relative path, driven through the built binary over the frozen
//! `graph_report` / `graph_report_lib` fixture pair.
//!
//! The pair is the evidence for three separate rules. The library is an ordinary
//! project, so it checks and tests standalone in its own directory. The application
//! resolves the library's helpers through its own `[dependencies]` alias, and the
//! report bytes it produces are unchanged by the move. And the application owns only
//! its own tree: `marrow fmt` reports a badly formatted dependency file but never
//! rewrites one.

use std::fs;
use std::path::Path;

use crate::common::{TempDir, conformance_dir, marrow_in};

/// The library's own directory name, which is also the final segment of the relative
/// path the application's manifest declares. Copying the pair keeps both names so the
/// declared path resolves unchanged.
const LIB: &str = "graph_report_lib";
const APP: &str = "graph_report";

/// Copy the fixture pair into a scratch directory, preserving both directory names so
/// the application's declared relative path resolves without editing its manifest.
fn two_trees(label: &str) -> TempDir {
    let scratch = TempDir::new(label);
    for name in [APP, LIB] {
        copy_tree(&conformance_dir(name), &scratch.join(name));
    }
    scratch
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create scratch tree");
    for entry in fs::read_dir(from).expect("read fixture tree") {
        let entry = entry.expect("fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("fixture entry kind").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

/// The library is a project in its own right: its in-source `test`s pass from its own
/// directory with no application present, which is what lets a developer change it and
/// check it where it lives.
#[test]
fn the_library_checks_and_tests_standalone() {
    let dir = conformance_dir(LIB);
    let output = marrow_in(&dir, &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{output:?}\n{stdout}");
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""errored":0"#), "{summary}");
    assert!(summary.contains(r#""total":3"#), "{summary}");

    let check = marrow_in(&dir, &["check"]);
    assert!(check.status.success(), "{check:?}");
}

/// The application resolves the library's exports through its alias and runs its own
/// export against them. The library contributes nothing the application must spell
/// differently: the alias roots the library's module path, and the library's own source
/// carries no prefix.
#[test]
#[ignore = "needs the compiler half of local dependencies"]
fn the_application_reaches_the_library_through_its_alias() {
    let dir = conformance_dir(APP);
    let check = marrow_in(&dir, &["check"]);
    assert!(
        check.status.success(),
        "check: {}",
        String::from_utf8_lossy(&check.stderr)
    );
    let run = marrow_in(&dir, &["run", "graph_report.report", "--", "-> a\na -> b"]);
    assert!(
        run.status.success(),
        "run: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

/// A diagnostic in a dependency file names the alias the consuming project reaches it
/// under, not a path in the consuming tree: the library's `src/text.mw` renders as
/// `graphtext:src/text.mw`. Identities stay root-relative, so the origin is a prefix the
/// renderer adds and never part of the identity itself.
#[test]
#[ignore = "needs the compiler half of local dependencies"]
fn a_dependency_diagnostic_names_its_alias() {
    let scratch = two_trees("dependency-diagnostic");
    let helper = scratch.join(LIB).join("src/text.mw");
    let source = fs::read_to_string(&helper).expect("read the library helper");
    fs::write(&helper, source.replace("return m[key] ?? fallback", "return m[key]"))
        .expect("write the broken library helper");

    let output = marrow_in(&scratch.join(APP), &["check"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|line| line.starts_with("graphtext:src/text.mw:")),
        "a dependency diagnostic names its alias: {stderr}"
    );
}

/// `marrow fmt` owns the root project's tree alone. It reports an unformatted
/// dependency file — silence would hide a real finding — and leaves the dependency tree
/// byte-identical, because the project that declares a file is the project that
/// formats it.
#[test]
fn fmt_reports_but_never_rewrites_a_dependency_file() {
    let scratch = two_trees("dependency-fmt");
    let helper = scratch.join(LIB).join("src/text.mw");
    let formatted = fs::read_to_string(&helper).expect("read the library helper");
    let unformatted = formatted.replace(
        "pub fn getOr<V>(m: Map<string, V>, key: string, fallback: V): V {",
        "pub fn getOr<V>(m: Map<string, V>,key: string,fallback: V):V{",
    );
    assert_ne!(formatted, unformatted, "the edit must unformat the helper");
    fs::write(&helper, &unformatted).expect("write the unformatted helper");

    let output = marrow_in(&scratch.join(APP), &["fmt", "--write", "."]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "an unformatted dependency file is a finding: {stderr}"
    );
    assert!(
        stderr.contains("graphtext:src/text.mw"),
        "the finding names the dependency file: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(&helper).expect("read the library helper back"),
        unformatted,
        "`marrow fmt --write` must leave a dependency file byte-identical",
    );
}

/// Minting is the root project's alone. A durable declaration the library owns has no
/// identity the application may draw: `marrow run` reports the gap and steers the
/// developer to the library, and the library's tree is untouched.
#[test]
#[ignore = "needs the compiler half of local dependencies"]
fn a_dependency_owned_identity_gap_is_not_minted_from_the_application() {
    let scratch = two_trees("dependency-mint");
    let helper = scratch.join(LIB).join("src/text.mw");
    let source = fs::read_to_string(&helper).expect("read the library helper");
    fs::write(
        &helper,
        format!("{source}\nresource Note {{\n    required body: string\n}}\n\nstore ^notes[id: int]: Note\n"),
    )
    .expect("write the durable library helper");

    let output = marrow_in(&scratch.join(APP), &["run", "graph_report.report", "--", ""]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("check.durable_identity"),
        "the gap is reported, not minted: {stderr}"
    );
    assert!(
        !scratch.join(LIB).join(".marrow/ids").exists(),
        "the application must not publish a ledger into a dependency tree",
    );
}
