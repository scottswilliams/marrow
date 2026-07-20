//! The borrowed capture-presentation facade type.
//!
//! A [`CapturePresentation`] borrows the caller's root spelling and a
//! [`CaptureFailure`], forwarding canonical pure-owner facts and owning
//! Physical/Overlay classification, path joining, and message rendering. It is
//! neither `Clone` nor an owned message, and has no `Debug`. Its rendering
//! methods — the typed code, the optional manifest position, and the streaming
//! message and location writers — are introduced in the capture baseline; this
//! declaration fixes the borrowed facade type first.

use std::path::Path;

use crate::failure::CaptureFailure;

/// A borrowed capture-presentation facade over a caller root spelling and a
/// [`CaptureFailure`].
// The borrowed fields are read by the rendering methods introduced in the
// capture baseline; the borrowed facade shape is fixed here first.
#[allow(dead_code)]
pub struct CapturePresentation<'a> {
    root: &'a Path,
    failure: &'a CaptureFailure,
}
