//! The closed failure vocabulary of the physical project adapter and the opaque
//! top-level [`CaptureFailure`].
//!
//! The support enums are crate-private: a consumer observes a failure only
//! through the presentation facade, which renders the typed code and message.
//! [`CaptureFailure`] is opaque: its family is a private enum with no public
//! accessor, constructor, destructuring surface, or family-bearing `Debug`. This
//! module owns that closed boundary and its `Send + Sync + 'static` guarantees.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use marrow_project::{CaptureError, ManifestError};

use crate::overlay::OverlayFailure;
use crate::presentation::CapturePresentation;
use crate::publication::IdsPublicationMarker;

/// A physical filesystem role the adapter admits while capturing a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhysicalRole {
    /// The selected project root.
    Root,
    /// The required `marrow.toml` manifest.
    Manifest,
    /// The optional `.marrow/ids` identity ledger.
    IdentityLedger,
    /// The optional `src` source root.
    SourceRoot,
    /// A directory below the source root.
    SourceDirectory,
    /// A selected `.mw` source file.
    SourceFile,
    /// A local dependency the root manifest declares, while it is being located
    /// and admitted as a project. Every refusal in this role is a
    /// dependency-path fault, whatever evidence it carries.
    Dependency,
}

/// A filesystem object's observed kind, without following symbolic links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhysicalKind {
    /// A regular file.
    RegularFile,
    /// A directory.
    Directory,
    /// Any other filesystem object.
    Other,
}

/// Where a refused symbolic link appeared relative to a role's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkPosition {
    /// The role's terminal path component is a symbolic link.
    Terminal,
    /// A component before the role's terminal path is a symbolic link.
    Intermediate,
}

/// A bounded physical resource the adapter enforces before retention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhysicalBound {
    /// Bounded `marrow.toml` bytes.
    ManifestBytes,
    /// Bounded `.marrow/ids` bytes.
    IdentityLedgerBytes,
    /// Total directory entries visited below `src`, including ignored entries.
    VisitedEntries,
    /// Directory edges traversed below `src`.
    TraversalDepth,
    /// Selected `.mw` source files.
    SourceFiles,
    /// Bytes retained for one selected source file.
    SourceFileBytes,
    /// Bytes retained for all selected source files.
    SourceTotalBytes,
    /// Simultaneously live platform-native path units the adapter retains.
    RetainedPathUnits,
    /// Aggregate platform-native path units the adapter works over.
    PathWorkUnits,
}

/// An operating-system I/O error whose `Debug` carries only the typed kind and raw
/// OS code, so operating-system prose reaches a consumer only through the CLI
/// writer's exact `Display`.
pub(crate) struct PhysicalIoError(io::Error);

impl PhysicalIoError {
    pub(crate) fn new(error: io::Error) -> Self {
        Self(error)
    }

    /// The raw error, for the CLI presentation writer's exact operating-system
    /// `Display` byte compatibility.
    pub(crate) fn as_io_error(&self) -> &io::Error {
        &self.0
    }
}

impl fmt::Debug for PhysicalIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhysicalIoError")
            .field("kind", &self.0.kind())
            .field("raw_os_error", &self.0.raw_os_error())
            .finish()
    }
}

/// Why a physical role could not be admitted.
#[derive(Debug)]
pub(crate) enum PhysicalRefusal {
    /// A required object was absent at its inspection checkpoint.
    Missing { error: PhysicalIoError },
    /// A symbolic link appeared at a prohibited component.
    Link { position: LinkPosition },
    /// The observed object had the wrong role kind.
    UnexpectedKind { expected: PhysicalKind },
    /// A retained regular file had more than one hardlink.
    Hardlink,
    /// A selected operating-system path could not be represented as UTF-8.
    InvalidPathEncoding,
    /// An operating-system or checked-allocation operation failed.
    Io { error: PhysicalIoError },
    /// Physical evidence changed between checkpoints.
    Changed,
    /// A physical resource exceeded its fixed limit.
    Bound {
        bound: PhysicalBound,
        limit: usize,
        actual: usize,
    },
    /// The identity ledger was found at its retired project-root path
    /// (`marrow.ids`) instead of its home (`.marrow/ids`).
    LegacyLedgerPath { home: LedgerHome },
    /// A declared dependency resolved to a directory that cannot serve as one.
    Dependency { reason: DependencyRefusal },
}

/// Why a resolved dependency directory cannot serve as a dependency. The location
/// faults a path spelling can carry — absolute, non-canonical, over-long — are
/// refused by the pure manifest owner before any of these are reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DependencyRefusal {
    /// The directory holds no `marrow.toml`, or no `src` source root.
    NotAProject,
    /// The directory's `marrow.toml` is not a valid manifest.
    InvalidManifest,
    /// The path resolves to the consuming project itself.
    SelfReference,
    /// The dependency declares `[dependencies]` of its own. This build admits no
    /// transitive dependency, so a dependency graph is always one edge deep and a
    /// cycle through one is unrepresentable.
    Transitive,
}

/// Whether the ledger's home path (`.marrow/ids`) also holds a file when its
/// retired project-root path is occupied. The two states carry different
/// remedies: a vacant home is a one-command move; an occupied home must be
/// reconciled by hand before the root copy is deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LedgerHome {
    /// `.marrow/ids` is absent; the ledger lives only at the retired root path.
    Vacant,
    /// `.marrow/ids` also holds a file; a project has exactly one ledger.
    Occupied,
}

/// A physical admission failure: the role, the caller-root-relative path the
/// refusal names, and the typed refusal. The
/// selected root and every pre-lease path-budget refusal carry no path.
#[derive(Debug)]
pub(crate) struct PhysicalFailure {
    pub(crate) role: PhysicalRole,
    pub(crate) path: Option<PathBuf>,
    pub(crate) refusal: PhysicalRefusal,
}

/// The private family a [`CaptureFailure`] wraps. It is neither constructible nor
/// matchable outside this crate; the presentation facade is its only external
/// reader.
pub(crate) enum CaptureFailureKind {
    /// Manifest bytes reached the pure manifest parser and were refused.
    Manifest(ManifestError),
    /// Admitted bytes reached the pure project owner and were refused.
    Project(CaptureError),
    /// A filesystem role could not be admitted.
    Physical(PhysicalFailure),
    /// Borrowed overlay input or physical membership was refused.
    OverlayInput(OverlayFailure),
    /// A live `.marrow/ids` publication marker makes the committed ledger
    /// indeterminate, so no front door reads it.
    IdsPublicationPending(IdsPublicationMarker),
}

/// A physical project capture did not produce a pure [`ProjectInput`].
///
/// This is an opaque wrapper over a private closed family. It exposes no public
/// family accessor, variant constructor, destructuring surface, or
/// family-bearing `Debug`: formatting reveals only the type name. A consumer
/// obtains a message and the typed code through [`CaptureFailure::presentation`].
///
/// [`ProjectInput`]: marrow_project::ProjectInput
pub struct CaptureFailure(CaptureFailureKind);

impl CaptureFailure {
    /// Wrap a pure manifest refusal.
    pub(crate) fn from_manifest(error: ManifestError) -> Self {
        Self(CaptureFailureKind::Manifest(error))
    }

    /// Wrap a pure project-capture refusal.
    pub(crate) fn from_project(error: CaptureError) -> Self {
        Self(CaptureFailureKind::Project(error))
    }

    /// Wrap a physical admission refusal.
    pub(crate) fn from_physical(failure: PhysicalFailure) -> Self {
        Self(CaptureFailureKind::Physical(failure))
    }

    /// Wrap a live publication marker: capture refuses before it reads a
    /// generation recovery may replace.
    pub(crate) fn from_ids_publication_marker(marker: IdsPublicationMarker) -> Self {
        Self(CaptureFailureKind::IdsPublicationPending(marker))
    }

    /// Wrap a borrowed-overlay-input refusal that occurred before capture. This is
    /// the sole public family constructor: it lets a consumer that received an
    /// [`OverlayFailure`] from snapshot construction carry it through the opaque
    /// boundary for presentation, performing no new classification or rendering.
    pub fn from_overlay_input(failure: OverlayFailure) -> Self {
        Self(CaptureFailureKind::OverlayInput(failure))
    }

    /// Borrow a presentation facade over this failure and a caller root spelling.
    pub fn presentation<'a>(&'a self, caller_root: &'a Path) -> CapturePresentation<'a> {
        CapturePresentation::new(caller_root, self)
    }

    /// The private family, for the crate's presentation facade only.
    pub(crate) fn kind(&self) -> &CaptureFailureKind {
        &self.0
    }
}

impl fmt::Debug for CaptureFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Opaque: the private family never appears in direct formatting.
        f.debug_struct("CaptureFailure").finish_non_exhaustive()
    }
}

// The two failures a consumer holds are transferable.
const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<CaptureFailure>();
    assert_send_sync_static::<OverlayFailure>();
};
