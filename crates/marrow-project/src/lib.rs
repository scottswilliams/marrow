//! The pure Marrow project-input owner.
//!
//! This crate owns the boundary between a project on disk and the compiler: the
//! closed versioned manifest schema ([`Manifest`]), deterministic contained
//! discovery over caller-supplied file listings and bytes ([`capture`]), the
//! root-relative canonical file identities and path-derived module names
//! ([`FileIdentity`], [`ModuleName`]), the durable-identity ledger and its
//! committed machine-written artifact ([`IdentityLedger`], `marrow.ids`), and
//! the immutable [`ProjectInput`] every later stage consumes.
//!
//! It is pure: it has no filesystem, Git, network, compiler, runtime, or store
//! edge. The physical adapter that walks `src`, reads bytes through admitted
//! opened handles, and enforces the bounded physical admission is the separate
//! `marrow-project-fs` crate; it feeds this owner, which validates its input and
//! rechecks the bounds. Keeping discovery pure makes it
//! deterministic and location-independent: the same files yield a byte-identical
//! [`ProjectInput`] regardless of arrival order or where the project lives.

mod capture;
mod identity;
mod ids;
mod manifest;

pub use capture::{
    CaptureBound, CaptureError, CaptureErrorKind, CaptureLimits, CapturedFile, CollisionReason,
    ModuleInput, ProjectInput, capture,
};
pub use identity::{FileIdentity, ModuleName, SOURCE_EXTENSION, SOURCE_ROOT, SourcePathReason};
pub use ids::{
    DurableIdentityId, IDS_FILE, IdentityAnchor, IdentityKind, IdentityLedger, IdentityTombstone,
    IdsError, IdsErrorKind, MAX_IDS_BYTES, MintError,
};
pub use manifest::{Edition, Manifest, ManifestError, ManifestErrorKind, Position};
