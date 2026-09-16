//! The typed physical project-input boundary for Marrow tools.
//!
//! This crate is the one filesystem adapter that sits below the tool consumers
//! and above the pure [`marrow_project`] owner. It declares the consumer-facing
//! capture facade: a closed public failure vocabulary, borrowed root-relative
//! overlay input, and an opaque [`CaptureFailure`] presented through a borrowed
//! [`CapturePresentation`]. It re-exports the canonical pure-owner facts a thin
//! consumer needs — the diagnostic [`Code`] registry and the pure
//! [`ProjectInput`], its [`SourceOrigin`], [`DependencyAlias`] and [`DependencyPath`],
//! [`ManifestError`], [`CaptureError`], and manifest [`Position`] — so a consumer
//! with only this edge can name the successful boundary without a direct
//! `marrow-project` edge.
//!
//! This module owns the closed public boundary, its `Send + Sync + 'static`
//! guarantees, and the sealed pure-owner facts it forwards.
//!
//! # Sealed boundaries
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
//! The re-exported pure errors seal their fields: an arbitrary field combination
//! is unrepresentable outside the pure owner.
//!
//! ```compile_fail
//! use marrow_project_fs::ManifestError;
//! let _ = ManifestError {
//!     code: std::process::abort(),
//!     kind: std::process::abort(),
//!     message: std::process::abort(),
//!     position: std::process::abort(),
//! };
//! ```
//!
//! ```compile_fail
//! use marrow_project_fs::CaptureError;
//! let _ = CaptureError {
//!     code: std::process::abort(),
//!     kind: std::process::abort(),
//!     message: std::process::abort(),
//! };
//! ```
//!
//! [`Code`]: marrow_codes::Code
//! [`ProjectInput`]: marrow_project::ProjectInput
//! [`ManifestError`]: marrow_project::ManifestError
//! [`CaptureError`]: marrow_project::CaptureError
//! [`Position`]: marrow_project::Position
//! [`SourceOrigin`]: marrow_project::SourceOrigin
//! [`DependencyAlias`]: marrow_project::DependencyAlias
//! [`DependencyPath`]: marrow_project::DependencyPath

#![warn(missing_docs)]

mod capture;
mod failure;
mod limits;
mod overlay;
mod path;
mod presentation;
mod publication;

/// The crate's one temporary-directory fixture, shared with the integration suites.
#[cfg(test)]
#[path = "../tests/common/scratch.rs"]
mod scratch;

#[cfg(test)]
mod dependency_kats;
#[cfg(test)]
mod kats;
#[cfg(test)]
mod publication_kats;

pub use capture::capture_project;
pub use failure::{
    CaptureFailure, DependencyRefusal, LedgerHome, LinkPosition, PhysicalBound, PhysicalFailure,
    PhysicalIoError, PhysicalKind, PhysicalOperation, PhysicalRefusal, PhysicalRole,
};
pub use overlay::{
    OverlayBound, OverlayEntry, OverlayEntryIndex, OverlayFailure, OverlayReason, OverlaySnapshot,
};
pub use presentation::CapturePresentation;
pub use publication::{
    IdsPublication, IdsPublicationError, IdsPublicationMarker, IdsPublicationPending,
    IdsPublishOutcome, IdsRefusal, ProjectMetadataWriteGuard, ids_publication_marker,
};

pub use marrow_codes::Code;
pub use marrow_project::{
    CaptureError, DependencyAlias, DependencyPath, FileIdentity, ManifestError, Position,
    ProjectInput, SourceOrigin,
};
