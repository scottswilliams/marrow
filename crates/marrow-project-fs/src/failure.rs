//! The closed public failure vocabulary of the physical project adapter and the
//! opaque top-level [`CaptureFailure`].
//!
//! The support enums are transparent and exhaustively matchable, but a producer
//! value cannot be constructed outside this crate because the evidence that
//! distinguishes it — a raw I/O error or a charged root-relative path — is
//! private. The top-level [`CaptureFailure`] is opaque: its family is a private
//! enum with no public accessor, constructor, destructuring surface, or
//! family-bearing `Debug`. Producers and the presentation facade that reads this
//! evidence are introduced in the capture baseline; this module fixes the closed
//! boundary and its `Send + Sync + 'static` guarantees first.

use std::fmt;
use std::io;
use std::path::Path;

use marrow_project::{CaptureError, ManifestError};

use crate::overlay::OverlayFailure;
use crate::path::OperationalPath;
use crate::presentation::CapturePresentation;

/// A physical filesystem role the adapter admits while capturing a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalRole {
    /// The selected project root.
    Root,
    /// The required `marrow.toml` manifest.
    Manifest,
    /// The optional `marrow.ids` identity ledger.
    IdentityLedger,
    /// The optional `src` source root.
    SourceRoot,
    /// A directory below the source root.
    SourceDirectory,
    /// A selected `.mw` source file.
    SourceFile,
}

/// A physical operation active when admission produced evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalOperation {
    /// Resolve the selected root to one canonical physical path.
    Canonicalize,
    /// Inspect a path without following symbolic links.
    Inspect,
    /// Open an inspected object.
    Open,
    /// Enumerate a source directory.
    Enumerate,
    /// Reserve or charge bounded adapter-owned storage or a native-path lease.
    Retain,
    /// Read bytes from an admitted handle.
    Read,
    /// Recheck retained physical evidence.
    Recheck,
}

/// A filesystem object's observed kind, without following symbolic links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalKind {
    /// A regular file.
    RegularFile,
    /// A directory.
    Directory,
    /// Any other filesystem object.
    Other,
}

/// Where a refused symbolic link appeared relative to a role's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkPosition {
    /// The role's terminal path component is a symbolic link.
    Terminal,
    /// A component before the role's terminal path is a symbolic link.
    Intermediate,
}

/// A bounded physical resource the adapter enforces before retention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalBound {
    /// Bounded `marrow.toml` bytes.
    ManifestBytes,
    /// Bounded `marrow.ids` bytes.
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
    /// UTF-8 bytes in one semantically valid selected-source spelling.
    SourceSpellingBytes,
    /// Simultaneously live platform-native path units the adapter retains.
    RetainedPathUnits,
    /// Aggregate platform-native path units the adapter works over.
    PathWorkUnits,
}

/// An opaque operating-system I/O error. Only the typed kind and raw OS code are
/// observable; the raw [`io::Error`] is private so operating-system prose never
/// leaks through this boundary.
pub struct PhysicalIoError(io::Error);

impl PhysicalIoError {
    /// Wrap a raw operating-system error as opaque evidence.
    pub(crate) fn new(error: io::Error) -> Self {
        Self(error)
    }

    /// The typed [`io::ErrorKind`] of the underlying error.
    pub fn kind(&self) -> io::ErrorKind {
        self.0.kind()
    }

    /// The raw operating-system error code, when the error carries one.
    pub fn raw_os_error(&self) -> Option<i32> {
        self.0.raw_os_error()
    }

    /// The raw error, available only to the crate's CLI presentation writer for
    /// exact operating-system `Display` byte compatibility.
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
pub enum PhysicalRefusal {
    /// A required object was absent at its inspection checkpoint.
    Missing {
        /// The opaque operating-system error.
        error: PhysicalIoError,
    },
    /// A symbolic link appeared at a prohibited component.
    Link {
        /// Whether the link was terminal or intermediate.
        position: LinkPosition,
    },
    /// The observed object had the wrong role kind.
    UnexpectedKind {
        /// Kind required by the role.
        expected: PhysicalKind,
        /// Kind observed without following links.
        actual: PhysicalKind,
    },
    /// A retained regular file had more than one hardlink.
    Hardlink,
    /// A selected operating-system path could not be represented as UTF-8.
    InvalidPathEncoding,
    /// An operating-system or checked-allocation operation failed.
    Io {
        /// The opaque operating-system or adapter-created error.
        error: PhysicalIoError,
    },
    /// Physical evidence changed between checkpoints.
    Changed,
    /// A physical resource exceeded its fixed limit.
    Bound {
        /// Resource that exceeded its limit.
        bound: PhysicalBound,
        /// Inclusive limit.
        limit: usize,
        /// Observed amount at refusal.
        actual: usize,
    },
    /// The target platform has no admitted physical-capture implementation.
    UnsupportedPlatform,
}

/// A physical admission failure: the role, the operation active at refusal, the
/// typed refusal, and — kept private — an already-charged root-relative path.
///
/// Role, operation, and refusal are publicly observable; the path is available
/// only to the presentation facade, never to a consumer directly, and the
/// selected root and every pre-lease path-budget refusal carry no path at all.
pub struct PhysicalFailure {
    role: PhysicalRole,
    operation: PhysicalOperation,
    // Already-charged root-relative evidence, read only by the presentation facade.
    path: Option<OperationalPath>,
    refusal: PhysicalRefusal,
}

impl PhysicalFailure {
    /// Build a physical admission failure from its role, operation, root-relative
    /// path evidence, and typed refusal.
    pub(crate) fn new(
        role: PhysicalRole,
        operation: PhysicalOperation,
        path: Option<OperationalPath>,
        refusal: PhysicalRefusal,
    ) -> Self {
        Self {
            role,
            operation,
            path,
            refusal,
        }
    }

    /// The physical role being admitted at refusal.
    pub fn role(&self) -> PhysicalRole {
        self.role
    }

    /// The physical operation active at refusal.
    pub fn operation(&self) -> PhysicalOperation {
        self.operation
    }

    /// The typed refusal evidence.
    pub fn refusal(&self) -> &PhysicalRefusal {
        &self.refusal
    }

    /// The already-charged root-relative path evidence, for the presentation
    /// facade only.
    pub(crate) fn path(&self) -> Option<&OperationalPath> {
        self.path.as_ref()
    }
}

impl fmt::Debug for PhysicalFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The charged path is deliberately omitted: it never appears in direct
        // formatting, only in facade rendering joined to the caller's root.
        f.debug_struct("PhysicalFailure")
            .field("role", &self.role)
            .field("operation", &self.operation)
            .field("refusal", &self.refusal)
            .finish_non_exhaustive()
    }
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

const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    // The transferable public failures and their opaque I/O evidence.
    assert_send_sync_static::<CaptureFailure>();
    assert_send_sync_static::<PhysicalFailure>();
    assert_send_sync_static::<OverlayFailure>();
    assert_send_sync_static::<PhysicalIoError>();
    assert_send_sync_static::<PhysicalRefusal>();
    assert_send_sync_static::<PhysicalRole>();
    assert_send_sync_static::<PhysicalOperation>();
    assert_send_sync_static::<PhysicalKind>();
    assert_send_sync_static::<PhysicalBound>();
    assert_send_sync_static::<LinkPosition>();
};
