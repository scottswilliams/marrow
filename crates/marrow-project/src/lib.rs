//! The pure Marrow project-input owner.
//!
//! This crate owns the boundary between a project on disk and the compiler: the
//! closed versioned manifest schema ([`Manifest`]), deterministic contained
//! discovery over caller-supplied file listings and bytes ([`capture`]), the
//! root-relative canonical file identities and path-derived module names
//! ([`FileIdentity`], [`ModuleName`]), the durable-identity ledger and its
//! committed machine-written artifact ([`IdentityLedger`], `.marrow/ids`), and
//! the immutable [`ProjectInput`] every later stage consumes.
//!
//! It is pure: no filesystem, Git, network, compiler, runtime, or store edge. The
//! separate `marrow-project-fs` crate walks `src`, reads bytes through admitted
//! handles, and enforces the bounded physical admission; it feeds this owner, which
//! validates its input and rechecks the bounds. Pure discovery is deterministic and
//! location-independent: the same files yield a byte-identical [`ProjectInput`]
//! regardless of arrival order or where the project lives.

mod capture;
mod dependency;
mod identity;
mod ids;
mod manifest;

pub use capture::{
    CaptureBound, CaptureError, CaptureErrorKind, CaptureLimits, CapturedDependency, CapturedFile,
    CollisionReason, ModuleInput, ProjectInput, capture, capture_origins,
};
pub use dependency::{
    Dependency, DependencyAlias, DependencyAliasReason, DependencyPath, DependencyPathReason,
    MAX_DEPENDENCY_ALIAS_BYTES, MAX_DEPENDENCY_PATH_BYTES,
};
pub use identity::is_placeholder;
pub use identity::{
    FileIdentity, MAX_FILE_IDENTITY_BYTES, ModuleName, SOURCE_EXTENSION, SOURCE_ROOT, SourceOrigin,
    SourcePathReason,
};
pub use ids::{
    DurableIdentityId, IDS_ENTRY, IDS_FILE, IdentityAnchor, IdentityKind, IdentityLedger,
    IdentityMintFailure, IdentityMutationError, IdentityTombstone, IdsError, IdsErrorKind,
    LEGACY_IDS_FILE, LedgerPublicationPlan, MAX_IDS_BYTES, MAX_IDS_ROWS, META_DIR,
};
pub use manifest::{
    DependencyShapeFault, Edition, Manifest, ManifestError, ManifestErrorKind, Position,
};
