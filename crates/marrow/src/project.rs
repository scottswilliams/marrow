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
use marrow_project::{FileIdentity, LedgerPublicationPlan, ProjectInput, SourceOrigin};
use marrow_project_fs::{
    CaptureFailure as PhysicalCaptureFailure, IdsPublication, IdsPublicationError,
    IdsPublishOutcome, OverlaySnapshot, ProjectMetadataWriteGuard,
};

use crate::term_style::{Stream, Style};

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

/// Which tree each captured module came from, keyed by the two spellings the CLI's
/// renderers hold: the [`FileIdentity`] a diagnostic carries and the dotted module name
/// an export entry carries.
///
/// A [`FileIdentity`] is relative to its own tree's root, so two origins may hold the
/// same one and it does not name a tree by itself. The origin is the sibling fact that
/// does, and the renderer — never the identity string — joins the two.
pub(crate) struct ProjectOrigins {
    /// The captured trees in canonical order, the root first.
    origins: Vec<SourceOrigin>,
    modules: Vec<CapturedModule>,
}

struct CapturedModule {
    identity: String,
    module: String,
    origin: SourceOrigin,
}

impl ProjectOrigins {
    pub(crate) fn of(project: &ProjectInput) -> Self {
        Self {
            origins: project.origins().to_vec(),
            modules: project
                .modules()
                .iter()
                .map(|module| CapturedModule {
                    identity: module.identity().as_str().to_owned(),
                    module: module.module().as_str().to_owned(),
                    origin: module.origin().clone(),
                })
                .collect(),
        }
    }

    /// The captured trees in canonical order: the root, then each declared dependency.
    pub(crate) fn origins(&self) -> &[SourceOrigin] {
        &self.origins
    }

    /// The tree that declares the module named `module`. Module names are unique across
    /// a capture, so this join is exact.
    pub(crate) fn of_module(&self, module: &str) -> Option<&SourceOrigin> {
        self.modules
            .iter()
            .find(|held| held.module == module)
            .map(|held| &held.origin)
    }

    /// Whether the root project declares `identity`. An identity no single origin
    /// claims is not the root's, so a caller that may only act on the root project's
    /// own files fails closed.
    pub(crate) fn is_root(&self, identity: &FileIdentity) -> bool {
        matches!(self.origin_of(identity), Some(SourceOrigin::Root))
    }

    /// The spelling a file is reported under: `<alias>:<identity>` for a dependency
    /// file, and the bare identity for the root project's own.
    fn spell(&self, identity: &FileIdentity) -> String {
        match self.origin_of(identity).and_then(SourceOrigin::alias) {
            Some(alias) => format!("{}:{}", alias.as_str(), identity.as_str()),
            None => identity.as_str().to_owned(),
        }
    }

    /// The one tree holding `identity`, or `None` when two trees hold it. An identity
    /// is relative to its own tree's root, so it does not name a tree by itself.
    //
    // Recovering the origin from the module list is exact only while no two trees hold
    // the same identity. Replace this with the `SourceOrigin` the compiler carries on
    // `SourceDiagnostic` once that field exists.
    fn origin_of(&self, identity: &FileIdentity) -> Option<&SourceOrigin> {
        let mut matching = self
            .modules
            .iter()
            .filter(|held| held.identity == identity.as_str());
        match (matching.next(), matching.next()) {
            (Some(held), None) => Some(&held.origin),
            _ => None,
        }
    }
}

/// Capture the project at `root` and compile it with `compile`, reporting a capture
/// failure or a compile failure on standard error. `hint` is one extra line printed
/// after source diagnostics, naming what the operator should do first. The captured
/// origins are returned with the compiler's result so a caller can attribute an export
/// or a file to the tree that declares it.
pub(crate) fn compile_project<T>(
    root: &Path,
    compile: impl FnOnce(&ProjectInput) -> Result<T, marrow_compile::CompileFailure>,
    hint: Option<&str>,
) -> Result<(T, ProjectOrigins), ExitCode> {
    let project = capture_project(root).map_err(|failure| {
        crate::report_simple_error(failure.code, &failure.message);
        ExitCode::FAILURE
    })?;
    let origins = ProjectOrigins::of(&project);
    let compiled = compile(&project).map_err(|failure| {
        report_compile_failure(&failure, &origins, hint);
        ExitCode::FAILURE
    })?;
    Ok((compiled, origins))
}

/// Report a compile failure on standard error: every source diagnostic with its span,
/// or the one fixed code line an exhausted bound or a failed internal check earns.
fn report_compile_failure(
    failure: &marrow_compile::CompileFailure,
    origins: &ProjectOrigins,
    hint: Option<&str>,
) {
    match failure {
        marrow_compile::CompileFailure::Diagnostics(diagnostics) => {
            for diagnostic in diagnostics {
                eprintln!("{}", diagnostic_line(diagnostic, origins));
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

/// One diagnostic rendered as `file:line:column: code: message`, painted for a terminal.
/// A file a dependency declares carries that dependency's alias: `graphtext:src/text.mw`.
fn diagnostic_line(
    diagnostic: &marrow_compile::SourceDiagnostic,
    origins: &ProjectOrigins,
) -> String {
    format!(
        "{}:{}:{}: {}: {}",
        paint(Style::Muted, &origins.spell(diagnostic.file())),
        diagnostic.line(),
        diagnostic.column(),
        paint(Style::Code, diagnostic.code().as_str()),
        diagnostic.message(),
    )
}

fn paint(style: Style, text: &str) -> String {
    crate::term_style::paint(Stream::Stderr, style, text)
}
