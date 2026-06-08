//! Tier-2 end-to-end coverage of run-time evolution auto-apply through the `marrow`
//! binary. When the activation fence reports schema drift at the current epoch, a bare
//! `marrow run` discharges the evolution itself if doing so mutates zero stored records,
//! and otherwise fences with an actionable diagnostic naming `evolve apply`.
//!
//! Oracles are typed: process exit codes, the accepted catalog file's epoch, the
//! structured error `code`, and decoded stored value bytes — never a substring of
//! human-rendered prose. The predicate under test is "does discharging the evolution
//! mutate any stored record", so every fixture pins the same affected store at empty vs
//! populated and asserts the opposite outcome.

use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

mod support;
mod support_evolve;

use support::{TempProject, marrow, marrow_sub, temp_project_uncommitted, write};
use support_evolve::{native_store_path, read_scalar, root_place};

/// A two-store project: `^log` is written by the `seed` default entry so the first run
/// stamps the store file, while `^books` is left to the test to populate or leave empty.
/// The affected store in every evolution below is `^books`, so seeding `^log` controls
/// only whether the store file carries a stamp, not whether the evolution has records to
/// mutate.
fn books_and_log_project(name: &str, source: &str) -> TempProject {
    temp_project_uncommitted(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::seed" } }"#,
        );
        write(root, "src/app.mw", source);
    })
}

fn dir(root: &TempProject) -> &str {
    root.to_str().expect("project path utf8")
}

fn accepted_epoch(root: &TempProject) -> u64 {
    support_evolve::accepted_catalog(root).epoch
}

/// The baseline: a `Book` with only `title`, plus a `Log` the default `seed` writes so
/// the store file is stamped. `seedBook` writes one `Book` so a test can populate the
/// affected store; the default `seed` never touches `^books`.
const BASELINE: &str = "module app\n\
     resource Log at ^log(id: int)\n\
     \x20   note: string\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     pub fn seedBook()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n";

/// A required `pages` field added to `Book` with a constant `evolve default`. Over an
/// empty `^books` it backfills nothing; over a populated `^books` it backfills each
/// record.
const REQUIRED_ADD: &str = "module app\n\
     resource Log at ^log(id: int)\n\
     \x20   note: string\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     \x20   required pages: int\n\
     evolve\n\
     \x20   default Book.pages = 0\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     pub fn seedBook()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     \x20       ^books(1).pages = 7\n";

/// `Book.subtitle` retired from source via an `evolve retire` intent. Over an empty (or
/// subtitle-free) `^books` the retire drops nothing; over records that carry a subtitle
/// it is a destructive drop.
const RETIRE_SUBTITLE: &str = "module app\n\
     resource Log at ^log(id: int)\n\
     \x20   note: string\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     evolve\n\
     \x20   retire Book.subtitle\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     pub fn seedBook()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n";

/// The baseline carrying a `subtitle` the retire above consumes, plus a `seedBook` that
/// writes a populated subtitle so the drop has data to lose.
const RETIRE_BASELINE: &str = "module app\n\
     resource Log at ^log(id: int)\n\
     \x20   note: string\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     pub fn seedBook()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     \x20       ^books(1).subtitle = \"a novel\"\n";

#[test]
fn a_required_add_against_an_empty_store_auto_applies_on_run() {
    // The affected store `^books` is empty when the evolution lands, so the required
    // field has zero records to backfill: emptiness discharges the obligation and the run
    // auto-applies, advancing the epoch by one.
    let root = books_and_log_project("run-autoapply-required-empty", BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/app.mw", REQUIRED_ADD);
    let rerun = marrow_sub("run", &[dir(&root)]);

    assert_eq!(
        rerun.status.code(),
        Some(0),
        "a required add over an empty store auto-applies: {rerun:?}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "the auto-apply advanced the epoch by one",
    );
}

#[test]
fn the_same_required_add_against_a_populated_store_fences_and_evolve_apply_backfills() {
    // The identical source edit over a populated `^books` has records to backfill, so the
    // run must fence with the actionable schema-drift diagnostic. The explicit
    // `evolve apply` then discharges the backfill and the constant default lands on the
    // pre-existing record.
    let root = books_and_log_project("run-autoapply-required-populated", BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let seed_book = marrow_sub("run", &["--entry", "app::seedBook", dir(&root)]);
    assert_eq!(seed_book.status.code(), Some(0), "seed book: {seed_book:?}");
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/app.mw", REQUIRED_ADD);
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        rerun.status.code(),
        Some(1),
        "a required add over a populated store fences: {rerun:?}",
    );
    let stderr = String::from_utf8(rerun.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift") && stderr.contains("evolve apply"),
        "the fence names schema drift and evolve apply: {stderr}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before,
        "a fenced run does not advance the epoch",
    );

    let apply = marrow(&["evolve", "apply", dir(&root)]);
    assert_eq!(apply.status.code(), Some(0), "evolve apply: {apply:?}");
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "explicit apply advances the epoch the fenced run left untouched",
    );
    let config_text = std::fs::read_to_string(root.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    let (report, program) =
        marrow_check::check_project(root.path(), &config).expect("re-check after apply");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let place = root_place(&program, "books");
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &place, 1, "pages", ScalarType::Int),
        Some(Scalar::Int(0)),
        "evolve apply backfilled the constant default onto the populated record",
    );
}

#[test]
fn a_drop_against_an_empty_target_auto_applies_but_a_populated_drop_fences() {
    // A retire whose target carries no stored cells drops nothing, so the run auto-applies
    // it. The same retire against records that carry the subtitle is a destructive drop:
    // losing data must never be a silent side effect of `run`, so it fences and stays
    // explicit even though the change is otherwise valid.
    let empty = books_and_log_project("run-autoapply-drop-empty", RETIRE_BASELINE);
    let first = marrow_sub("run", &[dir(&empty)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let epoch_before = accepted_epoch(&empty);
    write(&empty, "src/app.mw", RETIRE_SUBTITLE);
    let rerun = marrow_sub("run", &[dir(&empty)]);
    assert_eq!(
        rerun.status.code(),
        Some(0),
        "a retire whose target is empty auto-applies: {rerun:?}",
    );
    assert_eq!(
        accepted_epoch(&empty),
        epoch_before + 1,
        "the empty-target drop advanced the epoch by one",
    );

    let populated = books_and_log_project("run-autoapply-drop-populated", RETIRE_BASELINE);
    let first = marrow_sub("run", &[dir(&populated)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let seed_book = marrow_sub("run", &["--entry", "app::seedBook", dir(&populated)]);
    assert_eq!(seed_book.status.code(), Some(0), "seed book: {seed_book:?}");
    let epoch_before = accepted_epoch(&populated);
    write(&populated, "src/app.mw", RETIRE_SUBTITLE);
    let rerun = marrow_sub("run", &[dir(&populated)]);
    assert_eq!(
        rerun.status.code(),
        Some(1),
        "a destructive drop over populated data fences: {rerun:?}",
    );
    let stderr = String::from_utf8(rerun.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift") && stderr.contains("evolve apply"),
        "the destructive-drop fence names schema drift and evolve apply: {stderr}",
    );
    assert_eq!(
        accepted_epoch(&populated),
        epoch_before,
        "a fenced destructive drop does not advance the epoch",
    );
}

#[test]
fn an_auto_applied_binary_passes_its_own_fence_on_a_re_run() {
    // The crash-resume invariant: a binary that auto-applies an evolution writes the new
    // shape it expects, so running the same binary again finds the store at the matching
    // epoch and digest and proceeds with no spurious drift and no second epoch advance.
    let root = books_and_log_project("run-autoapply-resume", BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");

    write(&root, "src/app.mw", REQUIRED_ADD);
    let auto = marrow_sub("run", &[dir(&root)]);
    assert_eq!(auto.status.code(), Some(0), "auto-apply run: {auto:?}");
    let epoch_after_auto = accepted_epoch(&root);

    let resume = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "the same binary passes its own fence on re-run: {resume:?}",
    );
    let stderr = String::from_utf8(resume.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.schema_drift"),
        "a re-run after auto-apply must not read as drift: {stderr}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_after_auto,
        "the re-run does not advance the epoch a second time",
    );
}

/// A multi-store evolution: a required field is added to both `^books` and `^shelf` in
/// one edit. `^books` is left empty and `^shelf` is populated, so the evolution as a
/// whole carries a backfill obligation and must not auto-apply even though one store's
/// share of it is zero.
const MULTI_BASELINE: &str = "module app\n\
     resource Log at ^log(id: int)\n\
     \x20   note: string\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     resource Shelf at ^shelf(id: int)\n\
     \x20   required name: string\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     \x20       ^shelf(1).name = \"fiction\"\n";

const MULTI_REQUIRED_ADD: &str = "module app\n\
     resource Log at ^log(id: int)\n\
     \x20   note: string\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     \x20   required pages: int\n\
     resource Shelf at ^shelf(id: int)\n\
     \x20   required name: string\n\
     \x20   required capacity: int\n\
     evolve\n\
     \x20   default Book.pages = 0\n\
     \x20   default Shelf.capacity = 0\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     \x20       ^shelf(1).name = \"fiction\"\n";

#[test]
fn a_multi_store_evolution_with_one_empty_and_one_populated_store_fences_as_a_whole() {
    // The `seed` entry populates `^shelf` but never `^books`, so adding a required field
    // to both stores backfills the shelf record while the book store has nothing to do.
    // The obligation is computed over the whole evolution, so the populated half makes it
    // fence: a run never auto-applies a change that mutates any stored record.
    let root = books_and_log_project("run-autoapply-multi", MULTI_BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/app.mw", MULTI_REQUIRED_ADD);
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        rerun.status.code(),
        Some(1),
        "a multi-store evolution with a populated half fences: {rerun:?}",
    );
    let stderr = String::from_utf8(rerun.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift") && stderr.contains("evolve apply"),
        "the multi-store fence names schema drift and evolve apply: {stderr}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before,
        "the fenced multi-store run does not advance the epoch",
    );
}
