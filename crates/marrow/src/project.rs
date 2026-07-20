//! The CLI's project-capture boundary: a thin delegation to the physical adapter.
//!
//! The physical walker lives in [`marrow_project_fs`], the one filesystem owner
//! below the tool consumers. This module hard-cuts the CLI's capture through its
//! `capture_project` with an empty overlay and projects the opaque failure into the
//! CLI's terminal `{ code, message, location }` sink shape through the one
//! presentation facade. It reconstructs no discovery, identity, code, path, or
//! message here.

use std::path::Path;

use marrow_project::ProjectInput;
use marrow_project_fs::{CaptureFailure as PhysicalCaptureFailure, OverlaySnapshot};

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
