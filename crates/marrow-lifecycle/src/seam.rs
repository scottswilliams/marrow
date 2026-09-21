//! The one seam a durability sequence exposes to an observer.
//!
//! Provision, attach, apply, recovery, restore, and backup each cross a fixed series of
//! steps: the file and directory syncs that make a store's metadata durable in order, and
//! the checkpoints between them. A crash or an I/O failure can cut the sequence at any of
//! them, and the recovery contract is stated per cut. Production runs every sequence with no
//! observer; a test arms one that cuts, mutates, or records at a named step. The sequence
//! itself is the same code either way, so every fault fixture drives the path the product
//! runs rather than a second program interleaved with it.

use std::path::Path;
use std::rc::Rc;

use marrow_fs_journal::CustodyError;
use marrow_kernel::durable::PendingNativeStoreOwner;

use crate::store_dir::{AdmittedStoreDir, Artifact, Body};

/// One step of a store-directory sequence, named for the operation it precedes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The first append into a freshly created file.
    Append(Body),
    /// The file sync closing a freshly written file.
    FileSync(Body),
    /// The rename installing a replacement over its artifact.
    Install(Artifact),
    /// The directory sync closing a complete construction stage.
    ConstructionStage,
    /// The directory sync after a publication's Active envelope.
    Activation,
    /// The directory sync after a rebind's Pending envelope.
    RebindPending,
    /// The directory sync after a rebind's new head.
    RebindHead,
    /// The directory sync after a rebind's Active envelope.
    RebindActive,
    /// Attach: the read-only admission passed.
    Admitted,
    /// Attach: service was prepared under the retained owner.
    Prepared,
    /// Attach and apply: the Active envelope is durable; the final reread follows.
    Activated,
    /// The directory sync after a preserved replacement's move.
    Preservation,
    /// Recovery: the directory sync after the artifact syncs.
    RecoveryArtifacts,
    /// Recovery: the parent-directory sync.
    RecoveryParent,
    /// Recovery: the directory sync after the Active envelope.
    RecoveryActive,
    /// The final reread of both metadata artifacts that closes an activation.
    FinalRead,
}

/// Where a provisioning stage stands when an observer sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StagePoint {
    /// Complete under its owner, before the rename that publishes it.
    Built,
    /// Renamed onto its destination, before the parent barrier.
    Published,
}

/// What an observer is shown: a step over the directory it runs in, or one of the points
/// outside a store directory that a sequence passes. The payload is read by the observers
/// tests arm; production arms none.
#[cfg_attr(not(test), expect(dead_code, reason = "read only by armed observers"))]
pub(crate) enum Event<'a> {
    /// `step` is about to run over `dir`.
    Step {
        dir: &'a AdmittedStoreDir,
        step: Step,
    },
    /// The owner lock over the directory at `path` is held; nothing in it has been read.
    Locked { path: &'a Path },
    /// A provisioning stage is complete under `owner` and admitted as `admitted`, currently
    /// at `location`.
    Stage {
        at: StagePoint,
        location: &'a Path,
        owner: &'a PendingNativeStoreOwner,
        admitted: &'a AdmittedStoreDir,
    },
    /// The parent-directory sync that makes a published entry durable.
    ParentSync,
    /// The unpublished stage at `stage` is about to be removed.
    Removal { stage: &'a Path },
    /// A backup's output file, staged at `stage`, is about to be synced.
    OutputSync { stage: &'a Path },
}

/// A test's view of a sequence. An error returned for an event stands in for the operation
/// the event precedes failing with it.
pub(crate) trait Observer {
    fn at(&self, event: Event<'_>) -> Result<(), CustodyError>;
}

/// The seam a sequence runs through: no observer in production, one in a test.
#[derive(Clone)]
pub(crate) struct Seam(Option<Rc<dyn Observer>>);

impl Seam {
    /// The production seam: every event passes.
    pub(crate) const NONE: Self = Self(None);

    #[cfg(test)]
    pub(crate) fn armed(observer: Rc<dyn Observer>) -> Self {
        Self(Some(observer))
    }

    pub(crate) fn at(&self, event: Event<'_>) -> Result<(), CustodyError> {
        match &self.0 {
            None => Ok(()),
            Some(observer) => observer.at(event),
        }
    }
}
