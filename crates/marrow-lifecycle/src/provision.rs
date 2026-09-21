//! The persistent provision and open flow.
//!
//! Provision builds the whole store in a private sibling temporary directory and atomically
//! renames it into place. A rename onto any existing destination fails, so exactly one
//! provisioner wins a race and a crash before the rename leaves only a temporary directory —
//! the destination is never a partially-formed store. A failed parent-directory sync after
//! rename reports publication uncertainty and retains the destination; namespace completeness
//! does not confirm durability. Preflight is strictly non-creating.
//!
//! Open takes the single-owner lock first, and only then reads the store directory at all:
//! completeness, the envelope, and the head are one admission snapshot taken under that
//! owner, each artifact admitted within its own byte ceiling. Deciding exclusion ahead of
//! every read keeps a contender's verdict independent of the holder's bytes — a malformed,
//! truncated, or deleted artifact cannot turn "the store is locked" into a decode or
//! completeness error. The engine opens last, through the path kernel. A read-write open
//! after an unclean shutdown runs a full integrity audit.
//!
//! That audit covers crash-path corruption only: the fast open path does not re-verify page
//! checksums, so an externally flipped bit in a cleanly-closed store is not detected at open.
//! The read-only store audit (`crate::audit`) checks logical contents; it performs no
//! repairing integrity call and preserves an inherited unclean obligation.

use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_kernel::durable::{
    AuditReport, CommitRecovery, ContentDigest, DemandCoverage, DurableCommitState,
    InvocationGrant, NativeLockError, NativeOpenAccess, NativeOwnerAcquireError,
    NativeOwnerOpenError, NativeStore, NumberedProjection, ReadSession, SessionError, SessionHost,
    StoreError, TxnSession,
};

use crate::durable_fs::{Publication, custody_io};
use crate::envelope::StoreEnvelope;
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::head::LogicalHead;
use crate::instance::StoreInstanceId;
use crate::seam::{Event, Seam, StagePoint, Step};
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
    pub fn code(&self) -> Code {
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
    pub fn code(&self) -> Code {
        match self {
            ProvisionFault::AlreadyProvisioned => Code::StoreLocked,
            ProvisionFault::Admission(error) => error.code(),
            ProvisionFault::Store(error) => error.code(),
            ProvisionFault::Io(_) => Code::StoreIo,
            ProvisionFault::PublicationUncertain { .. } => Code::StorePublicationUncertain,
            ProvisionFault::ActivationUncertain { .. } => Code::StoreActivationUncertain,
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
    provision_observed(dest, request, Seam::NONE)
}

/// [`provision`] over `seam`: the production seam observes nothing; a test's cuts the
/// sequence at a named step.
pub(crate) fn provision_observed(
    dest: &Path,
    request: ProvisionRequest,
    seam: Seam,
) -> Result<Provisioned, ProvisionError> {
    check_engine_stamp(&request.envelope).map_err(ProvisionFault::Store)?;
    let instance = request.envelope.instance;
    let temp = temp_sibling(dest);
    let publication = Publication::admit(&temp, dest, seam.clone()).map_err(ProvisionFault::Io)?;
    create_private_dir(&temp).map_err(ProvisionFault::Io)?;
    // Build the store before publication; cleanup after a failed build is best-effort.
    let (owner, admitted) = match build_in_temp(&temp, &request, seam.clone()) {
        Ok(built) => built,
        Err(error) => {
            return Err(cleanup_after_failure(&temp, error, &seam));
        }
    };
    seam.at(Event::Stage {
        at: StagePoint::Built,
        location: &temp,
        owner: &owner,
        admitted: &admitted,
    })
    .map_err(|error| ProvisionFault::Io(custody_io(error)))?;

    // The retained parent performs the no-replace claim. Any occupied destination
    // refuses; a loser cleans only its unpublished stage.
    match publication.publish(admitted.identity()) {
        Ok(()) => {}
        Err(error) => {
            let fault = match error {
                marrow_fs_journal::CustodyError::AlreadyExists { .. } => {
                    ProvisionFault::AlreadyProvisioned
                }
                error => ProvisionFault::Io(custody_io(error)),
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
            return Err(cleanup_after_failure(&temp, fault, &seam));
        }
    }
    seam.at(Event::Stage {
        at: StagePoint::Published,
        location: dest,
        owner: &owner,
        admitted: &admitted,
    })
    .map_err(|error| ProvisionFault::Io(custody_io(error)))?;
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
    admitted
        .sync(Step::Activation)
        .map_err(|source| ProvisionFault::ActivationUncertain { instance, source })?;
    Ok(())
}

fn cleanup_after_failure(stage: &Path, fault: ProvisionFault, seam: &Seam) -> ProvisionError {
    let cleanup = remove_unpublished_stage(stage, seam)
        .err()
        .map(|source| ProvisionCleanupFailure {
            stage: stage.to_path_buf(),
            source,
        });
    ProvisionError { fault, cleanup }
}

fn remove_unpublished_stage(stage: &Path, seam: &Seam) -> std::io::Result<()> {
    seam.at(Event::Removal { stage }).map_err(custody_io)?;
    std::fs::remove_dir_all(stage)
}

/// Build metadata, create the engine, then sync the stage before returning its owner.
fn build_in_temp(
    temp: &Path,
    request: &ProvisionRequest,
    seam: Seam,
) -> Result<
    (
        marrow_kernel::durable::PendingNativeStoreOwner,
        AdmittedStoreDir,
    ),
    ProvisionFault,
> {
    let owner = NativeStore::acquire_existing(temp)
        .map_err(|error| ProvisionFault::Io(std::io::Error::other(error)))?;
    let admitted = AdmittedStoreDir::admit_under_owner(&owner, seam)
        .map_err(ProvisionFault::Admission)?;
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
    admitted
        .sync(Step::ConstructionStage)
        .map_err(ProvisionFault::Admission)?;
    Ok((owner, admitted))
}

/// A held-open provisioned store: the native store the kernel drives, its envelope and head,
/// and the single-owner lock. An ordinary close releases the lock; an unresolved commit
/// quarantines it until process exit. The engine and lock are private and inseparable, and
/// the metadata is read only through the [`crate::NativeAttachment`] that pairs the store
/// with the image lifecycle admitted it for; this crate's admitted open is the sole
/// constructor.
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
    /// Prepare service without replacing the accepted layout or releasing ownership.
    pub(crate) fn into_service(self) -> Result<Self, OpenError> {
        let Self {
            owner,
            directory,
            envelope,
            head,
            head_digest,
        } = self;
        let owner = owner.into_service().map_err(|error| match error {
            NativeOwnerOpenError::Lock(error) => OpenError::Lock(error),
            NativeOwnerOpenError::Store(error) => OpenError::Store(error),
            NativeOwnerOpenError::Refused(refusal) => OpenError::Corruption {
                message: format!("service preparation refused the owner state: {refusal:?}"),
            },
        })?;
        Ok(Self {
            owner,
            directory,
            envelope,
            head,
            head_digest,
        })
    }

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
    /// hold was established — distinct from [`OpenError::NotProvisioned`] and
    /// [`OpenError::Incomplete`], which state what the directory holds.
    Access(StoreAccessError),
    /// The store directory exists but is missing a durable artifact.
    Incomplete,
    /// The store is held by another owner, or the lock could not be taken.
    Lock(NativeLockError),
    /// One of the store directory's own artifacts could not be admitted under the owner:
    /// its entry was refused, it was reachable under a second name, it changed while it was
    /// being read, or its bytes did not decode. A decode failure carries the typed
    /// [`FormatError`] so an unknown writer version (`store.format_version`) or an over-bound
    /// length (`store.limit`) is reported as itself, not flattened to corruption.
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
    pub fn code(&self) -> Code {
        match self {
            OpenError::NotProvisioned => Code::StoreIo,
            OpenError::ActivationRequired { .. } => Code::StoreActivationRequired,
            OpenError::Access(error) => error.code(),
            OpenError::Incomplete | OpenError::Corruption { .. } => Code::StoreCorruption,
            OpenError::Admission(error) => error.code(),
            OpenError::Lock(error) => error.code(),
            OpenError::Store(error) => error.code(),
            OpenError::Io(_) => Code::StoreIo,
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
    access: NativeOpenAccess,
    seam: Seam,
    admit: impl FnOnce(&LogicalHead) -> Result<NumberedProjection, R>,
) -> Result<OpenStore, AdmitError<R>> {
    let locked = LockedStore::acquire(dir, seam).map_err(AdmitError::Open)?;
    if locked.envelope.state != EnvelopeState::Active {
        return Err(AdmitError::Open(OpenError::ActivationRequired {
            instance: locked.envelope.metadata.instance,
        }));
    }
    locked.open(access, |head, _| admit(head))
}

/// Owner-held directory and envelope before engine opening.
/// Ordinary open requires Active; explicit recovery validates the retained state.
pub(crate) struct LockedStore {
    pending: marrow_kernel::durable::PendingNativeStoreOwner,
    directory: AdmittedStoreDir,
    pub(crate) envelope: EnvelopeRecord,
}

/// Exact service or a read-only binding transition awaiting logical admission.
pub(crate) enum OpenBinding {
    Active(OpenStore),
    Rebind {
        opened: OpenStore,
        names: crate::audit::Names,
    },
}

impl LockedStore {
    pub(crate) fn acquire(dir: &Path, seam: Seam) -> Result<Self, OpenError> {
        decide_before_locking(dir)?;
        let pending = NativeStore::acquire_existing(dir).map_err(|error| match error {
            NativeOwnerAcquireError::Io(error) => OpenError::Io(error),
            NativeOwnerAcquireError::Lock(error) => OpenError::Lock(error),
        })?;
        seam.at(Event::Locked {
            path: pending.directory(),
        })
        .map_err(|error| OpenError::Io(custody_io(error)))?;
        let directory =
            AdmittedStoreDir::admit_under_owner(&pending, seam).map_err(OpenError::Admission)?;
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
        access: NativeOpenAccess,
        admit: impl FnOnce(&LogicalHead, marrow_image::StoreHeadDigest) -> Result<NumberedProjection, R>,
    ) -> Result<OpenStore, AdmitError<R>> {
        let (head, digest) = decode_head(&self.directory).map_err(AdmitError::Open)?;
        let layout = admit(&head, digest).map_err(AdmitError::Refused)?;
        self.open_decoded(access, head, digest, layout)
            .map_err(AdmitError::Open)
    }

    /// Select ordinary service only for an exact binding. A transition must
    /// complete its logical admission through read-only access first.
    pub(crate) fn open_compatible(
        self,
        admission: crate::actor::ImageAdmission<'_>,
    ) -> Result<OpenBinding, crate::actor::LifecycleError> {
        use crate::actor::{BindingStrictness, LifecycleError};
        if self.envelope.state != EnvelopeState::Active {
            return Err(LifecycleError::Open(OpenError::ActivationRequired {
                instance: self.envelope.metadata.instance,
            }));
        }
        let (head, digest) = decode_head(&self.directory).map_err(LifecycleError::Open)?;
        let exact = admission.incoming() == &head.binding;
        let names = (!exact).then(|| admission.audit_names());
        let layout = admission
            .admit(&head, BindingStrictness::Compatible)
            .map_err(LifecycleError::Refused)?;
        match names {
            None => self
                .open_decoded(NativeOpenAccess::ReadWrite, head, digest, layout)
                .map(OpenBinding::Active)
                .map_err(LifecycleError::Open),
            Some(names) => {
                let opened = self
                    .open_decoded(NativeOpenAccess::ReadOnly, head, digest, layout)
                    .map_err(LifecycleError::Open)?;
                Ok(OpenBinding::Rebind { opened, names })
            }
        }
    }

    fn open_decoded(
        self,
        access: NativeOpenAccess,
        head: LogicalHead,
        head_digest: marrow_image::StoreHeadDigest,
        layout: NumberedProjection,
    ) -> Result<OpenStore, OpenError> {
        let Self {
            pending,
            directory,
            envelope,
        } = self;
        let envelope = envelope.metadata;
        let owner = pending
            .bind_and_open_existing(access, *envelope.instance.bytes(), || {
                Ok::<_, std::convert::Infallible>(layout)
            })
            .map_err(|error| match error {
                NativeOwnerOpenError::Lock(error) => OpenError::Lock(error),
                NativeOwnerOpenError::Refused(never) => match never {},
                NativeOwnerOpenError::Store(StoreError::Corruption { message }) => {
                    OpenError::Corruption { message }
                }
                NativeOwnerOpenError::Store(error) => OpenError::Store(error),
            })?;
        Ok(OpenStore {
            owner,
            directory,
            envelope,
            head,
            head_digest,
        })
    }
}

/// Everything an open settles before it takes the owner lock, and nothing else.
///
/// Acquiring the lock creates the `lock` entry and writes a marker into it, so the two
/// questions decided here are exactly the two whose answers would make that write wrong: a
/// platform where no store could be admitted at all, and a directory that is not a store.
/// Neither reads a store artifact's bytes, so neither can preempt the exclusion verdict a
/// contender is owed: a holder creates the lock entry before it locks, so the completeness
/// condition is unreachable while a holder is live.
///
/// A directory this process cannot examine reaches [`OpenError::Access`] rather than a claim
/// about what it holds. What the directory *contains* is read under the owner.
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
/// bounded ASCII component contains only a marker, process id and monotonic counter, with no
/// destination spelling embedded. Exclusive directory creation claims the candidate; the name
/// alone grants no ownership, because a prior process with the same pid may have left it
/// behind.
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

#[cfg(test)]
#[path = "provision_tests.rs"]
mod tests;
