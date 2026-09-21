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
//! Inside the project's `.marrow` directory the protocol uses exactly seven
//! fixed entry names and enumerates the directory nowhere:
//!
//! ```text
//! ids                      the committed identity ledger
//! ids.publish.stage        the successor before it is installed
//! ids.publish.quarantine   holds one object while a removal judges it
//! ids.pending              the durable publication marker
//! ids.pending.create       the marker's pre-claim alias
//! publish.lock             the cooperative project-metadata write lock
//! .gitignore               keeps the other five out of version control
//! ```
//!
//! The directory and the ledger's entry name are [`marrow_project::META_DIR`]
//! and [`marrow_project::IDS_ENTRY`]; the five derived names are built from the
//! latter here, so no spelling of the ledger's location exists twice.
//! `ids` is the one committed artifact. The lock is machine-local runtime
//! state, and the four transient names are a publication in flight or the
//! debris an interrupted one left; the write owner writes the ignore entry
//! naming all five, so no checkout carries an entry that would make a fresh
//! clone read a ledger this protocol calls indeterminate.
//!
//! # Which writers the contract admits
//!
//! Ordinary Git operations are safe because they write tracked paths, and every
//! name here except `ids` is an untracked protocol transient. Outside the
//! contract are a writer holding a descriptor opened on a transient before
//! publication began (a descriptor survives every rename and no process can
//! revoke another's) and one deliberately writing the untracked names. Against
//! either, the interval between validating an object and unlinking the name that
//! held it is irreducible on POSIX — `unlinkat` names a path, and neither
//! qualified platform offers an unlink through a descriptor.
//! [`marrow_fs_journal::FsIdentity`] states the resulting bound.
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
//! Reverted:           stage=next nlink 1; target present and neither the
//!                     successor nor the generation the header binds
//! Phase Prepared:     the Prepared map, or the reverted map
//! Phase Installing:   either of those, or target=stage=next     nlink 2
//!                     (absent arm) or target=next; stage=base (replace arm)
//! Settled installed:  the installed reading, or target=next nlink 1 with the
//!                     stage absent after the exact cleanup
//! Settled reverted:   the reverted reading, or the artifact untouched with
//!                     the stage absent after the exact cleanup
//! ```
//!
//! Both `Prepared` and `Installing` can be read before any artifact mutation
//! has run, so both admit the reverted reading and classify the outside writer
//! that produced it the same way. `Installing` admits one reading more, because
//! the mutation between them can leave the successor installed.
//!
//! The reverted terminal is what the reverted reading, a destination refusal, or
//! a continuously proven third live inode settles into: the successor is not
//! installed, the artifact keeps whatever the concurrent writer left, and the
//! outcome is [`IdsPublication::ConcurrentChange`]. Reaching it takes a writer
//! the guard does not exclude.
//!
//! Which terminal a mutation reached is read back from the map rather than
//! decided from the mutation's own outcome, so the driver, the mutations, and
//! the crash-tail derivation cannot disagree about what was installed.
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
mod ignore;
mod marker;
mod protocol;

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use marrow_codes::Code;
use marrow_fs_journal::{
    AdmittedDir, CacheLock, CorruptionReason, CustodyError, EntryName, JournalError, LockError,
    PendingName, qualified_platform,
};
use marrow_project::{IDS_ENTRY, LedgerPublicationPlan, MAX_IDS_BYTES, META_DIR};

use header::HeaderCorruption;

pub(crate) use marker::IdsPublicationMarker;

/// The suffix the fixed stage entry adds to the ledger's entry name. The
/// directory and ledger spellings themselves belong to [`marrow_project`]; this
/// adapter derives its publication names from that owner rather than repeating
/// either one.
const STAGE_SUFFIX: &str = ".publish.stage";
/// The suffix the cleanup quarantine adds to the ledger's entry name. A removal
/// moves the object it is judging here before it opens it, so the object is
/// judged and unlinked under a name no cooperating writer ever touches rather
/// than under the name one may be writing.
const QUARANTINE_SUFFIX: &str = ".publish.quarantine";
/// The cooperative project-metadata write lock's entry name.
const LOCK_NAME: &str = "publish.lock";
/// The fixed stage entry's spelling, joined once per process from the pure
/// owner's ledger-entry constant so the source carries one spelling of it. The
/// encoded form in a durable header is frozen: renaming that constant decodes
/// every header already on disk as a `StageNameDrift` corruption.
pub(crate) fn stage_spelling() -> &'static str {
    static STAGE: OnceLock<String> = OnceLock::new();
    STAGE.get_or_init(|| format!("{IDS_ENTRY}{STAGE_SUFFIX}"))
}

/// The cleanup quarantine's entry name, derived from the same owner constant as
/// every other publication name. No durable header encodes it: it holds an
/// object only for the length of one removal, and a leftover is reconciled by
/// the next command.
pub(crate) fn quarantine_spelling() -> &'static str {
    static QUARANTINE: OnceLock<String> = OnceLock::new();
    QUARANTINE.get_or_init(|| format!("{IDS_ENTRY}{QUARANTINE_SUFFIX}"))
}
/// The fixed bound on either byte run the header carries.
const LEDGER_BYTE_CEILING: usize = MAX_IDS_BYTES;

/// Whether this process dropped an unrecovered publication, after which it
/// publishes nothing further. The dropped claim usually leaves a marker that
/// keeps gating capture until a fresh process recovers it; the one arm that
/// leaves no marker abandons the answer instead, since the publication happened
/// and no later command can say which one it was.
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
/// from parts, it retains the guard borrow and the cause that interrupted it,
/// and the only way to advance it is to consume [`recover`](Self::recover).
/// Dropping it instead quarantines publication in this process until exit.
///
/// It does not always retain a live journal or name a marker that still exists:
/// an interrupted publication resumes through its journal, a claim that never
/// received one adopts whatever marker is on disk, and a finish that refused
/// after its own unlink reports the terminal it had already recorded. Every arm
/// is a publication this process claimed and did not conclude.
#[must_use = "a durably claimed publication advances only by consuming `recover`"]
pub struct IdsPublicationPending<'a> {
    work: PendingWork<'a>,
    pub(crate) cause: IdsPublicationError,
    armed: bool,
}

/// What advancing a pending publication has to work with.
///
/// A publication interrupted with its journal intact resumes through it; a
/// claim that refused at or after its first link attempt was handed none, so
/// its only way forward is the recovery that adopts a durable marker from disk.
enum PendingWork<'a> {
    // Boxed: a live session is far larger than a guard reference, and this
    // value is already behind one indirection in the outcome it travels in.
    Session(Box<protocol::Session<'a>>),
    /// No live journal to resume through. `recovered` is what the interrupted
    /// publication had already recorded, when it had recorded anything: a
    /// finish removes the marker before its closing checks, so recovery can
    /// find nothing to adopt and still owe an answer.
    Marker {
        guard: &'a ProjectMetadataWriteGuard,
        recorded: Option<IdsPublication>,
    },
}

impl<'a> IdsPublicationPending<'a> {
    fn new(session: protocol::Session<'a>, cause: IdsPublicationError) -> Self {
        Self {
            work: PendingWork::Session(Box::new(session)),
            cause,
            armed: true,
        }
    }

    /// The pending value for a publication with no live journal to resume
    /// through. `recorded` carries the terminal such a session had reached,
    /// which is the only thing that distinguishes a finished publication from
    /// one that was never claimed once the marker is gone.
    fn unclaimed(
        guard: &'a ProjectMetadataWriteGuard,
        cause: IdsPublicationError,
        recorded: Option<IdsPublication>,
    ) -> Self {
        Self {
            work: PendingWork::Marker { guard, recorded },
            cause,
            armed: true,
        }
    }

    /// Consume the pending publication and drive it to its terminal state.
    ///
    /// # Errors
    ///
    /// Returns the fresh refusal when the publication still cannot settle, and
    /// quarantines publication in this process exactly as a drop does: the
    /// retained handles go either way, so a fresh process is what settles the
    /// project next.
    ///
    /// A refusing recovery is not a no-op: it reconciles first, putting back an
    /// entry an interrupted removal had moved aside, and it may finish a removal
    /// the durable record already authorized before a later step refuses. It
    /// leaves a settleable state, not the state it found.
    pub fn recover(mut self) -> Result<IdsPublication, IdsPublicationError> {
        // The refusal that produced this value can be a cleanup that had already
        // moved its object into quarantine, so this retry reconciles exactly as
        // any other entry does before reclassifying.
        let settled = match &mut self.work {
            PendingWork::Session(session) => session
                .reconcile()
                .and_then(|reconciled| session.drive(&reconciled)),
            PendingWork::Marker { guard, recorded } => {
                protocol::recover(guard).and_then(|settled| match (settled, *recorded) {
                    // A marker was there to adopt, and adopting it settled it.
                    (Some(settled), _) => Ok(settled),
                    // No marker, but this publication had already recorded its
                    // terminal — the finish that refused had removed the marker
                    // first, so the publication is what that terminal says.
                    (None, Some(recorded)) => Ok(recorded),
                    // No marker and nothing recorded: nothing was ever settled.
                    (None, None) => Err(IdsPublicationError::bare(IdsRefusal::UnclaimedIncomplete)),
                })
            }
        };
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
    /// The publication was claimed and did not settle. The retained guard
    /// borrow and cause are boxed so an ordinary settled publication does not
    /// carry them by value; whether a live journal or a marker is retained with
    /// them depends on where the interruption fell, and
    /// [`IdsPublicationPending`] says which arms exist.
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

/// The closed classification of a publication or recovery refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdsRefusal {
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
    /// not a state this protocol can have produced.
    ///
    /// No artifact byte is replaced and no cooperating writer's distinguishable
    /// content is lost, but the state is settleable rather than untouched: see
    /// [`IdsPublicationPending::recover`].
    Corrupt,
    /// A publication is durably claimed; recovery must settle it first.
    Interrupted,
    /// A filesystem operation refused.
    Custody,
    /// The pending journal refused.
    Journal,
    /// The entry that keeps this project's publication transients untracked
    /// could not be established, so the contract every removal in this
    /// protocol rests on does not hold here. Nothing was staged or claimed.
    UntrackedContract,
}

impl IdsRefusal {
    /// The stable outward code this refusal reports.
    pub(crate) const fn code(self) -> Code {
        match self {
            Self::UnclaimedIncomplete | Self::Corrupt | Self::Interrupted | Self::Quarantined => {
                Code::ProjectIdsPublicationPending
            }
            Self::UnqualifiedPlatform
            | Self::Contended
            | Self::Custody
            | Self::Journal
            | Self::UntrackedContract => Code::IoWrite,
        }
    }
}

/// Why an identity publication or its recovery could not proceed. A consumer
/// observes the stable code and the `Display` message; the classification and
/// the evidence behind it are crate-private.
#[derive(Debug)]
pub struct IdsPublicationError {
    pub(crate) refusal: IdsRefusal,
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
                 the committed ledger is unchanged and no cooperating writer's \
                 distinguishable content was removed. Recovery will not go further on \
                 its own",
            )?,
            IdsRefusal::Interrupted => formatter.write_str(
                "a `.marrow/ids` publication is durably claimed and must be recovered first",
            )?,
            IdsRefusal::UntrackedContract => formatter.write_str(
                "`.marrow/.gitignore` cannot be read or is past the size this owner reads, so \
                 whether it keeps the publication transients untracked is unknown. Every \
                 removal this protocol performs relies on those entries never being tracked: \
                 a committed transient is recreated by every checkout, and a checkout writing \
                 one during a publication can lose it. Make the entry readable and small \
                 enough to inspect, or remove it so the owner can write its own",
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
/// which is what serializes the protocol's identity witnesses. Dropping the
/// guard releases the lock. A further kind of metadata write takes this same
/// lock and admitted directory rather than opening a second write owner.
#[derive(Debug)]
pub struct ProjectMetadataWriteGuard {
    meta: AdmittedDir,
    ledger: EntryName,
    stage: EntryName,
    quarantine: EntryName,
    journal: PendingName,
    _lock: CacheLock,
}

impl ProjectMetadataWriteGuard {
    /// Acquire the exclusive write owner for the project rooted at `root`,
    /// creating `.marrow`, the lock entry, and the ignore entry that keeps the
    /// lock out of version control when they are absent.
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
            Err(CustodyError::NotFound { .. }) => admit_created_meta(&root_dir, &meta_name)?,
            Err(error) => return Err(error.into()),
        };
        let lock = CacheLock::acquire(&meta, &admitted_name(LOCK_NAME))?;
        ignore::install_untracked_ignore(&meta)?;
        let ledger = admitted_name(IDS_ENTRY);
        Ok(Self {
            journal: PendingName::derive(&ledger)
                .expect("the fixed journal names are admitted spellings"),
            stage: admitted_name(stage_spelling()),
            quarantine: admitted_name(quarantine_spelling()),
            ledger,
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
    /// or journal refusal.
    ///
    /// A refusal retains every byte the protocol was not already committed to
    /// removing, and replaces no artifact byte.
    pub fn recover_ids(&mut self) -> Result<Option<IdsPublication>, IdsPublicationError> {
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
        &mut self,
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

    fn quarantine_name(&self) -> &EntryName {
        &self.quarantine
    }

    fn journal_names(&self) -> &PendingName {
        &self.journal
    }
}

/// Create the metadata directory and admit it, or admit the one a concurrent
/// first publication created.
///
/// The directory is the shared rendezvous rather than one process's property, so
/// an occupied destination is re-admitted instead of refused; exclusion belongs
/// to the write lock inside it.
fn admit_created_meta(
    root: &AdmittedDir,
    name: &EntryName,
) -> Result<AdmittedDir, IdsPublicationError> {
    let admitted = match root.create_child_dir(name) {
        Ok(created) => created,
        Err(CustodyError::AlreadyExists { .. }) => root.admit_child(name)?,
        Err(error) => return Err(error.into()),
    };
    // The metadata directory's own entry must be durable before any entry
    // inside it can be, whichever process created it: the winner of the race
    // may not have synced yet, and this process is about to write inside.
    root.sync()?;
    Ok(admitted)
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
pub(crate) fn ids_publication_marker(root: &Path) -> Option<IdsPublicationMarker> {
    marker::probe(root)
}

/// Whether the project rooted at `root` carries a live publication marker, so a
/// command knows to recover before it captures. The same fail-closed probe.
pub fn ids_publication_pending(root: &Path) -> bool {
    ids_publication_marker(root).is_some()
}
