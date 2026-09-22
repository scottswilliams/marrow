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

use crate::common::{conformance_dir, marrow_in};
use marrow_test_support::Scratch;

/// The library's own directory name, which is also the final segment of the relative
/// path the application's manifest declares. Copying the pair keeps both names so the
/// declared path resolves unchanged.
const LIB: &str = "graph_report_lib";
const APP: &str = "graph_report";

/// Copy the fixture pair into a scratch directory, preserving both directory names so
/// the application's declared relative path resolves without editing its manifest.
fn two_trees(label: &str) -> Scratch {
    let scratch = Scratch::new(label);
    for name in [APP, LIB] {
        copy_tree(&conformance_dir(name), &scratch.path().join(name));
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
fn a_dependency_diagnostic_names_its_alias() {
    let scratch = two_trees("dependency-diagnostic");
    let helper = scratch.path().join(LIB).join("src/text.mw");
    let source = fs::read_to_string(&helper).expect("read the library helper");
    fs::write(
        &helper,
        source.replace("return m[key] ?? fallback", "return m[key]"),
    )
    .expect("write the broken library helper");

    let output = marrow_in(&scratch.path().join(APP), &["check"]);
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
    let helper = scratch.path().join(LIB).join("src/text.mw");
    let formatted = fs::read_to_string(&helper).expect("read the library helper");
    let unformatted = formatted.replace(
        "pub fn getOr<V>(m: Map<string, V>, key: string, fallback: V): V {",
        "pub fn getOr<V>(m: Map<string, V>,key: string,fallback: V):V{",
    );
    assert_ne!(formatted, unformatted, "the edit must unformat the helper");
    fs::write(&helper, &unformatted).expect("write the unformatted helper");

    let output = marrow_in(&scratch.path().join(APP), &["fmt", "--write", "."]);
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
fn a_dependency_owned_identity_gap_is_not_minted_from_the_application() {
    let scratch = two_trees("dependency-mint");
    let helper = scratch.path().join(LIB).join("src/text.mw");
    let source = fs::read_to_string(&helper).expect("read the library helper");
    fs::write(
        &helper,
        format!("{source}\nresource Note {{\n    required body: string\n}}\n\nstore ^notes[id: int]: Note\n"),
    )
    .expect("write the durable library helper");

    // The report steers to the tree that owns the declaration.
    let check = marrow_in(&scratch.path().join(APP), &["check"]);
    let reported = String::from_utf8_lossy(&check.stderr);
    assert!(!check.status.success(), "{reported}");
    assert!(
        reported.contains(
            "run `marrow run` in the `graphtext` directory and commit its updated .marrow/ids"
        ),
        "the gap steers to the library: {reported}"
    );

    let run = marrow_in(
        &scratch.path().join(APP),
        &["run", "graph_report.report", "--", ""],
    );
    let records = String::from_utf8_lossy(&run.stdout);
    assert!(!run.status.success(), "{records}");
    assert!(
        records.contains("check.durable_identity"),
        "the gap is reported, not minted: {records}"
    );
    assert!(
        !scratch.path().join(LIB).join(".marrow").exists(),
        "the application must not publish a ledger into a dependency tree",
    );
}

/// A library that owns a store, offers a writer with its own `transaction` block and a
/// reader, and drives the writer from its own `test`.
const SHELF_LIBRARY: &str = r#"module shelf

resource Book {
    required title: string
}

store ^shelf[id: int]: Book

pub fn add(id: int, title: string) {
    transaction {
        ^shelf[id] = Book(title: title)
    }
}

pub fn title(id: int): string? {
    return ^shelf[id].title
}

test "the library drives its own writer" {
    add(1, "Small Gods")
    assert title(1) ?? "" == "Small Gods"
}
"#;

/// Write a two-tree pair under `label`: a `library` project holding `library_source`
/// and an application that declares it at `../library`. Each tree commits the ledger
/// its own declarations need, so `check` reaches the transaction rules.
fn shelf_pair(label: &str, library_source: &str, app_source: &str) -> Scratch {
    let scratch = Scratch::new(label);
    write_project(
        &scratch.path().join("library"),
        "edition = \"2026\"\n",
        &[("src/shelf.mw", library_source)],
        "id application . 00000000000000000000000000000000\n\
         id root shelf 00000000000000000000000000000001\n\
         id product Book 00000000000000000000000000000002\n\
         id key shelf.id 00000000000000000000000000000003\n\
         id field Book.title 00000000000000000000000000000004\n",
    );
    write_project(
        &scratch.path().join("app"),
        "edition = \"2026\"\n\n[dependencies]\nlibrary = { path = \"../library\" }\n",
        &[("src/main.mw", app_source)],
        "id application . 00000000000000000000000000000005\n",
    );
    scratch
}

fn write_project(root: &Path, manifest: &str, sources: &[(&str, &str)], id_rows: &str) {
    fs::create_dir_all(root.join("src")).expect("create the project src tree");
    fs::write(root.join("marrow.toml"), manifest).expect("write the manifest");
    for (path, source) in sources {
        fs::write(root.join(path), source).expect("write a source file");
    }
    fs::create_dir_all(root.join(".marrow")).expect("create the ledger directory");
    fs::write(
        root.join(".marrow/ids"),
        format!(
            "marrow ids v0\nmachine-written by marrow; do not edit\n{id_rows}high-water 0\nend\n"
        ),
    )
    .expect("write the committed ledger");
}

/// A dependency's `pub fn` takes no export slot in the consuming image, so a
/// `transaction` block in one has no owning export there. `marrow check` in the
/// application refuses it in source terms, at the library's block and under the
/// library's alias — never by handing the verifier an image it rejects.
#[test]
fn a_dependency_transaction_owner_is_refused_in_source_terms() {
    let scratch = shelf_pair(
        "dependency-owner",
        SHELF_LIBRARY,
        "module main\n\npub fn f(): int {\n    return 1\n}\n",
    );
    let output = marrow_in(&scratch.path().join("app"), &["check"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("library:src/shelf.mw:10:17: check.transaction_misplaced"),
        "the block is refused where it is written: {stderr}"
    );
    assert!(
        !stderr.contains("image.flow"),
        "the checker must not defer this to the verifier: {stderr}"
    );

    // The library is an ordinary project in its own directory, where its writer is an
    // export and its own test drives it.
    let library = marrow_in(&scratch.path().join("library"), &["check"]);
    assert!(
        library.status.success(),
        "the library checks standalone: {}",
        String::from_utf8_lossy(&library.stderr)
    );
}

/// The same library without the writer: a mutating helper that carries no block of its
/// own, a reader, and a `test` that drives the reader where the library lives.
const SHELF_HELPER_LIBRARY: &str = r#"module shelf

resource Book {
    required title: string
}

store ^shelf[id: int]: Book

pub fn stage(id: int, title: string) {
    ^shelf[id] = Book(title: title)
}

pub fn title(id: int): string? {
    return ^shelf[id].title
}

test "the library checks its reader standalone" {
    assert title(1) ?? "(none)" == "(none)"
}
"#;

/// The siblings a consumer may hold: a library `test`, a library export with durable
/// demand, and a library helper the consuming export's own `transaction` block commits.
/// The checker accepts and the image verifies, which is what `check` reports together.
#[test]
fn a_dependency_reader_helper_and_test_check_in_the_application() {
    let scratch = shelf_pair(
        "dependency-helper",
        SHELF_HELPER_LIBRARY,
        r#"module main

use library::shelf

pub fn label(id: int): string {
    return shelf::title(id) ?? "(none)"
}

pub fn put(id: int, title: string) {
    transaction {
        shelf::stage(id, title)
    }
}
"#,
    );
    let output = marrow_in(&scratch.path().join("app"), &["check"]);
    assert!(
        output.status.success(),
        "check: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The same library under an application that calls none of it: the library's
    // reader, helper, and `test` reach the image as ordinary uncalled functions, and
    // the image still verifies.
    let unused = shelf_pair(
        "dependency-unused",
        SHELF_HELPER_LIBRARY,
        "module main\n\npub fn f(): int {\n    return 1\n}\n",
    );
    let output = marrow_in(&unused.path().join("app"), &["check"]);
    assert!(
        output.status.success(),
        "check: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
