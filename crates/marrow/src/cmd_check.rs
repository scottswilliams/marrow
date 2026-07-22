//! `marrow check [projectdir]`: capture, check, and describe durable demand.
//!
//! The minimal check surface. It captures the project, runs the resilient analysis
//! floor for the complete diagnostic set (every stage over every module, including
//! test bodies), and prints each diagnostic with its span. A project that checks clean
//! is compiled and verified so each exported function can be described by its
//! verifier-reconstructed durable **demand** — which durable places it reads and
//! writes, in source spelling. The demand describes access and never grants it; `check`
//! opens no store and runs no code.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use marrow_codes::Code;
use marrow_compile::{
    AnalysisFailure, CompileFailure, DurableNaming, ExportEntry, InputRevision, SourceDiagnostic,
    analyze, compile,
};
use marrow_verify::VerifiedImage;

use crate::demand::demand_lines;
use crate::project::capture_project;
use crate::report_simple_error;
use crate::term_style::{Stream, Style};

const HELP: &str = "\
Usage:
  marrow check [projectdir]

Capture and check a project's source, reporting every diagnostic with its span. A
project that checks clean prints one line per exported function describing its
durable access demand — which durable places it reads and writes — in source
spelling. Demand describes access and never grants it. `check` opens no store and
runs no code. It exits 0 when the project checks clean, 1 when any diagnostic is
reported or a fixed bound is reached, and 2 on a usage error.
";

pub(crate) fn check(rest: &[String]) -> ExitCode {
    let mut target: Option<String> = None;
    for arg in rest {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => return crate::unknown_option("check", value),
            value => {
                if let Err(code) =
                    crate::take_single_target(&mut target, value, "check", "project directory")
                {
                    return code;
                }
            }
        }
    }
    let root = PathBuf::from(target.as_deref().unwrap_or("."));

    let project = match capture_project(&root) {
        Ok(project) => project,
        Err(failure) => {
            report_simple_error(failure.code, &failure.message);
            return ExitCode::FAILURE;
        }
    };
    let project = Arc::new(project);

    // The resilient analysis floor owns the complete diagnostic set. A clean floor
    // guarantees a clean production compile, so demand is described only for a project
    // with no diagnostic.
    let snapshot = match analyze(Arc::clone(&project), InputRevision::new(0)) {
        Ok(snapshot) => snapshot,
        Err(failure) => return report_analysis_failure(&failure),
    };
    let diagnostics = snapshot.diagnostics();
    if !diagnostics.is_empty() {
        for diagnostic in diagnostics {
            eprintln!("{}", diagnostic_line(diagnostic));
        }
        return ExitCode::FAILURE;
    }

    // Compile the production exports (no test entries) and verify, so each export's
    // demand is the verifier's reconstruction, not a compiler claim.
    let compiled = match compile(&project) {
        Ok(compiled) => compiled,
        Err(failure) => return report_compile_failure(&failure),
    };
    let image = match marrow_verify::verify(&compiled.image.bytes) {
        Ok(image) => image,
        Err(rejection) => {
            report_simple_error(rejection.code(), "the compiled image did not verify");
            return ExitCode::FAILURE;
        }
    };

    describe_exports(&compiled.exports, &compiled.naming, &image)
}

/// Print one demand line per export, in `module.item` order, and exit success. Each
/// line names the export and its demand sentence, so a reader sees the whole program's
/// durable footprint export by export.
fn describe_exports(
    exports: &[ExportEntry],
    naming: &DurableNaming,
    image: &VerifiedImage,
) -> ExitCode {
    match demand_lines(exports, naming, image) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        // Both arms are compiler-coherence failures, not user errors: the same
        // compilation produced the export directory and the verified image.
        Err(error) => {
            eprintln!("{}", error.internal_message());
            ExitCode::FAILURE
        }
    }
}

/// One diagnostic rendered as `file:line:column: code: message`, painted for a terminal.
fn diagnostic_line(diagnostic: &SourceDiagnostic) -> String {
    format!(
        "{}:{}:{}: {}: {}",
        term_paint(Style::Muted, diagnostic.file().as_str()),
        diagnostic.line(),
        diagnostic.column(),
        term_paint(Style::Code, diagnostic.code),
        diagnostic.message,
    )
}

fn term_paint(style: Style, text: &str) -> String {
    crate::term_style::paint(Stream::Stderr, style, text)
}

/// A fixed analysis-floor failure with no diagnostic to report: an aggregate bound or an
/// opaque compiler-coherence failure. Reported as one fixed code line with no location.
fn report_analysis_failure(failure: &AnalysisFailure) -> ExitCode {
    let code = match failure {
        AnalysisFailure::ResourceLimit { .. } => Code::CliCompilerResourceLimit,
        AnalysisFailure::Invariant { .. } => Code::CliCompilerInvariant,
    };
    report_simple_error(code.as_str(), "the project could not be checked");
    ExitCode::FAILURE
}

/// A compile failure on the clean-analysis path. A clean floor makes the diagnostic arm
/// unreachable, but the mapping is total: diagnostics are printed with spans, and a
/// fixed bound or an invariant becomes its fixed code line.
fn report_compile_failure(failure: &CompileFailure) -> ExitCode {
    match failure {
        CompileFailure::Diagnostics(diagnostics) => {
            for diagnostic in diagnostics {
                eprintln!("{}", diagnostic_line(diagnostic));
            }
        }
        CompileFailure::ResourceLimit(_) => report_simple_error(
            Code::CliCompilerResourceLimit.as_str(),
            "the project could not be checked",
        ),
        CompileFailure::Invariant(_) => report_simple_error(
            Code::CliCompilerInvariant.as_str(),
            "the project could not be checked",
        ),
    }
    ExitCode::FAILURE
}
