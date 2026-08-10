//! Serialized crash-recoverable publication of the `.marrow/ids` identity
//! ledger.
//!
//! One owner compares an admitted [`LedgerPublicationPlan`] against the
//! filesystem and installs it, or refuses. The descriptor-rooted custody,
//! cooperative lock, and bounded pending-journal frame all belong to
//! [`marrow_fs_journal`]; this module owns only the kind-1 row header, the
//! closed publication state map, and the recovery order over them.
//!
//! # Names
//!
//! Inside the project's `.marrow` directory the protocol uses exactly four
//! fixed entry names and enumerates the directory nowhere:
//!
//! ```text
//! ids                    the committed identity ledger
//! ids.publish.stage      the successor before it is installed
//! ids.pending            the durable publication marker
//! ids.pending.create     the marker's pre-claim alias
//! publish.lock           the cooperative project-metadata write lock
//! ```
//!
//! # Protocol
//!
//! Under the write guard, publication creates and syncs the stage, durably
//! claims the marker, appends `Installing`, then either hard-links the stage
//! onto an absent target with a destination refusal or atomically exchanges the
//! target with the stage, retaining the exact displaced generation at the stage
//! name. It validates the resulting identities, syncs, appends the terminal
//! record, cleans the stage, proves the target, and only then unlinks the
//! marker and syncs. The closed map every phase is checked against:
//!
//! ```text
//! Prepared absent:    target absent;   stage=next               each nlink 1
//! Prepared replace:   target=base;     stage=next               each nlink 1
//! Installing absent:  the Prepared map, or target=stage=next    nlink 2
//! Installing replace: the Prepared map, or target=next; stage=base
//! Settled installed:  target=next nlink 1, stage absent after exact cleanup
//! Settled reverted:   target neither next nor cleaned by this owner;
//!                     stage=next until exact cleanup
//! ```
//!
//! The reverted terminal is the arm a destination refusal or a continuously
//! proven third live inode settles into: the successor is not installed, the
//! artifact keeps whatever the concurrent writer left, and the outcome is
//! [`IdsPublication::ConcurrentChange`]. It is a recorded terminal rather than
//! an abandoned journal because the frame's only exit is its terminal phase.
//!
//! Every state outside that map is retained corruption. In particular
//! `target=next` with a third inode at the stage name is, after process death,
//! indistinguishable from a legitimate install whose displaced generation was
//! substituted, so it authorizes no exchange and no cleanup.
//!
//! # Durability envelope
//!
//! Every sync here is the journal owner's plain `fsync` of a file or a
//! directory. The established claim is atomic publication plus process- and
//! OS-crash recovery inside that envelope. Sudden-power-loss durability is not
//! established, on any platform.

pub(crate) mod header;
mod marker;
mod protocol;

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use marrow_codes::Code;
use marrow_fs_journal::{
    AdmittedDir, CacheLock, CorruptionReason, CustodyError, EntryName, JournalError, LockError,
    PendingName, qualified_platform,
};
use marrow_project::{LedgerPublicationPlan, MAX_IDS_BYTES};

use header::HeaderCorruption;

pub use marker::IdsPublicationMarker;

/// The project metadata directory, relative to the project root.
const META_DIR: &str = ".marrow";
/// The committed ledger's entry name inside the metadata directory.
const LEDGER_NAME: &str = "ids";
/// The fixed stage entry name inside the metadata directory.
const STAGE_NAME: &str = "ids.publish.stage";
/// The cooperative project-metadata write lock's entry name.
const LOCK_NAME: &str = "publish.lock";
/// The fixed bound on either byte run the header carries.
const LEDGER_BYTE_CEILING: usize = MAX_IDS_BYTES;

/// Whether this process dropped an unrecovered publication. A dropped pending
/// publication leaves a durable marker whose live handles are gone, so this
/// process publishes nothing further; the marker keeps gating capture until a
/// fresh process recovers it.
static QUARANTINED: AtomicBool = AtomicBool::new(false);

/// How one identity publication settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdsPublication {
    /// The admitted successor is the committed `.marrow/ids`.
    Published,
    /// The exact state the plan was admitted against is no longer the committed
    /// artifact, so the successor was not installed and every byte of the
    /// artifact is the other writer's. Recapture and admit a fresh plan.
    ConcurrentChange,
}

/// A publication that is durably claimed and has not settled.
///
/// The value is affine: it cannot be cloned, copied, serialized, or rebuilt
/// from parts, it retains the guard borrow, the live journal, and the cause
/// that interrupted it, and the only way to advance it is to consume
/// [`recover`](Self::recover). Dropping it instead quarantines publication in
/// this process until exit.
///
/// ```compile_fail
/// fn duplicate(pending: marrow_project_fs::IdsPublicationPending<'_>) {
///     let _second = pending.clone();
/// }
/// ```
#[must_use = "a durably claimed publication advances only by consuming `recover`"]
pub struct IdsPublicationPending<'a> {
    session: protocol::Session<'a>,
    cause: IdsPublicationError,
    armed: bool,
}

impl<'a> IdsPublicationPending<'a> {
    fn new(session: protocol::Session<'a>, cause: IdsPublicationError) -> Self {
        Self {
            session,
            cause,
            armed: true,
        }
    }

    /// The refusal that interrupted the publication.
    pub fn cause(&self) -> &IdsPublicationError {
        &self.cause
    }

    /// Consume the pending publication and drive it to its terminal state.
    ///
    /// # Errors
    ///
    /// Returns the fresh refusal when the publication still cannot settle. The
    /// marker is retained, and a recovery that refuses quarantines publication
    /// in this process exactly as a drop does: the retained handles go either
    /// way, so a fresh process is what settles the marker next.
    pub fn recover(mut self) -> Result<IdsPublication, IdsPublicationError> {
        let settled = self.session.drive();
        self.armed = settled.is_err();
        settled
    }
}

impl Drop for IdsPublicationPending<'_> {
    fn drop(&mut self) {
        if self.armed {
            QUARANTINED.store(true, Ordering::SeqCst);
        }
    }
}

impl fmt::Debug for IdsPublicationPending<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdsPublicationPending")
            .field("cause", &self.cause)
            .finish_non_exhaustive()
    }
}

/// What one `publish` call produced.
#[must_use = "an unsettled publication must be recovered"]
pub enum IdsPublishOutcome<'a> {
    /// The publication reached a terminal state and the marker is gone.
    Settled(IdsPublication),
    /// The publication is durably claimed and did not settle. The retained
    /// guard borrow, live journal, and cause are boxed so an ordinary settled
    /// publication does not carry them by value.
    Pending(Box<IdsPublicationPending<'a>>),
}

impl fmt::Debug for IdsPublishOutcome<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Settled(publication) => {
                formatter.debug_tuple("Settled").field(publication).finish()
            }
            Self::Pending(pending) => formatter.debug_tuple("Pending").field(pending).finish(),
        }
    }
}

/// The closed public classification of a publication or recovery refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdsRefusal {
    /// This build does not qualify the running platform for descriptor-rooted
    /// publication. Nothing was opened, created, or stated.
    UnqualifiedPlatform,
    /// Another holder has the project-metadata write lock.
    Contended,
    /// This process dropped an unrecovered publication and publishes no more.
    Quarantined,
    /// A publication was staged or created but never durably claimed. Every
    /// byte is retained; an operator removes the named entries.
    UnclaimedIncomplete,
    /// Retained corruption: the marker, its evidence, or the artifact map is
    /// not a state this protocol can have produced. No byte was mutated.
    Corrupt,
    /// A publication is durably claimed; recovery must settle it first.
    Interrupted,
    /// A filesystem operation refused.
    Custody,
    /// The pending journal refused.
    Journal,
}

impl IdsRefusal {
    /// The stable outward code this refusal reports.
    pub const fn code(self) -> Code {
        match self {
            Self::UnclaimedIncomplete | Self::Corrupt | Self::Interrupted | Self::Quarantined => {
                Code::ProjectIdsPublicationPending
            }
            Self::UnqualifiedPlatform | Self::Contended | Self::Custody | Self::Journal => {
                Code::IoWrite
            }
        }
    }
}

/// Why an identity publication or its recovery could not proceed. The
/// classification is public and closed; the underlying filesystem or frame
/// evidence is private and reaches a consumer only through `Display`.
#[derive(Debug)]
pub struct IdsPublicationError {
    refusal: IdsRefusal,
    detail: Detail,
}

#[derive(Debug)]
enum Detail {
    None,
    Custody(CustodyError),
    Journal(JournalError),
    Retained(CorruptionReason),
    Header(HeaderCorruption),
    Map(protocol::MapFault),
}

impl IdsPublicationError {
    fn bare(refusal: IdsRefusal) -> Self {
        Self {
            refusal,
            detail: Detail::None,
        }
    }

    fn corrupt(reason: CorruptionReason) -> Self {
        Self {
            refusal: IdsRefusal::Corrupt,
            detail: Detail::Retained(reason),
        }
    }

    /// The closed classification of this refusal.
    pub fn refusal(&self) -> IdsRefusal {
        self.refusal
    }

    /// The stable outward code this refusal reports.
    pub fn code(&self) -> Code {
        self.refusal.code()
    }
}

impl fmt::Display for IdsPublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.refusal {
            IdsRefusal::UnqualifiedPlatform => formatter.write_str(
                "this build does not qualify the running platform for identity publication",
            )?,
            IdsRefusal::Contended => formatter
                .write_str("another Marrow process holds the project-metadata write lock")?,
            IdsRefusal::Quarantined => formatter.write_str(
                "this process dropped an unrecovered `.marrow/ids` publication and publishes no more",
            )?,
            IdsRefusal::UnclaimedIncomplete => formatter.write_str(
                "an unfinished `.marrow/ids` publication was never durably claimed; \
                 every byte is retained and `.marrow/ids` is unchanged. Remove \
                 `.marrow/ids.publish.stage` and `.marrow/ids.pending.create` to continue",
            )?,
            IdsRefusal::Corrupt => formatter.write_str(
                "the `.marrow/ids` publication state is not one this protocol can have produced; \
                 every byte is retained and nothing will be mutated",
            )?,
            IdsRefusal::Interrupted => formatter.write_str(
                "a `.marrow/ids` publication is durably claimed and must be recovered first",
            )?,
            IdsRefusal::Custody => formatter.write_str("a `.marrow` filesystem operation refused")?,
            IdsRefusal::Journal => {
                formatter.write_str("the `.marrow/ids` publication journal refused")?;
            }
        }
        match &self.detail {
            Detail::None => Ok(()),
            Detail::Custody(error) => write!(formatter, ": {error}"),
            Detail::Journal(error) => write!(formatter, ": {error}"),
            Detail::Retained(reason) => write!(formatter, ": {reason}"),
            Detail::Header(corruption) => write!(formatter, ": {corruption}"),
            Detail::Map(fault) => write!(formatter, ": {fault}"),
        }
    }
}

impl std::error::Error for IdsPublicationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.detail {
            Detail::Custody(error) => Some(error),
            Detail::Journal(error) => Some(error),
            Detail::None | Detail::Retained(_) | Detail::Header(_) | Detail::Map(_) => None,
        }
    }
}

impl From<CustodyError> for IdsPublicationError {
    fn from(error: CustodyError) -> Self {
        let refusal = match &error {
            CustodyError::UnqualifiedPlatform { .. } => IdsRefusal::UnqualifiedPlatform,
            _ => IdsRefusal::Custody,
        };
        Self {
            refusal,
            detail: Detail::Custody(error),
        }
    }
}

impl From<JournalError> for IdsPublicationError {
    fn from(error: JournalError) -> Self {
        let refusal = match &error {
            JournalError::Custody(_) => IdsRefusal::Custody,
            _ => IdsRefusal::Journal,
        };
        Self {
            refusal,
            detail: Detail::Journal(error),
        }
    }
}

impl From<LockError> for IdsPublicationError {
    fn from(error: LockError) -> Self {
        match error {
            LockError::Held => Self::bare(IdsRefusal::Contended),
            LockError::Custody(error) => Self::from(error),
        }
    }
}

impl From<HeaderCorruption> for IdsPublicationError {
    fn from(corruption: HeaderCorruption) -> Self {
        Self {
            refusal: IdsRefusal::Corrupt,
            detail: Detail::Header(corruption),
        }
    }
}

/// The exclusive project-metadata write owner.
///
/// Acquiring the guard admits the project root and its `.marrow` directory
/// through retained descriptors and takes the cooperative `publish.lock`. Every
/// mutation of a `.marrow` publication artifact happens under one live guard,
/// which is what makes the protocol's identity witnesses a serialization rather
/// than a hope. Dropping the guard releases the lock.
///
/// The guard is the cross-kind seam: identity publication is the first kind
/// under it, and a later lineage kind takes the same lock and the same admitted
/// directory rather than opening a second write owner.
#[derive(Debug)]
pub struct ProjectMetadataWriteGuard {
    meta: AdmittedDir,
    ledger: EntryName,
    stage: EntryName,
    journal: PendingName,
    _lock: CacheLock,
}

impl ProjectMetadataWriteGuard {
    /// Acquire the exclusive write owner for the project rooted at `root`,
    /// creating `.marrow` and the lock entry when they are absent.
    ///
    /// # Errors
    ///
    /// Returns a typed refusal when the platform is unqualified, this process
    /// is quarantined, another holder has the lock, or a custody operation
    /// refused.
    pub fn acquire(root: &Path) -> Result<Self, IdsPublicationError> {
        qualified_platform()?;
        if QUARANTINED.load(Ordering::SeqCst) {
            return Err(IdsPublicationError::bare(IdsRefusal::Quarantined));
        }
        let meta_name = admitted_name(META_DIR);
        let root_dir = AdmittedDir::admit_trusted_root(root)?;
        let meta = match root_dir.admit_child(&meta_name) {
            Ok(meta) => meta,
            Err(CustodyError::NotFound { .. }) => {
                let created = root_dir.create_child_dir(&meta_name)?;
                // The metadata directory's own entry must be durable before any
                // entry inside it can be.
                root_dir.sync()?;
                created
            }
            Err(error) => return Err(error.into()),
        };
        let lock = CacheLock::acquire(&meta, &admitted_name(LOCK_NAME))?;
        let ledger = admitted_name(LEDGER_NAME);
        Ok(Self {
            journal: PendingName::derive(&ledger)
                .expect("the fixed journal names are admitted spellings"),
            ledger,
            stage: admitted_name(STAGE_NAME),
            meta,
            _lock: lock,
        })
    }

    /// Settle any durably claimed identity publication this project carries.
    ///
    /// Returns `None` when no marker and no stage entry exist. This is the call
    /// `marrow run` makes before it captures the project or draws entropy: the
    /// committed ledger is indeterminate while a claim is live, so nothing may
    /// read or extend it first.
    ///
    /// # Errors
    ///
    /// Returns a typed refusal for a retained manual state or a fresh custody
    /// or journal refusal. Every byte is retained on refusal.
    pub fn recover_ids(&self) -> Result<Option<IdsPublication>, IdsPublicationError> {
        protocol::recover(self)
    }

    /// Compare `plan` against the filesystem and install it.
    ///
    /// # Errors
    ///
    /// Returns a typed refusal when nothing was durably claimed. Once the
    /// marker is durable, an interruption is reported as
    /// [`IdsPublishOutcome::Pending`] instead.
    pub fn publish_ids(
        &self,
        plan: LedgerPublicationPlan,
    ) -> Result<IdsPublishOutcome<'_>, IdsPublicationError> {
        protocol::publish(self, plan)
    }

    fn meta(&self) -> &AdmittedDir {
        &self.meta
    }

    fn ledger_name(&self) -> &EntryName {
        &self.ledger
    }

    fn stage_name(&self) -> &EntryName {
        &self.stage
    }

    fn journal_names(&self) -> &PendingName {
        &self.journal
    }
}

/// Admit one of this module's fixed entry names.
fn admitted_name(name: &str) -> EntryName {
    EntryName::admit(name).expect("a fixed publication entry name is an admitted spelling")
}

/// Probe the project's publication marker without opening, creating, or
/// mutating anything.
///
/// Every read-only front door calls this before it reads the ledger: while a
/// marker exists the committed `.marrow/ids` is indeterminate, so capture
/// refuses rather than reading a generation that recovery may replace. The
/// probe fails closed — an entry whose existence cannot be determined counts as
/// present.
pub fn ids_publication_marker(root: &Path) -> Option<IdsPublicationMarker> {
    marker::probe(root)
}
