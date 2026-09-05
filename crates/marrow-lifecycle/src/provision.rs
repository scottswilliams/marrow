//! The persistent provision and open flow.
//!
//! Provision publishes a store *complete-or-not-at-all*: it builds the whole store in a
//! private sibling temporary directory and atomically renames it into place. A rename onto
//! an existing non-empty store directory fails, so exactly one provisioner wins a race and a
//! crash before the rename leaves only a temporary directory — the destination is never a
//! partially-formed store (the publication-uncertainty boundary). Preflight is strictly
//! non-creating, so probing a destination never leaves a file behind.
//!
//! Open takes the single-owner lock first (naming the live owner on contention), and only
//! then reads the store directory at all: completeness, the envelope, and the head are one
//! admission snapshot taken under that owner, each artifact admitted from the retained
//! directory within its own byte ceiling. Deciding exclusion ahead of every read is what
//! keeps a contender's verdict independent of the holder's bytes — a malformed, truncated,
//! or deleted artifact cannot turn "the store is locked" into a decode or completeness
//! error. The engine opens last, through the path kernel, and when the prior shutdown was
//! unclean (a stale owner descriptor in the lock) it runs a full integrity audit.
//!
//! **Coverage honesty.** The unclean-open audit covers crash-path corruption only: the fast
//! open path does not re-verify page checksums, so an externally flipped bit in a
//! cleanly-closed store stays undetected here until the FR01 §2 data-root digest is populated
//! by a later full-walk operation (audit/backup/restore at F04+). No mitigation is claimed
//! for that class at open.

use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_kernel::durable::{
    CommitRecovery, DemandCoverage, DurableCommitState, InvocationGrant, NativeOwnerAcquireError,
    NativeOwnerOpenError, NativeStore, ReadSession, SessionError, SessionHost, StoreError,
    StoreProjection, TxnSession,
};

use crate::durable_fs::{sync_dir, write_file};
use crate::envelope::StoreEnvelope;
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
/// caller (the lifecycle actor) derives these from a verified image; F02a provisions an
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
pub enum ProvisionError {
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
}

impl ProvisionError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            ProvisionError::AlreadyProvisioned => Code::StoreLocked.as_str(),
            ProvisionError::Admission(error) => error.code(),
            ProvisionError::Store(error) => error.code(),
            ProvisionError::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionError::AlreadyProvisioned => {
                write!(f, "a store already exists at the destination")
            }
            ProvisionError::Admission(error) => write!(f, "{error}"),
            ProvisionError::Store(error) => {
                write!(f, "the store engine could not be created: {error}")
            }
            ProvisionError::Io(error) => write!(f, "provisioning failed: {error}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

/// Provision a fresh store at `dest`, publishing it complete-or-not-at-all. Builds the whole
/// store in a private sibling temporary directory (owner-only, mode `0700`) — the engine
/// database created through the path kernel, then the envelope and head bytes written and
/// flushed — then atomically renames it onto `dest`. A rename onto an existing non-empty
/// destination fails, so exactly one racing provisioner wins and the destination is never
/// left partial. On any failure before the rename, the temporary directory is removed, so a
/// failed provision leaves no published file.
pub fn provision(dest: &Path, request: ProvisionRequest) -> Result<Provisioned, ProvisionError> {
    let instance = request.envelope.instance;
    let temp = temp_sibling(dest);
    // Build the whole store in the temp directory; on any error, remove it and surface.
    match build_in_temp(&temp, &request) {
        Ok(()) => {}
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(error);
        }
    }

    // The one-winner atomic claim: rename the fully-formed temp directory onto the
    // destination. A rename onto an existing non-empty directory fails (the destination is
    // an existing store or another winner's claim), so the loser cleans up its temp and
    // reports the destination taken.
    match std::fs::rename(&temp, dest) {
        Ok(()) => {}
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp);
            // A destination that now exists is another winner (or a prior store), not our
            // I/O fault.
            return if dest.exists() {
                Err(ProvisionError::AlreadyProvisioned)
            } else {
                Err(ProvisionError::Io(error))
            };
        }
    }
    // Make the new directory entry durable in the parent.
    if let Some(parent) = dest.parent() {
        sync_dir(parent).map_err(ProvisionError::Io)?;
    }
    Ok(Provisioned { instance })
}

/// Build the store's artifacts in the private temporary directory `temp`: create the
/// owner-only directory, create the engine database through the path kernel, write the
/// envelope and head bytes, and flush every file and the directory to disk.
fn build_in_temp(temp: &Path, request: &ProvisionRequest) -> Result<(), ProvisionError> {
    create_private_dir(temp).map_err(ProvisionError::Io)?;

    // Provisioning is the sole create/stamp path. It returns no engine or store
    // capability, so the newly created body cannot escape without an owner lock.
    NativeStore::provision(temp).map_err(ProvisionError::Store)?;

    write_file(&store_dir::envelope_path(temp), &request.envelope.encode())
        .map_err(ProvisionError::Io)?;
    write_file(&store_dir::head_path(temp), &request.head.encode()).map_err(ProvisionError::Io)?;
    sync_dir(temp).map_err(ProvisionError::Io)?;

    // Fail closed where an open never could succeed. An open admits the store directory as a
    // descriptor, and the platforms that support those operations are narrower than the
    // platforms this crate builds for; publishing a store that could never be opened again
    // would move that refusal from the provision to every later open.
    AdmittedStoreDir::admit(temp)
        .map(drop)
        .map_err(ProvisionError::Admission)
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
    pub(crate) envelope: StoreEnvelope,
    pub(crate) head: LogicalHead,
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
            envelope,
            head,
        } = self;
        let (state, owner) = owner.resolve_recovery(recovery);
        (
            state,
            owner.map(|owner| Self {
                owner,
                envelope,
                head,
            }),
        )
    }
}

/// Why an open failed.
#[derive(Debug)]
pub enum OpenError {
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
/// descriptor) a full integrity audit runs, mapping a failure to corruption. On success the
/// returned [`OpenStore`] holds the lock for the store's whole open life. The lifecycle actor
/// and the importer supply an `admit` that admits the presented image against the head; a
/// refusal is surfaced as [`AdmitError::Refused`]. This is the only constructor of an
/// [`OpenStore`].
pub(crate) fn open_admitted<R>(
    dir: &Path,
    projection: StoreProjection,
    admit: impl FnOnce(&LogicalHead) -> Result<(), R>,
) -> Result<OpenStore, AdmitError<R>> {
    decide_before_locking(dir).map_err(AdmitError::Open)?;

    // Acquiring pins the store directory to its canonical path and takes the lock without
    // naming an instance or touching the engine. Retaining caller-relative text would allow a
    // later cwd change to redirect indeterminate-commit recovery while the original store's
    // owner lock remained held.
    let pending = NativeStore::acquire_existing(dir).map_err(|error| match error {
        NativeOwnerAcquireError::Io(error) => AdmitError::Open(OpenError::Io(error)),
        NativeOwnerAcquireError::Lock(error) => {
            AdmitError::Open(OpenError::Lock(LockError::from(error)))
        }
    })?;
    let dir = pending.directory().to_path_buf();

    // From here to the engine open, one owner holds the store: the completeness verdict, the
    // envelope, and the head are one admission snapshot rather than three separately racing
    // observations, all read through one retained directory descriptor.
    let admitted = AdmittedStoreDir::admit(&dir)
        .map_err(|error| AdmitError::Open(OpenError::Admission(error)))?;
    if !admitted
        .is_complete()
        .map_err(|error| AdmitError::Open(OpenError::Admission(error)))?
    {
        return Err(AdmitError::Open(OpenError::Incomplete));
    }
    let envelope = decode_envelope(&admitted).map_err(AdmitError::Open)?;
    let mut admitted_head = None;

    // The kernel capsule binds the instance the envelope named into the owner marker, runs
    // admission, and composes the existing-only engine open with any inherited audit without
    // exposing its lower owner.
    let owner = pending
        .bind_and_open_existing(*envelope.instance.bytes(), projection, || {
            // Read and admit the mutable logical head under the same owner. The callback
            // receives no store capability.
            let head = decode_head(&admitted).map_err(Ok)?;
            admit(&head).map_err(Err)?;
            admitted_head = Some(head);
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

    Ok(OpenStore {
        owner,
        envelope,
        head: admitted_head.expect("a successful open completed head admission"),
    })
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

fn decode_envelope(dir: &AdmittedStoreDir) -> Result<StoreEnvelope, OpenError> {
    let bytes = dir
        .read(Artifact::Envelope, crate::envelope::file_ceiling)
        .map_err(OpenError::Admission)?;
    StoreEnvelope::decode(&bytes)
        .map_err(|error| OpenError::Admission(AdmissionError::format(StoreEntry::Envelope, error)))
}

fn decode_head(dir: &AdmittedStoreDir) -> Result<LogicalHead, OpenError> {
    let bytes = dir
        .read(Artifact::Head, crate::head::file_ceiling)
        .map_err(OpenError::Admission)?;
    LogicalHead::decode(&bytes)
        .map_err(|error| OpenError::Admission(AdmissionError::format(StoreEntry::Head, error)))
}

/// A private sibling temporary directory for building a store before its atomic claim: the
/// destination's own name prefixed with a recognizable marker plus the process id and a
/// monotonic counter, so concurrent provisioners never collide and a leaked temp (from a
/// crash before the rename) is identifiable and never mistaken for a published store.
pub(crate) fn temp_sibling(dest: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = dest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "store".to_string());
    let temp_name = format!(".{name}.provisioning.{}.{counter}", std::process::id());
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
mod tests {
    use super::*;
    use crate::head::ActiveBinding;
    use crate::headmap::HeadMap;
    use marrow_image::LedgerIdBytes;

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
                    image_format_version: 0,
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

    /// An open with a no-op admission gate, over the rootless shape.
    fn open_unadmitted(dir: &Path) -> Result<OpenStore, OpenError> {
        open_admitted(dir, rootless(), |_| Ok::<(), std::convert::Infallible>(())).map_err(
            |error| match error {
                AdmitError::Open(error) => error,
                AdmitError::Refused(never) => match never {},
            },
        )
    }

    fn open_owner(dir: &Path, instance: [u8; 16]) -> NativeStore {
        NativeStore::acquire_existing(dir)
            .expect("acquire the owner")
            .bind_and_open_existing(instance, rootless(), || {
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
        let opened = open_admitted(&store, rootless(), |head| {
            contended = Some(match open_unadmitted(&store) {
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
            std::fs::write(store_dir::envelope_path(&store), replacement.encode())
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
