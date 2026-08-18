//! The CLI's project-capture boundary: a thin delegation to the physical adapter.
//!
//! The physical walker lives in [`marrow_project_fs`], the one filesystem owner
//! below the tool consumers. This module hard-cuts the CLI's capture through its
//! `capture_project` with an empty overlay and projects the opaque failure into the
//! CLI's terminal `{ code, message, location }` sink shape through the one
//! presentation facade. It reconstructs no discovery, identity, code, path, or
//! message here.
//!
//! The same adapter owns `.marrow/ids` publication and its recovery. Capture
//! refuses while a publication marker is live, so `marrow run` — the one command
//! that writes the ledger — settles any interrupted publication before it
//! captures the project or draws entropy, and every other front door reports the
//! marker. The CLI compares no state, writes no entry, and models no durability
//! itself.

use std::path::Path;

use marrow_project::{LedgerPublicationPlan, ProjectInput};
use marrow_project_fs::{
    CaptureFailure as PhysicalCaptureFailure, IdsPublication, IdsPublicationError,
    IdsPublishOutcome, OverlaySnapshot, ProjectMetadataWriteGuard,
};

/// The manifest file at a project root. Retained for `cmd_init`.
pub(crate) const MANIFEST_FILE: &str = "marrow.toml";

/// A project-capture failure, rendered by the CLI as a typed `code: message`
/// line. `location` names the manifest and 1-based position when the fault is a
/// located manifest syntax error.
pub(crate) struct CaptureFailure {
    pub(crate) code: &'static str,
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
        code: error.code().as_str(),
        message: error.to_string(),
        location: None,
    }
}

/// Copy the opaque physical failure into the CLI terminal sink shape through the
/// one presentation facade. This materializer classifies nothing: it copies the
/// facade-owned code, the streamed message body, and the optional located file.
fn terminal_projection(root: &Path, failure: &PhysicalCaptureFailure) -> CaptureFailure {
    let presentation = failure.presentation(root);
    let code = presentation.code().as_str();

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
