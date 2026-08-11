//! Two store roots over one resource, driven through the real `marrow` binary.
//!
//! A resource is a declaration and a store root is an occurrence of it, so declaring the
//! same resource at two placements is an ordinary program. Every other test of that split
//! reaches it through the library; these reach it the way a person does — `marrow check`,
//! `marrow check --demand`, `marrow run`, and `marrow image` over a project directory —
//! because a split the CLI cannot carry is not one a program can use.
//!
//! Before this row the whole family stopped at the same place: the compiled image did not
//! verify, so `check` printed `image.table`, `run` never reached its export, and `image`
//! never reached its ceiling review. Each case below is paired with the identical
//! single-root project, so what is asserted is that sharing a resource changes the outcome
//! in no way at all.

mod common;

use common::Project;

/// The identity ledger for the shared-resource project: one `product Book` row and one row
/// per Product-scoped member, against one `root`/`key` pair per occurrence.
const SHARED_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

/// The same ledger without `^b`, for the single-root control.
const SINGLE_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

const RESOURCE: &str = "resource Book {\n\
    \x20   required title: string\n\
    \x20   notes[noteId: int] {\n\
    \x20       required text: string\n\
    \x20   }\n\
}\n\n";

/// Two keyed roots over one `Book`, each with its own export writing its own root's nested
/// branch.
fn shared() -> Project {
    Project::single(&format!(
        "{RESOURCE}\
         store ^a[id: int]: Book\n\
         store ^b[id: int]: Book\n\n\
         pub fn addA(id: int, t: string) {{\n\
         \x20   transaction {{\n\
         \x20       ^a[id].notes[1].text = t\n\
         \x20   }}\n\
         }}\n\n\
         pub fn addB(id: int, t: string) {{\n\
         \x20   transaction {{\n\
         \x20       ^b[id].notes[1].text = t\n\
         \x20   }}\n\
         }}\n"
    ))
    .ids(SHARED_IDS)
}

/// The identical project with only `^a`.
fn single() -> Project {
    Project::single(&format!(
        "{RESOURCE}\
         store ^a[id: int]: Book\n\n\
         pub fn addA(id: int, t: string) {{\n\
         \x20   transaction {{\n\
         \x20       ^a[id].notes[1].text = t\n\
         \x20   }}\n\
         }}\n"
    ))
    .ids(SINGLE_IDS)
}

/// Red R1: `marrow check` over a project whose two keyed roots share one resource with a
/// nested branch prints the ordinary two-export demand summary and exits 0.
///
/// At the lane base this printed `image.table: the compiled image did not verify` and
/// exited 1 — the compiler emitted the Product's branch entry record once per root, and the
/// independent verifier rejected the duplicate durable identity.
#[test]
fn check_reports_a_clean_two_export_summary_for_two_roots_over_one_resource() {
    let output = shared().run_cli("shared-check", &["check"]);
    assert!(
        output.success(),
        "two roots over one resource is an ordinary program: {}",
        output.stderr_text(),
    );
    assert_eq!(
        output.stdout_text(),
        "2 exports across 1 module\n\
         \n\
         main: 2 exports\n\
         \x20 addA\n\
         \x20   writes ^a (+1 place)\n\
         \x20 addB\n\
         \x20   writes ^b (+1 place)\n",
    );
}

/// Red R27: each occurrence's demand sentence stays qualified by the root it names.
///
/// The two exports touch the same Product-scoped declaration nodes — `Book.notes` and
/// `Book.notes.text` carry one ledger identity between them — so a demand keyed on the
/// declaration rather than the occurrence would render one sentence for both roots, or the
/// same root twice. The sentences must name `^a` and `^b`, and the child place each
/// reaches must be spelled identically from either root.
#[test]
fn each_occurrence_demand_sentence_names_its_own_root() {
    let output = shared().run_cli("shared-demand", &["check", "--demand"]);
    assert!(output.success(), "{}", output.stderr_text());
    assert_eq!(
        output.stdout_text(),
        "main.addA writes ^a.notes.text\n\
         main.addB writes ^b.notes.text\n",
    );
}

/// Red R2: `marrow run` over the shared-resource project reaches exactly the point the
/// identical single-root project reaches.
///
/// The assertion is the *pairing*: whatever the run path answers for one root, it answers
/// for two, because a second occurrence of one declaration is not a fact about execution.
/// At the lane base the shared project stopped at `image.table` while the control ran on.
#[test]
fn run_reaches_the_same_point_as_the_single_root_control() {
    let shared = shared().run_cli("shared-run", &["run", "addA", "--", "1", "x"]);
    let control = single().run_cli("single-run", &["run", "addA", "--", "1", "x"]);
    assert_eq!(
        (shared.success(), shared.code()),
        (control.success(), control.code()),
        "shared: {}\ncontrol: {}",
        shared.stderr_text(),
        control.stderr_text(),
    );
    assert_eq!(
        typed_code(&shared.stderr_text()),
        typed_code(&control.stderr_text()),
        "the two runs report the same typed outcome",
    );
}

/// Red R3: `marrow image` over the shared-resource project reaches its ceiling review.
///
/// An unaccepted ceiling id is the point the command is meant to stop at, and an unrelated
/// `--accept-ceiling` value must be refused as an unaccepted ceiling naming the real id —
/// not as an image that failed verification, which is where the lane base stopped.
#[test]
fn image_reaches_its_ceiling_review_for_two_roots_over_one_resource() {
    let workspace = shared().materialize("shared-image");
    let outcome = workspace.marrow(&["image", "--out", "deploy", "--accept-ceiling", "zzz"]);

    assert!(!outcome.success(), "an unrelated ceiling id is refused");
    let stderr = outcome.stderr_text();
    assert!(
        stderr.contains("cli.ceiling_unaccepted"),
        "the command reached its ceiling review: {stderr}",
    );
    let named = stderr
        .split_whitespace()
        .find(|word| {
            let id = word.trim_end_matches('.');
            id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit())
        })
        .unwrap_or_else(|| panic!("the refusal names this image's real ceiling id: {stderr}"));
    assert!(named.starts_with(|c: char| c.is_ascii_hexdigit()));

    // The review the owner is sent to renders both occurrences' demand, each named by its
    // own root: one declaration under two roots is still two sets of durable places.
    let unaccepted = workspace.marrow(&["image", "--out", "review"]);
    let review = unaccepted.stderr_text();
    assert!(
        review.contains("writes ^a") && review.contains("writes ^b"),
        "both occurrences' demand is rendered for review: {review}",
    );
    assert!(
        !workspace.path("deploy/program.image").exists()
            && !workspace.path("review/program.image").exists(),
        "no image is written when the ceiling is unaccepted",
    );
}

/// The first `cli.*` or `check.*` typed code named on `stderr`, or the whole text when it
/// names none.
fn typed_code(stderr: &str) -> String {
    stderr
        .split_whitespace()
        .find(|word| word.starts_with("cli.") || word.starts_with("check."))
        .unwrap_or(stderr)
        .trim_end_matches(':')
        .to_string()
}
