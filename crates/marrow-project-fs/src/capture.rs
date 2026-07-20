//! The physical project-capture entry point: opened-handle admission, one bounded
//! iterative source traversal, and composition into the pure `marrow_project` owner.
//!
//! Every retained file uses an admitted opened handle whose pre-observed
//! `(dev, ino, kind, nlink)` identity matches the opened handle and rechecks before
//! and after a bounded limit-plus-one read. A directory is one atomic order-
//! independent admission batch; traversal is iterative over one `Vec<DirectoryFrame>`
//! with no native recursion. Native paths are charged through one live/aggregate
//! [`PathBudget`]; an overlaid source retains overlay bytes and masks only disk-body
//! state, never open, role, or identity. The pure owner alone publishes
//! `FileIdentity`, `ModuleName`, and `ProjectInput`.

use std::path::Path;

use marrow_project::ProjectInput;

use crate::failure::CaptureFailure;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use crate::failure::{PhysicalFailure, PhysicalOperation, PhysicalRefusal, PhysicalRole};
use crate::limits::AdapterLimits;
use crate::overlay::OverlaySnapshot;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) mod unix;

/// Read and capture the project rooted at `root` into an immutable [`ProjectInput`],
/// enforcing the frozen [`AdapterLimits`]. Overlay entries replace disk bodies for
/// admitted members; every current CLI capture supplies the empty overlay.
///
/// # Errors
///
/// Returns an opaque [`CaptureFailure`] for a manifest, source, ledger, physical, or
/// overlay-input refusal; present it through [`CaptureFailure::presentation`].
pub fn capture_project(
    root: &Path,
    overlay: OverlaySnapshot<'_>,
) -> Result<ProjectInput, CaptureFailure> {
    capture_project_with_limits(root, overlay, &AdapterLimits::DEFAULT)
}

/// The limit-parameterized capture seam. Production capture always uses
/// [`AdapterLimits::DEFAULT`]; small policies drive owner tests only.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn capture_project_with_limits(
    root: &Path,
    overlay: OverlaySnapshot<'_>,
    limits: &AdapterLimits,
) -> Result<ProjectInput, CaptureFailure> {
    unix::capture(root, overlay, limits)
}

/// On a target with no admitted physical implementation, fail closed at the first
/// capture boundary with the one canonical pathless tuple, before any filesystem
/// operation.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn capture_project_with_limits(
    _root: &Path,
    _overlay: OverlaySnapshot<'_>,
    _limits: &AdapterLimits,
) -> Result<ProjectInput, CaptureFailure> {
    Err(CaptureFailure::from_physical(PhysicalFailure::new(
        PhysicalRole::Root,
        PhysicalOperation::Open,
        None,
        PhysicalRefusal::UnsupportedPlatform,
    )))
}
