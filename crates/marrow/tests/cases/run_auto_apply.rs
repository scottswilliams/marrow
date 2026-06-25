//! Tier-2 end-to-end coverage of run-time evolution auto-apply through the `marrow`
//! binary. When the activation fence reports schema drift at the current epoch, a bare
//! `marrow run` discharges the evolution itself if doing so mutates zero stored records,
//! and otherwise fences with the `run.schema_drift` diagnostic.
//!
//! Oracles are typed: process exit codes, the accepted catalog file's epoch, the
//! structured error `code`, and decoded stored value bytes — never a substring of
//! human-rendered prose. The predicate under test is "does discharging the evolution
//! mutate any stored record", so every fixture pins the same affected store at empty vs
//! populated and asserts the opposite outcome.

use crate::support;
use crate::support_evolve;
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};
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

/// Assert the stored `code` of `^books(1)`, re-checking the project against its committed
/// catalog so the saved-place fact resolves the member's current stable id. This is the
/// typed oracle for "did the transform run", reading decoded value bytes rather than prose.
fn assert_code(
    root: &TempProject,
    expected: i64,
    context: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_text = std::fs::read_to_string(root.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    let accepted = TreeStore::open_read_only(&native_store_path(root))
        .expect("open store read-only")
        .read_catalog_snapshot()
        .expect("read store catalog snapshot");
    let (report, program) =
        marrow_check::check_project_with_catalog(root.path(), &config, accepted.as_ref())
            .expect("re-check project");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let place = root_place(&program, "books")?;
    let store = TreeStore::open(&native_store_path(root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &place, 1, "code", ScalarType::Int),
        Some(Scalar::Int(expected)),
        "{context}",
    );
    Ok(())
}

fn commit_stamp(root: &TempProject) -> marrow_store::tree::CommitMetadata {
    TreeStore::open_read_only(&native_store_path(root))
        .expect("open store read-only")
        .read_commit_metadata()
        .expect("read commit stamp")
        .expect("store has a commit stamp")
}

fn committed_lock(root: &TempProject) -> marrow_catalog::CatalogLock {
    marrow_check::read_committed_lock(root.path())
        .expect("read committed lock")
        .expect("project has a committed lock")
}

/// The baseline: a `Book` with only `title`, plus a `Log` the default `seed` writes so
/// the store file is stamped. `seedBook` writes one `Book` so a test can populate the
/// affected store; the default `seed` never touches `^books`.
const BASELINE: &str = "module app\n\
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
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
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required pages: int\n\
     store ^books(id: int): Book\n\
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
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
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
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     store ^books(id: int): Book\n\
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
        String::from_utf8(rerun.stdout).expect("stdout utf8"),
        "",
        "auto-apply notice must not contaminate program stdout"
    );
    let notice = String::from_utf8(rerun.stderr).expect("stderr utf8");
    let notice_lines: Vec<&str> = notice.lines().collect();
    // Auto-apply re-projects the committed lock and applies the saved-data change, so the run
    // announces both in plain language. The epoch transition is a storage internal reserved for
    // the JSON envelope, not the everyday human line.
    assert!(
        notice_lines
            .iter()
            .any(|line| line.contains("applied saved-data changes") && line.contains("marrow.lock")),
        "auto-apply must announce the applied saved-data change in plain language: {notice:?}"
    );
    assert!(
        !notice.contains("catalog epoch") && !notice.contains("auto-applied evolution"),
        "the human auto-apply line must not leak the catalog epoch transition: {notice:?}"
    );
    assert!(
        notice_lines
            .iter()
            .any(|line| line.contains("marrow.lock") && line.contains("commit")),
        "auto-apply re-projects marrow.lock and announces it for commit: {notice:?}"
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "the auto-apply advanced the epoch by one",
    );
}

#[test]
fn the_same_required_add_against_a_populated_store_fences_and_evolve_apply_backfills()
-> Result<(), Box<dyn std::error::Error>> {
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
        stderr.contains("run.schema_drift"),
        "the fence reports the schema-drift code: {stderr}",
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
    // Bind the program against the store's now-advanced accepted catalog so its saved roots
    // carry the catalog ids the post-apply store keys cells under.
    let accepted = TreeStore::open_read_only(&native_store_path(&root))
        .expect("open store read-only")
        .read_catalog_snapshot()
        .expect("read store catalog snapshot");
    let (report, program) =
        marrow_check::check_project_with_catalog(root.path(), &config, accepted.as_ref())
            .expect("re-check after apply");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let place = root_place(&program, "books")?;
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &place, 1, "pages", ScalarType::Int),
        Some(Scalar::Int(0)),
        "evolve apply backfilled the constant default onto the populated record",
    );

    Ok(())
}

/// A baseline whose `Book` already carries both `title` and `code`, so an `evolve
/// transform` over `code` recomputes an existing member in place and proposes no new
/// catalog entry — a shape-neutral data migration that does not move the source digest.
const TRANSFORM_BASELINE: &str = "module app\n\
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required code: int\n\
     store ^books(id: int): Book\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     pub fn seedBook()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     \x20       ^books(1).code = 5\n";

/// The same shape, plus a live `evolve transform` that recomputes `Book.code` from
/// `old.title`. The transform changes no durable shape, so its target epoch and source
/// digest equal the ones the store already carries; only the live transform intent
/// distinguishes a pending migration from a settled store.
const TRANSFORM_IN_PLACE: &str = "module app\n\
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required code: int\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   transform Book.code\n\
     \x20       return std::text::length(old.title)\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     pub fn seedBook()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     \x20       ^books(1).code = 5\n";

#[test]
fn an_in_place_transform_over_a_populated_store_fences_run_until_applied()
-> Result<(), Box<dyn std::error::Error>> {
    // A live in-place transform rewrites the `code` of every populated record, so a bare
    // run must not proceed against the un-migrated store: a pending evolution blocks run
    // until it is applied or withdrawn. The transform is shape-neutral, so the activation
    // fence sees a matching epoch and digest and would let the run through — the run path
    // must consult the evolution obligation, not the shape stamp alone, to catch it.
    let root = books_and_log_project("run-in-place-transform-populated", TRANSFORM_BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let seed_book = marrow_sub("run", &["--entry", "app::seedBook", dir(&root)]);
    assert_eq!(seed_book.status.code(), Some(0), "seed book: {seed_book:?}");
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/app.mw", TRANSFORM_IN_PLACE);
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        rerun.status.code(),
        Some(1),
        "a live in-place transform over a populated store fences the run: {rerun:?}",
    );
    let stderr = String::from_utf8(rerun.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift"),
        "the in-place-transform fence reports the schema-drift code: {stderr}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before,
        "a fenced run does not advance the epoch",
    );

    // The explicit apply discharges the transform, recomputing `code` to the title length.
    let apply = marrow(&["evolve", "apply", dir(&root)]);
    assert_eq!(apply.status.code(), Some(0), "evolve apply: {apply:?}");
    let config_text = std::fs::read_to_string(root.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    let accepted = TreeStore::open_read_only(&native_store_path(&root))
        .expect("open store read-only")
        .read_catalog_snapshot()
        .expect("read store catalog snapshot");
    let (report, program) =
        marrow_check::check_project_with_catalog(root.path(), &config, accepted.as_ref())
            .expect("re-check after apply");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let place = root_place(&program, "books")?;
    {
        let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        assert_eq!(
            read_scalar(&store, &place, 1, "code", ScalarType::Int),
            Some(Scalar::Int(4)),
            "evolve apply recomputed code to the length of the title",
        );
    }

    // Apply advances the accepted epoch: a record-rewriting transform is a real catalog
    // change, so the new accepted catalog records the transform as discharged on its target.
    let epoch_after_apply = accepted_epoch(&root);
    assert_eq!(
        epoch_after_apply,
        epoch_before + 1,
        "discharging the in-place transform advances the accepted epoch",
    );

    // The transform is now consumed: a second `evolve apply` with the block still in source
    // discharges nothing rather than re-executing. Were it not stamped, the commit id would
    // climb on every apply and `code` would keep being rewritten unbounded.
    let reapply = marrow(&["evolve", "apply", "--format", "json", dir(&root)]);
    assert_eq!(reapply.status.code(), Some(0), "re-apply: {reapply:?}");
    let reapply_record: serde_json::Value =
        serde_json::from_slice(&reapply.stdout).expect("re-apply json envelope");
    assert_eq!(
        reapply_record.get("records_transformed"),
        Some(&serde_json::json!(0)),
        "a consumed transform re-applies as a no-op: {reapply_record}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_after_apply,
        "a no-op re-apply does not advance the epoch again",
    );

    // `marrow run` recovers: the fence the pending transform raised is gone once the
    // transform is discharged, so a bare run proceeds instead of fencing forever.
    let run_after = marrow_sub("run", &["--entry", "app::seedBook", dir(&root)]);
    assert_eq!(
        run_after.status.code(),
        Some(0),
        "run exits 0 once the in-place transform is applied: {run_after:?}",
    );
    assert!(
        !String::from_utf8(run_after.stderr)
            .expect("stderr utf8")
            .contains("run.schema_drift"),
        "a discharged transform no longer fences the run",
    );

    Ok(())
}

#[test]
fn an_in_place_transform_over_an_empty_store_auto_applies_and_re_runs_cleanly() {
    // The same live in-place transform over an empty `^books` rewrites zero records, so
    // the run discharges it itself rather than fencing. Discharging it advances the epoch
    // and records the transform as consumed on its target, so re-running with the transform
    // still in source must not re-fence or climb the epoch a second time.
    let root = books_and_log_project("run-in-place-transform-empty", TRANSFORM_BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/app.mw", TRANSFORM_IN_PLACE);
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        rerun.status.code(),
        Some(0),
        "an in-place transform over an empty store auto-applies: {rerun:?}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "discharging the transform records it on its target and advances the epoch",
    );

    let resume = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "re-running with the transform still in source stays clean: {resume:?}",
    );
    assert!(
        !String::from_utf8(resume.stderr)
            .expect("stderr utf8")
            .contains("run.schema_drift"),
        "a settled transform does not read as drift on re-run",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "a settled transform does not advance the epoch a second time",
    );
}

/// The discharged in-place transform, kept in source, plus an unrelated durable edit — a
/// new module `const`. The const changes the program's whole-program shape but touches
/// neither the transform's target nor its body, so the transform's per-transform identity
/// is unchanged and it must stay discharged rather than re-execute against current data.
const TRANSFORM_IN_PLACE_PLUS_UNRELATED_CONST: &str = "module app\n\
     const PAD: int = 1\n\
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required code: int\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   transform Book.code\n\
     \x20       return std::text::length(old.title)\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     pub fn seedBook()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     \x20       ^books(1).code = 5\n";

/// The same shape, plus a *changed* transform body (multiplied by ten). The new body is a
/// fresh obligation: its per-transform identity differs from the stored mark, so it must
/// re-execute and advance the epoch again, recomputing `code` to `10 * length(title)`.
const TRANSFORM_IN_PLACE_CHANGED_BODY: &str = "module app\n\
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required code: int\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   transform Book.code\n\
     \x20       return std::text::length(old.title) * 10\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     pub fn seedBook()\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"Mort\"\n\
     \x20       ^books(1).code = 5\n";

#[test]
fn a_discharged_transform_does_not_re_run_on_a_later_unrelated_durable_edit()
-> Result<(), Box<dyn std::error::Error>> {
    // The corruption guard. Once a transform is applied, its mark is keyed on the
    // transform's own identity (target + body), not the whole-program shape. An unrelated
    // durable edit moves the program shape but not that identity, so the transform stays
    // discharged: it must not re-execute against current data, fence the run, or climb the
    // epoch on every subsequent edit.
    let root = books_and_log_project("transform-no-rerun-on-unrelated-edit", TRANSFORM_BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let seed_book = marrow_sub("run", &["--entry", "app::seedBook", dir(&root)]);
    assert_eq!(seed_book.status.code(), Some(0), "seed book: {seed_book:?}");

    write(&root, "src/app.mw", TRANSFORM_IN_PLACE);
    let apply = marrow(&["evolve", "apply", dir(&root)]);
    assert_eq!(apply.status.code(), Some(0), "evolve apply: {apply:?}");
    let epoch_after_apply = accepted_epoch(&root);
    assert_code(
        &root,
        4,
        "apply ran the transform once (length of \"Mort\")",
    )?;

    // An unrelated durable edit: a new module-level constant. It must neither re-run the
    // discharged transform nor fence the run, and the epoch must not climb.
    write(&root, "src/app.mw", TRANSFORM_IN_PLACE_PLUS_UNRELATED_CONST);
    let run_after_const = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        run_after_const.status.code(),
        Some(0),
        "an unrelated const does not re-fence the discharged transform: {run_after_const:?}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_after_apply,
        "an unrelated edit does not re-run the transform or advance the epoch",
    );

    // A preview after the unrelated edit reports nothing to transform: the obligation is
    // settled by identity, not by a digest that the const moved.
    let preview = marrow(&["evolve", "preview", "--format", "json", dir(&root)]);
    assert_eq!(preview.status.code(), Some(0), "preview: {preview:?}");
    let preview_record: serde_json::Value =
        serde_json::from_slice(&preview.stdout).expect("preview json envelope");
    assert_eq!(
        preview_record.get("records_to_transform"),
        Some(&serde_json::json!(0)),
        "a settled transform has no records left to transform: {preview_record}",
    );

    // A *changed* transform body is a genuinely new obligation: it re-executes and advances
    // the epoch again, recomputing `code` to ten times the title length (40).
    write(&root, "src/app.mw", TRANSFORM_IN_PLACE_CHANGED_BODY);
    let reapply = marrow(&["evolve", "apply", dir(&root)]);
    assert_eq!(
        reapply.status.code(),
        Some(0),
        "changed-body apply: {reapply:?}"
    );
    assert_code(&root, 40, "a changed transform body is a fresh obligation")?;
    let epoch_after_changed = accepted_epoch(&root);
    assert_eq!(
        epoch_after_changed,
        epoch_after_apply + 1,
        "discharging the changed-body transform advances the epoch again",
    );

    // Deleting the consumed block (run cleanly with no block), then re-declaring the identical
    // changed body, reproduces the same identity already recorded on the target — so the
    // re-declaration is recognized as work already done and does not re-run. The block only
    // describes a completed migration.
    write(&root, "src/app.mw", TRANSFORM_BASELINE);
    let run_without_block = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        run_without_block.status.code(),
        Some(0),
        "deleting a consumed block runs cleanly: {run_without_block:?}",
    );
    write(&root, "src/app.mw", TRANSFORM_IN_PLACE_CHANGED_BODY);
    let run_after_readd = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        run_after_readd.status.code(),
        Some(0),
        "an identical re-declaration is suppressed, not re-run: {run_after_readd:?}",
    );
    assert_code(
        &root,
        40,
        "an identical re-declaration does not re-run the transform",
    )?;
    assert_eq!(
        accepted_epoch(&root),
        epoch_after_changed,
        "an identical re-declaration does not advance the epoch",
    );

    Ok(())
}

#[test]
fn a_discharged_transform_survives_backup_and_restore() -> Result<(), Box<dyn std::error::Error>> {
    // `applied_transform` is durable catalog state. A backup taken after apply and restored
    // into a fresh store must still report the transform discharged: a restored store does
    // not re-run the migration, and `code` keeps its applied value.
    let root = books_and_log_project("transform-survives-backup-restore", TRANSFORM_BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let seed_book = marrow_sub("run", &["--entry", "app::seedBook", dir(&root)]);
    assert_eq!(seed_book.status.code(), Some(0), "seed book: {seed_book:?}");

    write(&root, "src/app.mw", TRANSFORM_IN_PLACE);
    let apply = marrow(&["evolve", "apply", dir(&root)]);
    assert_eq!(apply.status.code(), Some(0), "evolve apply: {apply:?}");
    assert_code(&root, 4, "apply ran the transform once")?;
    let epoch_after_apply = accepted_epoch(&root);

    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().expect("archive path utf8").to_string();
    let backup = marrow(&["backup", dir(&root), &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    // Lose the store, then restore: the backup carries the durable catalog rows, so the
    // restored snapshot must replay the recorded transform mark.
    std::fs::remove_dir_all(root.join(".data")).expect("remove store data");
    let restore = marrow(&["restore", dir(&root), &archive_arg]);
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");

    // The transform block is still in source. A restored store that lost the discharge mark
    // would re-fence the run and re-run the migration; a sound restore keeps the mark, so the
    // run is a clean no-op that leaves both the value and the epoch untouched.
    let run_after = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        run_after.status.code(),
        Some(0),
        "a restored store reports the transform discharged: {run_after:?}",
    );
    assert_code(&root, 4, "the restored store did not re-run the transform")?;
    assert_eq!(
        accepted_epoch(&root),
        epoch_after_apply,
        "a restored discharged transform does not advance the epoch again",
    );

    Ok(())
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
        stderr.contains("run.schema_drift"),
        "the destructive-drop fence reports the schema-drift code: {stderr}",
    );
    assert_eq!(
        accepted_epoch(&populated),
        epoch_before,
        "a fenced destructive drop does not advance the epoch",
    );
}

#[test]
fn an_auto_applied_binary_passes_its_own_fence_on_a_re_run() {
    // The same-binary rerun invariant: a binary that auto-applies an evolution writes the
    // new shape it expects, so running it again finds the store at the matching epoch and
    // digest and proceeds with no spurious drift and no second epoch advance.
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

#[test]
fn an_auto_applying_run_reprojects_the_committed_lock_in_one_pass() {
    // A single auto-applying run must converge the committed lock, not just the store. The
    // first run projects marrow.lock at the baseline epoch and digest. An additive edit
    // drifts the source from that lock, so the next run auto-applies the evolution and
    // advances the store. The committed lock is the store's source-tree projection, so the
    // same write path must re-project it: a single run leaves `check --locked` green and the
    // committed lock's recorded source digest and high-water epoch matching the new source.
    // Before the fix the auto-apply path re-ran the fence without re-projecting, so the lock
    // kept the stale baseline digest and `check --locked` stayed fatal until a second run.
    let root = books_and_log_project("run-autoapply-reprojects-lock", BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let lock_before = committed_lock(&root);
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/app.mw", REQUIRED_ADD);
    let auto = marrow_sub("run", &[dir(&root)]);
    assert_eq!(auto.status.code(), Some(0), "auto-apply run: {auto:?}");
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "the auto-apply advanced the accepted epoch by one",
    );

    let locked = marrow(&["check", "--locked", dir(&root)]);
    assert_eq!(
        locked.status.code(),
        Some(0),
        "a single auto-applying run converges the lock so --locked passes: {locked:?}",
    );
    assert!(
        !String::from_utf8(locked.stderr)
            .expect("stderr utf8")
            .contains("check.stale_lock"),
        "the converged lock raises no stale-lock condition",
    );

    let lock_after = committed_lock(&root);
    assert_ne!(
        lock_after.source_digest, lock_before.source_digest,
        "the re-projected lock records the new source digest",
    );
    assert_eq!(
        lock_after.epoch_high_water,
        epoch_before + 1,
        "the re-projected lock advances its high-water epoch to the new source",
    );
}

#[test]
fn an_auto_apply_surfaces_the_epoch_transition_in_the_json_envelope() {
    // A JSON-mode run that auto-applies an evolution must report the schema change in the
    // structured envelope so a tool consuming stdout — not just a human reading stderr —
    // learns the store advanced. The envelope carries the `from -> to` epoch transition
    // alongside the advanced store stamp; a run that applies nothing carries no such field.
    let root = books_and_log_project("run-autoapply-json-notice", BASELINE);
    let first = marrow_sub("run", &["--format", "json", dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let epoch_before = accepted_epoch(&root);
    let first_envelope: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("first run json envelope");
    assert!(
        first_envelope.get("auto_applied").is_none(),
        "a run that applies no evolution carries no auto_applied field: {first_envelope}",
    );

    write(&root, "src/app.mw", REQUIRED_ADD);
    let auto = marrow_sub("run", &["--format", "json", dir(&root)]);
    assert_eq!(auto.status.code(), Some(0), "auto-apply run: {auto:?}");
    let envelope: serde_json::Value =
        serde_json::from_slice(&auto.stdout).expect("auto-apply json envelope");
    assert_eq!(
        envelope.get("auto_applied"),
        Some(&serde_json::json!({
            "from_epoch": epoch_before,
            "to_epoch": epoch_before + 1,
        })),
        "the JSON envelope names the auto-applied epoch transition: {envelope}",
    );
}

/// A multi-store evolution: a required field is added to both `^books` and `^shelf` in
/// one edit. `^books` is left empty and `^shelf` is populated, so the evolution as a
/// whole carries a backfill obligation and must not auto-apply even though one store's
/// share of it is zero.
const MULTI_BASELINE: &str = "module app\n\
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     resource Shelf\n\
     \x20   required name: string\n\
     store ^shelf(id: int): Shelf\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     \x20       ^shelf(1).name = \"fiction\"\n";

const MULTI_REQUIRED_ADD: &str = "module app\n\
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required pages: int\n\
     store ^books(id: int): Book\n\
     resource Shelf\n\
     \x20   required name: string\n\
     \x20   required capacity: int\n\
     store ^shelf(id: int): Shelf\n\
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
        stderr.contains("run.schema_drift"),
        "the multi-store fence reports the schema-drift code: {stderr}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before,
        "the fenced multi-store run does not advance the epoch",
    );
}

const ENUM_REORDER_BASELINE: &str = "module app\n\
     enum Status\n\
     \x20   active\n\
     \x20   archived\n\
     resource Log\n\
     \x20   required state: Status\n\
     store ^log(id: int): Log\n\
     pub fn seed()\n\
     \x20   var log: Log\n\
     \x20   log.state = Status::active\n\
     \x20   transaction\n\
     \x20       ^log(1) = log\n";

const ENUM_REORDERED: &str = "module app\n\
     enum Status\n\
     \x20   archived\n\
     \x20   active\n\
     resource Log\n\
     \x20   required state: Status\n\
     store ^log(id: int): Log\n\
     pub fn seed()\n\
     \x20   var log: Log\n\
     \x20   log.state = Status::active\n\
     \x20   transaction\n\
     \x20       ^log(1) = log\n";

#[test]
fn an_enum_member_reorder_restamps_instead_of_fencing_run() {
    let root = books_and_log_project("run-autoapply-enum-reorder", ENUM_REORDER_BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let epoch_before = accepted_epoch(&root);
    let before = commit_stamp(&root);

    write(&root, "src/app.mw", ENUM_REORDERED);
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        rerun.status.code(),
        Some(0),
        "a pure enum-member reorder is an identity-preserving restamp: {rerun:?}",
    );

    let after = commit_stamp(&root);
    assert_eq!(
        accepted_epoch(&root),
        epoch_before,
        "member reordering does not advance catalog identity",
    );
    assert_ne!(
        after.source_digest, before.source_digest,
        "the reordered durable shape is stamped under its own digest",
    );
    assert!(
        after.commit_id > before.commit_id,
        "the zero-mutation auto-apply writes a fresh stamp"
    );
}

/// `Book.title` renamed to `Book.label` via an in-source `evolve rename`. Over an empty
/// `^books` the rename re-addresses no record; over a populated `^books` it moves the
/// stored cell's address, a non-additive identity change the run-time auto-apply set
/// excludes.
const RENAME_LABEL: &str = "module app\n\
     resource Log\n\
     \x20   note: string\n\
     store ^log(id: int): Log\n\
     resource Book\n\
     \x20   required label: string\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   rename Book.title -> Book.label\n\
     pub fn seed()\n\
     \x20   transaction\n\
     \x20       ^log(1).note = \"ran\"\n\
     pub fn seedBook()\n\
     \x20   transaction\n\
     \x20       ^books(1).label = \"Mort\"\n";

#[test]
fn a_rename_over_a_populated_store_fences_run_and_evolve_apply_re_addresses()
-> Result<(), Box<dyn std::error::Error>> {
    // A rename re-addresses a populated cell. It writes no record bytes, so the apply is
    // catalog-only, but moving how stored data is addressed is a non-additive identity
    // change: a bare run must not silently advance the epoch and re-spell the cell. It
    // fences with the actionable schema-drift diagnostic, and the explicit `evolve apply`
    // then discharges the rename and advances the epoch.
    let root = books_and_log_project("run-rename-populated", BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let seed_book = marrow_sub("run", &["--entry", "app::seedBook", dir(&root)]);
    assert_eq!(seed_book.status.code(), Some(0), "seed book: {seed_book:?}");
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/app.mw", RENAME_LABEL);
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        rerun.status.code(),
        Some(1),
        "a rename over a populated store fences: {rerun:?}",
    );
    let stderr = String::from_utf8(rerun.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift"),
        "the rename fence reports the schema-drift code: {stderr}",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before,
        "a fenced rename does not auto-apply or advance the epoch",
    );

    let apply = marrow(&["evolve", "apply", dir(&root)]);
    assert_eq!(apply.status.code(), Some(0), "evolve apply: {apply:?}");
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "explicit apply discharges the rename the fenced run left untouched",
    );
    // The populated cell carries over under its new spelling: the rename preserves the
    // stored value rather than dropping it.
    let config_text = std::fs::read_to_string(root.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    let accepted = TreeStore::open_read_only(&native_store_path(&root))
        .expect("open store read-only")
        .read_catalog_snapshot()
        .expect("read store catalog snapshot");
    let (report, program) =
        marrow_check::check_project_with_catalog(root.path(), &config, accepted.as_ref())
            .expect("re-check after apply");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let place = root_place(&program, "books")?;
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &place, 1, "label", ScalarType::Str),
        Some(Scalar::Str("Mort".to_string())),
        "the rename carried the stored cell to its new spelling",
    );
    Ok(())
}

#[test]
fn a_pending_rename_preview_reports_activatable_work_not_nothing_to_discharge() {
    // A pending rename over a populated store is real activatable work: the lock still
    // names the old spelling and `check` reports a stale lock, so `evolve preview` must
    // report it as pending rather than claim the store already matches the source. A
    // rename mutates no record bytes, so the zero-backfill/zero-transform proxy would
    // misread it as nothing to discharge.
    let root = books_and_log_project("preview-pending-rename", BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let seed_book = marrow_sub("run", &["--entry", "app::seedBook", dir(&root)]);
    assert_eq!(seed_book.status.code(), Some(0), "seed book: {seed_book:?}");

    write(&root, "src/app.mw", RENAME_LABEL);
    let preview = marrow(&["evolve", "preview", "--format", "json", dir(&root)]);
    assert_eq!(preview.status.code(), Some(0), "preview: {preview:?}");
    let record: serde_json::Value =
        serde_json::from_slice(&preview.stdout).expect("preview json envelope");
    assert_eq!(
        record.get("nothing_to_discharge"),
        Some(&serde_json::json!(false)),
        "a pending rename over populated data is activatable work, not nothing to discharge: {record}",
    );
    assert_eq!(
        record.get("status"),
        Some(&serde_json::json!("activatable")),
        "the pending rename is activatable: {record}",
    );
}

#[test]
fn a_rename_over_an_empty_store_auto_applies_on_run() {
    // The same rename against an empty `^books` re-addresses no record, so emptiness
    // discharges it and the run auto-applies, advancing the epoch by one. This is the
    // spec's empty-store auto-apply: a non-additive change against an empty store has
    // nothing to re-address.
    let root = books_and_log_project("run-rename-empty", BASELINE);
    let first = marrow_sub("run", &[dir(&root)]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let epoch_before = accepted_epoch(&root);

    write(&root, "src/app.mw", RENAME_LABEL);
    let rerun = marrow_sub("run", &[dir(&root)]);
    assert_eq!(
        rerun.status.code(),
        Some(0),
        "a rename over an empty store auto-applies: {rerun:?}",
    );
    assert!(
        !String::from_utf8(rerun.stderr)
            .expect("stderr utf8")
            .contains("run.schema_drift"),
        "an empty-store rename does not fence",
    );
    assert_eq!(
        accepted_epoch(&root),
        epoch_before + 1,
        "the empty-store rename advanced the epoch by one",
    );
}
