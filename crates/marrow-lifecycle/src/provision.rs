//! The persistent provision and open flow.
//!
//! Provision builds the whole store in a
//! private sibling temporary directory and atomically renames it into place. A rename onto
//! any existing destination fails, so exactly one provisioner wins a race and a
//! crash before the rename leaves only a temporary directory — the destination is never a
//! partially-formed store. Failed parent-directory sync after rename reports publication
//! uncertainty and retains the destination; namespace completeness does not confirm durability.
//! Preflight is strictly
//! non-creating, so probing a destination never leaves a file behind.
//!
//! Open takes the single-owner lock first (naming the live owner on contention), and only
//! then reads the store directory at all: completeness, the envelope, and the head are one
//! admission snapshot taken under that owner, each artifact admitted from the retained
//! directory within its own byte ceiling. Deciding exclusion ahead of every read is what
//! keeps a contender's verdict independent of the holder's bytes — a malformed, truncated,
//! or deleted artifact cannot turn "the store is locked" into a decode or completeness
//! error. The engine opens last, through the path kernel. A read-write open after an
//! unclean shutdown (a stale owner descriptor in the lock) runs a full integrity audit.
//!
//! The unclean-open audit covers crash-path corruption only: the fast open path does not
//! re-verify page checksums, so an externally flipped bit in a cleanly-closed store is not
//! detected at open. The read-only store audit (`crate::audit`) checks logical contents;
//! it performs no repairing integrity call and preserves an inherited unclean obligation.

use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_kernel::durable::{
    AuditReport, CommitRecovery, ContentDigest, DemandCoverage, DurableCommitState,
    InvocationGrant, NativeOpenAccess, NativeOwnerAcquireError, NativeOwnerOpenError, NativeStore,
    ReadSession, SessionError, SessionHost, StoreError, StoreProjection, TxnSession,
};

use crate::durable_fs::Publication;
use crate::envelope::StoreEnvelope;
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::head::LogicalHead;
use crate::instance::StoreInstanceId;
use crate::lock::LockError;
use crate::store_dir::{
    self, AdmissionError, AdmittedStoreDir, Artifact, StoreAccessError, StoreEntry,
};

/// A non-creating classification of a store directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preflight {
    /// No store directory exists at the path.
    Absent,
    /// The directory exists but is missing at least one durable artifact — a partially
    /// formed store, never published as complete (a leftover of an interrupted build).
    Incomplete,
    /// The directory exists with all durable artifacts present.
    Complete,
}

/// Classify the store at `dir` without creating or modifying anything. Reads only: it stats
/// the directory and its artifacts. A missing directory is [`Preflight::Absent`]; a directory
/// missing any of the engine, envelope, or head is [`Preflight::Incomplete`]; a directory
/// with all three is [`Preflight::Complete`].
///
/// A directory this process cannot examine is none of the three. Its classification is
/// unknown, and that is returned as a [`StoreAccessError`] rather than resolved by assuming
/// what could not be seen is not there — which would report an intact store as partially
/// formed.
pub fn preflight(dir: &Path) -> Result<Preflight, StoreAccessError> {
    match std::fs::metadata(dir) {
        Ok(found) if found.is_dir() => {}
        Ok(_) => return Ok(Preflight::Absent),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Preflight::Absent);
        }
        Err(error) => return Err(StoreAccessError::at(dir, error)),
    }
    if store_dir::artifacts_present(dir)? {
        Ok(Preflight::Complete)
    } else {
        Ok(Preflight::Incomplete)
    }
}

/// The inputs to a provision: the persisted envelope and logical head to publish. The
/// caller (the lifecycle actor) derives these from a verified image; provision creates an
/// empty engine (no user data), so no store shape is needed to create it.
pub struct ProvisionRequest {
    pub envelope: StoreEnvelope,
    pub head: LogicalHead,
}

/// The outcome of a successful provision: the store instance now published at the
/// destination. The store is left closed; the caller opens it to drive sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provisioned {
    pub instance: StoreInstanceId,
}

/// Why a provision failed.
#[derive(Debug)]
pub enum ProvisionFault {
    /// A complete or partially-formed store already occupies the destination: the caller lost
    /// the one-winner claim, or a prior provision is present. The destination is untouched.
    AlreadyProvisioned,
    /// The store directory this provision built could not be admitted, so a store published
    /// here could never be opened again. The destination is untouched.
    Admission(AdmissionError),
    /// The ordered-byte engine could not be created.
    Store(StoreError),
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// The complete store was published, but parent-directory durability is unconfirmed.
    PublicationUncertain {
        instance: StoreInstanceId,
        source: std::io::Error,
    },
    /// Publication passed its parent barrier, but Active completion was not acknowledged.
    ActivationUncertain {
        instance: StoreInstanceId,
        source: AdmissionError,
    },
}

/// Failed provision and independent cleanup evidence for its unpublished stage.
#[derive(Debug)]
pub struct ProvisionError {
    pub fault: ProvisionFault,
    pub cleanup: Option<ProvisionCleanupFailure>,
}

/// Cleanup of an unpublished stage failed or its changed identity prevented safe
/// removal. An attempted removal may have been partial.
#[derive(Debug)]
pub struct ProvisionCleanupFailure {
    pub stage: PathBuf,
    pub source: std::io::Error,
}

impl From<ProvisionFault> for ProvisionError {
    fn from(fault: ProvisionFault) -> Self {
        Self {
            fault,
            cleanup: None,
        }
    }
}

impl ProvisionError {
    /// The primary failure's code, independent of cleanup.
    pub fn code(&self) -> &'static str {
        self.fault.code()
    }
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fault.fmt(f)?;
        if let Some(cleanup) = &self.cleanup {
            write!(
                f,
                "; cleanup failed for unpublished stage {}: {}",
                cleanup.stage.display(),
                cleanup.source
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for ProvisionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.fault)
    }
}

impl ProvisionFault {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            ProvisionFault::AlreadyProvisioned => Code::StoreLocked.as_str(),
            ProvisionFault::Admission(error) => error.code(),
            ProvisionFault::Store(error) => error.code(),
            ProvisionFault::Io(_) => Code::StoreIo.as_str(),
            ProvisionFault::PublicationUncertain { .. } => Code::StorePublicationUncertain.as_str(),
            ProvisionFault::ActivationUncertain { .. } => Code::StoreActivationUncertain.as_str(),
        }
    }
}

impl std::fmt::Display for ProvisionFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionFault::AlreadyProvisioned => {
                write!(f, "a store already exists at the destination")
            }
            ProvisionFault::Admission(error) => write!(f, "{error}"),
            ProvisionFault::Store(error) => {
                write!(f, "the store engine could not be created: {error}")
            }
            ProvisionFault::Io(error) => write!(f, "provisioning failed: {error}"),
            ProvisionFault::PublicationUncertain { instance, source } => write!(
                f,
                "store {} was published, but directory durability is unconfirmed: {source}",
                instance.to_hex()
            ),
            ProvisionFault::ActivationUncertain { instance, source } => write!(
                f,
                "store {} was published, but activation completion is unconfirmed: {source}",
                instance.to_hex()
            ),
        }
    }
}

impl std::error::Error for ProvisionFault {}

/// Provision a fresh store at `dest` in a private sibling directory (mode `0700`).
/// Write and flush the envelope and head, create the engine through the path kernel,
/// sync the completed stage, then atomically rename it onto `dest`. A rename onto any existing
/// destination fails, so exactly one racing provisioner wins and the destination is never
/// left partial. A creation failure leaves an existing temporary path untouched. After
/// successful creation, a failure before rename attempts to remove this invocation's
/// temporary directory. Cleanup failure can leave that stage behind.
/// A parent-directory sync failure after rename retains `dest` and returns its instance
/// identity in `PublicationUncertain`; it does not confirm publication durability.
pub fn provision(dest: &Path, request: ProvisionRequest) -> Result<Provisioned, ProvisionError> {
    check_engine_stamp(&request.envelope).map_err(ProvisionFault::Store)?;
    let instance = request.envelope.instance;
    let temp = temp_sibling(dest);
    let publication = Publication::admit(&temp, dest).map_err(ProvisionFault::Io)?;
    create_private_dir(&temp).map_err(ProvisionFault::Io)?;
    // Build the store before publication; cleanup after a failed build is best-effort.
    #[cfg(test)]
    let built = tests::construct(dest, &temp, || build_in_temp(&temp, &request));
    #[cfg(not(test))]
    let built = build_in_temp(&temp, &request);
    let (owner, admitted) = match built {
        Ok(owner) => owner,
        Err(error) => {
            return Err(cleanup_after_failure(&temp, error));
        }
    };

    #[cfg(all(test, unix))]
    tests::observe_publication(
        tests::PublicationPoint::Staged,
        dest,
        &temp,
        &owner,
        &admitted,
    );

    // The retained parent performs the no-replace claim. Any occupied destination
    // refuses; a loser cleans only its unpublished stage.
    match publication.publish(admitted.identity()) {
        Ok(()) => {}
        Err(error) => {
            let fault = match error {
                marrow_fs_journal::CustodyError::AlreadyExists { .. } => {
                    ProvisionFault::AlreadyProvisioned
                }
                error => ProvisionFault::Io(crate::durable_fs::custody_io(error)),
            };
            if let Err(error) = admitted.verify_location(&temp) {
                return Err(ProvisionError {
                    fault,
                    cleanup: Some(ProvisionCleanupFailure {
                        stage: temp,
                        source: std::io::Error::other(error),
                    }),
                });
            }
            drop(admitted);
            drop(owner);
            return Err(cleanup_after_failure(&temp, fault));
        }
    }
    #[cfg(all(test, unix))]
    tests::observe_publication(
        tests::PublicationPoint::Renamed,
        dest,
        dest,
        &owner,
        &admitted,
    );
    complete_publication(dest, &publication, &admitted, request.envelope)?;
    drop(owner);
    Ok(Provisioned { instance })
}

/// Confirm the published name and parent barrier, then complete Active under
/// the retained store descriptor. The caller keeps its native owner alive.
pub(crate) fn complete_publication(
    dest: &Path,
    publication: &Publication,
    admitted: &AdmittedStoreDir,
    envelope: StoreEnvelope,
) -> Result<(), ProvisionFault> {
    let instance = envelope.instance;
    admitted
        .verify_location(dest)
        .map_err(|source| ProvisionFault::PublicationUncertain {
            instance,
            source: std::io::Error::other(source),
        })?;
    // Make the new directory entry durable in the parent.
    #[cfg(test)]
    publication_sync_fault::check(dest, publication_sync_fault::Point::Publication)
        .map_err(|source| ProvisionFault::PublicationUncertain { instance, source })?;
    publication
        .sync()
        .map_err(|source| ProvisionFault::PublicationUncertain { instance, source })?;
    let active = EnvelopeRecord {
        metadata: envelope,
        state: EnvelopeState::Active,
    }
    .encode()
    .map_err(|error| {
        ProvisionFault::Admission(AdmissionError::format(StoreEntry::Envelope, error))
    })?;
    admitted
        .replace(Artifact::Envelope, &active)
        .map_err(|source| ProvisionFault::ActivationUncertain { instance, source })?;
    #[cfg(test)]
    publication_sync_fault::check(dest, publication_sync_fault::Point::Activation).map_err(
        |source| ProvisionFault::ActivationUncertain {
            instance,
            source: AdmissionError {
                entry: StoreEntry::Directory,
                fault: store_dir::AdmissionFault::Custody(marrow_fs_journal::CustodyError::Io {
                    op: "activation directory sync",
                    source,
                }),
            },
        },
    )?;
    admitted
        .sync()
        .map_err(|source| ProvisionFault::ActivationUncertain { instance, source })?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod publication_sync_fault {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Point {
        Publication,
        Activation,
    }

    thread_local! {
        static DESTINATION: RefCell<Option<(PathBuf, Point)>> = const { RefCell::new(None) };
    }

    pub(crate) fn with_failure<T>(
        destination: &Path,
        point: Point,
        action: impl FnOnce() -> T,
    ) -> T {
        struct Restore(Option<(PathBuf, Point)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                DESTINATION.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        let previous = DESTINATION.with(|slot| slot.replace(Some((destination.to_owned(), point))));
        let _restore = Restore(previous);
        action()
    }

    pub(super) fn check(destination: &Path, point: Point) -> std::io::Result<()> {
        let fail = DESTINATION.with(|slot| {
            let mut armed = slot.borrow_mut();
            if armed
                .as_ref()
                .is_some_and(|(path, at)| path == destination && *at == point)
            {
                armed.take();
                true
            } else {
                false
            }
        });
        if fail {
            Err(std::io::Error::from(std::io::ErrorKind::Other))
        } else {
            Ok(())
        }
    }
}

fn cleanup_after_failure(stage: &Path, fault: ProvisionFault) -> ProvisionError {
    let cleanup = remove_unpublished_stage(stage)
        .err()
        .map(|source| ProvisionCleanupFailure {
            stage: stage.to_path_buf(),
            source,
        });
    ProvisionError { fault, cleanup }
}

fn remove_unpublished_stage(stage: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    tests::removal_fault(stage)?;
    std::fs::remove_dir_all(stage)
}

/// Build metadata, create the engine, then sync the stage before returning its owner.
fn build_in_temp(
    temp: &Path,
    request: &ProvisionRequest,
) -> Result<
    (
        marrow_kernel::durable::PendingNativeStoreOwner,
        AdmittedStoreDir,
    ),
    ProvisionFault,
> {
    let owner = NativeStore::acquire_existing(temp)
        .map_err(|error| ProvisionFault::Io(std::io::Error::other(error)))?;
    let admitted =
        AdmittedStoreDir::admit_under_owner(&owner).map_err(ProvisionFault::Admission)?;
    let (head_bytes, head) = request.head.encode_with_digest();
    let pending = EnvelopeRecord {
        metadata: request.envelope.clone(),
        state: EnvelopeState::Provision { head },
    }
    .encode()
    .map_err(|error| {
        ProvisionFault::Admission(AdmissionError::format(StoreEntry::Envelope, error))
    })?;
    admitted
        .write_new(Artifact::Envelope, &pending)
        .map_err(ProvisionFault::Admission)?;
    admitted
        .write_new(Artifact::Head, &head_bytes)
        .map_err(ProvisionFault::Admission)?;
    // Provisioning is the sole create/stamp path. It returns no engine or store
    // capability, so the newly created body cannot escape without an owner lock.
    NativeStore::provision(temp).map_err(ProvisionFault::Store)?;

    #[cfg(test)]
    crate::store_dir::barrier_fault::check(
        &admitted,
        crate::store_dir::barrier_fault::Point::ConstructionStage,
    )
    .map_err(ProvisionFault::Admission)?;
    admitted.sync().map_err(ProvisionFault::Admission)?;
    Ok((owner, admitted))
}

/// A held-open provisioned store: the native store the kernel drives, its envelope and head,
/// and the single-owner lock. An ordinary close releases the lock; an unresolved commit
/// quarantines it until process exit. The engine and lock are private and inseparable, and
/// the metadata is read only through the [`crate::NativeAttachment`] that pairs the store
/// with the image lifecycle admitted it for; this crate's admitted open is the sole
/// constructor.
///
/// ```compile_fail
/// use marrow_kernel::durable::NativeStore;
/// use marrow_lifecycle::OpenStore;
/// fn detach_engine(mut opened: OpenStore) {
///     let _: &mut NativeStore = &mut opened.store;
///     let _lock = opened.lock;
/// }
/// ```
///
/// ```compile_fail
/// use marrow_lifecycle::OpenStore;
/// fn rewrite_head(opened: &mut OpenStore, head: marrow_lifecycle::LogicalHead) {
///     opened.head = head;
/// }
/// ```
pub struct OpenStore {
    owner: NativeStore,
    pub(crate) directory: AdmittedStoreDir,
    pub(crate) envelope: StoreEnvelope,
    pub(crate) head: LogicalHead,
    pub(crate) head_digest: marrow_image::StoreHeadDigest,
}

impl SessionHost for OpenStore {
    type Engine = <NativeStore as SessionHost>::Engine;

    fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, Self::Engine>, SessionError> {
        self.owner.read_session(grant, demand)
    }

    fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, Self::Engine>, SessionError> {
        self.owner.txn_session(grant, demand)
    }
}

impl OpenStore {
    pub(crate) fn export_cells(
        &self,
        digest: &mut dyn ContentDigest,
        sink: &mut dyn marrow_kernel::durable::ExportSink,
    ) -> Result<AuditReport, marrow_kernel::durable::ExportError> {
        self.owner.export_cells(digest, sink)
    }

    /// The kernel's bounded read-only logical walk under the retained lock, with no session.
    pub(crate) fn logical_audit(
        &self,
        digest: &mut dyn ContentDigest,
    ) -> Result<AuditReport, SessionError> {
        self.owner.logical_audit(digest)
    }

    /// Consume an indeterminate commit's sole affine fact while retaining the
    /// same owner lock across old-engine close, fresh reopen, full integrity
    /// audit, and exact witness comparison. A known result returns the freshly
    /// opened owner for later invocations in this process, but quarantine stays
    /// irreversible and its drop cannot release the lock. Unknown retires the
    /// owner under the same process-lifetime quarantine.
    pub(crate) fn resolve_recovery(
        self,
        recovery: CommitRecovery,
    ) -> (DurableCommitState, Option<Self>) {
        let Self {
            owner,
            directory,
            envelope,
            head,
            head_digest,
        } = self;
        let (state, owner) = owner.resolve_recovery(recovery);
        (
            state,
            owner.map(|owner| Self {
                owner,
                directory,
                envelope,
                head,
                head_digest,
            }),
        )
    }
}

/// Why an open failed.
#[derive(Debug)]
pub enum OpenError {
    /// A pending transition or legacy envelope requires explicit validated activation.
    ActivationRequired { instance: StoreInstanceId },
    /// No store exists at the path.
    NotProvisioned,
    /// The store directory could not be examined at all, so nothing about the store it may
    /// hold was established. Kept apart from [`OpenError::NotProvisioned`] and
    /// [`OpenError::Incomplete`] on purpose: those two state what the directory holds, and
    /// this one states that it could not be seen.
    Access(StoreAccessError),
    /// The store directory exists but is missing a durable artifact.
    Incomplete,
    /// The store is held by another owner, or the lock could not be taken.
    Lock(LockError),
    /// One of the store directory's own artifacts could not be admitted under the owner:
    /// its entry was refused, it was reachable under a second name, it changed while it was
    /// being read, or its bytes did not decode. A decode failure carries the typed
    /// [`FormatError`] so an unknown writer version (`store.format_version`) or an over-bound
    /// length (`store.limit`) is reported as itself, not flattened to corruption (FR01 §6).
    Admission(AdmissionError),
    /// The unclean-open integrity audit found the engine's stored bytes corrupt.
    Corruption { message: String },
    /// The ordered-byte engine could not be opened.
    Store(StoreError),
    /// A filesystem operation failed.
    Io(std::io::Error),
}

impl OpenError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            OpenError::NotProvisioned => Code::StoreIo.as_str(),
            OpenError::ActivationRequired { .. } => Code::StoreActivationRequired.as_str(),
            OpenError::Access(error) => error.code(),
            OpenError::Incomplete | OpenError::Corruption { .. } => Code::StoreCorruption.as_str(),
            OpenError::Admission(error) => error.code(),
            OpenError::Lock(error) => error.code(),
            OpenError::Store(error) => error.code(),
            OpenError::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NotProvisioned => write!(f, "no store exists at the destination"),
            OpenError::ActivationRequired { instance } => write!(
                f,
                "store {} requires explicit validated activation before service",
                instance.to_hex()
            ),
            OpenError::Access(error) => write!(f, "{error}"),
            OpenError::Incomplete => {
                write!(
                    f,
                    "the store directory is incomplete (a partially-formed store)"
                )
            }
            OpenError::Lock(error) => write!(f, "{error}"),
            OpenError::Admission(error) => write!(f, "{error}"),
            OpenError::Corruption { message } => write!(f, "the store is corrupt: {message}"),
            OpenError::Store(error) => write!(f, "the store engine could not be opened: {error}"),
            OpenError::Io(error) => write!(f, "opening the store failed: {error}"),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<StoreAccessError> for OpenError {
    fn from(error: StoreAccessError) -> Self {
        OpenError::Access(error)
    }
}

/// A failure to open a store under an admission gate: either the ordinary open failed, or the
/// gate refused the presented image after the lock was taken and before any engine call. The
/// `R` is the caller's refusal type (the lifecycle actor's demand-exceeds-ceiling refusal).
pub(crate) enum AdmitError<R> {
    /// The store could not be opened (see [`OpenError`]).
    Open(OpenError),
    /// The admission gate refused the image with zero engine calls; the lock was released.
    Refused(R),
}

/// Open the complete store at `dir` under `projection`, taking the single-owner lock and
/// running `admit` against the persisted head **after** the lock is taken and **before** any
/// engine call, so a refusal makes zero engine calls and releases the lock on return. A
/// non-complete directory is refused without opening; a store held by another owner returns
/// [`OpenError::Lock`] naming the owner. When the prior shutdown was unclean (a stale lock
/// descriptor), a read-write open runs the full integrity audit; read-only access preserves
/// that obligation and never repairs. On success the
/// returned [`OpenStore`] holds the lock for the store's whole open life. The lifecycle actor
/// and the importer supply an `admit` that admits the presented image against the head; a
/// refusal is surfaced as [`AdmitError::Refused`]. Explicit recovery shares the
/// same [`LockedStore`] owner composition with a distinct state eligibility check.
pub(crate) fn open_admitted<R>(
    dir: &Path,
    projection: StoreProjection,
    access: NativeOpenAccess,
    admit: impl FnOnce(&LogicalHead) -> Result<(), R>,
) -> Result<OpenStore, AdmitError<R>> {
    let locked = LockedStore::acquire(dir).map_err(AdmitError::Open)?;
    if locked.envelope.state != EnvelopeState::Active {
        return Err(AdmitError::Open(OpenError::ActivationRequired {
            instance: locked.envelope.metadata.instance,
        }));
    }
    locked.open(projection, access, |head, _| admit(head))
}

/// Owner-held directory and envelope before engine opening.
/// Ordinary open requires Active; explicit recovery validates the retained state.
pub(crate) struct LockedStore {
    pending: marrow_kernel::durable::PendingNativeStoreOwner,
    directory: AdmittedStoreDir,
    pub(crate) envelope: EnvelopeRecord,
}

impl LockedStore {
    pub(crate) fn acquire(dir: &Path) -> Result<Self, OpenError> {
        decide_before_locking(dir)?;
        let pending = NativeStore::acquire_existing(dir).map_err(|error| match error {
            NativeOwnerAcquireError::Io(error) => OpenError::Io(error),
            NativeOwnerAcquireError::Lock(error) => OpenError::Lock(LockError::from(error)),
        })?;
        #[cfg(test)]
        admission_substitution::apply(pending.directory());
        let directory =
            AdmittedStoreDir::admit_under_owner(&pending).map_err(OpenError::Admission)?;
        if !directory.is_complete().map_err(OpenError::Admission)? {
            return Err(OpenError::Incomplete);
        }
        let envelope = decode_record(&directory)?;
        check_engine_stamp(&envelope.metadata).map_err(OpenError::Store)?;
        Ok(Self {
            pending,
            directory,
            envelope,
        })
    }

    pub(crate) fn directory_path(&self) -> &Path {
        self.pending.directory()
    }

    pub(crate) fn open<R>(
        self,
        projection: StoreProjection,
        access: NativeOpenAccess,
        admit: impl FnOnce(&LogicalHead, marrow_image::StoreHeadDigest) -> Result<(), R>,
    ) -> Result<OpenStore, AdmitError<R>> {
        let Self {
            pending,
            directory,
            envelope,
        } = self;
        let envelope = envelope.metadata;
        let mut admitted_head = None;
        let owner = pending
            .bind_and_open_existing(access, *envelope.instance.bytes(), projection, || {
                let (head, digest) = decode_head(&directory).map_err(Ok)?;
                admit(&head, digest).map_err(Err)?;
                admitted_head = Some((head, digest));
                Ok::<(), Result<OpenError, R>>(())
            })
            .map_err(|error| match error {
                NativeOwnerOpenError::Lock(error) => {
                    AdmitError::Open(OpenError::Lock(LockError::from(error)))
                }
                NativeOwnerOpenError::Refused(Ok(error)) => AdmitError::Open(error),
                NativeOwnerOpenError::Refused(Err(refusal)) => AdmitError::Refused(refusal),
                NativeOwnerOpenError::Store(StoreError::Corruption { message }) => {
                    AdmitError::Open(OpenError::Corruption { message })
                }
                NativeOwnerOpenError::Store(error) => AdmitError::Open(OpenError::Store(error)),
            })?;
        let (head, head_digest) =
            admitted_head.expect("a successful open completed head admission");
        Ok(OpenStore {
            owner,
            directory,
            envelope,
            head,
            head_digest,
        })
    }
}

#[cfg(test)]
pub(crate) mod admission_substitution {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    struct Swap {
        original: PathBuf,
        displaced: PathBuf,
        replacement: PathBuf,
    }

    thread_local! {
        static SWAP: RefCell<Option<Swap>> = const { RefCell::new(None) };
    }

    pub(crate) fn with_swap<T>(
        original: &Path,
        displaced: &Path,
        replacement: &Path,
        action: impl FnOnce() -> T,
    ) -> T {
        struct Restore(Option<Swap>);
        impl Drop for Restore {
            fn drop(&mut self) {
                SWAP.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        let swap = Swap {
            original: std::fs::canonicalize(original).expect("existing original"),
            displaced: displaced.to_owned(),
            replacement: replacement.to_owned(),
        };
        let _restore = Restore(SWAP.with(|slot| slot.replace(Some(swap))));
        action()
    }

    pub(super) fn apply(directory: &Path) {
        let swap = SWAP.with(|slot| {
            let mut armed = slot.borrow_mut();
            if armed
                .as_ref()
                .is_some_and(|swap| swap.original == directory)
            {
                armed.take()
            } else {
                None
            }
        });
        if let Some(swap) = swap {
            std::fs::rename(&swap.original, &swap.displaced).expect("move held directory");
            std::fs::rename(&swap.replacement, &swap.original).expect("substitute directory");
        }
    }
}

/// Everything an open settles before it takes the owner lock, and nothing else.
///
/// Acquiring the lock creates the `lock` entry and writes a marker into it, so the two
/// questions decided here are exactly the two whose answers would make that write wrong: a
/// platform where no store could be admitted at all, and a directory that is not a store —
/// where the entry would be left behind in an ordinary directory. Neither reads a store
/// artifact's bytes, so neither can preempt the exclusion verdict a contender is owed: a
/// store with a live holder always has the lock entry, because a holder creates it before it
/// locks, so the completeness condition is unreachable while a holder is live.
///
/// Nothing here resolves a failure to look into an observation. A directory this process
/// cannot examine reaches [`OpenError::Access`] rather than a claim about what it holds.
/// What the directory *contains* — whether its artifacts are the artifacts, whether they
/// decode — is read under the owner.
fn decide_before_locking(dir: &Path) -> Result<(), OpenError> {
    store_dir::qualified_platform().map_err(OpenError::Admission)?;
    match preflight(dir)? {
        Preflight::Absent => Err(OpenError::NotProvisioned),
        Preflight::Incomplete if !store_dir::lock_entry_present(dir)? => Err(OpenError::Incomplete),
        Preflight::Incomplete | Preflight::Complete => Ok(()),
    }
}

fn check_engine_stamp(envelope: &StoreEnvelope) -> Result<(), StoreError> {
    let supported = marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION;
    if envelope.engine_format_version != supported {
        return Err(StoreError::FormatVersion {
            found: envelope.engine_format_version,
            supported,
        });
    }
    Ok(())
}

pub(crate) fn decode_record(dir: &AdmittedStoreDir) -> Result<EnvelopeRecord, OpenError> {
    let bytes = dir
        .read(Artifact::Envelope, crate::envelope::file_ceiling)
        .map_err(OpenError::Admission)?;
    EnvelopeRecord::decode(&bytes)
        .map_err(|error| OpenError::Admission(AdmissionError::format(StoreEntry::Envelope, error)))
}

pub(crate) fn decode_head(
    dir: &AdmittedStoreDir,
) -> Result<(LogicalHead, marrow_image::StoreHeadDigest), OpenError> {
    let bytes = dir
        .read(Artifact::Head, crate::head::file_ceiling)
        .map_err(OpenError::Admission)?;
    LogicalHead::decode_with_digest(&bytes)
        .map_err(|error| OpenError::Admission(AdmissionError::format(StoreEntry::Head, error)))
}

/// A private sibling temporary directory for building a store before its atomic claim: the
/// bounded ASCII component contains only a marker, process id and monotonic counter,
/// with no destination spelling embedded. Exclusive directory creation claims the candidate;
/// the name alone
/// grants no ownership because a prior process with the same pid may have left it behind.
pub(crate) fn temp_sibling(dest: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    // A caller may choose a name in this private namespace. Use a different decimal
    // suffix even when the filesystem folds the marker's case. This skips a candidate
    // before creation; an occupied selected candidate still refuses without retry.
    if dest.file_name().is_some_and(|name| {
        name.as_encoded_bytes()
            .ends_with(format!(".{counter}").as_bytes())
    }) {
        counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    }
    let temp_name = format!(".marrow-provisioning.{}.{counter}", std::process::id());
    match dest.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(temp_name),
        _ => PathBuf::from(temp_name),
    }
}

#[cfg(unix)]
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().create(dir)
}

/// An open with a no-op admission gate: the directory lifecycle under test, with no image to
/// admit. Test-only; every production open admits an image.
#[cfg(test)]
pub(crate) fn open_unadmitted(
    dir: &Path,
    projection: StoreProjection,
) -> Result<OpenStore, OpenError> {
    open_admitted(dir, projection, NativeOpenAccess::ReadWrite, |_| {
        Ok::<(), std::convert::Infallible>(())
    })
    .map_err(|error| match error {
        AdmitError::Open(error) => error,
        AdmitError::Refused(never) => match never {},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::head::ActiveBinding;
    use crate::headmap::HeadMap;
    use marrow_image::LedgerIdBytes;

    #[test]
    fn provision_never_replaces_an_existing_empty_directory() {
        let scratch = ScratchDir::new("occupied-empty");
        let destination = scratch.0.join("destination");
        std::fs::create_dir(&destination).unwrap();
        let (_, request) = compiled_request();
        let result = provision(&destination, request);
        assert!(matches!(
            result,
            Err(ProvisionError {
                fault: ProvisionFault::AlreadyProvisioned,
                ..
            })
        ));
        assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn occupied_files_and_dangling_links_refuse_without_replacement() {
        let scratch = ScratchDir::new("occupied-entries");
        for dangling in [false, true] {
            let destination = scratch.0.join(if dangling { "link" } else { "file" });
            if dangling {
                std::os::unix::fs::symlink("missing-target", &destination).unwrap();
            } else {
                std::fs::write(&destination, b"existing bytes").unwrap();
            }
            let (_, request) = compiled_request();
            assert!(matches!(
                provision(&destination, request),
                Err(ProvisionError {
                    fault: ProvisionFault::AlreadyProvisioned,
                    ..
                })
            ));
            if dangling {
                assert_eq!(
                    std::fs::read_link(&destination).unwrap(),
                    Path::new("missing-target")
                );
            } else {
                assert_eq!(std::fs::read(&destination).unwrap(), b"existing bytes");
            }
        }
    }

    struct ConstructionFault {
        destination: PathBuf,
        point: Option<crate::store_dir::barrier_fault::Point>,
        removal: Option<std::io::ErrorKind>,
        observed: Option<(PathBuf, Vec<std::ffi::OsString>)>,
    }

    thread_local! {
        static CONSTRUCTION: std::cell::RefCell<Option<ConstructionFault>> = const { std::cell::RefCell::new(None) };
    }

    pub(super) fn construct<T>(destination: &Path, stage: &Path, action: impl FnOnce() -> T) -> T {
        let point = CONSTRUCTION.with(|slot| {
            slot.borrow()
                .as_ref()
                .filter(|fault| fault.destination == destination)
                .map(|fault| fault.point)
        });
        let Some(point) = point else { return action() };
        let result = match point {
            Some(point) => crate::store_dir::barrier_fault::with_failure(stage, point, action),
            None => action(),
        };
        let mut names: Vec<_> = std::fs::read_dir(stage)
            .expect("owned stage before cleanup")
            .map(|entry| entry.expect("stage entry").file_name())
            .collect();
        names.sort();
        CONSTRUCTION.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .expect("armed construction")
                .observed = Some((stage.to_path_buf(), names))
        });
        result
    }

    pub(super) fn removal_fault(stage: &Path) -> std::io::Result<()> {
        let failure = CONSTRUCTION.with(|slot| {
            let mut slot = slot.borrow_mut();
            let fault = slot.as_mut()?;
            if fault.observed.as_ref()?.0 == stage {
                fault.removal.take()
            } else {
                None
            }
        });
        match failure {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        }
    }

    #[test]
    fn provision_retains_the_original_failure_and_failed_cleanup_location() {
        use crate::store_dir::barrier_fault::Point;
        struct Restore(Option<ConstructionFault>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CONSTRUCTION.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        for point in [Some(Point::NewBody(Artifact::Envelope)), None] {
            let scratch = std::mem::ManuallyDrop::new(ScratchDir::new("cleanup-failure"));
            eprintln!("cleanup-failure fixture: {}", scratch.0.display());
            let destination = scratch.0.join("destination");
            if point.is_none() {
                std::fs::create_dir(&destination).expect("occupied destination");
                std::fs::write(destination.join("sentinel"), b"existing destination")
                    .expect("sentinel");
            }
            let (_, request) = compiled_request();
            let _restore = Restore(CONSTRUCTION.with(|slot| {
                slot.replace(Some(ConstructionFault {
                    destination: destination.clone(),
                    point,
                    removal: Some(std::io::ErrorKind::PermissionDenied),
                    observed: None,
                }))
            }));
            let error = provision(&destination, request).expect_err("provision failed");
            let stage = CONSTRUCTION.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .expect("armed")
                    .observed
                    .as_ref()
                    .expect("observed stage")
                    .0
                    .clone()
            });
            assert!(stage.exists(), "forced cleanup failure retains owned stage");
            let cleanup = error.cleanup.as_ref().expect("typed cleanup evidence");
            assert_eq!(cleanup.stage, stage);
            assert_eq!(cleanup.source.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(matches!(
                (&point, &error.fault),
                (Some(_), ProvisionFault::Admission(_))
                    | (None, ProvisionFault::AlreadyProvisioned)
            ));
            assert_eq!(
                error.code(),
                if point.is_some() {
                    Code::StoreIo.as_str()
                } else {
                    Code::StoreLocked.as_str()
                }
            );
            if point.is_none() {
                assert_eq!(
                    std::fs::read(destination.join("sentinel")).expect("destination untouched"),
                    b"existing destination"
                );
            } else {
                assert!(!destination.exists());
            }
            // The user-facing failure must identify the actual directory whose cleanup failed.
            assert!(
                error
                    .to_string()
                    .contains(stage.to_str().expect("fixture path")),
                "failure lost the owned stage location: {error}"
            );
            drop(std::mem::ManuallyDrop::into_inner(scratch));
        }
    }

    #[test]
    fn construction_failures_remove_only_the_stage_this_invocation_created() {
        use crate::store_dir::barrier_fault::Point;
        struct Restore(Option<ConstructionFault>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CONSTRUCTION.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        for point in [
            Point::NewBody(Artifact::Envelope),
            Point::NewBody(Artifact::Head),
            Point::ConstructionStage,
        ] {
            let scratch = std::mem::ManuallyDrop::new(ScratchDir::new("construction-prefix"));
            eprintln!("construction-prefix fixture: {}", scratch.0.display());
            let destination = scratch.0.join("destination");
            let unrelated = scratch.0.join("unrelated.provisioning");
            std::fs::create_dir(&unrelated).expect("unrelated sibling");
            std::fs::write(unrelated.join("sentinel"), b"keep me").expect("sentinel");
            let (_, request) = compiled_request();
            let _restore = Restore(CONSTRUCTION.with(|slot| {
                slot.replace(Some(ConstructionFault {
                    destination: destination.clone(),
                    point: Some(point),
                    removal: None,
                    observed: None,
                }))
            }));
            let error =
                provision(&destination, request).expect_err("failed construction cannot publish");
            assert_eq!(error.code(), Code::StoreIo.as_str());
            assert!(
                error.cleanup.is_none(),
                "successful cleanup has no failure evidence"
            );
            assert!(matches!(
                error.fault,
                ProvisionFault::Admission(AdmissionError {
                    fault: store_dir::AdmissionFault::Custody(
                        marrow_fs_journal::CustodyError::Io { .. }
                    ),
                    ..
                })
            ));
            let (stage, names) = CONSTRUCTION.with(|slot| {
                slot.borrow_mut()
                    .as_mut()
                    .expect("armed")
                    .observed
                    .take()
                    .expect("actual stage observed")
            });
            let mut expected: Vec<std::ffi::OsString> =
                vec![store_dir::ENVELOPE_FILE.into(), store_dir::LOCK_FILE.into()];
            if point != Point::NewBody(Artifact::Envelope) {
                expected.push(store_dir::HEAD_FILE.into());
            }
            if point == Point::ConstructionStage {
                expected.push(store_dir::ENGINE_FILE.into());
            }
            expected.sort();
            assert_eq!(names, expected);
            assert!(!destination.exists());
            assert!(!stage.exists(), "only the owned stage is removed");
            assert_eq!(
                std::fs::read(unrelated.join("sentinel")).expect("unrelated retained"),
                b"keep me"
            );
            assert_eq!(std::fs::read_dir(&scratch.0).expect("parent").count(), 1);
            drop(std::mem::ManuallyDrop::into_inner(scratch));
        }
    }

    fn compiled_request() -> (marrow_verify::VerifiedImage, ProvisionRequest) {
        let source = "resource Item { required value: int }\nstore ^items[key: int]: Item\npub fn read(key: int): int { return ^items[key].value ?? 0 }\n";
        let ids = "marrow ids v0\nmachine-written by marrow; do not edit\nid application . 01010101010101010101010101010101\nid product Item 02020202020202020202020202020202\nid field Item.value 03030303030303030303030303030303\nid root items 04040404040404040404040404040404\nid key items.key 05050505050505050505050505050505\nhigh-water 0\nend\n";
        let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
        let project = marrow_project::capture(
            &manifest,
            vec![marrow_project::CapturedFile::new(
                "src/main.mw".into(),
                source.as_bytes().to_vec(),
            )],
            Some(ids.as_bytes()),
            &marrow_project::CaptureLimits::DEFAULT,
        )
        .expect("capture");
        let compiled = marrow_compile::compile(&project).expect("compile");
        let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
        let request = ProvisionRequest {
            envelope: StoreEnvelope {
                instance: StoreInstanceId::draw().expect("instance"),
                writer_toolchain: env!("CARGO_PKG_VERSION").into(),
                engine_kind: crate::EngineKind::Redb,
                engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
            },
            head: LogicalHead::provision(
                crate::active_binding(&image),
                crate::accepted_ceiling(&image),
                crate::head_map(&image).expect("head map"),
            ),
        };
        (image, request)
    }

    #[test]
    fn a_complete_unpublished_stage_is_adopted_at_its_current_location() {
        let scratch = ScratchDir::new("complete-stage");
        let destination = scratch.0.join("destination");
        let stage = temp_sibling(&destination);
        create_private_dir(&stage).expect("private stage");
        let (image, request) = compiled_request();
        let instance = request.envelope.instance;
        let (owner, admitted) = build_in_temp(&stage, &request).expect("complete production stage");
        let (head, digest) = decode_head(&admitted).expect("head");
        let head = head.encode();
        assert_eq!(
            decode_record(&admitted).expect("record").state,
            EnvelopeState::Provision { head: digest }
        );
        assert!(!destination.exists());
        let held = crate::recover(&stage, crate::prepare(image.clone()))
            .expect_err("construction owner held");
        assert!(matches!(
            held.fault,
            crate::RecoveryFault::Validation(crate::AuditError::Open(OpenError::Lock(
                LockError::StoreInUse { .. }
            )))
        ));
        drop(admitted);
        drop(owner);
        assert!(matches!(
            crate::attach(&stage, crate::prepare(image.clone())),
            Err(crate::LifecycleError::Open(
                OpenError::ActivationRequired { .. }
            ))
        ));
        let recovered =
            crate::recover(&stage, crate::prepare(image.clone())).expect("adopt current stage");
        assert_eq!(recovered.instance, instance);
        assert_eq!(recovered.image_id, image.image_id());
        assert_eq!(
            std::fs::read(stage.join(crate::HEAD_FILE)).expect("head unchanged"),
            head
        );
        assert!(matches!(
            crate::attach(&stage, crate::prepare(image)).expect("active at stage"),
            crate::AttachOutcome::AlreadyActive(_)
        ));
        assert!(
            !destination.exists(),
            "recovery must not reconstruct a former destination"
        );
    }

    #[cfg(unix)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum PublicationPoint {
        Staged,
        Renamed,
    }

    #[cfg(unix)]
    struct PublicationObservation {
        destination: PathBuf,
        seen: Vec<(PublicationPoint, u64, u64)>,
        mutation: Option<(PublicationPoint, PublicationMutation)>,
    }

    #[cfg(unix)]
    enum PublicationMutation {
        Occupy,
        Substitute(PathBuf),
    }

    #[cfg(unix)]
    thread_local! {
        static PUBLICATION_OBSERVATION: std::cell::RefCell<Option<PublicationObservation>> = const { std::cell::RefCell::new(None) };
    }

    #[cfg(unix)]
    pub(super) fn observe_publication(
        point: PublicationPoint,
        destination: &Path,
        location: &Path,
        owner: &marrow_kernel::durable::PendingNativeStoreOwner,
        admitted: &AdmittedStoreDir,
    ) {
        use std::os::unix::fs::MetadataExt;
        PUBLICATION_OBSERVATION.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(observation) = slot
                .as_mut()
                .filter(|observation| observation.destination == destination)
            else {
                return;
            };
            let held = owner
                .directory_metadata()
                .expect("retained owner directory");
            let actual = std::fs::metadata(location).expect("current location");
            assert_eq!((held.dev(), held.ino()), (actual.dev(), actual.ino()));
            admitted
                .verify_location(location)
                .expect("same admitted descriptor");
            assert!(matches!(
                NativeStore::acquire_existing(location),
                Err(NativeOwnerAcquireError::Lock(
                    marrow_kernel::durable::NativeLockError::StoreInUse { .. }
                ))
            ));
            observation.seen.push((point, actual.dev(), actual.ino()));
            if observation
                .mutation
                .as_ref()
                .is_some_and(|(at, _)| *at == point)
            {
                match observation.mutation.take().unwrap().1 {
                    PublicationMutation::Occupy => std::fs::create_dir(destination).unwrap(),
                    PublicationMutation::Substitute(saved) => {
                        std::fs::rename(location, saved).unwrap();
                        std::fs::create_dir(location).unwrap();
                        std::fs::write(location.join("replacement"), b"do not delete").unwrap();
                    }
                }
            }
        });
    }

    #[cfg(unix)]
    #[test]
    fn public_provision_holds_the_same_directory_owner_across_rename() {
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                PUBLICATION_OBSERVATION.with(|slot| {
                    slot.borrow_mut().take();
                });
            }
        }
        let scratch = ScratchDir::new("rename-owner");
        let destination = scratch.0.join("store");
        let (image, request) = compiled_request();
        let _clear = Clear;
        PUBLICATION_OBSERVATION.with(|slot| {
            *slot.borrow_mut() = Some(PublicationObservation {
                destination: destination.clone(),
                seen: Vec::new(),
                mutation: None,
            })
        });
        provision(&destination, request).expect("public provision");
        PUBLICATION_OBSERVATION.with(|slot| {
            let observation = slot.borrow_mut().take().expect("observations");
            assert_eq!(observation.seen.len(), 2);
            let (point, device, inode) = observation.seen[0];
            assert_eq!(point, PublicationPoint::Staged);
            assert_eq!(
                observation.seen[1],
                (PublicationPoint::Renamed, device, inode)
            );
        });
        assert!(matches!(
            crate::attach(&destination, crate::prepare(image))
                .expect("released completed provision"),
            crate::AttachOutcome::AlreadyActive(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn late_collision_and_identity_changes_preserve_actual_custody() {
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                PUBLICATION_OBSERVATION.with(|slot| *slot.borrow_mut() = None);
            }
        }
        for case in 0..3 {
            let scratch = std::mem::ManuallyDrop::new(ScratchDir::new("publication-custody"));
            eprintln!(
                "preserved publication custody fixture {case}: {}",
                scratch.0.display()
            );
            let destination = scratch.0.join("store");
            let saved = scratch.0.join("retained-original");
            let (at, action) = match case {
                0 => (PublicationPoint::Staged, PublicationMutation::Occupy),
                1 => (
                    PublicationPoint::Staged,
                    PublicationMutation::Substitute(saved.clone()),
                ),
                _ => (
                    PublicationPoint::Renamed,
                    PublicationMutation::Substitute(saved.clone()),
                ),
            };
            let (_, request) = compiled_request();
            let instance = request.envelope.instance;
            let _clear = Clear;
            PUBLICATION_OBSERVATION.with(|slot| {
                *slot.borrow_mut() = Some(PublicationObservation {
                    destination: destination.clone(),
                    seen: Vec::new(),
                    mutation: Some((at, action)),
                })
            });
            let error = provision(&destination, request).unwrap_err();
            match case {
                0 => {
                    assert!(matches!(error.fault, ProvisionFault::AlreadyProvisioned));
                    assert!(error.cleanup.is_none());
                    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
                }
                1 => {
                    assert!(matches!(error.fault, ProvisionFault::Io(_)));
                    let cleanup = error.cleanup.expect("replacement must not be removed");
                    assert_eq!(
                        std::fs::read(cleanup.stage.join("replacement")).unwrap(),
                        b"do not delete"
                    );
                    assert!(saved.join(crate::HEAD_FILE).is_file());
                    assert!(!destination.exists());
                }
                _ => {
                    assert!(
                        matches!(error.fault, ProvisionFault::PublicationUncertain { instance: found, .. } if found == instance)
                    );
                    assert!(error.cleanup.is_none());
                    assert_eq!(
                        std::fs::read(destination.join("replacement")).unwrap(),
                        b"do not delete"
                    );
                    assert!(saved.join(crate::HEAD_FILE).is_file());
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn inadmissible_destination_names_refuse_before_stage_creation() {
        use std::os::unix::ffi::OsStringExt;
        let scratch = ScratchDir::new("publication-name-admission");
        for name in [
            std::ffi::OsString::from("a\\b"),
            std::ffi::OsString::from("a:store"),
            std::ffi::OsString::from("control\u{1}"),
            std::ffi::OsString::from_vec(vec![0xff]),
        ] {
            let (_, request) = compiled_request();
            let error = provision(&scratch.0.join(name), request).unwrap_err();
            assert!(
                matches!(error.fault, ProvisionFault::Io(source) if source.kind() == std::io::ErrorKind::InvalidInput)
            );
            assert!(error.cleanup.is_none());
            assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_parent_and_missing_owner_permissions_refuse_before_staging() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = ScratchDir::new("publication-parent-admission");
        let parent = scratch.0.join("parent");
        std::fs::create_dir(&parent).unwrap();
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&parent, &link).unwrap();
        let (_, request) = compiled_request();
        assert!(provision(&link.join("store"), request).is_err());
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        let (_, request) = compiled_request();
        let result = provision(&parent.join("store"), request);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
    }

    /// The empty store shape: no roots, so no site to resolve. These cases exercise the
    /// directory lifecycle, not the store's own shape.
    fn rootless() -> StoreProjection {
        StoreProjection::builder()
            .finish()
            .expect("a rootless projection has no site to resolve")
    }

    /// The smallest complete provision request: one durable node, a two-byte accepted
    /// ceiling payload, and no store shape.
    fn test_request(instance: StoreInstanceId) -> ProvisionRequest {
        let head_map = HeadMap::assign(&[LedgerIdBytes::from_bytes([0x01; 16])]).expect("head map");
        ProvisionRequest {
            envelope: StoreEnvelope {
                instance,
                writer_toolchain: "0.1.0".into(),
                engine_kind: crate::envelope::EngineKind::Redb,
                engine_format_version: 1,
            },
            head: LogicalHead::provision(
                ActiveBinding {
                    image_format_version: marrow_image::IMAGE_FORMAT_VERSION,
                    image_id: [0x11; 32],
                    durable_contract: [0x22; 32],
                    interface: [0x33; 32],
                },
                vec![0x44, 0x45],
                head_map,
            ),
        }
    }

    struct ScratchDir(std::path::PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "marrow-lifecycle-{tag}-{}-{}",
                std::process::id(),
                now_nonce(),
            ));
            std::fs::create_dir_all(&path).expect("create scratch directory");
            Self(path)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The classification of a directory this test can examine. A preflight that cannot look
    /// is a distinct outcome with its own coverage; nothing here should reach it.
    fn classify(dir: &Path) -> Preflight {
        preflight(dir).expect("this test's directories are examinable")
    }

    #[test]
    fn preflight_classifies_absent_incomplete_complete_without_creating() {
        let base = std::env::temp_dir().join(format!(
            "marrow-lifecycle-preflight-{}-{}",
            std::process::id(),
            now_nonce(),
        ));
        let dir = base.join("store");

        // Absent: no directory. Preflight creates nothing.
        assert_eq!(classify(&dir), Preflight::Absent);
        assert!(
            !base.exists(),
            "preflight must not create the base directory"
        );
        assert!(
            !dir.exists(),
            "preflight must not create the store directory"
        );

        // Incomplete: the directory exists but lacks artifacts.
        std::fs::create_dir_all(&dir).expect("create dir");
        assert_eq!(classify(&dir), Preflight::Incomplete);
        let before: Vec<_> = read_dir_names(&dir);
        assert_eq!(classify(&dir), Preflight::Incomplete);
        assert_eq!(
            read_dir_names(&dir),
            before,
            "preflight must not add a file"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    fn open_owner(dir: &Path, instance: [u8; 16]) -> NativeStore {
        NativeStore::acquire_existing(dir)
            .expect("acquire the owner")
            .bind_and_open_existing(NativeOpenAccess::ReadWrite, instance, rootless(), || {
                Ok::<_, std::convert::Infallible>(())
            })
            .expect("bind and open")
    }

    #[test]
    fn opaque_native_owner_holds_exclusion_and_clean_drop_releases_it() {
        let scratch = ScratchDir::new("opaque-owner");
        NativeStore::provision(&scratch.0).expect("provision native engine");
        let owner = open_owner(&scratch.0, [0x51; 16]);
        assert!(matches!(
            NativeStore::acquire_existing(&scratch.0),
            Err(NativeOwnerAcquireError::Lock(_)),
        ));
        drop(owner);
        drop(open_owner(&scratch.0, [0x52; 16]));
    }

    /// Every admission read and callback happens under one owner, and the pair of artifacts
    /// the open reports is the snapshot taken under it. A competing open attempted from
    /// inside the admission callback — the innermost point of the sequence — is refused as
    /// contention, and an envelope rewritten at that same point does not reach the caller,
    /// because the envelope is read once under this owner and never re-read behind the head.
    #[test]
    fn admission_reads_are_one_snapshot_under_one_owner() {
        let scratch = ScratchDir::new("one-snapshot");
        let store = scratch.0.join("store");
        let original = StoreInstanceId::draw().expect("entropy");
        provision(&store, test_request(original)).expect("provision");

        let mut contended = None;
        let opened = open_admitted(&store, rootless(), NativeOpenAccess::ReadWrite, |head| {
            contended = Some(match open_unadmitted(&store, rootless()) {
                Err(OpenError::Lock(error)) => error.code(),
                Ok(_) => panic!("a competing open ran inside the admission callback"),
                Err(other) => panic!("admission ran outside its owner: {other}"),
            });
            // Rewrite the envelope at the innermost point of the sequence. A second read
            // behind the head would pick this up; one snapshot cannot.
            let replacement = StoreEnvelope {
                instance: StoreInstanceId::draw().expect("entropy"),
                writer_toolchain: "9.9.9".into(),
                engine_kind: crate::envelope::EngineKind::Redb,
                engine_format_version: 1,
            };
            let replacement = EnvelopeRecord {
                metadata: replacement,
                state: EnvelopeState::Active,
            }
            .encode()
            .expect("encode replacement record");
            std::fs::write(store.join(store_dir::ENVELOPE_FILE), replacement)
                .expect("rewrite the envelope mid-admission");
            assert_eq!(head.binding.image_id, [0x11; 32]);
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap_or_else(|_| panic!("the open completes under its own owner"));

        assert_eq!(contended, Some("store.locked"));
        assert_eq!(
            opened.envelope.instance, original,
            "the open reports the envelope its owner admitted, not one rewritten behind it",
        );
    }

    fn read_dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn now_nonce() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }
}
