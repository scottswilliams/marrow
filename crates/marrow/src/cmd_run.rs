//! `marrow run`: check a project, then run an entry function over its store.

use std::cell::RefCell;
use std::process::ExitCode;

use crate::cmd_check::report_runtime_fault;
use crate::{CheckFormat, load_checked_project, report_simple_error, resolve_store_path};

/// Run a project's entry function. Unlike `check`, `run` has no `--format`: its
/// output is the program's own `print`/`write` stream, which has no JSON envelope;
/// failures still report a dotted error code on stderr.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let mut entry = None;
    let mut dir = None;
    let mut maintenance = false;
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
            // Grants the maintenance capability (whole-root delete, required-field
            // delete, raw quoted-segment access). An operator must type it; the
            // default run and `run.defaultEntry` can never inject it.
            "--maintenance" => maintenance = true,
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow run [--entry <module::function>] [--maintenance] <projectdir>

Check a Marrow project, then run an entry function over the store its
`marrow.json` selects (an in-memory store when none is configured). The entry
is `--entry` if given, else the project's `run.defaultEntry`. Output written
with `print`/`write` goes to stdout.

  --maintenance  Run with the maintenance capability, for migration, repair,
                 and restore tooling. It permits whole managed-root deletes,
                 required-field deletes, and raw quoted-segment access that the
                 default run rejects. Use it deliberately.
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
    run_project_dir(&dir, entry.as_deref(), maintenance)
}

/// Load and check `<dir>/marrow.json`'s project, then run its entry (the
/// `--entry` override, else `run.defaultEntry`) over the configured store. A
/// project must check cleanly before it runs.
fn run_project_dir(dir: &str, entry_override: Option<&str>, maintenance: bool) -> ExitCode {
    let (config, program) = match load_checked_project(dir) {
        Ok(checked) => checked,
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

    match resolve_store_path(dir, &config) {
        Err(code) => code,
        Ok(None) => {
            let store = RefCell::new(marrow_store::mem::MemStore::new());
            execute(&program, &store, entry, maintenance)
        }
        Ok(Some(path)) => match marrow_store::redb::RedbStore::open(&path) {
            Ok(store) => execute(&program, &RefCell::new(store), entry, maintenance),
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                ExitCode::FAILURE
            }
        },
    }
}

/// Run `entry` from a checked `program` over `store`, printing its output. The run
/// gets the real system clock, environment, and filesystem, and sends `std::log`
/// output to standard error. `maintenance` grants the maintenance capability only
/// when the operator passed `--maintenance`.
fn execute(
    program: &marrow_check::CheckedProgram,
    store: &RefCell<dyn marrow_store::backend::Backend>,
    entry: &str,
    maintenance: bool,
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
    let result = marrow_run::run_entry_with_host(program, store, &host, entry, &[]);
    // Log output is collected even on a failing run; it goes to stderr, off the
    // program's own stdout stream.
    eprint!("{}", log.borrow());
    match result {
        Ok(outcome) => {
            print!("{}", outcome.output);
            ExitCode::SUCCESS
        }
        Err(error) => {
            report_runtime_fault(program, &error);
            ExitCode::FAILURE
        }
    }
}
