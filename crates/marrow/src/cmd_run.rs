//! `marrow run`: check a project, then run an entry function over its store.

use std::cell::RefCell;
use std::process::ExitCode;

use crate::cmd_check::report_runtime_fault;
use crate::dry_run::{self, DryRunHook};
use crate::trace::TraceHook;
use crate::{
    CheckFormat, commit_pending_identity, load_checked_project, report_simple_error,
    resolve_store_path,
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

    let Some(entry) = entry_override.or(config.default_entry.as_deref()) else {
        report_simple_error(
            "run.no_entry",
            "no entry to run; pass --entry <name> or set `run.defaultEntry` in marrow.json",
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    };
    let runtime_program = program.runtime();

    let store = match open_run_store(dir, &program, &config, observe.format()) {
        Ok(Some(store)) => store,
        Ok(None) => marrow_store::tree::TreeStore::memory(),
        Err(code) => return code,
    };
    execute(&runtime_program, &store, entry, maintenance, observe)
}

/// Open the project's configured native store for writing, fenced against the store's
/// durable activation context and checked for an unstamped-but-populated state. A
/// store a newer binary evolved past this program's accepted catalog, one that
/// predates it, or one whose storage layout drifted is refused rather than written
/// with a stale shape. `Ok(None)` is the in-memory default, which has no durable
/// context to fence and is never stamp-checked.
fn open_run_store(
    dir: &str,
    program: &marrow_check::CheckedProgram,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<Option<marrow_store::tree::TreeStore>, ExitCode> {
    let Some(path) = resolve_store_path(dir, config, format)? else {
        return Ok(None);
    };
    let store = marrow_store::tree::TreeStore::open(&path).map_err(|error| {
        report_simple_error(error.code(), &error.to_string(), format);
        ExitCode::FAILURE
    })?;
    if let Err(error) = marrow_run::evolution::fence(
        program.catalog.accepted_epoch,
        &program.source_digest(),
        &marrow_run::evolution::current_engine_profile(),
        &store,
    ) {
        report_simple_error(error.code(), &error.message(), CheckFormat::Text);
        return Err(ExitCode::FAILURE);
    }
    match populated_unstamped_store(program, &store) {
        Ok(false) => Ok(Some(store)),
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
