//! The CLI's project-capture boundary: a thin delegation to the physical adapter.
//!
//! The physical walker lives in [`marrow_project_fs`], the one filesystem owner
//! below the tool consumers, which also owns `.marrow/ids` publication and its
//! recovery. This module delegates capture to its `capture_project` with an empty
//! overlay and projects the opaque failure into the CLI's terminal
//! `{ code, message, location }` sink shape through the one presentation facade.
//! It reconstructs no discovery, identity, code, path, or message, compares no
//! state, writes no entry, and models no durability itself.

use std::path::Path;
use std::process::ExitCode;

use marrow_codes::Code;
use marrow_compile::{ProjectFile, SourceDiagnostic};
use marrow_project::{LedgerPublicationPlan, ProjectInput};
use marrow_project_fs::{
    CaptureFailure as PhysicalCaptureFailure, IdsPublication, IdsPublicationError,
    IdsPublishOutcome, OverlaySnapshot, ProjectMetadataWriteGuard,
};

use crate::term_style::{Palette, Stream};

/// The manifest file at a project root. Retained for `cmd_init`.
pub(crate) const MANIFEST_FILE: &str = "marrow.toml";

/// A project-capture failure, rendered by the CLI as a typed `code: message`
/// line. `location` names the manifest and 1-based position when the fault is a
/// located manifest syntax error.
pub(crate) struct CaptureFailure {
    pub(crate) code: Code,
    pub(crate) message: String,
    pub(crate) location: Option<ManifestLocation>,
}

/// A located manifest fault: the manifest path and its 1-based line and column.
pub(crate) struct ManifestLocation {
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) column: u32,
}

/// Capture the project rooted at `root` into an immutable [`ProjectInput`] through
/// the shared physical adapter with an empty overlay.
pub(crate) fn capture_project(root: &Path) -> Result<ProjectInput, CaptureFailure> {
    marrow_project_fs::capture_project(root, OverlaySnapshot::empty())
        .map_err(|failure| terminal_projection(root, &failure))
}

/// Settle any interrupted `.marrow/ids` publication under `root`.
///
/// A live publication marker makes the committed ledger indeterminate, so this
/// runs before `marrow run` captures the project and again before it draws
/// entropy. A project with no marker is untouched: the probe stats two fixed
/// names, creating nothing and taking no lock, so an ordinary run neither writes
/// nor locks anything here.
pub(crate) fn recover_identity_publication(root: &Path) -> Result<(), CaptureFailure> {
    if marrow_project_fs::ids_publication_marker(root).is_none() {
        return Ok(());
    }
    let mut guard = ProjectMetadataWriteGuard::acquire(root).map_err(publication_projection)?;
    guard
        .recover_ids()
        .map(|_| ())
        .map_err(publication_projection)
}

/// Publish one admitted successor over the exact state its plan was admitted
/// against.
///
/// The adapter compares the plan against the filesystem, installs it through the
/// pending-journal protocol, and reports which terminal it reached. A durably
/// claimed publication that does not settle is recovered here rather than
/// dropped, because dropping it would quarantine publication for the rest of the
/// process. Durability is the adapter's documented file-and-directory-`fsync`
/// envelope: atomic publication plus process- and OS-crash recovery, with no
/// sudden-power-loss claim on any platform.
pub(crate) fn publish_identity_ledger(
    root: &Path,
    plan: LedgerPublicationPlan,
) -> Result<IdsPublication, CaptureFailure> {
    let mut guard = ProjectMetadataWriteGuard::acquire(root).map_err(publication_projection)?;
    match guard.publish_ids(plan).map_err(publication_projection)? {
        IdsPublishOutcome::Settled(publication) => Ok(publication),
        IdsPublishOutcome::Pending(pending) => pending.recover().map_err(publication_projection),
    }
}

/// Copy a publication refusal into the CLI terminal sink shape. The adapter owns
/// the code and the message; nothing is classified here.
fn publication_projection(error: IdsPublicationError) -> CaptureFailure {
    CaptureFailure {
        code: error.code(),
        message: error.to_string(),
        location: None,
    }
}

/// Copy the opaque physical failure into the CLI terminal sink shape through the
/// one presentation facade. This materializer classifies nothing: it copies the
/// facade-owned code, the streamed message body, and the optional located file.
fn terminal_projection(root: &Path, failure: &PhysicalCaptureFailure) -> CaptureFailure {
    let presentation = failure.presentation(root);
    let code = presentation.code();

    let mut message = String::new();
    // Writing into a `String` never fails at the `fmt::Write` boundary.
    presentation
        .write_cli_message(&mut message)
        .expect("writing a capture message into a String cannot fail");

    let location = presentation.position().map(|position| {
        let mut file = String::new();
        presentation
            .write_position_file(&mut file)
            .expect("writing a located file into a String cannot fail");
        ManifestLocation {
            file,
            line: position.line,
            column: position.column,
        }
    });

    CaptureFailure {
        code,
        message,
        location,
    }
}

/// Capture the project at `root` and compile it with `compile`, reporting a capture
/// failure or a compile failure on standard error. `hint` is one extra line printed
/// after source diagnostics, naming what the operator should do first.
pub(crate) fn compile_project<T>(
    root: &Path,
    compile: impl FnOnce(&ProjectInput) -> Result<T, marrow_compile::CompileFailure>,
    hint: Option<&str>,
) -> Result<T, ExitCode> {
    let project = capture_project(root).map_err(|failure| {
        crate::report_simple_error(failure.code, &failure.message);
        ExitCode::FAILURE
    })?;
    compile(&project).map_err(|failure| {
        report_compile_failure(&failure, hint);
        ExitCode::FAILURE
    })
}

/// Report a compile failure on standard error: every source diagnostic in the one
/// diagnostic form, or the one fixed code line an exhausted bound or a failed internal
/// check earns.
fn report_compile_failure(failure: &marrow_compile::CompileFailure, hint: Option<&str>) {
    match failure {
        marrow_compile::CompileFailure::Diagnostics(diagnostics) => {
            let palette = Palette::for_stream(Stream::Stderr);
            for diagnostic in diagnostics {
                eprintln!("{}", diagnostic_line(palette, diagnostic));
            }
            if let Some(hint) = hint {
                eprintln!("{hint}");
            }
        }
        marrow_compile::CompileFailure::ResourceLimit(limit) => crate::report_simple_error(
            Code::CliCompilerResourceLimit,
            &crate::resource_limit_message(limit.kind().description()),
        ),
        marrow_compile::CompileFailure::Invariant(_) => crate::report_simple_error(
            Code::CliCompilerInvariant,
            "the compiler failed an internal consistency check",
        ),
    }
}

/// One compile diagnostic through the one renderer, under the compiler's own file
/// spelling: a file a dependency declares carries that dependency's alias, as in
/// `graphtext:src/text.mw`.
fn diagnostic_line(palette: Palette, diagnostic: &SourceDiagnostic) -> String {
    palette.diagnostic(
        &diagnostic_file(diagnostic),
        diagnostic.line(),
        diagnostic.column(),
        diagnostic.code().as_str(),
        diagnostic.message(),
    )
}

/// The compiler's spelling of the file a diagnostic is located in.
pub(crate) fn diagnostic_file(diagnostic: &SourceDiagnostic) -> String {
    ProjectFile::new(diagnostic.origin().clone(), diagnostic.file().clone()).spelling()
}
