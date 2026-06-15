//! `marrow run`: check a project, then run an entry function over its store.

use std::cell::RefCell;
use std::io::Write;
use std::process::ExitCode;

use marrow_run::{
    Nondeterminism, ProjectInvokeError, ProjectOpen, ProjectSession, ProjectSessionError,
    ProjectSessionNotice, RunOutput, RunOutputSink, RuntimeError, SessionEntry, StoreStamp,
    SystemNondeterminism, Value,
};
use serde_json::json;

use crate::cmd_check::{report_runtime_fault, runtime_fault_json};
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
    DryRun(dry_run::ReportFormat),
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
            Self::DryRun(format) => dry_run_check_format(format),
        }
    }

    fn dry_report_format(self) -> Option<dry_run::ReportFormat> {
        match self {
            Self::DryRun(format) => Some(format),
            Self::TraceDryRun => Some(dry_run::ReportFormat::Text),
            Self::Plain | Self::Trace => None,
        }
    }
}

fn dry_run_check_format(format: dry_run::ReportFormat) -> CheckFormat {
    match format {
        dry_run::ReportFormat::Text => CheckFormat::Text,
        dry_run::ReportFormat::Json => CheckFormat::Json,
    }
}

/// Run a project's entry function. Text mode leaves the program's `print`
/// stream on stdout; JSON mode captures it inside a result envelope. `--trace`
/// is text-only, and dry-run reports are tooling output on stderr.
/// Failures report a dotted error code on stderr.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let mut entry = None;
    let mut entry_args: Vec<(String, String)> = Vec::new();
    let mut dir = None;
    let mut maintenance = false;
    let mut trace = false;
    let mut dry_run = false;
    let mut format = CheckFormat::Text;
    let mut dry_report_format = dry_run::ReportFormat::Text;
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
            "--arg" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --arg");
                    return ExitCode::from(2);
                };
                let Some((name, text)) = value.split_once('=') else {
                    eprintln!("--arg expects name=value");
                    return ExitCode::from(2);
                };
                if name.is_empty() {
                    eprintln!("--arg expects a non-empty parameter name");
                    return ExitCode::from(2);
                }
                entry_args.push((name.to_string(), text.to_string()));
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
                dry_report_format = match format {
                    CheckFormat::Text => dry_run::ReportFormat::Text,
                    CheckFormat::Json => dry_run::ReportFormat::Json,
                    CheckFormat::Jsonl => {
                        eprintln!("unknown format: jsonl");
                        return ExitCode::from(2);
                    }
                };
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow run [--entry <entry>] [--arg name=value]... [--maintenance] [--trace] [--dry-run] \
[--format text|json] <projectdir>

Check a Marrow project, then run an entry function over the store its
`marrow.json` selects (an in-memory store when none is configured). The entry
is `--entry` if given, else the project's `run.defaultEntry`; qualify it as
module::function unless the bare public function name is unique. Output written
with `print` goes to stdout.

  --arg          Supply one entry parameter as name=value. Repeat --arg in argv
                 order; repeated values collect only for supported sequence
                 parameters.
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
  --format       Use json for the run result envelope, or for the --dry-run
                 tooling report. Text mode preserves program stdout.
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
    if trace && saw_format && !dry_run {
        eprintln!("--format with --trace is only supported when --dry-run is also present");
        return ExitCode::from(2);
    }
    let observe = match (trace, dry_run) {
        (false, false) => RunObservation::Plain,
        (true, false) => RunObservation::Trace,
        (false, true) => RunObservation::DryRun(dry_report_format),
        (true, true) => RunObservation::TraceDryRun,
    };
    run_project_dir(
        &dir,
        entry.as_deref(),
        &entry_args,
        maintenance,
        observe,
        format,
    )
}

/// Load and check `<dir>/marrow.json`'s project, then run its entry (the
/// `--entry` override, else `run.defaultEntry`) over the configured store. A
/// project must check cleanly before it runs.
fn run_project_dir(
    dir: &str,
    entry_override: Option<&str>,
    entry_args: &[(String, String)],
    maintenance: bool,
    observe: RunObservation,
    output_format: CheckFormat,
) -> ExitCode {
    let format = if observe.isolates_writes() {
        observe.format()
    } else {
        output_format
    };
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
    let Some(entry) = session.run_entry() else {
        return report_session_open_error(dir, ProjectSessionError::NoEntry, format);
    };
    if !observe.isolates_writes() {
        for notice in session.notices() {
            eprintln!("{}", notice.message());
        }
    }
    let actions = preview_actions(session.notices());
    if let Some(report_style) = observe.dry_report_format()
        && actions.would_fence
    {
        dry_run::report(&[], report_style, session.runtime_program(), &actions);
        return ExitCode::SUCCESS;
    }
    let nondeterminism = SystemNondeterminism::new();
    execute(
        RunExecution {
            dir,
            session: &session,
            entry,
            entry_args,
            maintenance,
            observe,
            output_format,
            actions,
        },
        &nondeterminism,
    )
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
pub(crate) fn base_host<S>(
    log: std::rc::Rc<RefCell<S>>,
    nondeterminism: &impl Nondeterminism,
) -> marrow_run::Host
where
    S: marrow_run::LogSink + 'static,
{
    marrow_run::Host::new()
        .with_nondeterminism(nondeterminism)
        .with_system_environment()
        .with_log_sink(log)
        .with_filesystem()
}

struct ProgramOutputSink {
    json_envelope: bool,
    captured: String,
}

impl ProgramOutputSink {
    fn new(json_envelope: bool) -> Self {
        Self {
            json_envelope,
            captured: String::new(),
        }
    }
}

impl RunOutputSink for ProgramOutputSink {
    fn write(&mut self, text: &str) {
        if self.json_envelope {
            self.captured.push_str(text);
            return;
        }
        let mut stdout = std::io::stdout();
        if let Err(error) = stdout.write_all(text.as_bytes()) {
            exit_after_program_stdout_error(error);
        }
        if let Err(error) = stdout.flush() {
            exit_after_program_stdout_error(error);
        }
    }
}

fn exit_after_program_stdout_error(error: std::io::Error) -> ! {
    match error.kind() {
        std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        _ => {
            eprintln!("io.write: failed to write program stdout: {error}");
            std::process::exit(1);
        }
    }
}

/// Run `entry` through an admitted session, printing its output to stdout and
/// sending `std::log` output to stderr so the two streams stay separate.
/// `maintenance` grants the maintenance capability, set only when the operator passed
/// `--maintenance`. `observe` selects a plain run, a traced run, a dry run
/// against an isolated store, or both composed.
struct RunExecution<'a> {
    dir: &'a str,
    session: &'a ProjectSession,
    entry: &'a str,
    entry_args: &'a [(String, String)],
    maintenance: bool,
    observe: RunObservation,
    output_format: CheckFormat,
    actions: dry_run::PreviewActions,
}

fn execute(request: RunExecution<'_>, nondeterminism: &impl Nondeterminism) -> ExitCode {
    let RunExecution {
        dir,
        session,
        entry,
        entry_args,
        maintenance,
        observe,
        output_format,
        actions,
    } = request;
    let program = session.runtime_program();
    let log = std::rc::Rc::new(RefCell::new(String::new()));
    let mut host = base_host(std::rc::Rc::clone(&log), nondeterminism);
    let json_envelope = matches!(output_format, CheckFormat::Json) && !observe.isolates_writes();
    let program_output = std::rc::Rc::new(RefCell::new(ProgramOutputSink::new(json_envelope)));
    host = host.with_output_sink(std::rc::Rc::clone(&program_output));
    if maintenance {
        host = host.with_maintenance();
    }
    let format = if observe.isolates_writes() {
        observe.format()
    } else {
        output_format
    };

    // A dry run executes on the store selected by `run_project_dir`; for native
    // stores that is a disposable copy, so user transactions cannot consume the
    // isolation boundary. It observes every managed write through the same hook the
    // trace uses. A `--trace` (with or without `--dry-run`) installs the trace hook;
    // a plain run installs none.
    let before_stamp = session.store_stamp().ok().flatten();
    let (result, report) = {
        let mut fallback_output = String::new();
        let text_args: Vec<(&str, &str)> = entry_args
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        if let Some(report_style) = observe.dry_report_format() {
            let trace = observe.traces().then(|| TraceHook::new("", program));
            let mut hook = DryRunHook::new(trace);
            let result = session.invoke(
                SessionEntry::new(entry, &host, &mut fallback_output)
                    .with_text_args(text_args.clone())
                    .with_hook(&mut hook)
                    .with_isolated_writes(),
            );
            let (planned, trace) = hook.into_report();
            (
                result,
                Report::Dry {
                    planned,
                    trace,
                    style: report_style,
                    actions,
                },
            )
        } else if observe.traces() {
            let mut hook = TraceHook::new("", program);
            let result = session.invoke(
                SessionEntry::new(entry, &host, &mut fallback_output)
                    .with_text_args(text_args.clone())
                    .with_hook(&mut hook),
            );
            (result, Report::Trace(hook))
        } else {
            let result = session.invoke(
                SessionEntry::new(entry, &host, &mut fallback_output).with_text_args(text_args),
            );
            (result, Report::None)
        }
    };
    let after_stamp = session.store_stamp().ok().flatten();

    // Log output is collected even on a failing run; it goes to stderr, off the
    // program's own stdout stream.
    eprint!("{}", log.borrow());
    let invocation: Result<RunOutput, RuntimeError> = match result {
        Ok(output) => Ok(output),
        Err(ProjectInvokeError::Runtime(error)) => Err(error),
        Err(ProjectInvokeError::Session(error)) => {
            return report_session_open_error(dir, error, format);
        }
    };
    report.emit(program);
    match invocation {
        Ok(output) => {
            if json_envelope {
                let captured_output = &program_output.borrow().captured;
                match render_run_json(
                    captured_output,
                    &output,
                    after_stamp.as_ref(),
                    before_stamp.as_ref(),
                ) {
                    Ok(()) => {}
                    Err(error) => {
                        report_runtime_fault_with_run_state(
                            program,
                            &error,
                            format,
                            after_stamp.as_ref(),
                            before_stamp.as_ref(),
                        );
                        return ExitCode::FAILURE;
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            if json_envelope {
                report_runtime_fault_with_run_state(
                    program,
                    &error,
                    format,
                    after_stamp.as_ref(),
                    before_stamp.as_ref(),
                );
            } else {
                report_runtime_fault(program, &error, format);
            }
            ExitCode::FAILURE
        }
    }
}

/// The tooling report a run produced alongside the program's own result stream.
/// Trace and dry-run reports are written to stderr; stdout remains either the
/// text-mode program output or the JSON result envelope.
enum Report {
    None,
    Trace(TraceHook),
    Dry {
        planned: Vec<dry_run::PlannedWrite>,
        trace: Option<TraceHook>,
        style: dry_run::ReportFormat,
        actions: dry_run::PreviewActions,
    },
}

impl Report {
    fn emit(self, program: &marrow_check::CheckedRuntimeProgram) {
        match self {
            Report::None => {}
            Report::Trace(mut hook) => hook.flush(),
            Report::Dry {
                planned,
                trace,
                style,
                actions,
            } => {
                if let Some(mut trace) = trace {
                    trace.flush();
                }
                dry_run::report(&planned, style, program, &actions);
            }
        }
    }
}

fn preview_actions(notices: &[ProjectSessionNotice]) -> dry_run::PreviewActions {
    let mut actions = dry_run::PreviewActions::default();
    for notice in notices {
        match notice {
            ProjectSessionNotice::DryRunWouldFreeze => {
                actions.would_freeze = true;
                actions.messages.push(notice.message());
            }
            ProjectSessionNotice::DryRunWouldApply { .. } => {
                actions.would_apply = true;
                actions.messages.push(notice.message());
            }
            ProjectSessionNotice::DryRunWouldFence { .. } => {
                actions.would_fence = true;
                actions.messages.push(notice.message());
            }
            ProjectSessionNotice::AutoApplied { .. } => {}
        }
    }
    actions
}

fn render_run_json(
    output: &str,
    result: &RunOutput,
    stamp: Option<&StoreStamp>,
    before: Option<&StoreStamp>,
) -> Result<(), RuntimeError> {
    let mut envelope = serde_json::Map::new();
    envelope.insert("output".to_string(), json!(output));
    envelope.insert(
        "return".to_string(),
        match &result.value {
            Some(value) => render_return_value(value)?,
            None => serde_json::Value::Null,
        },
    );
    envelope.insert("signature_digest".to_string(), serde_json::Value::Null);
    envelope.insert("raises".to_string(), serde_json::Value::Null);
    insert_run_store_state(&mut envelope, stamp, before);
    crate::write_json(serde_json::Value::Object(envelope));
    Ok(())
}

fn insert_run_store_state(
    envelope: &mut serde_json::Map<String, serde_json::Value>,
    stamp: Option<&StoreStamp>,
    before: Option<&StoreStamp>,
) {
    if let Some(stamp) = stamp {
        envelope.insert(
            "store_stamp".to_string(),
            json!({
                "store_uid": stamp.store_uid.as_str(),
                "catalog_epoch": stamp.catalog_epoch,
                "commit_id": stamp.commit_id,
            }),
        );
        if run_committed(stamp, before) {
            envelope.insert("committed".to_string(), serde_json::Value::Bool(true));
        }
    } else {
        envelope.insert("store_stamp".to_string(), serde_json::Value::Null);
    }
}

fn run_committed(stamp: &StoreStamp, before: Option<&StoreStamp>) -> bool {
    before
        .map(|before| before.commit_id != stamp.commit_id)
        .unwrap_or(false)
}

fn report_runtime_fault_with_run_state(
    program: &marrow_check::CheckedRuntimeProgram,
    error: &RuntimeError,
    format: CheckFormat,
    stamp: Option<&StoreStamp>,
    before: Option<&StoreStamp>,
) {
    match format {
        CheckFormat::Text => report_runtime_fault(program, error, format),
        CheckFormat::Json | CheckFormat::Jsonl => {
            let mut envelope = runtime_fault_json(program, error);
            insert_run_store_state(&mut envelope, stamp, before);
            crate::write_json_err(serde_json::Value::Object(envelope));
        }
    }
}

fn render_return_value(value: &Value) -> Result<serde_json::Value, RuntimeError> {
    Ok(match value {
        Value::Int(value) => json!({ "kind": "int", "value": value }),
        Value::Bool(value) => json!({ "kind": "bool", "value": value }),
        Value::Str(value) => json!({ "kind": "string", "value": value }),
        Value::Decimal(value) => json!({ "kind": "decimal", "value": value.to_text() }),
        Value::Date(value) => json!({ "kind": "date", "value": value }),
        Value::Duration(value) => json!({ "kind": "duration", "value": value.to_string() }),
        Value::Instant(value) => json!({ "kind": "instant", "value": value.to_string() }),
        Value::Bytes(value) => {
            json!({ "kind": "bytes", "value_b64": marrow_run::base64::encode(value) })
        }
        Value::Enum(value) => json!({
            "kind": "enum",
            "enum_id": value.enum_id().0,
            "member_id": value.member_id().0,
        }),
        Value::Identity(identity) => json!({
            "kind": "identity",
            "root": identity.root(),
            "keys": identity
                .keys()
                .iter()
                .map(crate::cmd_data::saved_key_json)
                .collect::<Vec<_>>(),
        }),
        Value::Sequence(items) => {
            let values = items
                .iter()
                .map(render_return_value)
                .collect::<Result<Vec<_>, _>>()?;
            json!({ "kind": "sequence", "values": values })
        }
        Value::Resource(_) | Value::LocalTree(_) => {
            return Err(RuntimeError::entry_surface(
                "entry return value is outside the run JSON result surface",
            ));
        }
    })
}
