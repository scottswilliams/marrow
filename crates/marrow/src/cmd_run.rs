//! `marrow run`: check a project, then run an entry function over its store.

use std::cell::RefCell;
use std::process::ExitCode;

use marrow_check::evolution::preview;
use marrow_run::evolution::{AutoApplyOutcome, FenceError, RunObligation, fence, try_auto_apply};

use crate::cmd_check::report_runtime_fault;
use crate::dry_run::{self, DryRunHook};
use crate::trace::TraceHook;
use crate::{
    CheckFormat, commit_pending_identity, load_checked_project, report_simple_error,
    resolve_store_path, write_accepted_catalog,
};

/// How a run observes itself: a plain run with no tooling report, an execution
/// `--trace`, a `--dry-run` that rolls its writes back, or both composed (trace the
/// run, then discard). The report `--format` lives only on the variants that emit a
/// report, so a plain run cannot carry one.
#[derive(Clone, Copy)]
enum RunObservation {
    Plain,
    Trace(CheckFormat),
    DryRun(CheckFormat),
    TraceDryRun(CheckFormat),
}

impl RunObservation {
    fn traces(self) -> bool {
        matches!(self, Self::Trace(_) | Self::TraceDryRun(_))
    }

    fn rolls_back(self) -> bool {
        matches!(self, Self::DryRun(_) | Self::TraceDryRun(_))
    }

    /// The report format of an observing run. A plain run emits no report, so it has
    /// no format; callers must only ask once they know the run observes.
    fn format(self) -> CheckFormat {
        match self {
            Self::Plain => CheckFormat::Text,
            Self::Trace(format) | Self::DryRun(format) | Self::TraceDryRun(format) => format,
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
                entry = Some(value.clone());
            }
            // Grants the maintenance capability. An operator must type it; the
            // default run and `run.defaultEntry` can never inject it.
            "--maintenance" => maintenance = true,
            // Report each statement and managed write as the run executes.
            "--trace" => trace = true,
            // Run the entry, report the saved-data writes it would commit, then roll
            // them back so no saved data changes.
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
  --dry-run      Run the entry, report the saved-data writes it would commit,
                 then roll them back: no saved data changes. Side effects outside
                 saved data are not rewound. The plan is tooling output on stderr.
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
    // Running the program is an authorized state-establishing flow, so a clean source
    // with a pending catalog proposal has its durable identity frozen here before the
    // run touches the store. A clean accepted catalog proposes no change and is left
    // untouched.
    // A plain run reports its errors as text; only the trace/dry-run report takes
    // `--format`, so an identity-commit failure renders as text here.
    let program = match commit_pending_identity(dir, &config, program, CheckFormat::Text) {
        Ok(program) => program,
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

    // The store open binds the program against the catalog the store sits at: an
    // auto-applied evolution advances the epoch, so the returned program may be a
    // re-check past the one loaded above.
    let (program, store) = match open_run_store(dir, &config, program, observe.format()) {
        Ok(OpenStore::Memory(program)) => (program, marrow_store::tree::TreeStore::memory()),
        Ok(OpenStore::Native { program, store }) => (program, store),
        Err(code) => return code,
    };
    let entry = entry_override
        .or(config.default_entry.as_deref())
        .expect("entry presence proven before the store opened");
    let runtime_program = program.runtime();
    execute(&runtime_program, &store, entry, maintenance, observe)
}

/// The store a run executes over, paired with the program bound against the catalog the
/// store now sits at. An auto-applied evolution advances the accepted catalog, so the
/// returned program may be a re-check past the one the run loaded.
enum OpenStore {
    Memory(marrow_check::CheckedProgram),
    Native {
        program: marrow_check::CheckedProgram,
        store: marrow_store::tree::TreeStore,
    },
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
) -> Result<OpenStore, ExitCode> {
    let Some(store) = open_store_file(dir, config, format)? else {
        return Ok(OpenStore::Memory(program));
    };
    match fence_run(&program, &store) {
        Ok(()) => finish_open(program, store),
        Err(FenceError::SchemaDrift) => auto_apply_then_reopen(dir, config, program, store, format),
        Err(error) => {
            report_simple_error(error.code(), &error.message(), CheckFormat::Text);
            Err(ExitCode::FAILURE)
        }
    }
}

/// Open the project's configured native store file for writing, or `Ok(None)` when the
/// project configures the in-memory default. The redb backend holds a process-level lock
/// on the file, so a caller re-opening after an auto-apply must drop its first handle
/// first.
fn open_store_file(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<Option<marrow_store::tree::TreeStore>, ExitCode> {
    let Some(path) = resolve_store_path(dir, config, format)? else {
        return Ok(None);
    };
    marrow_store::tree::TreeStore::open(&path)
        .map(Some)
        .map_err(|error| {
            report_simple_error(error.code(), &error.to_string(), format);
            ExitCode::FAILURE
        })
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
    store: marrow_store::tree::TreeStore,
) -> Result<OpenStore, ExitCode> {
    match populated_unstamped_store(&program, &store) {
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
/// stamps the new shape and advances the epoch under the write lock with the witness
/// commit-id pin, so a concurrent write fails the apply closed rather than letting a
/// stale decision stamp. After the store commits, the accepted catalog file advances in
/// lockstep and the project is re-checked so the run binds against the activated epoch
/// and re-fences clean.
fn auto_apply_then_reopen(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    program: marrow_check::CheckedProgram,
    store: marrow_store::tree::TreeStore,
    format: CheckFormat,
) -> Result<OpenStore, ExitCode> {
    let witness = match preview(&program, &store) {
        Ok((witness, _diagnostics)) => witness,
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            return Err(ExitCode::FAILURE);
        }
    };
    match try_auto_apply(&witness, &program, &store) {
        Ok(AutoApplyOutcome::Applied) => {}
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
    // The store transaction committed the new epoch; advance the accepted catalog file to
    // match, exactly as an explicit `evolve apply` does as its final step.
    if let Some(proposal) = &program.catalog.proposal {
        write_accepted_catalog(dir, config, proposal, format)?;
    }
    // Release the file lock the native backend holds before re-opening below; one process
    // may not hold two handles to the same store file at once.
    drop(store);
    // Re-check the project so the program binds against the freshly accepted epoch, then
    // re-open and re-fence the store, which now matches. The reload sees the catalog file
    // this auto-apply just advanced, so it proposes no further change and fences clean.
    let (config, program) = load_checked_project(dir)?;
    let Some(store) = open_store_file(dir, &config, format)? else {
        return Ok(OpenStore::Memory(program));
    };
    if let Err(error) = fence_run(&program, &store) {
        report_simple_error(error.code(), &error.message(), CheckFormat::Text);
        return Err(ExitCode::FAILURE);
    }
    finish_open(program, store)
}

/// The actionable diagnostic a fenced obligation reports, naming `evolve apply` and the
/// backfill count where the witness proved one. A zero-mutation obligation never fences,
/// so it is not a reachable cause here.
fn fence_message(obligation: &RunObligation) -> String {
    match obligation {
        RunObligation::Backfill { records } => format!(
            "store was stamped under a different schema at this catalog epoch; the change backfills {records} record(s). Run `marrow evolve apply` to discharge it."
        ),
        RunObligation::Transform { records } => format!(
            "store was stamped under a different schema at this catalog epoch; the change rewrites {records} record(s). Run `marrow evolve apply` to discharge it."
        ),
        RunObligation::DestructiveDrop { populated } => format!(
            "store was stamped under a different schema at this catalog epoch; the change drops {populated} populated record(s). Run `marrow evolve apply --maintenance` and confirm the retire to discharge it."
        ),
        RunObligation::Repair => "store was stamped under a different schema at this catalog epoch; the change cannot be discharged against the stored data. Run `marrow evolve preview` to see the required repair.".to_string(),
        RunObligation::ZeroMutation { .. } => {
            "store was stamped under a different schema at this catalog epoch".to_string()
        }
    }
}

fn populated_unstamped_store(
    program: &marrow_check::CheckedProgram,
    store: &marrow_store::tree::TreeStore,
) -> Result<bool, marrow_store::StoreError> {
    if program.catalog.accepted_epoch.is_none() || store.read_catalog_epoch()?.is_some() {
        return Ok(false);
    }
    for module in &program.modules {
        for saved in &module.stores {
            let Some(place) = marrow_check::checked_saved_root_place(
                program,
                &saved.root,
                marrow_syntax::SourceSpan::default(),
            ) else {
                continue;
            };
            let Some(raw_store_id) = place.store_catalog_id else {
                continue;
            };
            let Ok(store_id) = marrow_store::cell::CatalogId::new(raw_store_id) else {
                continue;
            };
            if store.record_child_count(&store_id, &[])? > 0 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The host capabilities a run and a test share: the real system clock, environment,
/// and filesystem, with `std::log` output collected into `log`. The maintenance
/// capability is added by `run` only when the operator passed `--maintenance`, so it
/// stays local to that command rather than the shared base.
pub(crate) fn base_host(log: std::rc::Rc<RefCell<String>>) -> marrow_run::Host {
    marrow_run::Host::new()
        .with_system_clock()
        .with_system_environment()
        .with_log_sink(log)
        .with_filesystem()
}

/// Run `entry` from a checked `program` over `store`, printing its output. The run
/// gets the real system clock, environment, and filesystem, and sends `std::log`
/// output to standard error. `maintenance` grants the maintenance capability only
/// when the operator passed `--maintenance`. `observe` selects a plain run, a
/// traced run, a dry run (run-then-rollback), or both composed.
fn execute(
    program: &marrow_check::CheckedRuntimeProgram,
    store: &marrow_store::tree::TreeStore,
    entry: &str,
    maintenance: bool,
    observe: RunObservation,
) -> ExitCode {
    let log = std::rc::Rc::new(RefCell::new(String::new()));
    let mut host = base_host(std::rc::Rc::clone(&log));
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

    // A dry run brackets the whole run in one outer savepoint it always rolls back,
    // so no saved data changes; it observes every managed write through the same
    // hook the trace uses. A `--trace` (with or without `--dry-run`) installs the
    // trace hook; a plain run installs none.
    let format = observe.format();
    let result = if observe.rolls_back() {
        let trace = observe
            .traces()
            .then(|| TraceHook::new(format, "", program));
        let mut hook = DryRunHook::new(trace);
        if let Err(error) = store.begin() {
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            return ExitCode::FAILURE;
        }
        let result = marrow_run::run_entry_with_debugger(store, &host, &mut hook, &call);
        // Discard everything the run staged, whatever its outcome.
        if let Err(error) = store.rollback() {
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            return ExitCode::FAILURE;
        }
        let (planned, trace) = hook.into_report();
        result.map(|outcome| (outcome, Report::Dry { planned, trace }))
    } else if observe.traces() {
        let mut hook = TraceHook::new(format, "", program);
        let result = marrow_run::run_entry_with_debugger(store, &host, &mut hook, &call);
        result.map(|outcome| (outcome, Report::Trace(hook)))
    } else {
        marrow_run::run_entry_with_host(store, &host, &call).map(|outcome| (outcome, Report::None))
    };

    // Log output is collected even on a failing run; it goes to stderr, off the
    // program's own stdout stream.
    eprint!("{}", log.borrow());
    match result {
        Ok((outcome, report)) => {
            print!("{}", outcome.output);
            report.emit(format, program);
            ExitCode::SUCCESS
        }
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
