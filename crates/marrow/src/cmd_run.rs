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

/// How a run observes itself: a plain run, an execution `--trace`, a `--dry-run`
/// that rolls its writes back, or both composed (trace the run, then discard).
#[derive(Clone, Copy)]
struct Observe {
    trace: bool,
    dry_run: bool,
    /// The report format for `--trace`/`--dry-run` output; ignored by a plain run,
    /// whose only output is the program's own `print`/`write` stream.
    format: CheckFormat,
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
            // them back so the store is left byte-for-byte unchanged.
            "--dry-run" => dry_run = true,
            "--format" => {
                if saw_format {
                    eprintln!("duplicate --format");
                    return ExitCode::from(2);
                }
                saw_format = true;
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --format");
                    return ExitCode::from(2);
                };
                let Some(parsed) = CheckFormat::parse(value) else {
                    eprintln!("unknown format: {value}");
                    return ExitCode::from(2);
                };
                format = parsed;
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
  --dry-run      Run the entry, report the saved-data writes it would commit,
                 then roll them back: the store is left byte-for-byte unchanged.
                 Side effects outside saved data are not rewound.
  --format       The report format for --trace/--dry-run (default text). A plain
                 run's stdout is the program's own output and takes no --format.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => {
                eprintln!("unknown run option: {value}");
                return ExitCode::from(2);
            }
            value => {
                if dir.replace(value.to_string()).is_some() {
                    eprintln!("marrow run accepts one project directory");
                    return ExitCode::from(2);
                }
            }
        }
        index += 1;
    }

    let Some(dir) = dir else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    let observe = Observe {
        trace,
        dry_run,
        format,
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
    observe: Observe,
) -> ExitCode {
    let (config, program) = match load_checked_project(dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    // Running the program is an authorized state-establishing flow, so a clean source
    // with a pending catalog proposal has its durable identity frozen here before the
    // run touches the store. A clean accepted catalog proposes no change and is left
    // untouched.
    let program = match commit_pending_identity(dir, &config, program) {
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

    match resolve_store_path(dir, &config) {
        Err(code) => code,
        Ok(None) => {
            let store = marrow_store::tree::TreeStore::memory();
            execute(&runtime_program, &store, entry, maintenance, observe)
        }
        Ok(Some(path)) => match marrow_store::tree::TreeStore::open(&path) {
            Ok(store) => {
                // Fence the binary against the store's durable activation context before
                // running: a store a newer binary evolved past this program's accepted
                // catalog, one that predates it, or one whose storage layout drifted is
                // refused rather than written with a stale shape. A memory store has no
                // durable context and is never fenced.
                if let Err(error) = marrow_run::evolution::fence(
                    program.catalog.accepted_epoch,
                    &program.source_digest(),
                    &marrow_run::evolution::current_engine_profile(),
                    &store,
                ) {
                    report_simple_error(error.code(), &error.message(), CheckFormat::Text);
                    return ExitCode::FAILURE;
                }
                match populated_unstamped_store(&program, &store) {
                    Ok(false) => {}
                    Ok(true) => {
                        report_simple_error(
                            "run.store_unstamped",
                            "store has saved records but no catalog activation stamp; run `marrow check --data` and `marrow evolve apply` before running this accepted catalog",
                            CheckFormat::Text,
                        );
                        return ExitCode::FAILURE;
                    }
                    Err(error) => {
                        report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                        return ExitCode::FAILURE;
                    }
                }
                execute(&runtime_program, &store, entry, maintenance, observe)
            }
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                ExitCode::FAILURE
            }
        },
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
    observe: Observe,
) -> ExitCode {
    let log = std::rc::Rc::new(RefCell::new(String::new()));
    let mut host = marrow_run::Host::new()
        .with_system_clock()
        .with_system_environment()
        .with_log_sink(std::rc::Rc::clone(&log))
        .with_filesystem();
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
    // so saved data is left byte-for-byte unchanged; it observes every managed
    // write through the same hook the trace uses. A `--trace` (with or without
    // `--dry-run`) installs the trace hook; a plain run installs none.
    let result = if observe.dry_run {
        let trace = observe
            .trace
            .then(|| TraceHook::new(observe.format, "", program));
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
    } else if observe.trace {
        let mut hook = TraceHook::new(observe.format, "", program);
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
            report.emit(observe.format, program);
            ExitCode::SUCCESS
        }
        Err(error) => {
            report_runtime_fault(program, &error);
            ExitCode::FAILURE
        }
    }
}

/// The tooling report a run produced alongside the program's own output, emitted
/// after the run's stdout so the program output and the report do not interleave.
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
