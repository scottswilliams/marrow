//! The persistent provision and open flow.
//!
//! Provision publishes a store *complete-or-not-at-all*: it builds the whole store in a
//! private sibling temporary directory and atomically renames it into place. A rename onto
//! an existing non-empty store directory fails, so exactly one provisioner wins a race and a
//! crash before the rename leaves only a temporary directory — the destination is never a
//! partially-formed store (the publication-uncertainty boundary). Preflight is strictly
//! non-creating, so probing a destination never leaves a file behind.
//!
//! Open requires a complete store, takes the single-owner lock (naming the live owner on
//! contention), decodes the envelope and head, and opens the engine through the path kernel.
//! When the prior shutdown was unclean (a stale owner descriptor in the lock) it runs a full
//! integrity audit.
//!
//! **Coverage honesty.** The unclean-open audit covers crash-path corruption only: the fast
//! open path does not re-verify page checksums, so an externally flipped bit in a
//! cleanly-closed store stays undetected here until the FR01 §2 data-root digest is populated
//! by a later full-walk operation (audit/backup/restore at F04+). No mitigation is claimed
//! for that class at open.

use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_kernel::durable::{
    CommitRecovery, DemandCoverage, DurableCommitState, InvocationGrant, NativeStore, ReadSession,
    SessionError, SessionHost, SiteSpec, StoreError, StoreSchema, TxnSession,
};

use crate::codec::FormatError;
use crate::durable_fs::{sync_dir, write_file};
use crate::envelope::StoreEnvelope;
use crate::head::LogicalHead;
use crate::instance::StoreInstanceId;
use crate::lock::{LockError, OwnerLock};
use crate::store_dir;

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
pub fn preflight(dir: &Path) -> Preflight {
    if !dir.is_dir() {
        return Preflight::Absent;
    }
    if store_dir::artifacts_present(dir) {
        Preflight::Complete
    } else {
        Preflight::Incomplete
    }
}

/// The inputs to a provision: the persisted envelope and logical head to publish, and the
/// schema and site tables the engine is created under. The caller (the lifecycle actor)
/// derives these from a verified image; F02a provisions an empty engine (no user data).
pub struct ProvisionRequest {
    pub envelope: StoreEnvelope,
    pub head: LogicalHead,
    pub schemas: Vec<StoreSchema>,
    pub sites: Vec<SiteSpec>,
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

    // Create the engine database through the kernel (the path kernel is the engine's only
    // consumer). Opening creates and stamps an empty store; dropping it closes the file,
    // which persists.
    let store = NativeStore::open_native(
        &store_dir::engine_path(temp),
        request.schemas.clone(),
        request.sites.clone(),
    )
    .map_err(ProvisionError::Store)?;
    drop(store);

    write_file(&store_dir::envelope_path(temp), &request.envelope.encode())
        .map_err(ProvisionError::Io)?;
    write_file(&store_dir::head_path(temp), &request.head.encode()).map_err(ProvisionError::Io)?;
    sync_dir(temp).map_err(ProvisionError::Io)?;
    Ok(())
}

/// A held-open provisioned store: the native store the kernel drives, its envelope and head,
/// and the single-owner lock. An ordinary close releases the lock; an unresolved commit
/// quarantines it until process exit. The engine and lock are private and inseparable; callers
/// receive only this session-host capability.
///
/// ```compile_fail
/// use marrow_kernel::durable::NativeStore;
/// use marrow_lifecycle::OpenStore;
/// fn detach_engine(mut opened: OpenStore) {
///     let _: &mut NativeStore = &mut opened.store;
///     let _lock = opened.lock;
/// }
/// ```
struct LockedNativeStore {
    store: Option<NativeStore>,
    lock: OwnerLock,
}

impl Drop for LockedNativeStore {
    fn drop(&mut self) {
        // An indeterminate engine verdict poisons the kernel handle before its affine
        // recovery fact leaves the transaction. If safe code loses that fact or drops the
        // owner without resolving it, the owner lock must not record a clean shutdown.
        if self
            .store
            .as_ref()
            .is_some_and(NativeStore::has_unresolved_recovery)
        {
            self.lock.quarantine_until_process_exit();
        }
    }
}

pub struct OpenStore {
    owner: LockedNativeStore,
    pub envelope: StoreEnvelope,
    pub head: LogicalHead,
    dir: PathBuf,
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
}

impl SessionHost for OpenStore {
    type Engine = <NativeStore as SessionHost>::Engine;

    fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, Self::Engine>, SessionError> {
        self.owner
            .store
            .as_mut()
            .expect("a live owner retains its native store")
            .read_session(grant, demand)
    }

    fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, Self::Engine>, SessionError> {
        self.owner
            .store
            .as_mut()
            .expect("a live owner retains its native store")
            .txn_session(grant, demand)
    }
}

impl OpenStore {
    /// Consume an indeterminate commit's sole affine fact while retaining the
    /// same owner lock across old-engine close, fresh reopen, full integrity
    /// audit, and exact witness comparison. A known result re-arms clean close
    /// and returns the freshly opened owner for later independent invocations;
    /// unknown retires it and leaves the descriptor unclean.
    pub fn resolve_recovery(
        mut self,
        recovery: CommitRecovery,
    ) -> (DurableCommitState, Option<Self>) {
        self.owner.lock.mark_unclean();
        drop(
            self.owner
                .store
                .take()
                .expect("recovery consumes the original native store"),
        );

        let engine_path = store_dir::engine_path(&self.dir);
        let mut reopened = match NativeStore::open_native_with_recovery_scope(
            &engine_path,
            self.schemas.clone(),
            self.sites.clone(),
            *self.envelope.instance.bytes(),
        ) {
            Ok(store) => store,
            Err(_) => {
                self.owner.lock.quarantine_until_process_exit();
                return (DurableCommitState::Unknown, None);
            }
        };
        if reopened.audit().is_err() {
            self.owner.lock.quarantine_until_process_exit();
            return (DurableCommitState::Unknown, None);
        }
        let state = reopened.resolve_recovery(recovery);
        if state == DurableCommitState::Unknown {
            self.owner.lock.quarantine_until_process_exit();
            return (state, None);
        }
        self.owner.store = Some(reopened);
        self.owner.lock.mark_clean();
        (state, Some(self))
    }
}

/// Why an open failed.
#[derive(Debug)]
pub enum OpenError {
    /// No store exists at the path.
    NotProvisioned,
    /// The store directory exists but is missing a durable artifact.
    Incomplete,
    /// The store is held by another owner, or the lock could not be taken.
    Lock(LockError),
    /// The persisted envelope or head bytes did not decode. Carries the typed
    /// [`FormatError`] so an unknown writer version (`store.format_version`) or an over-bound
    /// field (`store.limit`) is reported as itself, not flattened to corruption (FR01 §6).
    Decode(FormatError),
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
            OpenError::Incomplete | OpenError::Corruption { .. } => Code::StoreCorruption.as_str(),
            OpenError::Decode(error) => error.code(),
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
            OpenError::Incomplete => {
                write!(
                    f,
                    "the store directory is incomplete (a partially-formed store)"
                )
            }
            OpenError::Lock(error) => write!(f, "{error}"),
            OpenError::Decode(error) => write!(f, "the store {error}"),
            OpenError::Corruption { message } => write!(f, "the store is corrupt: {message}"),
            OpenError::Store(error) => write!(f, "the store engine could not be opened: {error}"),
            OpenError::Io(error) => write!(f, "opening the store failed: {error}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// A failure to open a store under an admission gate: either the ordinary open failed, or the
/// gate refused the presented image after the lock was taken and before any engine call. The
/// `R` is the caller's refusal type (the lifecycle actor's demand-exceeds-ceiling refusal).
pub(crate) enum AdmitError<R> {
    /// The store could not be opened (see [`OpenError`]).
    Open(OpenError),
    /// The admission gate refused the image with zero engine calls; the lock was released.
    Refused(R),
}

/// Open the complete store at `dir` under `schemas`/`sites`, taking the single-owner lock. A
/// non-complete directory is refused without opening; a store held by another owner returns
/// [`OpenError::Lock`] naming the owner. When the prior shutdown was unclean (a stale lock
/// descriptor) a full integrity audit runs, mapping a failure to corruption. On success the
/// returned [`OpenStore`] holds the lock for the store's whole open life.
pub fn open(
    dir: &Path,
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
) -> Result<OpenStore, OpenError> {
    open_admitted(dir, schemas, sites, |_| {
        Ok::<(), std::convert::Infallible>(())
    })
    .map_err(|error| match error {
        AdmitError::Open(open) => open,
        // The no-op admit never refuses.
        AdmitError::Refused(never) => match never {},
    })
}

/// Open the complete store at `dir`, running `admit` against the persisted head **after** the
/// single-owner lock is taken and **before** any engine call, so a refusal makes zero engine
/// calls and releases the lock on return. The lifecycle actor supplies an `admit` that
/// reconstructs the store's accepted deployment ceiling from the head and intersects it with
/// the presented image's demand; a refusal is surfaced as [`AdmitError::Refused`]. The plain
/// [`open`] passes a no-op admit.
pub(crate) fn open_admitted<R>(
    dir: &Path,
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
    admit: impl FnOnce(&LogicalHead) -> Result<(), R>,
) -> Result<OpenStore, AdmitError<R>> {
    match preflight(dir) {
        Preflight::Absent => return Err(AdmitError::Open(OpenError::NotProvisioned)),
        Preflight::Incomplete => return Err(AdmitError::Open(OpenError::Incomplete)),
        Preflight::Complete => {}
    }

    // Pin the complete store directory before any callback or engine open. Retaining caller-
    // relative text would allow a later cwd change to redirect indeterminate-commit recovery
    // while the original store's owner lock remained held.
    let dir = std::fs::canonicalize(dir).map_err(|error| AdmitError::Open(OpenError::Io(error)))?;
    if preflight(&dir) != Preflight::Complete {
        return Err(AdmitError::Open(OpenError::Incomplete));
    }

    let envelope = decode_envelope(&dir).map_err(AdmitError::Open)?;
    let head = decode_head(&dir).map_err(AdmitError::Open)?;

    // Take the single-owner lock before opening the engine; a second owner is named here.
    let acquired = OwnerLock::acquire(&dir, envelope.instance)
        .map_err(|error| AdmitError::Open(OpenError::Lock(error)))?;

    // Admission runs after the lock and before any engine open: a refusal drops the lock on
    // return and touches no engine, so a refused image never opens the store.
    admit(&head).map_err(AdmitError::Refused)?;

    let mut acquired = acquired;
    let mut store = NativeStore::open_native_with_recovery_scope(
        &store_dir::engine_path(&dir),
        schemas.clone(),
        sites.clone(),
        *envelope.instance.bytes(),
    )
    .map_err(|error| AdmitError::Open(OpenError::Store(error)))?;

    // Unclean prior shutdown: run the crash-path integrity audit. (See the module note — this
    // covers crash-path corruption only; it makes no claim about an externally flipped bit in
    // a cleanly-closed store.) On audit failure the lock drops un-marked, so the uncleanness
    // persists and the next open audits again rather than skipping the check.
    if acquired.prior_unclean {
        store.audit().map_err(|error| match error {
            StoreError::Corruption { message } => {
                AdmitError::Open(OpenError::Corruption { message })
            }
            other => AdmitError::Open(OpenError::Store(other)),
        })?;
    }

    // The open is healthy: record it so a clean close truncates the lock body to the clean
    // signal. Until this point a drop preserves the unclean descriptor.
    acquired.lock.mark_clean();

    Ok(OpenStore {
        owner: LockedNativeStore {
            store: Some(store),
            lock: acquired.lock,
        },
        envelope,
        head,
        dir,
        schemas,
        sites,
    })
}

fn decode_envelope(dir: &Path) -> Result<StoreEnvelope, OpenError> {
    let bytes = std::fs::read(store_dir::envelope_path(dir)).map_err(OpenError::Io)?;
    StoreEnvelope::decode(&bytes).map_err(OpenError::Decode)
}

fn decode_head(dir: &Path) -> Result<LogicalHead, OpenError> {
    let bytes = std::fs::read(store_dir::head_path(dir)).map_err(OpenError::Io)?;
    LogicalHead::decode(&bytes).map_err(OpenError::Decode)
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
    use marrow_kernel::codec::key::KeyScalar;
    use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
    use marrow_kernel::durable::{CommitResult, Durable, EntryValue, FieldSchema, SiteTarget};
    use marrow_kernel::equality::ValueDomain;

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

    #[test]
    fn preflight_classifies_absent_incomplete_complete_without_creating() {
        let base = std::env::temp_dir().join(format!(
            "marrow-lifecycle-preflight-{}-{}",
            std::process::id(),
            now_nonce(),
        ));
        let dir = base.join("store");

        // Absent: no directory. Preflight creates nothing.
        assert_eq!(preflight(&dir), Preflight::Absent);
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
        assert_eq!(preflight(&dir), Preflight::Incomplete);
        let before: Vec<_> = read_dir_names(&dir);
        assert_eq!(preflight(&dir), Preflight::Incomplete);
        assert_eq!(
            read_dir_names(&dir),
            before,
            "preflight must not add a file"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    fn create_empty_native_store(dir: &Path) {
        let store = NativeStore::open_native(&store_dir::engine_path(dir), Vec::new(), Vec::new())
            .expect("create native engine through the provisioning constructor");
        drop(store);
    }

    fn create_seeded_native_store(dir: &Path) -> (Vec<StoreSchema>, Vec<SiteSpec>) {
        let schemas = vec![StoreSchema {
            root_name: "audit".into(),
            key: vec![ScalarKind::Str],
            fields: vec![FieldSchema::scalar("value", ScalarKind::Int, true)],
            groups: Vec::new(),
            branches: Vec::new(),
            indexes: Vec::new(),
        }];
        let sites = vec![SiteSpec {
            root: 0,
            target: SiteTarget::WholePayload,
        }];
        let mut store =
            NativeStore::open_native(&store_dir::engine_path(dir), schemas.clone(), sites.clone())
                .expect("create seeded native engine");
        let mut txn = store
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("seed transaction");
        let site = txn.site(0);
        for index in 0..64 {
            txn.create_entry(
                &site,
                &[KeyScalar::Str(format!("k{index:03}"))],
                EntryValue {
                    groups: Vec::new(),
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(index)))],
                },
            )
            .expect("seed entry");
        }
        assert!(matches!(txn.commit(), CommitResult::Committed));
        drop(txn);
        drop(store);
        (schemas, sites)
    }

    #[cfg(unix)]
    fn assert_child_lock_probe(dir: &Path, instance: StoreInstanceId, expected: &str) {
        let status = std::process::Command::new(std::env::current_exe().expect("current test exe"))
            .args([
                "--exact",
                "provision::tests::owner_lock_probe_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("MARROW_COMMIT01_LOCK_DIR", dir)
            .env("MARROW_COMMIT01_LOCK_INSTANCE", instance.to_hex())
            .env("MARROW_COMMIT01_LOCK_EXPECTED", expected)
            .status()
            .expect("spawn competing owner probe");
        assert!(status.success(), "competing owner probe failed: {status}");
    }

    /// A subprocess-only helper for the owner-local recovery KATs. A normal unit-test run
    /// skips it; a broad ignored run without the coordination environment is a no-op.
    #[cfg(unix)]
    #[test]
    #[ignore = "child-process helper for owner-lock recovery fixtures"]
    fn owner_lock_probe_helper() {
        let Ok(dir) = std::env::var("MARROW_COMMIT01_LOCK_DIR") else {
            return;
        };
        let spelling = std::env::var("MARROW_COMMIT01_LOCK_INSTANCE").expect("probe instance");
        assert_eq!(spelling.len(), 32, "probe instance width");
        let mut bytes = [0u8; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&spelling[index * 2..index * 2 + 2], 16)
                .expect("decode probe instance");
        }
        let instance = StoreInstanceId::from_bytes(bytes);
        let expected = std::env::var("MARROW_COMMIT01_LOCK_EXPECTED").expect("probe expectation");
        match (
            expected.as_str(),
            OwnerLock::acquire(Path::new(&dir), instance),
        ) {
            ("locked", Err(LockError::StoreInUse { .. })) => {}
            ("available", Ok(mut acquired)) => {
                acquired.lock.mark_clean();
            }
            (expected, _) => {
                panic!("owner-lock probe did not observe expected state {expected}");
            }
        }
    }

    /// Recovery keeps the same real advisory lock while the old engine is closed, the
    /// existing file is reopened, and its complete integrity audit runs. Only after those
    /// fallible steps succeed may the lifecycle owner restore clean-on-drop.
    #[cfg(unix)]
    #[test]
    fn recovery_reopen_and_audit_keep_a_competing_process_excluded() {
        let scratch = ScratchDir::new("commit-recovery-owner");
        let instance = StoreInstanceId::from_bytes([0x51; 16]);
        create_empty_native_store(&scratch.0);

        let mut acquired = OwnerLock::acquire(&scratch.0, instance).expect("take owner lock");
        acquired.lock.mark_clean();
        acquired.lock.mark_unclean();
        assert_child_lock_probe(&scratch.0, instance, "locked");

        let mut reopened = NativeStore::open_native_with_recovery_scope(
            &store_dir::engine_path(&scratch.0),
            Vec::new(),
            Vec::new(),
            *instance.bytes(),
        )
        .expect("existing-only recovery reopen");
        assert_child_lock_probe(&scratch.0, instance, "locked");

        reopened.audit().expect("full recovery audit");
        assert_child_lock_probe(&scratch.0, instance, "locked");
        assert_ne!(
            std::fs::metadata(store_dir::lock_path(&scratch.0))
                .expect("unclean owner descriptor")
                .len(),
            0,
            "the owner descriptor stays unclean throughout recovery",
        );

        drop(reopened);
        acquired.lock.mark_clean();
        drop(acquired.lock);
        assert_child_lock_probe(&scratch.0, instance, "available");
    }

    /// Once recovery starts, a missing engine cannot be recreated and the lock remains in
    /// process-lifetime quarantine after the failed existing-only reopen.
    #[cfg(unix)]
    #[test]
    fn missing_engine_during_recovery_is_not_recreated_and_quarantines_the_owner() {
        let scratch = ScratchDir::new("commit-recovery-missing");
        let instance = StoreInstanceId::from_bytes([0x52; 16]);
        let engine = store_dir::engine_path(&scratch.0);
        create_empty_native_store(&scratch.0);

        let mut lock = OwnerLock::acquire(&scratch.0, instance)
            .expect("take owner lock")
            .lock;
        lock.mark_clean();
        lock.mark_unclean();
        std::fs::remove_file(&engine).expect("remove engine during recovery");

        assert!(
            NativeStore::open_native_with_recovery_scope(
                &engine,
                Vec::new(),
                Vec::new(),
                *instance.bytes(),
            )
            .is_err(),
            "recovery must refuse a missing engine",
        );
        assert!(!engine.exists(), "recovery must not recreate the engine");
        drop(lock);
        assert_child_lock_probe(&scratch.0, instance, "locked");
        assert_ne!(
            std::fs::metadata(store_dir::lock_path(&scratch.0))
                .expect("quarantined owner descriptor")
                .len(),
            0,
            "failed recovery must retain the unclean descriptor",
        );
    }

    /// A malformed replacement is neither stamped nor adopted during recovery, and the
    /// fail-stop lock remains held after the owner drops.
    #[cfg(unix)]
    #[test]
    fn malformed_replacement_during_recovery_is_unchanged_and_quarantines_the_owner() {
        let scratch = ScratchDir::new("commit-recovery-malformed");
        let instance = StoreInstanceId::from_bytes([0x53; 16]);
        let engine = store_dir::engine_path(&scratch.0);
        create_empty_native_store(&scratch.0);

        let mut lock = OwnerLock::acquire(&scratch.0, instance)
            .expect("take owner lock")
            .lock;
        lock.mark_clean();
        lock.mark_unclean();
        std::fs::remove_file(&engine).expect("remove original engine");
        let replacement = b"not a Marrow redb store";
        std::fs::write(&engine, replacement).expect("install malformed replacement");

        assert!(
            NativeStore::open_native_with_recovery_scope(
                &engine,
                Vec::new(),
                Vec::new(),
                *instance.bytes(),
            )
            .is_err(),
            "recovery must refuse a malformed replacement",
        );
        assert_eq!(
            std::fs::read(&engine).expect("read replacement"),
            replacement,
            "recovery must not stamp or rewrite a malformed replacement",
        );
        drop(lock);
        assert_child_lock_probe(&scratch.0, instance, "locked");
        assert_ne!(
            std::fs::metadata(store_dir::lock_path(&scratch.0))
                .expect("quarantined owner descriptor")
                .len(),
            0,
            "failed recovery must retain the unclean descriptor",
        );
    }

    /// A valid existing reopen that later fails its full integrity audit is also fail-stop:
    /// no clean marker is armed, and dropping the recovery owner retains cross-process
    /// exclusion until this process exits.
    #[cfg(unix)]
    #[test]
    fn failed_recovery_audit_quarantines_the_owner_and_keeps_the_marker_unclean() {
        use std::io::{Read, Seek, SeekFrom, Write};

        let scratch = ScratchDir::new("commit-recovery-audit-failure");
        let instance = StoreInstanceId::from_bytes([0x55; 16]);
        let engine = store_dir::engine_path(&scratch.0);
        let (schemas, sites) = create_seeded_native_store(&scratch.0);

        let mut lock = OwnerLock::acquire(&scratch.0, instance)
            .expect("take owner lock")
            .lock;
        lock.mark_clean();
        lock.mark_unclean();
        let mut reopened = NativeStore::open_native_with_recovery_scope(
            &engine,
            schemas,
            sites,
            *instance.bytes(),
        )
        .expect("existing-only recovery reopen");
        assert_child_lock_probe(&scratch.0, instance, "locked");

        // Mutate a spread of persisted bytes while the valid existing handle is open. The
        // storage adapter contains redb's response and the full audit must return a typed
        // failure rather than accepting the changed body or panicking out of this boundary.
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&engine)
            .expect("open engine body for hostile mutation");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read engine body");
        for offset in (0..bytes.len()).step_by(97) {
            bytes[offset] ^= 0xFF;
        }
        file.seek(SeekFrom::Start(0)).expect("rewind engine body");
        file.write_all(&bytes).expect("write hostile mutation");
        file.sync_all().expect("sync hostile mutation");
        drop(file);

        assert!(
            reopened.audit().is_err(),
            "a corrupted body must fail audit"
        );
        drop(reopened);
        drop(lock);
        assert_child_lock_probe(&scratch.0, instance, "locked");
        assert_ne!(
            std::fs::metadata(store_dir::lock_path(&scratch.0))
                .expect("quarantined owner descriptor")
                .len(),
            0,
            "an audit failure must retain the unclean descriptor",
        );
    }

    /// A failure before recovery quarantine preserves the unclean marker but releases the
    /// advisory lock, allowing a later audited owner to retry.
    #[cfg(unix)]
    #[test]
    fn ordinary_failed_open_before_quarantine_releases_the_lock_unclean() {
        let scratch = ScratchDir::new("ordinary-open-release");
        let instance = StoreInstanceId::from_bytes([0x54; 16]);

        let acquired = OwnerLock::acquire(&scratch.0, instance).expect("take owner lock");
        drop(acquired.lock);

        let retry = OwnerLock::acquire(&scratch.0, instance).expect("retry owner lock");
        assert!(
            retry.prior_unclean,
            "the failed ordinary open must leave the next owner an audit obligation",
        );
        drop(retry.lock);
        assert_child_lock_probe(&scratch.0, instance, "available");
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
