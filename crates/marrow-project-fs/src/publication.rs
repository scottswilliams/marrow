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
//! Inside the project's `.marrow` directory the protocol uses exactly six
//! fixed entry names and enumerates the directory nowhere:
//!
//! ```text
//! ids                    the committed identity ledger
//! ids.publish.stage      the successor before it is installed
//! ids.pending            the durable publication marker
//! ids.pending.create     the marker's pre-claim alias
//! publish.lock           the cooperative project-metadata write lock
//! .gitignore             keeps the other four out of version control
//! ```
//!
//! The directory and the ledger's entry name are [`marrow_project::META_DIR`]
//! and [`marrow_project::IDS_ENTRY`]; the four derived names are built from the
//! latter here, so no spelling of the ledger's location exists twice.
//! `ids` is the one committed artifact. The lock is machine-local runtime
//! state, and the three transient names are a publication in flight or the
//! debris an interrupted one left; the write owner writes the ignore entry
//! naming all four, so no project carries a hand-written line and no checkout
//! carries an entry that would make a fresh clone read a ledger this protocol
//! calls indeterminate.
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
//! Settled installed:  the Installing map's installed reading, or target=next
//!                     nlink 1 with the stage absent after the exact cleanup
//! Settled reverted:   the Installing map's reverted reading, or the artifact
//!                     untouched with the stage absent after the exact cleanup
//! ```
//!
//! The reverted terminal is what a destination refusal or a continuously proven
//! third live inode settles into: the successor is not installed, the artifact
//! keeps whatever the concurrent writer left, and the outcome is
//! [`IdsPublication::ConcurrentChange`]. It is a recorded terminal rather than
//! an abandoned journal because the frame's only exit is its terminal phase.
//! Reaching it takes a writer the guard does not exclude, which is a writer
//! that took no lock: `.marrow/ids` is committed, so an ordinary Git operation
//! creates or replaces it. The stage name is covered by the ignore entry this
//! owner writes, so an ordinary Git operation neither tracks nor recreates it —
//! except in a repository that tracks the name anyway, which a force-add or a
//! commit predating that coverage leaves. A writer otherwise reaches the same
//! readings there through an edit outside the lock.
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
use marrow_project::{IDS_ENTRY, LedgerPublicationPlan, MAX_IDS_BYTES, META_DIR};

use header::HeaderCorruption;

pub use marker::IdsPublicationMarker;

/// The suffix the fixed stage entry adds to the ledger's entry name. The
/// directory and ledger spellings themselves belong to [`marrow_project`]; this
/// adapter derives its publication names from that owner rather than repeating
/// either one.
const STAGE_SUFFIX: &str = ".publish.stage";
/// The cooperative project-metadata write lock's entry name.
const LOCK_NAME: &str = "publish.lock";
/// The version-control ignore entry the write owner keeps beside the entries
/// it names.
const IGNORE_NAME: &str = ".gitignore";
/// The comment the written ignore block carries above the entry names.
const IGNORE_COMMENT: &str = "\
# Machine-written by Marrow. The cooperative write lock is machine-local runtime
# state, and the other entries are a publication in flight or the debris an
# interrupted one left. No checkout carries any of them; only `ids` is committed.
";
/// The opening every comment this owner has written begins with, and the whole
/// of what tells an entry this owner wrote from a developer's own file.
///
/// The comment's remaining words describe the name set it was written above, so
/// they change when that set does and a completed entry would stop matching its
/// own header. This prefix does not, so it stays the mark: an entry that
/// carries it gains only the names it lacks, and one that does not carries the
/// comment in full above them.
const IGNORE_COMMENT_MARK: &str = "# Machine-written by Marrow.";
/// How much of an existing ignore entry is read to decide whether it already
/// names every entry this owner keeps untracked. A file this owner wrote is
/// seven lines; anything past this bound belongs to whoever wrote it and is
/// left exactly as found.
const IGNORE_READ_CEILING: usize = 4096;

/// The fixed stage entry's spelling. The frozen row header embeds it and the
/// guard admits it, and both take it from here so the ledger's entry name has
/// one owner across the pure/adapter boundary.
pub(crate) fn stage_spelling() -> String {
    format!("{IDS_ENTRY}{STAGE_SUFFIX}")
}
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
/// directory rather than opening a second write owner. The admitted directory
/// and the lock are shared as they stand; the three kind-1 entry names are
/// fields, so a second kind extends this struct rather than instantiating a
/// second one per kind.
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
        install_untracked_ignore(&meta)?;
        let ledger = admitted_name(IDS_ENTRY);
        Ok(Self {
            journal: PendingName::derive(&ledger)
                .expect("the fixed journal names are admitted spellings"),
            stage: admitted_name(&stage_spelling()),
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

/// Create the metadata directory and admit it, or admit the one a concurrent
/// first publication created.
///
/// The directory is the shared rendezvous rather than one process's property, so
/// an occupied destination is re-admitted instead of refused: exclusion belongs
/// to the write lock inside it, and a loser that refused here would report a
/// filesystem refusal for what is an ordinary contended first publication.
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

/// Keep every entry no checkout may carry out of version control from the owner
/// that creates them, so a project carries no hand-written ignore line and a
/// fresh clone is correct without one.
///
/// The entry is completed rather than rewritten: a name is appended only when
/// the file does not already carry it, so a second acquisition writes nothing,
/// whatever a developer added survives, an entry that predates a name gains
/// exactly that name and nothing else, and the empty file a crash between the
/// create and the fill leaves is finished by the next acquisition. An entry
/// this owner wrote under an earlier name set is completed under the comment it
/// already carries, so no entry ends up with a second comment standing over a
/// stale first block. It runs under the write lock, so one process at a time is
/// inside it and two first publications cannot both append.
///
/// The block is a convenience the owner maintains when it can, so an entry it
/// cannot write is left exactly as found — like one past the read bound — and
/// no publication or recovery is refused over it. That reading covers a
/// withheld write and nothing else: every other custody refusal here is a
/// metadata directory this owner did not produce, and stays a typed refusal.
fn install_untracked_ignore(meta: &AdmittedDir) -> Result<(), IdsPublicationError> {
    let name = admitted_name(IGNORE_NAME);
    let (created, found) = match meta.create_file_excl(&name) {
        Ok(created) => (Some(created), Vec::new()),
        Err(CustodyError::AlreadyExists { .. }) => {
            // Whether the entry is already complete is a read-only question, so
            // it is asked read-only: a checkout may carry the entry unwritable,
            // and an open that demanded write to decide it would refuse every
            // publication and recovery of a project that needs no append.
            //
            // A mode that withholds even that read leaves the entry as found
            // for the same reason a withheld write does: the read exists only
            // to decide a cosmetic append, so refusing here would refuse every
            // publication and recovery of a project over a file this owner
            // merely wanted to tidy.
            match meta.open_file_readonly(&name) {
                Ok(opened) => (None, opened.read_prefix(IGNORE_READ_CEILING + 1)?),
                Err(error) if access_withheld(&error) => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    };
    // A file larger than the read bound is not this owner's, and the part of it
    // that decides the question was never read, so it is left as found. The
    // same check bounds an entry this owner's own appends pushed past the bound:
    // past it no acquisition can see the names it already wrote, so without
    // stopping here every acquisition would append them again forever.
    if found.len() > IGNORE_READ_CEILING {
        return Ok(());
    }
    let missing: Vec<String> = untracked_entry_names()
        .into_iter()
        .filter(|entry| !ignore_names_entry(&found, entry))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut block = String::new();
    if found.last().is_some_and(|byte| *byte != b'\n') {
        block.push('\n');
    }
    // An entry that already carries this owner's comment gains only the names,
    // so an entry written above an earlier name set is completed under the
    // header it has. A second copy of the comment would leave the entry with
    // two of them, the first standing over a name set the file no longer has.
    if !ignore_carries_comment(&found) {
        block.push_str(IGNORE_COMMENT);
    }
    for entry in missing {
        block.push_str(&entry);
        block.push('\n');
    }
    let mut entry = match created {
        Some(created) => created,
        // An entry this process may not write is left as found for the same
        // reason one past the read bound is: the append is a convenience, not
        // a step any durable state depends on, and refusing here would refuse
        // every publication and every recovery of a project whose ignore entry
        // a checkout carries read-only. Only a withheld write reads that way —
        // a node kind this owner never wrote is a corrupted metadata
        // directory, and every other custody refusal stays one.
        None => match meta.open_file(&name) {
            Ok(opened) => opened,
            Err(error) if access_withheld(&error) => return Ok(()),
            Err(error) => return Err(error.into()),
        },
    };
    entry.append(block.as_bytes())?;
    entry.sync()?;
    meta.sync()?;
    Ok(())
}

/// Whether a refused open says this process may not reach the entry's bytes,
/// rather than that the entry is not one this owner can maintain at all. Both
/// of the ignore entry's opens read their refusals through here: the mode that
/// withholds the deciding read and the mode that withholds the append are the
/// same permission-class condition on the same cosmetic file.
///
/// The custody owner reads a permission refusal over a regular file whose owner
/// bits fall short as [`CustodyError::ModeDenied`]; a permission refusal it
/// could not attribute to those bits — another user's entry, a restrictive
/// security policy — arrives unclassified and is the same withheld access from
/// this caller's side. An environmental write failure is not in this family: a
/// read-only mount refuses the lock open long before the ignore entry, and a
/// full or read-only filesystem carries its own error kind and stays a refusal.
fn access_withheld(error: &CustodyError) -> bool {
    match error {
        CustodyError::ModeDenied { .. } => true,
        CustodyError::Io { source, .. } => source.kind() == std::io::ErrorKind::PermissionDenied,
        _ => false,
    }
}

/// Every `.marrow` entry this protocol can leave that no checkout may carry:
/// the machine-local write lock, the successor stage, and the journal owner's
/// two marker names. Each is derived from the same constant the protocol
/// mutates through, so a renamed or added transient reaches the ignore entry
/// with it rather than through a second hand-kept list.
fn untracked_entry_names() -> Vec<String> {
    let ledger = admitted_name(IDS_ENTRY);
    let journal =
        PendingName::derive(&ledger).expect("the fixed journal names are admitted spellings");
    vec![
        LOCK_NAME.to_owned(),
        stage_spelling(),
        journal.pending().as_str().to_owned(),
        journal.claim().as_str().to_owned(),
    ]
}

/// The bytes read from the ignore entry as lines, each without the trailing
/// carriage return a CRLF checkout leaves.
fn ignore_lines(found: &[u8]) -> impl Iterator<Item = &[u8]> {
    found
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
}

/// Whether the bytes read from the ignore entry already name `entry`.
///
/// A line matches without the optional leading `/` that anchors a pattern to
/// the ignore file's own directory, as well as without the carriage return.
/// Every such spelling names exactly what this owner would append, and a
/// semantic duplicate is the one thing an entry shared with a developer must
/// not accumulate. The form this owner writes stays the bare name.
fn ignore_names_entry(found: &[u8], entry: &str) -> bool {
    ignore_lines(found).any(|line| line.strip_prefix(b"/").unwrap_or(line) == entry.as_bytes())
}

/// Whether the bytes read from the ignore entry already carry this owner's
/// comment, in any wording it has been written with.
fn ignore_carries_comment(found: &[u8]) -> bool {
    ignore_lines(found).any(|line| line.starts_with(IGNORE_COMMENT_MARK.as_bytes()))
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
