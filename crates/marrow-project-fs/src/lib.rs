//! The typed physical project-input boundary for Marrow tools.
//!
//! This crate is the one filesystem adapter that sits below the tool consumers
//! and above the pure [`marrow_project`] owner. It declares the consumer-facing
//! capture facade — borrowed root-relative overlay input and an opaque
//! [`CaptureFailure`] presented through a borrowed [`CapturePresentation`] — and
//! the serialized publication of `.marrow/ids`. It re-exports the pure-owner
//! facts a thin consumer names beside a captured [`ProjectInput`] — its
//! [`SourceOrigin`], [`DependencyAlias`], [`DependencyPath`], and
//! [`FileIdentity`] — so a consumer with only this edge can name the successful
//! boundary without a direct `marrow-project` edge.
//!
//! [`CaptureFailure`] is opaque: its family cannot be constructed, matched, or
//! destructured from outside this crate.
//!
//! ```compile_fail
//! use marrow_project_fs::CaptureFailure;
//! fn classify(failure: CaptureFailure) {
//!     match failure {
//!         CaptureFailure::Project(_) => {}
//!     }
//! }
//! ```
//!
//! ```compile_fail
//! use marrow_project_fs::CaptureFailure;
//! let _ = CaptureFailure(std::process::abort());
//! ```
//!
//! [`ProjectInput`]: marrow_project::ProjectInput
//! [`SourceOrigin`]: marrow_project::SourceOrigin
//! [`DependencyAlias`]: marrow_project::DependencyAlias
//! [`DependencyPath`]: marrow_project::DependencyPath
//! [`FileIdentity`]: marrow_project::FileIdentity

#![warn(missing_docs)]

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("marrow-project-fs builds on Linux and macOS only");

mod capture;
mod failure;
mod limits;
mod overlay;
mod path;
mod presentation;
mod publication;

#[cfg(test)]
mod dependency_kats;
#[cfg(test)]
mod kats;
#[cfg(test)]
mod publication_kats;

pub use capture::capture_project;
pub use failure::CaptureFailure;
pub use overlay::{OverlayEntry, OverlayFailure, OverlaySnapshot};
pub use presentation::CapturePresentation;
pub use publication::{
    IdsPublication, IdsPublicationError, IdsPublicationPending, IdsPublishOutcome,
    ProjectMetadataWriteGuard, ids_publication_pending,
};

pub use marrow_project::{
    DependencyAlias, DependencyPath, FileIdentity, ProjectInput, SourceOrigin,
};
