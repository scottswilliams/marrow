//! `marrow run`: check a project, then run an entry function over its store.

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_check::evolution::preview;
use marrow_run::evolution::{AutoApplyOutcome, FenceError, RunObligation, fence, try_auto_apply};
use marrow_run::{Nondeterminism, SystemNondeterminism};

use crate::cmd_check::report_runtime_fault;
use crate::dry_run::{self, DryRunHook};
use crate::trace::TraceHook;
use crate::{
    CheckFormat, load_checked_project, report_io_error, report_simple_error, resolve_store_path,
};

/// How a run observes itself: a plain run with no tooling report, an execution
/// `--trace`, a `--dry-run` that isolates its writes from the configured store, or
/// both composed. The report `--format` lives only on the variants that emit a
/// report, so a plain run cannot carry one.
#[derive(Clone, Copy)]
enum RunObservation {
    Plain,
    Trace(CheckFormat),
    DryRun(CheckFormat),
    TraceDryRun(CheckFormat),
    #[allow(dead_code)]
    Profile(CheckFormat),
}

impl RunObservation {
    fn traces(self) -> bool {
        matches!(self, Self::Trace(_) | Self::TraceDryRun(_))
    }

    fn isolates_writes(self) -> bool {
        matches!(self, Self::DryRun(_) | Self::TraceDryRun(_))
    }

    fn format(self) -> CheckFormat {
        match self {
            Self::Plain => CheckFormat::Text,
            Self::Trace(format)
            | Self::DryRun(format)
            | Self::TraceDryRun(format)
            | Self::Profile(format) => format,
        }
    }
}

/// Run a project's entry function. A plain run's only output is the program's own
/// `print`/`write` stream, which carries no envelope; `--trace` and `--dry-run`
/// are tooling reports that take `--format` and report separately from that stream.
/// Failures report a dotted error code on stderr.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let mut entry = None;
    let mut dir = None;
    let mut maintenance = false;
    let mut trace = false;
    let mut dry_run = false;
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--entry" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --entry");
                    return ExitCode::from(2);
                };
                if entry.is_some() {
                    eprintln!("duplicate --entry");
                    return ExitCode::from(2);
                }
                entry = Some(value.clone());
            }
            // An operator must type `--maintenance`; the default run and
            // `run.defaultEntry` can never inject the maintenance capability.
            "--maintenance" => maintenance = true,
            "--trace" => trace = true,
            "--dry-run" => dry_run = true,
            "--format" => {
                if let Err(code) =
                    crate::parse_format_flag(args, &mut index, &mut saw_format, &mut format)
                {
                    return code;
                }
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow run [--entry <entry>] [--maintenance] [--trace] [--dry-run] \
[--format text|json|jsonl] <projectdir>

Check a Marrow project, then run an entry function over the store its
`marrow.json` selects (an in-memory store when none is configured). The entry
is `--entry` if given, else the project's `run.defaultEntry`; qualify it as
module::function unless the bare public function name is unique. Output written
with `print`/`write` goes to stdout.

  --maintenance  Run with the maintenance capability, for data evolution and
                 repair tooling. It permits whole managed-root
                 deletes and required-field deletes that the default run
                 rejects. Use it deliberately.
  --trace        Report each statement (file:line, call depth, visible locals)
                 and each managed write as the run executes, in execution order.
                 The trace is tooling output on stderr, leaving stdout for the
                 program's own output.
  --dry-run      Run the entry against an isolated store, report the saved-data
                 writes it would commit, and leave the configured store unchanged.
                 Side effects outside saved data are not rewound. The plan is
                 tooling output on stderr.
  --format       The report format for --trace/--dry-run (default text). A plain
                 run's stdout is the program's own output and takes no --format.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => return crate::unknown_option("run", value),
            value => {
                if let Err(code) =
                    crate::take_single_target(&mut dir, value, "run", "project directory")
                {
                    return code;
                }
            }
        }
        index += 1;
    }

    let Some(dir) = dir else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    let observe = match (trace, dry_run) {
        (false, false) => {
            // A plain run's only output is the program's own `print`/`write` stream,
            // which carries no envelope, so `--format` has nothing to shape and is a
            // usage error rather than a silently ignored flag.
            if saw_format {
                eprintln!("--format applies to --trace/--dry-run output; a plain run takes none");
                return ExitCode::from(2);
            }
            RunObservation::Plain
        }
        (true, false) => RunObservation::Trace(format),
        (false, true) => RunObservation::DryRun(format),
        (true, true) => RunObservation::TraceDryRun(format),
    };
    run_project_dir(&dir, entry.as_deref(), maintenance, observe)
}

/// Load and check `<dir>/marrow.json`'s project, then run its entry (the
/// `--entry` override, else `run.defaultEntry`) over the configured store. A
/// project must check cleanly before it runs.
fn run_project_dir(
    dir: &str,
    entry_override: Option<&str>,
    maintenance: bool,
    observe: RunObservation,
) -> ExitCode {
    let (config, program) = match load_checked_project(dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };

    if entry_override.or(config.default_entry.as_deref()).is_none() {
        report_simple_error(
            "run.no_entry",
            "no entry to run; pass --entry <name> or set `run.defaultEntry` in marrow.json",
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    }

    // The store open binds the program against the catalog the store sits at. Running is
    // an authorized state-establishing flow, so a pending durable identity is frozen into
    // the store here as its baseline, and an auto-applied evolution advances the epoch, so
    // the returned program may be a re-check past the one loaded above.
    let mut nondeterminism = SystemNondeterminism::new();
    let open_store =
        match open_run_store(dir, &config, program, observe.format(), &mut nondeterminism) {
            Ok(open_store) => open_store,
            Err(code) => return code,
        };
    let entry = entry_override
        .or(config.default_entry.as_deref())
        .expect("entry presence proven before the store opened");
    match open_store {
        OpenStore::Memory(program) => {
            let runtime_program = program.runtime();
            let store = marrow_store::tree::TreeStore::memory();
            execute(
                &runtime_program,
                &store,
                entry,
                maintenance,
                observe,
                &nondeterminism,
            )
        }
        OpenStore::Native { program, store } => {
            let runtime_program = program.runtime();
            if observe.isolates_writes() {
                let isolated = match isolated_dry_run_store(store, observe.format()) {
                    Ok(isolated) => isolated,
                    Err(code) => return code,
                };
                execute(
                    &runtime_program,
                    &isolated.store,
                    entry,
                    maintenance,
                    observe,
                    &nondeterminism,
                )
            } else {
                execute(
                    &runtime_program,
                    &store.store,
                    entry,
                    maintenance,
                    observe,
                    &nondeterminism,
                )
            }
        }
    }
}

/// The store a run executes over, paired with the program bound against the catalog the
/// store now sits at. An auto-applied evolution advances the accepted catalog, so the
/// returned program may be a re-check past the one the run loaded.
enum OpenStore {
    Memory(marrow_check::CheckedProgram),
    Native {
        program: marrow_check::CheckedProgram,
        store: NativeRunStore,
    },
}

struct NativeRunStore {
    path: PathBuf,
    store: marrow_store::tree::TreeStore,
}

struct IsolatedDryRunStore {
    store: marrow_store::tree::TreeStore,
    _dir: TempStoreDir,
}

struct TempStoreDir {
    path: PathBuf,
}

impl Drop for TempStoreDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn isolated_dry_run_store(
    source: NativeRunStore,
    format: CheckFormat,
) -> Result<IsolatedDryRunStore, ExitCode> {
    let NativeRunStore {
        path: source_path,
        store,
    } = source;
    drop(store);

    let temp_dir = create_temp_store_dir(format)?;
    let isolated_path = temp_dir.path.join("marrow.redb");
    fs::copy(&source_path, &isolated_path).map_err(|error| {
        report_io_error(&source_path.display().to_string(), &error, format);
        ExitCode::FAILURE
    })?;
    let store = marrow_store::tree::TreeStore::open(&isolated_path).map_err(|error| {
        report_simple_error(error.code(), &error.to_string(), format);
        ExitCode::FAILURE
    })?;
    Ok(IsolatedDryRunStore {
        store,
        _dir: temp_dir,
    })
}

fn create_temp_store_dir(format: CheckFormat) -> Result<TempStoreDir, ExitCode> {
    let temp_root = std::env::temp_dir();
    for attempt in 0..100 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = temp_root.join(format!(
            "marrow-dry-run-store-{}-{nanos}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(TempStoreDir { path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                report_io_error(&path.display().to_string(), &error, format);
                return Err(ExitCode::FAILURE);
            }
        }
    }
    report_simple_error(
        "run.dry_run_isolation",
        "could not allocate a temporary dry-run store directory",
        format,
    );
    Err(ExitCode::FAILURE)
}

/// Open the project's configured native store for writing, fenced against the store's
/// durable activation context and checked for an unstamped-but-populated state. A
/// store a newer binary evolved past this program's accepted catalog, one that
/// predates it, or one whose storage layout drifted is refused rather than written
/// with a stale shape. An in-memory default has no durable context to fence and is
/// never stamp-checked.
///
/// Schema drift at the current epoch is the run-time evolution case: the store holds a
/// structurally different shape at this binary's epoch, which a sparse add or any other
/// zero-record-mutation change produces. Such a change is auto-applied here — the
/// production apply path stamps the new shape and advances the epoch — and the run
/// proceeds against the re-checked program. A change that would backfill, transform, or
/// destructively drop populated data fences instead, naming `evolve apply`.
fn open_run_store(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    program: marrow_check::CheckedProgram,
    format: CheckFormat,
    nondeterminism: &mut impl Nondeterminism,
) -> Result<OpenStore, ExitCode> {
    let Some(store) = open_store_file(dir, config, format, nondeterminism)? else {
        return open_memory_store(program, format);
    };
    let program = crate::establish_store_baseline(dir, config, &store.store, program, format)?;
    match fence_run(&program, &store.store) {
        Ok(()) => finish_open(program, store),
        Err(FenceError::SchemaDrift) => {
            auto_apply_then_reopen(dir, program, store, format, nondeterminism)
        }
        Err(error) => {
            report_simple_error(error.code(), &error.message(), CheckFormat::Text);
            Err(ExitCode::FAILURE)
        }
    }
}

/// Admit an in-memory-backed run, or refuse one whose durable surface needs a baseline.
/// A plain script with no resources, stores, or enums proposes no catalog and runs over
/// the throwaway memory store. A project with a durable surface proposes durable identity
/// that only a persistent store can hold, so it fails closed rather than running with an
/// identity nothing ever stamps.
fn open_memory_store(
    program: marrow_check::CheckedProgram,
    format: CheckFormat,
) -> Result<OpenStore, ExitCode> {
    if pending_baseline(&program) {
        report_simple_error(
            "run.durable_store_required",
            "a durable store is required to establish accepted identity; configure a native store in marrow.json",
            format,
        );
        return Err(ExitCode::FAILURE);
    }
    Ok(OpenStore::Memory(program))
}

/// Whether the program holds a pending catalog baseline: an unaccepted proposal that
/// declares at least one durable entry. A project past its baseline or a plain script
/// has none.
fn pending_baseline(program: &marrow_check::CheckedProgram) -> bool {
    program.catalog.accepted_epoch.is_none()
        && program
            .catalog
            .proposal
            .as_ref()
            .is_some_and(|proposal| !proposal.entries.is_empty())
}

/// Open the project's configured native store file for writing, or `Ok(None)` when the
/// project configures the in-memory default. The redb backend holds a process-level lock
/// on the file, so a caller re-opening after an auto-apply must drop its first handle
/// first.
fn open_store_file(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
    nondeterminism: &mut impl Nondeterminism,
) -> Result<Option<NativeRunStore>, ExitCode> {
    let Some(path) = resolve_store_path(dir, config, format)? else {
        return Ok(None);
    };
    let store = marrow_store::tree::TreeStore::open(&path).map_err(|error| {
        report_simple_error(error.code(), &error.to_string(), format);
        ExitCode::FAILURE
    })?;
    if let Err(error) = crate::backup::ensure_store_uid(&store, nondeterminism) {
        report_simple_error(error.code(), &error.to_string(), format);
        return Err(ExitCode::FAILURE);
    }
    Ok(Some(NativeRunStore { path, store }))
}

/// Fence a run's program against the store's stamped activation context.
fn fence_run(
    program: &marrow_check::CheckedProgram,
    store: &marrow_store::tree::TreeStore,
) -> Result<(), FenceError> {
    fence(
        program.catalog.accepted_epoch,
        &program.source_digest(),
        &marrow_run::evolution::current_engine_profile(),
        store,
    )
}

/// Confirm a fenced store is not populated-but-unstamped, then admit it for the run.
fn finish_open(
    program: marrow_check::CheckedProgram,
    store: NativeRunStore,
) -> Result<OpenStore, ExitCode> {
    match populated_unstamped_store(&program, &store.store) {
        Ok(false) => Ok(OpenStore::Native { program, store }),
        Ok(true) => {
            report_simple_error(
                "run.store_unstamped",
                "store has saved records but no catalog activation stamp; run `marrow check --data` and `marrow evolve apply` before running this accepted catalog",
                CheckFormat::Text,
            );
            Err(ExitCode::FAILURE)
        }
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            Err(ExitCode::FAILURE)
        }
    }
}

/// On schema drift, compute the evolution against committed data and either auto-apply a
/// zero-record-mutation change or fence with the obligation as the actionable cause.
///
/// A zero-mutation evolution is discharged through the production apply path, which
/// stamps the new shape, publishes the activated catalog snapshot, and advances the epoch
/// in one transaction under the write lock with the witness commit-id pin, so a concurrent
/// write fails the apply closed rather than letting a stale decision stamp. The project is
/// then re-checked so the run binds against the activated epoch and re-fences clean.
fn auto_apply_then_reopen(
    dir: &str,
    program: marrow_check::CheckedProgram,
    store: NativeRunStore,
    format: CheckFormat,
    nondeterminism: &mut impl Nondeterminism,
) -> Result<OpenStore, ExitCode> {
    let witness = match preview(&program, &store.store) {
        Ok((witness, _diagnostics)) => witness,
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            return Err(ExitCode::FAILURE);
        }
    };
    let from_epoch = witness
        .store_catalog
        .as_ref()
        .map(|catalog| catalog.epoch)
        .unwrap_or(witness.accepted_catalog.epoch);
    let to_epoch = witness
        .proposal_catalog
        .as_ref()
        .map(|catalog| catalog.epoch)
        .unwrap_or(witness.accepted_catalog.epoch);
    match try_auto_apply(&witness, &program, &store.store) {
        Ok(AutoApplyOutcome::Applied) => {
            eprintln!("auto-applied evolution: catalog epoch {from_epoch} -> {to_epoch}");
        }
        Ok(AutoApplyOutcome::MustFence(obligation)) => {
            report_simple_error(
                "run.schema_drift",
                &fence_message(&obligation),
                CheckFormat::Text,
            );
            return Err(ExitCode::FAILURE);
        }
        Err(_) => {
            // The apply re-verifies the witness against committed data under the write
            // lock; a failure here means the store moved between the probe and the stamp
            // (a concurrent write), so the auto-apply decision is stale. Fail closed
            // rather than stamp against state the witness no longer describes.
            report_simple_error(
                FenceError::SchemaDrift.code(),
                "store changed under the auto-apply probe; re-run to recompute the evolution against current data",
                CheckFormat::Text,
            );
            return Err(ExitCode::FAILURE);
        }
    }
    // Release the file lock the native backend holds before re-opening below; one process
    // may not hold two handles to the same store file at once.
    drop(store);
    // Re-check the project so the program binds against the freshly accepted epoch, then
    // re-open and re-fence the store, which now matches. The reload reads the catalog
    // snapshot this auto-apply just advanced, so it proposes no further change and fences
    // clean.
    let (config, program) = load_checked_project(dir)?;
    let Some(store) = open_store_file(dir, &config, format, nondeterminism)? else {
        return Ok(OpenStore::Memory(program));
    };
    if let Err(error) = fence_run(&program, &store.store) {
        report_simple_error(error.code(), &error.message(), CheckFormat::Text);
        return Err(ExitCode::FAILURE);
    }
    finish_open(program, store)
}

/// The actionable diagnostic a fenced obligation reports, naming `evolve apply` and the
/// backfill count where the witness proved one. A zero-mutation obligation never fences,
/// so it is not a reachable cause here.
fn fence_message(obligation: &RunObligation) -> String {
    let base = "store was stamped under a different schema at this catalog epoch";
    let cause = match obligation {
        RunObligation::Backfill { records } => format!(
            "; the change backfills {records} record(s). Run `marrow evolve apply` to discharge it."
        ),
        RunObligation::Transform { records } => format!(
            "; the change rewrites {records} record(s). Run `marrow evolve apply` to discharge it."
        ),
        RunObligation::DestructiveDrop { populated } => format!(
            "; the change drops {populated} populated record(s). Run `marrow evolve apply --maintenance` and confirm the retire to discharge it."
        ),
        RunObligation::Repair => "; the change cannot be discharged against the stored data. Run `marrow evolve preview` to see the required repair.".to_string(),
        RunObligation::ZeroMutation { .. } => String::new(),
    };
    format!("{base}{cause}")
}

/// Whether the store holds saved records but no accepted catalog the run can bind. A
/// project past its baseline whose store epoch stamp is missing is one such case; a store
/// that holds records under no accepted catalog at all (a pending baseline this run refused
/// to stamp because the store was not empty) is the other. Either way the run must refuse
/// rather than execute against records it cannot place.
fn populated_unstamped_store(
    program: &marrow_check::CheckedProgram,
    store: &marrow_store::tree::TreeStore,
) -> Result<bool, marrow_store::StoreError> {
    // A pending baseline over a non-empty store: the store carries records but no accepted
    // catalog, so the baseline commit declined to stamp it. The records cannot be placed
    // against an identity nothing accepted.
    if pending_baseline(program) {
        return Ok(!store.is_empty()?);
    }
    if program.catalog.accepted_epoch.is_none() || store.read_commit_metadata()?.is_some() {
        return Ok(false);
    }
    for module in &program.modules {
        for saved in &module.stores {
            if saved_root_holds_records(program, store, &saved.root)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether one saved root holds any record in `store`. A root that does not resolve to a
/// checked place, carries no store catalog id, or whose id does not parse cannot anchor
/// stored records, so it holds none.
fn saved_root_holds_records(
    program: &marrow_check::CheckedProgram,
    store: &marrow_store::tree::TreeStore,
    root: &str,
) -> Result<bool, marrow_store::StoreError> {
    let Some(place) =
        marrow_check::checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
    else {
        return Ok(false);
    };
    let Some(raw_store_id) = place.store_catalog_id else {
        return Ok(false);
    };
    let Ok(store_id) = marrow_store::cell::CatalogId::new(raw_store_id) else {
        return Ok(false);
    };
    store.record_identity_exists_under(&store_id, &[], place.identity_keys.len())
}

/// The host capabilities a run and a test share: the real system clock, environment,
/// and filesystem, with `std::log` output collected into `log`. The maintenance
/// capability is added by `run` only when the operator passed `--maintenance`, so it
/// stays local to that command rather than the shared base.
pub(crate) fn base_host(
    log: std::rc::Rc<RefCell<String>>,
    nondeterminism: &impl Nondeterminism,
) -> marrow_run::Host {
    marrow_run::Host::new()
        .with_nondeterminism(nondeterminism)
        .with_system_environment()
        .with_log_sink(log)
        .with_filesystem()
}

/// Run `entry` from a checked `program` over `store`, printing its output to stdout
/// and sending `std::log` output to stderr so the two streams stay separate.
/// `maintenance` grants the maintenance capability, set only when the operator passed
/// `--maintenance`. `observe` selects a plain run, a traced run, a dry run
/// against an isolated store, or both composed.
fn execute(
    program: &marrow_check::CheckedRuntimeProgram,
    store: &marrow_store::tree::TreeStore,
    entry: &str,
    maintenance: bool,
    observe: RunObservation,
    nondeterminism: &impl Nondeterminism,
) -> ExitCode {
    let log = std::rc::Rc::new(RefCell::new(String::new()));
    let mut host = base_host(std::rc::Rc::clone(&log), nondeterminism);
    if maintenance {
        host = host.with_maintenance();
    }
    let call = match marrow_run::CheckedEntryCall::new(program, entry, Vec::new()) {
        Ok(call) => call,
        Err(error) => {
            report_runtime_fault(program, &error);
            return ExitCode::FAILURE;
        }
    };

    // A dry run executes on the store selected by `run_project_dir`; for native
    // stores that is a disposable copy, so user transactions cannot consume the
    // isolation boundary. It observes every managed write through the same hook the
    // trace uses. A `--trace` (with or without `--dry-run`) installs the trace hook;
    // a plain run installs none.
    let format = observe.format();
    let mut stdout = std::io::stdout();
    let mut program_output = |text: &str| {
        stdout
            .write_all(text.as_bytes())
            .expect("program stdout write failed");
        stdout.flush().expect("program stdout flush failed");
    };
    let (result, report) = if observe.isolates_writes() {
        let trace = observe
            .traces()
            .then(|| TraceHook::new(format, "", program));
        let mut hook = DryRunHook::new(trace);
        let result = marrow_run::run_entry_with_debugger(
            store,
            &host,
            &mut hook,
            &call,
            &mut program_output,
        );
        let (planned, trace) = hook.into_report();
        (result, Report::Dry { planned, trace })
    } else if observe.traces() {
        let mut hook = TraceHook::new(format, "", program);
        let result = marrow_run::run_entry_with_debugger(
            store,
            &host,
            &mut hook,
            &call,
            &mut program_output,
        );
        (result, Report::Trace(hook))
    } else {
        (
            marrow_run::run_entry_with_host(store, &host, &call, &mut program_output),
            Report::None,
        )
    };

    // Log output is collected even on a failing run; it goes to stderr, off the
    // program's own stdout stream.
    eprint!("{}", log.borrow());
    report.emit(format, program);
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            report_runtime_fault(program, &error);
            ExitCode::FAILURE
        }
    }
}

/// The tooling report a run produced alongside the program's own output. The
/// program's output owns stdout; the report is written to stderr, so under any
/// `--format` the two streams stay separate and a consumer parsing stdout sees only
/// program output.
enum Report {
    None,
    Trace(TraceHook),
    Dry {
        planned: Vec<dry_run::PlannedWrite>,
        trace: Option<TraceHook>,
    },
}

impl Report {
    fn emit(self, format: CheckFormat, program: &marrow_check::CheckedRuntimeProgram) {
        match self {
            Report::None => {}
            Report::Trace(mut hook) => hook.flush(),
            Report::Dry { planned, trace } => {
                if let Some(mut trace) = trace {
                    trace.flush();
                }
                dry_run::report(&planned, format, program);
            }
        }
    }
}
