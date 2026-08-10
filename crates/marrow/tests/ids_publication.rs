//! The command-line contract around a live `.marrow/ids` publication marker.
//!
//! Every front door captures the project, so every front door reports the
//! marker: while a publication is claimed the committed ledger is whichever
//! generation recovery settles on, and a command that read one anyway would be
//! reading a generation about to be replaced. `marrow run` is the one command
//! that writes the ledger, so it recovers first and then proceeds.

mod common;

use std::fs;

use common::Project;

const PENDING: &str = "project.ids_publication_pending";

/// A source file with one durable declaration whose identity `marrow run` mints.
const DURABLE_SOURCE: &str = r#"resource Counter {
    required value: int
}

store ^counters[name: string]: Counter

pub fn get(name: string): int? {
    return ^counters[name].value
}

pub fn answer(): int {
    return 42
}
"#;

fn project() -> Project {
    Project::single(DURABLE_SOURCE)
}

/// Every read-only front door refuses under a durable claim. `marrow init` is
/// excluded because it creates a project rather than capturing one, and `marrow
/// run` is excluded because it is the recovery owner.
#[test]
fn every_read_only_front_door_reports_the_pending_marker() {
    let workspace = project().materialize("ids-pending-front-doors");
    fs::create_dir_all(workspace.path(".marrow")).expect("create the metadata directory");
    fs::write(workspace.path(".marrow/ids.pending"), b"").expect("plant the marker");

    for args in [
        vec!["check"],
        vec!["fmt", "."],
        vec!["test"],
        vec!["image", "--out", "out"],
        vec!["client", "typescript"],
    ] {
        let outcome = workspace.marrow(&args);
        let text = format!("{}{}", outcome.stdout_text(), outcome.stderr_text());
        assert!(
            !outcome.success(),
            "`marrow {}` succeeded under a live publication marker: {text}",
            args.join(" ")
        );
        assert!(
            text.contains(PENDING),
            "`marrow {}` did not report `{PENDING}`: {text}",
            args.join(" ")
        );
    }
}

/// An entry that was never durably claimed gates capture the same way, and its
/// bytes are retained rather than swept.
#[test]
fn an_unclaimed_create_gates_capture_and_is_retained() {
    let workspace = project().materialize("ids-unclaimed-create");
    fs::create_dir_all(workspace.path(".marrow")).expect("create the metadata directory");
    fs::write(workspace.path(".marrow/ids.pending.create"), b"partial")
        .expect("plant the unclaimed create");

    let outcome = workspace.marrow(&["check"]);
    let text = format!("{}{}", outcome.stdout_text(), outcome.stderr_text());
    assert!(!outcome.success(), "check succeeded: {text}");
    assert!(
        text.contains(PENDING),
        "check did not report `{PENDING}`: {text}"
    );
    assert_eq!(
        fs::read(workspace.path(".marrow/ids.pending.create")).expect("the entry is retained"),
        b"partial",
        "no command sweeps an entry it cannot prove is its own"
    );
}

/// `marrow run` settles an interrupted publication before it captures the
/// project, then proceeds through the ordinary path.
#[test]
fn run_recovers_an_interrupted_publication_before_it_captures() {
    let workspace = project().materialize("ids-run-recovers");
    let successor = "marrow-ids v1\n";
    fs::create_dir_all(workspace.path(".marrow")).expect("create the metadata directory");
    fs::write(workspace.path(".marrow/ids.publish.stage"), successor).expect("stage a successor");

    // A publication that was never durably claimed is the manual state: `run`
    // reports it rather than sweeping the staged bytes.
    let refused = workspace.marrow(&["run", "answer"]);
    let text = format!("{}{}", refused.stdout_text(), refused.stderr_text());
    assert!(
        !refused.success(),
        "run succeeded over an unclaimed stage: {text}"
    );
    assert!(
        text.contains(PENDING),
        "run did not report `{PENDING}`: {text}"
    );
    assert_eq!(
        fs::read_to_string(workspace.path(".marrow/ids.publish.stage")).expect("retained"),
        successor
    );

    // Ordering evidence: the two owners word the same code differently, so
    // which message surfaces says which ran first. `run` reaches the
    // publication owner (whose refusal names both entries to remove) rather
    // than the capture facade's read-only marker refusal, which is what
    // "recovery before capture" means at this boundary.
    fs::write(workspace.path(".marrow/ids.pending.create"), b"partial")
        .expect("plant an unclaimed create");
    let refused = workspace.marrow(&["run", "answer"]);
    let text = format!("{}{}", refused.stdout_text(), refused.stderr_text());
    assert!(
        text.contains("every byte is retained and `.marrow/ids` is unchanged"),
        "run reported the capture facade's marker refusal, so it captured before it recovered: \
         {text}"
    );
    fs::remove_file(workspace.path(".marrow/ids.pending.create")).expect("clear the create");

    // Clearing the manual state by hand returns the project to ordinary
    // service, and the storeless mint then publishes through the new owner.
    fs::remove_file(workspace.path(".marrow/ids.publish.stage")).expect("clear the stage");
    let ran = workspace.marrow(&["run", "answer"]);
    let text = format!("{}{}", ran.stdout_text(), ran.stderr_text());
    assert!(
        ran.success(),
        "run failed after the manual state was cleared: {text}"
    );
    assert!(
        ran.stdout_text().contains("42"),
        "run printed no value: {text}"
    );
    assert!(
        workspace.path(".marrow/ids").exists(),
        "the storeless mint published the ledger"
    );
    assert!(
        !workspace.path(".marrow/ids.pending").exists()
            && !workspace.path(".marrow/ids.publish.stage").exists(),
        "a settled publication leaves no marker or stage behind"
    );
}

/// The mint publishes through the adapter's owner, and a second run over the
/// published ledger neither republishes nor disturbs it.
#[test]
fn a_published_ledger_is_stable_across_runs() {
    let workspace = project().materialize("ids-stable");
    assert!(workspace.marrow(&["run", "answer"]).success());
    let first = fs::read(workspace.path(".marrow/ids")).expect("the published ledger");

    assert!(workspace.marrow(&["run", "answer"]).success());
    assert_eq!(
        fs::read(workspace.path(".marrow/ids")).expect("the ledger"),
        first,
        "a run with no missing identity leaves the ledger byte-for-byte unchanged"
    );
    assert_eq!(
        fs::read(workspace.path(".marrow/publish.lock")).expect("the rendezvous entry"),
        b"",
        "the write lock is a zero-byte rendezvous entry committed with the rest of `.marrow`"
    );
}

/// A project with no durable declaration mints nothing, so publication creates
/// nothing: `.marrow` does not appear at all.
#[test]
fn a_storeless_project_grows_no_metadata_directory() {
    let workspace =
        Project::single("pub fn answer(): int {\n    return 42\n}\n").materialize("ids-storeless");
    let outcome = workspace.marrow(&["run", "answer"]);
    assert!(outcome.success(), "{}", outcome.stderr_text());
    assert!(
        !workspace.path(".marrow").exists(),
        "a project that mints nothing grows no metadata directory"
    );
}
