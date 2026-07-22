//! Diagnostic actionability: the E07-M3 seeded-defect corpus as a permanent red suite.
//!
//! The M3 gate scored the diagnostic each seeded memory-path defect produces on four
//! axes — location, cause, steer, and cascade cleanliness — and flagged five as merely
//! detected or cascade-obscured. This suite reconstructs each flagged defect on the
//! frozen fixtures it was measured on (`club_locker`, `cross_module_roots`) and pins the
//! *actionable* shape the fix establishes: the typed code, the span at the defect, the
//! bound cascade count, and the load-bearing steer the diagnostic now carries (an
//! unclosed-block report, the `at most` bound law, a did-you-mean candidate). The codes
//! and spans are the contract; a message substring is asserted only where it is the
//! actionable payload the gate scores, never for prose style.

mod common;

use std::path::PathBuf;

use common::{Diagnostics, Project};

/// The frozen fixture corpus root, resolved from the crate manifest so a mutation reads
/// the same bytes the ensemble suites check.
fn fixture_file(fixture: &str, relative: &str) -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "v01",
        fixture,
        relative,
    ]
    .iter()
    .collect();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read fixture `{}`: {error}", path.display()))
}

/// The `club_locker` source with one `old -> new` mutation applied exactly once, so a
/// defect is one edit against the frozen flagship rather than a hand-copied program.
fn club_locker_mutated(old: &str, new: &str) -> Project {
    let source = fixture_file("club_locker", "src/clublocker.mw");
    assert!(
        source.matches(old).count() == 1,
        "the mutation anchor `{old}` must occur exactly once in the frozen fixture",
    );
    Project::from_fixture("club_locker").source("src/clublocker.mw", &source.replace(old, new))
}

/// Compile a project and return its typed source diagnostics, failing the test if it
/// unexpectedly checks clean (a defect that no longer reproduces is a corpus break).
fn diagnostics(project: &Project) -> Diagnostics {
    project
        .try_image()
        .expect_err("a seeded defect must fail the check")
}

// ---------------------------------------------------------------------------
// Item 1 — D11: an unclosed delimiter names its open site and does not cascade.
// ---------------------------------------------------------------------------

/// A function body missing its closing `}` swallows every following declaration as body
/// content. The parser reports one `parse.syntax` at the opening brace of the unclosed
/// body — never a parse error per following declaration — so the everyday truncation
/// edit-state points at the real fix instead of the next `pub fn`.
#[test]
fn d11_unclosed_brace_names_the_open_site_without_a_cascade() {
    // Drop the closing brace of `suspendMember`, opened on line 113.
    let project = club_locker_mutated(
        "    transaction {\n        place member = ^members[memberId]\n        if exists(member) {\n            member.standing = Standing::suspended\n        }\n    }\n}\n\npub fn reinstateMember",
        "    transaction {\n        place member = ^members[memberId]\n        if exists(member) {\n            member.standing = Standing::suspended\n        }\n    }\n\npub fn reinstateMember",
    );
    let diagnostics = diagnostics(&project);
    assert_eq!(
        diagnostics.count_code("parse.syntax"),
        1,
        "one missing `}}` reports once, not a cascade: {:?}",
        diagnostics.all()
    );
    let unclosed = diagnostics.only("parse.syntax");
    assert_eq!(
        unclosed.line(),
        113,
        "the diagnostic points at the opening brace of the unclosed body: {:?}",
        diagnostics.all()
    );
}

/// A truncated source that ends inside an open body — the H00f2-O2 closure — must
/// produce an Error-severity diagnostic at the open brace, never recover silently.
#[test]
fn d11_a_truncated_body_is_reported_not_silently_recovered() {
    let project = Project::single("pub fn open(): int {\n    return 1\n");
    let diagnostics = diagnostics(&project);
    assert_eq!(
        diagnostics.count_code("parse.syntax"),
        1,
        "{:?}",
        diagnostics.all()
    );
    assert_eq!(
        diagnostics.only("parse.syntax").line(),
        1,
        "the open brace on line 1 is named: {:?}",
        diagnostics.all()
    );
}

// ---------------------------------------------------------------------------
// Item 2 — D13: an unbounded durable traversal names the bound law.
// ---------------------------------------------------------------------------

/// A durable `for` head written without `at most` reaches the checker's bounded-traversal
/// law instead of desyncing the parser on the trailing `on more` block. The diagnostic is
/// a `check.type` at the `for` head that names the `at most N` and `on more` fix — not a
/// generic `parse.syntax: expected an expression`.
#[test]
fn d13_unbounded_traversal_names_the_bound_law_at_the_head() {
    let project = club_locker_mutated(
        "for seq in ^assets[assetId].serviceLog at most 100 {",
        "for seq in ^assets[assetId].serviceLog {",
    );
    let diagnostics = diagnostics(&project);
    assert_eq!(
        diagnostics.count_code("parse.syntax"),
        0,
        "the missing bound is a checked law, not a parse desync: {:?}",
        diagnostics.all()
    );
    let unbounded = diagnostics.only("check.type");
    assert_eq!(
        unbounded.line(),
        278,
        "at the `for` head: {:?}",
        diagnostics.all()
    );
    assert!(
        unbounded.message.contains("unbounded") && unbounded.message.contains("at most"),
        "the message names the bound law and the fix: {}",
        unbounded.message
    );
}

// ---------------------------------------------------------------------------
// Item 3 — D07: a dropped root reports once, not at every reference.
// ---------------------------------------------------------------------------

/// One missing `marrow.ids` field row drops `^members` from the durable registry. The
/// primary `check.durable_identity` names the exact fix once; the reference sites are
/// steered to it a single time, and every dependent read of the dropped root (and of the
/// bindings it fed) is suppressed — so a one-line fix does not read as a project-wide
/// failure.
#[test]
fn d07_a_dropped_root_reports_one_primary_and_one_steer() {
    let ids = fixture_file("club_locker", "marrow.ids");
    let stripped = ids
        .lines()
        .filter(|line| !line.contains("id field Member.email"))
        .collect::<Vec<_>>()
        .join("\n");
    let project = Project::from_fixture("club_locker").ids(&format!("{stripped}\n"));
    let diagnostics = diagnostics(&project);
    assert_eq!(
        diagnostics.count_code("check.durable_identity"),
        1,
        "one primary per missing row: {:?}",
        diagnostics.all()
    );
    assert_eq!(
        diagnostics.count_code("check.type"),
        1,
        "one reference steer, not an echo at every use: {:?}",
        diagnostics.all()
    );
    assert!(
        diagnostics
            .messages()
            .iter()
            .all(|message| !message.contains("is not in scope")),
        "no dependent binding re-reports as an unknown name: {:?}",
        diagnostics.all()
    );
    assert!(
        diagnostics
            .only("check.type")
            .message
            .contains("failed identity admission"),
        "the single steer points at the identity reports",
    );
}

// ---------------------------------------------------------------------------
// Item 4 — D04: a failed initializer poisons its name; no scope cascade.
// ---------------------------------------------------------------------------

/// A forgotten `??` leaves an optional in arithmetic. The precise primary
/// (`+` is not defined for int? and int`) fires once; the `const` whose initializer
/// failed is poisoned, so its later uses raise no `is not in scope` cascade.
#[test]
fn d04_a_failed_binding_does_not_cascade_not_in_scope() {
    let project = club_locker_mutated(
        "^idseq[\"member\"]\n        const next = (seq.value ?? 0) + 1",
        "^idseq[\"member\"]\n        const next = (seq.value) + 1",
    );
    let diagnostics = diagnostics(&project);
    let primary = diagnostics.only("check.type");
    assert_eq!(
        primary.line(),
        74,
        "at the optional arithmetic: {:?}",
        diagnostics.all()
    );
    assert!(
        primary.message.contains("int?"),
        "the primary names the optional operand: {}",
        primary.message
    );
    assert!(
        diagnostics
            .messages()
            .iter()
            .all(|message| !message.contains("is not in scope")),
        "the poisoned `next` raises no scope cascade: {:?}",
        diagnostics.all()
    );
}

// ---------------------------------------------------------------------------
// Item 5 — D08/D10: not-in-scope distinguishes family and offers one candidate.
// ---------------------------------------------------------------------------

/// A misspelled store root offers the nearest declared root, spelled as a root.
#[test]
fn d08_a_misspelled_root_suggests_the_nearest_store_root() {
    let project = club_locker_mutated(
        "    return ^members[memberId].name\n}",
        "    return ^membrs[memberId].name\n}",
    );
    let diagnostics = diagnostics(&project);
    let unknown = diagnostics.only("check.type");
    assert_eq!(
        unknown.line(),
        235,
        "at the reference: {:?}",
        diagnostics.all()
    );
    assert!(
        unknown
            .message
            .contains("Did you mean the store root `^members`?"),
        "the root family is named with its nearest candidate: {}",
        unknown.message
    );
}

/// A misspelled cross-module callee offers the nearest function in the named module,
/// spelled as a function — the family distinct from a store root.
#[test]
fn d10_a_misspelled_callee_suggests_the_nearest_function() {
    let source = fixture_file("cross_module_roots", "src/teller.mw");
    let mutated = source.replace(
        "        return updated\n",
        "        bank::opnAccount(id)\n        return updated\n",
    );
    let project = Project::from_fixture("cross_module_roots").source("src/teller.mw", &mutated);
    let diagnostics = diagnostics(&project);
    let unknown = diagnostics.only("check.type");
    assert!(
        unknown
            .message
            .contains("Did you mean the function `openAccount`?"),
        "the function family is named with its nearest candidate: {}",
        unknown.message
    );
}
