//! `marrow check [projectdir]`: capture, check, and describe durable demand.
//!
//! The minimal check surface. It captures the project and drives the compiler once,
//! tests included, for the complete diagnostic set (every stage over every module,
//! including test bodies), and prints each diagnostic with its span. A project that
//! checks clean has its test-inclusive image encoded from that same drive and verified,
//! so each exported function's verifier-reconstructed durable **demand** — which durable
//! places it reads and writes, in source spelling — can be described by
//! [`crate::demand`]. The demand describes access and never grants it; `check` opens no
//! store and runs no code.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::Command;
use crate::command_output::{finish, once, unknown_option};
use crate::demand::{demand_report, write_demand_report};
use crate::project::compile_project;
use crate::report_simple_error;

pub(crate) const HELP: &str = "\
Usage:
  marrow check [projectdir]

Capture and check a project's source, reporting every diagnostic with its span. A
project that checks clean prints its durable access demand grouped by module: every
durable path each exported function reads and writes, in source spelling. Adjacent
exports that share an identical demand are listed once, and storeless exports collapse
to one note per module. Demand describes access and never grants it. `check` opens no store
and runs no code. It exits 0 when the project checks clean, 1 when any diagnostic is
reported or a fixed bound is reached, and 2 on a usage error.
";

pub(crate) fn check(rest: &[String]) -> ExitCode {
    let mut target: Option<String> = None;
    for arg in rest {
        match arg.as_str() {
            value if value.starts_with('-') => return unknown_option(Command::Check, value),
            value => {
                if let Err(code) = once(
                    &mut target,
                    value.to_string(),
                    Command::Check,
                    "project directory",
                ) {
                    return code;
                }
            }
        }
    }
    let root = PathBuf::from(target.as_deref().unwrap_or("."));

    // One drive, tests included: the complete diagnostic set, then the test-inclusive
    // image it checked. The image is verified so each export's demand is the verifier's
    // reconstruction, not a compiler claim.
    let compiled = match compile_project(&root, marrow_compile::check, None) {
        Ok(compiled) => compiled,
        Err(code) => return code,
    };
    let image = match marrow_verify::verify(&compiled.image.bytes) {
        Ok(image) => image,
        Err(rejection) => {
            report_simple_error(rejection.code(), "the compiled image did not verify");
            return ExitCode::FAILURE;
        }
    };

    // Every export listed is the project's own: a dependency's `pub fn` takes no export
    // slot here and is run where the dependency is. The report is resolved whole before
    // a byte is written, so a coherence failure — the same compilation produced the
    // export directory and the verified image, so it is never a user error — prints its
    // one internal-error line and nothing else.
    let report = match demand_report(&compiled.exports, &compiled.naming, &image) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{}", error.internal_message());
            return ExitCode::FAILURE;
        }
    };
    finish(write_demand_report(&mut io::stdout().lock(), &report).map(|()| ExitCode::SUCCESS))
}
