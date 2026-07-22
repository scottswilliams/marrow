//! D00 slice 2: the durable-identity ledger through the production path.
//!
//! `.marrow/ids` is the optional machine-written identity artifact. The compiler
//! is the fail-precisely owner: a durable declaration without a complete ledger
//! identity is a typed `check.durable_identity` diagnostic, and the compiler
//! never writes the artifact. The one convenience mint action is scoped to
//! `marrow run`, which mints missing identities from OS entropy and publishes
//! them atomically; `marrow test` (the CI path) never mutates the tree.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

/// The counter tracer project: one durable root over one resource.
const COUNTER_SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[name: string]: Counter

pub fn set(name: string, v: int) {
    transaction {
        ^counters[name] = Counter(value: v)
    }
}

pub fn get(name: string): int? {
    return ^counters[name].value
}

test "storeless arithmetic" {
    assert 1 + 1 == 2
}
"#;

struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("marrow-d00-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp dir");
        TempDir { root }
    }
}

impl Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn project(dir: &Path, source: &str) {
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    write(&dir.join("src").join("main.mw"), source);
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run marrow binary")
}

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Compile-and-verify a one-module project through the production owners,
/// returning the sealed image's durable-contract identity. Used by the
/// rename-preserves journey, which must observe the contract id across a
/// source rename with a moved ledger anchor.
fn contract_of(source: &str, ids: &str) -> marrow_verify::DurableContractId {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
    image.durable_contract()
}

// --- The failing production-path check: identity completeness is a compile
// fact, enforced on every storeless path. ---

/// Declaring a durable shape without complete identity fails precisely on the
/// CI path: `marrow test` reports the typed `check.durable_identity` diagnostic
/// and writes nothing into the tree.
#[test]
fn a_durable_declaration_without_ledger_identity_fails_the_ci_path() {
    let temp = TempDir::new("no-ledger-test");
    project(&temp, COUNTER_SOURCE);

    let output = run_in(&temp, &["test"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        combined(&output).contains("check.durable_identity"),
        "expected the typed missing-identity diagnostic, got: {}",
        combined(&output)
    );
    assert!(
        !temp.join(".marrow/ids").exists(),
        "`marrow test` must never write .marrow/ids (CI never mutates the tree)"
    );
}

// --- The `marrow run` convenience mint (the one interim mint action; deleted
// when the accepted apply action lands at F03). ---

/// `marrow run` mints the missing identities from OS entropy and publishes a
/// well-formed `.marrow/ids` before the durable export parks in the trough, and a
/// second run reuses the artifact byte-for-byte: the mint pass is the only writer
/// and re-running on a complete ledger is a no-op (mint fires only for a genuinely
/// new declaration).
#[test]
fn run_mints_missing_identities_once_and_reuses_them() {
    let temp = TempDir::new("run-mints");
    project(&temp, COUNTER_SOURCE);

    // The durable export parks in the trough, but the mint pre-pass publishes
    // .marrow/ids from OS entropy first.
    let set = run_in(&temp, &["run", "set", "--", "hits", "5"]);
    assert!(
        combined(&set).contains("cli.durable_unsupported"),
        "a durable run parks in the trough: {}",
        combined(&set)
    );
    let published = fs::read(temp.join(".marrow/ids")).expect("run published .marrow/ids");
    let text = String::from_utf8(published.clone()).expect("artifact is UTF-8");
    assert!(text.starts_with("marrow ids v0\n"), "header: {text}");
    assert!(text.contains("do not edit"), "notice: {text}");
    for row in [
        "id application .",
        "id product Counter",
        "id field Counter.value",
        "id field Counter.label",
        "id root counters",
        "id key counters.name",
    ] {
        assert!(text.contains(row), "missing `{row}` in: {text}");
    }
    assert!(text.ends_with("end\n"), "end marker: {text}");
    assert!(
        !temp
            .join(format!(".marrow/ids.tmp.{}", std::process::id()))
            .exists(),
        "no temp file survives a successful publication"
    );

    // A second durable run finds a complete ledger: it mints nothing and leaves the
    // committed artifact byte-identical (re-running on a complete ledger is a no-op),
    // before parking in the trough again.
    let get = run_in(&temp, &["run", "get", "--", "hits"]);
    assert!(
        combined(&get).contains("cli.durable_unsupported"),
        "{}",
        combined(&get)
    );
    assert_eq!(
        fs::read(temp.join(".marrow/ids")).unwrap(),
        published,
        "a second run leaves the committed artifact byte-identical"
    );
}

/// The clone/relocation journeys: a fresh checkout (a byte copy of the project,
/// committed `.marrow/ids` included) at a different location reuses the
/// committed ids — nothing re-mints and the artifact stays byte-identical.
#[test]
fn a_cloned_and_relocated_checkout_reuses_the_committed_ids() {
    let temp = TempDir::new("clone-src");
    project(&temp, COUNTER_SOURCE);
    // The durable export parks, but its mint pre-pass publishes the committed ids.
    let set = run_in(&temp, &["run", "set", "--", "hits", "1"]);
    assert!(
        combined(&set).contains("cli.durable_unsupported"),
        "{}",
        combined(&set)
    );
    let committed = fs::read(temp.join(".marrow/ids")).expect("committed artifact");

    // Clone: manifest, source, and .marrow/ids — no store, as a checkout would be.
    let clone = TempDir::new("clone-dst");
    write(
        &clone.join("marrow.toml"),
        &fs::read_to_string(temp.join("marrow.toml")).unwrap(),
    );
    write(
        &clone.join("src").join("main.mw"),
        &fs::read_to_string(temp.join("src").join("main.mw")).unwrap(),
    );
    fs::create_dir_all(clone.join(".marrow")).expect("create metadata dir");
    fs::write(clone.join(".marrow/ids"), &committed).expect("clone the artifact");

    // The storeless CI path compiles and passes in the clone (identity is
    // complete from the committed artifact alone), and a durable run parks in the
    // trough — with the artifact untouched in both.
    let test = run_in(&clone, &["test"]);
    assert!(test.status.success(), "{test:?}");
    let run = run_in(&clone, &["run", "set", "--", "hits", "2"]);
    assert!(
        combined(&run).contains("cli.durable_unsupported"),
        "{}",
        combined(&run)
    );
    assert_eq!(
        fs::read(clone.join(".marrow/ids")).unwrap(),
        committed,
        "a relocated checkout neither re-mints nor rewrites the artifact"
    );
}

/// The parallel-branch/merge-conflict journeys: unresolved conflict markers and
/// a merged double-mint (two rows claiming one anchor) are both rejected whole
/// with the typed corruption code, and the run never rewrites the artifact.
#[test]
fn conflicted_and_double_minted_artifacts_reject_whole() {
    let temp = TempDir::new("merge-conflict");
    project(&temp, COUNTER_SOURCE);
    // The durable run parks, but its mint pre-pass seeds a well-formed .marrow/ids.
    let seeded = run_in(&temp, &["run", "set", "--", "hits", "1"]);
    assert!(
        combined(&seeded).contains("cli.durable_unsupported"),
        "{}",
        combined(&seeded)
    );
    let good = fs::read_to_string(temp.join(".marrow/ids")).unwrap();

    // Unresolved Git conflict markers.
    let conflicted = good.replace("high-water", "<<<<<<< ours\nhigh-water");
    fs::write(temp.join(".marrow/ids"), &conflicted).unwrap();
    let output = run_in(&temp, &["test"]);
    assert!(!output.status.success());
    let rendered = combined(&output);
    assert!(rendered.contains("project.ids_corrupt"), "{rendered}");
    // The text channel carries the typed message, so a corrupt artifact names the
    // file and the reason rather than only the bare code.
    assert!(
        rendered.contains(".marrow/ids is corrupt")
            && rendered.contains("unresolved Git conflict markers"),
        "text output must name the file and reason: {rendered}"
    );

    // A textual merge that kept both branches' mints for one anchor.
    let value_row = good
        .lines()
        .find(|line| line.starts_with("id field Counter.value"))
        .unwrap()
        .to_string();
    let doubled = good.replace(
        &value_row,
        &format!("{value_row}\nid field Counter.value ffffffffffffffffffffffffffffffff"),
    );
    fs::write(temp.join(".marrow/ids"), &doubled).unwrap();
    let output = run_in(&temp, &["test"]);
    assert!(!output.status.success());
    assert!(
        combined(&output).contains("project.ids_corrupt"),
        "{}",
        combined(&output)
    );
    assert_eq!(
        fs::read_to_string(temp.join(".marrow/ids")).unwrap(),
        doubled,
        "a corrupt artifact is rejected, never repaired or rewritten"
    );
}

/// The interrupted-publication journey: a torn artifact (truncated before its
/// end marker) is rejected whole with the typed corruption code — never
/// half-read, never silently re-minted over.
#[test]
fn a_torn_artifact_rejects_whole_and_is_never_reminted_over() {
    let temp = TempDir::new("torn");
    project(&temp, COUNTER_SOURCE);
    // The durable run parks, but its mint pre-pass seeds a well-formed .marrow/ids.
    let seeded = run_in(&temp, &["run", "set", "--", "hits", "1"]);
    assert!(
        combined(&seeded).contains("cli.durable_unsupported"),
        "{}",
        combined(&seeded)
    );
    let good = fs::read_to_string(temp.join(".marrow/ids")).unwrap();

    let torn = good.replace("end\n", "");
    fs::write(temp.join(".marrow/ids"), &torn).unwrap();
    for command in [&["test"][..], &["run", "get", "--", "hits"][..]] {
        let output = run_in(&temp, command);
        assert!(!output.status.success(), "{output:?}");
        assert!(
            combined(&output).contains("project.ids_corrupt"),
            "{}",
            combined(&output)
        );
    }
    assert_eq!(
        fs::read_to_string(temp.join(".marrow/ids")).unwrap(),
        torn,
        "the torn artifact is left for version control to restore"
    );
}

/// The tombstone journey: a retired anchor can never be reused. Declaring at a
/// tombstoned `(kind, path)` fails precisely on every path — `marrow run` does
/// not mint over it and the artifact stays byte-identical.
#[test]
fn a_retired_anchor_cannot_be_redeclared_or_reminted() {
    let temp = TempDir::new("tombstone");
    project(&temp, COUNTER_SOURCE);

    // A committed artifact whose `counters` root was retired: complete rows for
    // everything else, plus the tombstone at the root anchor.
    let ids = "marrow ids v0\n\
               machine-written by marrow; do not edit\n\
               id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
               id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
               id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
               id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
               id key counters.name 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
               retired root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b 1\n\
               high-water 1\n\
               end\n";
    fs::create_dir_all(temp.join(".marrow")).unwrap();
    fs::write(temp.join(".marrow/ids"), ids).unwrap();

    for command in [&["test"][..], &["run", "set", "--", "hits", "1"][..]] {
        let output = run_in(&temp, command);
        assert!(!output.status.success(), "{output:?}");
        assert!(
            combined(&output).contains("check.durable_identity"),
            "{}",
            combined(&output)
        );
    }
    assert_eq!(
        fs::read_to_string(temp.join(".marrow/ids")).unwrap(),
        ids,
        "a retired anchor is never minted over; the ledger bytes are unchanged"
    );
}

// --- Rename preserves identity: the contract id follows the ledger ids. ---

const LIBRARY_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     high-water 0\n\
     end\n";

const LIBRARY_SOURCE: &str = r#"resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn title(id: int): string? {
    return ^books[id].title
}
"#;

/// Renaming a durable field preserves the durable-contract identity when the
/// ledger anchor moves with it (same id at the new path), and a delete-then-
/// re-add (a fresh id at the same path) changes it. This is the D00 exit
/// property the descriptor-over-ledger-ids payload exists for, observed
/// through the full production path: capture → compile → verify.
#[test]
fn a_rename_with_a_moved_anchor_preserves_the_contract_id() {
    let base = contract_of(LIBRARY_SOURCE, LIBRARY_IDS);

    // Rename `title` → `heading` in source, moving the ledger anchor while the
    // id stays: identity preserved.
    let renamed_source = LIBRARY_SOURCE.replace("title", "heading");
    let renamed_ids = LIBRARY_IDS.replace("Book.title", "Book.heading");
    assert_eq!(
        base,
        contract_of(&renamed_source, &renamed_ids),
        "a rename whose anchor moved (id unchanged) preserves durable identity"
    );

    // The same rename minted as a fresh identity instead: a different graph.
    let re_minted_ids = renamed_ids.replace(
        "0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e",
        "1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e",
    );
    assert_ne!(
        base,
        contract_of(&renamed_source, &re_minted_ids),
        "a fresh id at the same shape is a different durable identity"
    );

    // A semantic change under unchanged ids still changes the identity.
    let required_off = LIBRARY_SOURCE.replace("required title", "title");
    assert_ne!(
        base,
        contract_of(&required_off, LIBRARY_IDS),
        "a field made sparse changes the durable identity"
    );
}

// --- The ledger's one home (`.marrow/ids`) and the retired root path. ---

/// A ledger at the retired project-root path `marrow.ids` is refused before any
/// read with the typed location fault and its one-line move steer; files at
/// both paths fail closed with the reconcile steer. Never two live copies.
#[test]
fn a_ledger_at_the_retired_root_path_is_refused_with_a_move_steer() {
    let temp = TempDir::new("legacy-ledger");
    project(&temp, COUNTER_SOURCE);
    let set = run_in(&temp, &["run", "set", "--", "hits", "5"]);
    assert!(
        combined(&set).contains("cli.durable_unsupported"),
        "{}",
        combined(&set)
    );
    let ids = fs::read(temp.join(".marrow/ids")).expect("published ledger");
    fs::write(temp.join("marrow.ids"), &ids).expect("plant the legacy copy");
    fs::remove_file(temp.join(".marrow/ids")).expect("vacate the home");

    let output = run_in(&temp, &["test"]);
    assert!(!output.status.success(), "{output:?}");
    let text = combined(&output);
    assert!(text.contains("project.ids_location"), "{text}");
    assert!(text.contains("git mv marrow.ids .marrow/ids"), "{text}");

    fs::write(temp.join(".marrow/ids"), &ids).expect("occupy the home too");
    let output = run_in(&temp, &["test"]);
    assert!(!output.status.success(), "{output:?}");
    let text = combined(&output);
    assert!(text.contains("project.ids_location"), "{text}");
    assert!(text.contains("exactly one ledger"), "{text}");
}

/// A mint inside a Git repository whose index lacks the ledger prints the
/// one-line commit steer on stderr; the published artifact and records are
/// unaffected. (Outside a repository — every other test here — it is silent.)
#[test]
fn a_mint_inside_a_git_repository_steers_toward_committing_the_ledger() {
    let temp = TempDir::new("mint-steer");
    project(&temp, COUNTER_SOURCE);
    fs::create_dir_all(temp.join(".git")).expect("fake repository");

    let set = run_in(&temp, &["run", "set", "--", "hits", "5"]);
    let stderr = String::from_utf8_lossy(&set.stderr).to_string();
    assert!(stderr.contains(".marrow/ids"), "{stderr}");
    assert!(stderr.contains("not tracked by Git"), "{stderr}");
    assert!(
        temp.join(".marrow/ids").exists(),
        "the steer never blocks the publish"
    );
}

/// A stale publish temp left by a crashed earlier run is swept by the next
/// publish. A crash between temp write and rename leaves the mint gap open
/// (the rename never happened), so the next durable run mints again and its
/// publish removes the debris: the committed metadata directory holds only
/// the ledger.
#[test]
fn a_stale_publish_temp_from_a_crashed_run_is_swept_on_the_next_publish() {
    let temp = TempDir::new("stale-temp-sweep");
    project(&temp, COUNTER_SOURCE);
    fs::create_dir_all(temp.join(".marrow")).expect("metadata directory");
    fs::write(temp.join(".marrow/ids.tmp.99999"), b"debris from a crash").expect("stale temp");

    let set = run_in(&temp, &["run", "set", "--", "hits", "5"]);
    assert!(
        combined(&set).contains("cli.durable_unsupported"),
        "{}",
        combined(&set)
    );
    let mut entries: Vec<String> = fs::read_dir(temp.join(".marrow"))
        .expect("metadata directory listing")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec!["ids".to_string()],
        "publish sweeps stale temp siblings"
    );
}
