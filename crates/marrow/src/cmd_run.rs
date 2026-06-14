//! `marrow run`: check a project, then run an entry function over its store.

use std::cell::RefCell;
use std::io::Write;
use std::process::ExitCode;

use marrow_run::{
    Nondeterminism, ProjectInvokeError, ProjectOpen, ProjectSession, ProjectSessionError,
    SessionEntry, SystemNondeterminism,
};

use crate::cmd_check::report_runtime_fault;
use crate::dry_run::{self, DryRunHook};
use crate::trace::TraceHook;
use crate::{CheckFormat, report_io_error, report_project, report_simple_error};

/// How a run observes itself: a plain run with no tooling report, an execution
/// trace, a `--dry-run` that isolates its writes from the configured store, or
/// both composed. Trace output is always text; only dry-run carries a selected
/// structured report format.
#[derive(Clone, Copy)]
enum RunObservation {
    Plain,
    Trace,
    DryRun(CheckFormat),
    TraceDryRun,
}

impl RunObservation {
    fn traces(self) -> bool {
        matches!(self, Self::Trace | Self::TraceDryRun)
    }

    fn isolates_writes(self) -> bool {
        matches!(self, Self::DryRun(_) | Self::TraceDryRun)
    }

    fn format(self) -> CheckFormat {
        match self {
            Self::Plain | Self::Trace | Self::TraceDryRun => CheckFormat::Text,
            Self::DryRun(format) => format,
        }
    }
}

/// Run a project's entry function. A plain run's only output is the program's
/// `print` stream, which carries no envelope; `--trace` is text-only, and
/// `--dry-run` is the only run mode whose tooling report takes `--format`.
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
                if matches!(format, CheckFormat::Jsonl) {
                    eprintln!("unknown format: jsonl");
                    return ExitCode::from(2);
                }
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow run [--entry <entry>] [--maintenance] [--trace] [--dry-run] \
[--format text|json] <projectdir>

Check a Marrow project, then run an entry function over the store its
`marrow.json` selects (an in-memory store when none is configured). The entry
is `--entry` if given, else the project's `run.defaultEntry`; qualify it as
module::function unless the bare public function name is unique. Output written
with `print` goes to stdout.

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
  --format       The report format for --dry-run (default text). A plain
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
    if trace && !matches!(format, CheckFormat::Text) {
        eprintln!("--trace only supports --format text");
        return ExitCode::from(2);
    }
    if saw_format && !dry_run {
        eprintln!("--format applies to --dry-run output; this run takes none");
        return ExitCode::from(2);
    }
    let observe = match (trace, dry_run) {
        (false, false) => RunObservation::Plain,
        (true, false) => RunObservation::Trace,
        (false, true) => RunObservation::DryRun(format),
        (true, true) => RunObservation::TraceDryRun,
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
    let format = observe.format();
    let mut open = ProjectOpen::run();
    if let Some(entry) = entry_override {
        open = open.with_entry_override(entry);
    }
    if observe.isolates_writes() {
        open = open.with_isolated_writes();
    }
    let session = match ProjectSession::open(dir, open) {
        Ok(session) => session,
        Err(error) => return report_session_open_error(dir, error, format),
    };
    for notice in session.notices() {
        eprintln!("{}", notice.message());
    }
    let entry = session
        .run_entry()
        .expect("run sessions carry the selected entry");
    let nondeterminism = SystemNondeterminism::new();
    execute(dir, &session, entry, maintenance, observe, &nondeterminism)
}

pub(crate) fn report_session_open_error(
    dir: &str,
    error: ProjectSessionError,
    format: CheckFormat,
) -> ExitCode {
    match error {
        ProjectSessionError::Io { path, error } => {
            report_io_error(&path.display().to_string(), &error, format);
        }
        ProjectSessionError::Check { report } => {
            report_project(dir, &report, format);
        }
        ProjectSessionError::CheckLoad {
            code,
            path,
            message,
        } => {
            report_simple_error(code, &format!("{}: {message}", path.display()), format);
        }
        error => {
            report_simple_error(error.code(), &error.message(), format);
        }
    }
    ExitCode::FAILURE
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

/// Run `entry` through an admitted session, printing its output to stdout and
/// sending `std::log` output to stderr so the two streams stay separate.
/// `maintenance` grants the maintenance capability, set only when the operator passed
/// `--maintenance`. `observe` selects a plain run, a traced run, a dry run
/// against an isolated store, or both composed.
fn execute(
    dir: &str,
    session: &ProjectSession,
    entry: &str,
    maintenance: bool,
    observe: RunObservation,
    nondeterminism: &impl Nondeterminism,
) -> ExitCode {
    let program = session.runtime_program();
    let log = std::rc::Rc::new(RefCell::new(String::new()));
    let mut host = base_host(std::rc::Rc::clone(&log), nondeterminism);
    if maintenance {
        host = host.with_maintenance();
    }
    let format = observe.format();

    // A dry run executes on the store selected by `run_project_dir`; for native
    // stores that is a disposable copy, so user transactions cannot consume the
    // isolation boundary. It observes every managed write through the same hook the
    // trace uses. A `--trace` (with or without `--dry-run`) installs the trace hook;
    // a plain run installs none.
    let mut stdout = std::io::stdout();
    let mut program_output = |text: &str| {
        stdout
            .write_all(text.as_bytes())
            .expect("program stdout write failed");
        stdout.flush().expect("program stdout flush failed");
    };
    let (result, report) = if observe.isolates_writes() {
        let trace = observe.traces().then(|| TraceHook::new("", program));
        let mut hook = DryRunHook::new(trace);
        let result = session.invoke(
            SessionEntry::new(entry, &host, &mut program_output)
                .with_hook(&mut hook)
                .with_isolated_writes(),
        );
        let (planned, trace) = hook.into_report();
        (result, Report::Dry { planned, trace })
    } else if observe.traces() {
        let mut hook = TraceHook::new("", program);
        let result = session
            .invoke(SessionEntry::new(entry, &host, &mut program_output).with_hook(&mut hook));
        (result, Report::Trace(hook))
    } else {
        let result = session.invoke(SessionEntry::new(entry, &host, &mut program_output));
        (result, Report::None)
    };

    // Log output is collected even on a failing run; it goes to stderr, off the
    // program's own stdout stream.
    eprint!("{}", log.borrow());
    if let Err(ProjectInvokeError::Session(error)) = result {
        return report_session_open_error(dir, error, format);
    }
    report.emit(format, program);
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(ProjectInvokeError::Runtime(error)) => {
            report_runtime_fault(program, &error, format);
            ExitCode::FAILURE
        }
        Err(ProjectInvokeError::Session(_)) => {
            unreachable!("session errors returned before report")
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
