//! Opaque ownership of one native engine and its process owner lock.
//!
//! The store directory is the unit of ownership. This module alone derives the
//! `lock` and `store.redb` paths, acquires the advisory lock before admission or
//! engine open, and keeps that lock inseparable from the native engine. An
//! indeterminate commit irreversibly quarantines the lock until process exit.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use marrow_codes::Code;

use crate::engine::{ByteEngine, Cell, CommitOutcome, ReadView, WriteTxn};
use crate::error::StoreError;
use crate::redb::{NativeEngine, RedbTxn, RedbView};

/// The native engine file inside a Marrow store directory.
pub const NATIVE_ENGINE_FILE: &str = "store.redb";
/// The permanent owner-lock file inside a Marrow store directory.
pub const NATIVE_LOCK_FILE: &str = "lock";
/// The native engine format written and accepted by this build.
pub const NATIVE_ENGINE_FORMAT_VERSION: u32 = NativeEngine::FORMAT_VERSION;

const LOCK_MAGIC: &[u8; 4] = b"MWSL";
const LOCK_VERSION: u8 = 0;
const OWNER_BYTES: usize = 4 + 1 + 4 + 16 + 8;

/// The best-effort identity recorded for a live native-store owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeLockOwner {
    /// The owning process id.
    pub pid: u32,
    /// The lifecycle store instance bytes.
    pub instance: [u8; 16],
    /// The acquisition time in Unix-epoch seconds. This is forensic only.
    pub acquired_unix_secs: u64,
}

impl NativeLockOwner {
    fn encode(self) -> [u8; OWNER_BYTES] {
        let mut bytes = [0; OWNER_BYTES];
        bytes[0..4].copy_from_slice(LOCK_MAGIC);
        bytes[4] = LOCK_VERSION;
        bytes[5..9].copy_from_slice(&self.pid.to_be_bytes());
        bytes[9..25].copy_from_slice(&self.instance);
        bytes[25..33].copy_from_slice(&self.acquired_unix_secs.to_be_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != OWNER_BYTES || &bytes[0..4] != LOCK_MAGIC || bytes[4] != LOCK_VERSION {
            return None;
        }
        Some(Self {
            pid: u32::from_be_bytes(bytes[5..9].try_into().ok()?),
            instance: bytes[9..25].try_into().ok()?,
            acquired_unix_secs: u64::from_be_bytes(bytes[25..33].try_into().ok()?),
        })
    }
}

/// Why the native owner lock could not be acquired.
#[derive(Debug)]
pub enum NativeLockError {
    /// Another live owner holds the store.
    StoreInUse { owner: Option<NativeLockOwner> },
    /// The lock file or directory could not be read or synchronized.
    Io(std::io::Error),
}

impl NativeLockError {
    /// The stable diagnostic code for this lock failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::StoreInUse { .. } => Code::StoreLocked.as_str(),
            Self::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for NativeLockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StoreInUse { owner: Some(owner) } => write!(
                formatter,
                "the store is already open by process {}; close it, then retry",
                owner.pid,
            ),
            Self::StoreInUse { owner: None } => write!(
                formatter,
                "the store is already open by another process; close it, then retry",
            ),
            Self::Io(error) => write!(formatter, "the store lock could not be taken: {error}"),
        }
    }
}

impl std::error::Error for NativeLockError {}

/// A failure while acquiring and opening an existing native owner.
#[derive(Debug)]
pub enum NativeOwnerOpenError<R> {
    /// The store directory could not be pinned to a canonical path.
    Io(std::io::Error),
    /// The process owner lock could not be acquired.
    Lock(NativeLockError),
    /// The zero-capability admission callback refused the open.
    Refused(R),
    /// The existing native engine could not be opened or audited.
    Store(StoreError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropDisposition {
    PreserveUnclean,
    Clean,
    Quarantine,
}

struct OwnerLock {
    file: Option<File>,
    disposition: DropDisposition,
}

struct AcquiredLock {
    lock: OwnerLock,
    prior_unclean: bool,
}

impl OwnerLock {
    fn acquire(dir: &Path, instance: [u8; 16]) -> Result<AcquiredLock, NativeLockError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(NATIVE_LOCK_FILE))
            .map_err(NativeLockError::Io)?;

        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                let owner = read_owner(&mut file).map_err(NativeLockError::Io)?;
                return Err(NativeLockError::StoreInUse { owner });
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(NativeLockError::Io(error));
            }
        }

        let prior_unclean = file.metadata().map_err(NativeLockError::Io)?.len() != 0;
        let owner = NativeLockOwner {
            pid: std::process::id(),
            instance,
            acquired_unix_secs: now_unix_secs(),
        };
        write_owner(&mut file, owner).map_err(NativeLockError::Io)?;
        sync_dir(dir).map_err(NativeLockError::Io)?;

        Ok(AcquiredLock {
            lock: OwnerLock {
                file: Some(file),
                disposition: DropDisposition::PreserveUnclean,
            },
            prior_unclean,
        })
    }

    fn mark_clean(&mut self) {
        debug_assert_ne!(self.disposition, DropDisposition::Quarantine);
        if self.disposition != DropDisposition::Quarantine {
            self.disposition = DropDisposition::Clean;
        }
    }

    fn quarantine(&mut self) {
        self.disposition = DropDisposition::Quarantine;
    }
}

impl Drop for OwnerLock {
    fn drop(&mut self) {
        match self.disposition {
            DropDisposition::PreserveUnclean => {}
            DropDisposition::Clean => {
                if let Some(file) = &self.file {
                    let _ = file.set_len(0);
                    let _ = file.sync_all();
                }
            }
            DropDisposition::Quarantine => {
                if let Some(file) = self.file.take() {
                    std::mem::forget(file);
                }
            }
        }
    }
}

/// The only public native-engine capability. The raw engine and owner lock are
/// private and cannot be detached or replaced by safe dependents.
///
/// ```compile_fail
/// use marrow_store::NativeEngineOwner;
/// fn detach(owner: NativeEngineOwner) {
///     let _raw_engine = owner.engine;
///     let _raw_lock = owner.lock;
/// }
/// ```
pub struct NativeEngineOwner {
    engine: Option<NativeEngine>,
    lock: OwnerLock,
    directory: PathBuf,
}

impl NativeEngineOwner {
    /// Create and stamp a new native engine in `store_dir`, returning no live
    /// engine capability. An existing engine path is refused without opening or
    /// modifying it.
    pub fn provision(store_dir: &Path) -> Result<(), StoreError> {
        let directory = std::fs::canonicalize(store_dir).map_err(|error| StoreError::Io {
            op: "provision",
            message: error.to_string(),
        })?;
        let engine = NativeEngine::create_new(&directory.join(NATIVE_ENGINE_FILE))?;
        drop(engine);
        Ok(())
    }

    /// Acquire the owner lock, run a zero-capability admission callback, and
    /// open an existing write-capable engine. The callback runs after the lock
    /// is held and before any engine call.
    pub fn open_existing_admitted<R>(
        store_dir: &Path,
        instance: [u8; 16],
        admit: impl FnOnce() -> Result<(), R>,
    ) -> Result<Self, NativeOwnerOpenError<R>> {
        let directory = std::fs::canonicalize(store_dir).map_err(NativeOwnerOpenError::Io)?;
        let mut acquired =
            OwnerLock::acquire(&directory, instance).map_err(NativeOwnerOpenError::Lock)?;
        admit().map_err(NativeOwnerOpenError::Refused)?;

        let mut engine = NativeEngine::open_existing(&directory.join(NATIVE_ENGINE_FILE))
            .map_err(NativeOwnerOpenError::Store)?;
        if acquired.prior_unclean {
            engine
                .audit_integrity()
                .map_err(NativeOwnerOpenError::Store)?;
        }
        acquired.lock.mark_clean();
        Ok(Self {
            engine: Some(engine),
            lock: acquired.lock,
            directory,
        })
    }

    /// Irreversibly quarantine this owner's lock, close the old engine, reopen
    /// the existing file under the same lock, and run a full integrity audit.
    /// No successful result can restore clean-on-drop behavior.
    pub fn reopen_existing_and_audit(mut self) -> Result<Self, StoreError> {
        self.lock.quarantine();
        drop(self.engine.take());
        let mut engine = NativeEngine::open_existing(&self.directory.join(NATIVE_ENGINE_FILE))?;
        engine.audit_integrity()?;
        self.engine = Some(engine);
        Ok(self)
    }

    fn engine(&self) -> &NativeEngine {
        self.engine
            .as_ref()
            .expect("a live native owner retains its engine")
    }

    fn engine_mut(&mut self) -> &mut NativeEngine {
        self.engine
            .as_mut()
            .expect("a live native owner retains its engine")
    }
}

/// A coherent read view that cannot outlive its native owner.
pub struct NativeOwnerView<'a> {
    inner: RedbView<'a>,
}

impl ReadView for NativeOwnerView<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key)
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        self.inner.scan_after(prefix, cursor)
    }
}

/// A native transaction whose commit verdict controls the physical owner lock.
pub struct NativeOwnerTxn<'a> {
    inner: RedbTxn<'a>,
    lock: &'a mut OwnerLock,
}

impl ReadView for NativeOwnerTxn<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key)
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        self.inner.scan_after(prefix, cursor)
    }
}

impl WriteTxn for NativeOwnerTxn<'_> {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.inner.put(key, value)
    }

    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        self.inner.remove(key)
    }

    fn commit(self) -> CommitOutcome {
        let Self { inner, lock } = self;
        commit_and_latch(inner, lock)
    }
}

fn commit_and_latch<T: WriteTxn>(inner: T, lock: &mut OwnerLock) -> CommitOutcome {
    let outcome = inner.commit();
    if outcome == CommitOutcome::Indeterminate {
        lock.quarantine();
    }
    outcome
}

impl ByteEngine for NativeEngineOwner {
    type View<'a> = NativeOwnerView<'a>;
    type Txn<'a> = NativeOwnerTxn<'a>;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        Ok(NativeOwnerView {
            inner: self.engine().read_view()?,
        })
    }

    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        let Self { engine, lock, .. } = self;
        let inner = engine
            .as_mut()
            .expect("a live native owner retains its engine")
            .begin()?;
        Ok(NativeOwnerTxn { inner, lock })
    }

    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        self.engine().require_write_access(op)
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        self.engine_mut().audit_integrity()
    }
}

fn read_owner(file: &mut File) -> std::io::Result<Option<NativeLockOwner>> {
    if file.metadata()?.len() != OWNER_BYTES as u64 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = [0; OWNER_BYTES];
    file.read_exact(&mut bytes)?;
    Ok(NativeLockOwner::decode(&bytes))
}

fn write_owner(file: &mut File, owner: NativeLockOwner) -> std::io::Result<()> {
    file.set_len(OWNER_BYTES as u64)?;
    file.sync_all()?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&owner.encode())?;
    file.sync_all()
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(any(unix, windows))]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(any(unix, windows)))]
fn sync_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::redb::{Database, ReadableDatabase, TableDefinition};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "marrow-native-owner-{tag}-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("scratch directory");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn provision_is_create_only_and_existing_open_holds_the_lock() {
        let scratch = Scratch::new("provision");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        assert!(NativeEngineOwner::provision(&scratch.0).is_err());

        let owner = NativeEngineOwner::open_existing_admitted(&scratch.0, [7; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("open owner");
        assert!(matches!(
            NativeEngineOwner::open_existing_admitted(&scratch.0, [8; 16], || {
                Ok::<_, std::convert::Infallible>(())
            }),
            Err(NativeOwnerOpenError::Lock(
                NativeLockError::StoreInUse { .. }
            ))
        ));
        drop(owner);
        NativeEngineOwner::open_existing_admitted(&scratch.0, [8; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("clean close releases lock");
    }

    #[test]
    fn admission_runs_under_lock_before_engine_open() {
        let scratch = Scratch::new("admission");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let error = NativeEngineOwner::open_existing_admitted(&scratch.0, [9; 16], || {
            let competing = NativeEngineOwner::open_existing_admitted(&scratch.0, [10; 16], || {
                Ok::<_, std::convert::Infallible>(())
            });
            assert!(matches!(
                competing,
                Err(NativeOwnerOpenError::Lock(
                    NativeLockError::StoreInUse { .. }
                ))
            ));
            Err::<(), _>("refused")
        });
        assert!(matches!(
            error,
            Err(NativeOwnerOpenError::Refused("refused"))
        ));
        NativeEngineOwner::open_existing_admitted(&scratch.0, [10; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("a pre-engine refusal releases its non-quarantined lock");
    }

    #[test]
    fn existing_owner_open_refuses_missing_and_invalid_bodies_without_adopting_them() {
        let missing = Scratch::new("missing-existing");
        let missing_path = missing.0.join(NATIVE_ENGINE_FILE);
        for _ in 0..2 {
            assert!(matches!(
                NativeEngineOwner::open_existing_admitted(&missing.0, [0x21; 16], || {
                    Ok::<_, std::convert::Infallible>(())
                }),
                Err(NativeOwnerOpenError::Store(_))
            ));
            assert!(
                !missing_path.exists(),
                "an owner open must leave a missing engine path absent",
            );
        }

        for (tag, bytes) in [
            ("empty-existing", b"".as_slice()),
            ("bad-existing", b"not redb"),
        ] {
            let scratch = Scratch::new(tag);
            let path = scratch.0.join(NATIVE_ENGINE_FILE);
            std::fs::write(&path, bytes).expect("write invalid engine body");
            assert!(matches!(
                NativeEngineOwner::open_existing_admitted(&scratch.0, [0x22; 16], || {
                    Ok::<_, std::convert::Infallible>(())
                }),
                Err(NativeOwnerOpenError::Store(_))
            ));
            assert_eq!(
                std::fs::read(&path).expect("read refused engine body"),
                bytes,
                "an owner open must not rewrite or stamp an invalid engine body",
            );
        }

        let unstamped = Scratch::new("unstamped-existing");
        let path = unstamped.0.join(NATIVE_ENGINE_FILE);
        drop(Database::create(&path).expect("create an unstamped redb database"));
        assert!(matches!(
            NativeEngineOwner::open_existing_admitted(&unstamped.0, [0x23; 16], || {
                Ok::<_, std::convert::Infallible>(())
            }),
            Err(NativeOwnerOpenError::Store(_))
        ));
        let db = Database::open(&path).expect("reopen refused unstamped database");
        let read = db.begin_read().expect("read unstamped database");
        const META: TableDefinition<&str, u32> = TableDefinition::new("marrow.meta");
        assert!(
            matches!(
                read.open_table(META),
                Err(::redb::TableError::TableDoesNotExist(_))
            ),
            "an owner open must not stamp an otherwise valid foreign database",
        );
    }

    #[test]
    fn recovery_reopen_is_irreversibly_quarantined_after_success() {
        let scratch = Scratch::new("quarantine-success");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let owner = NativeEngineOwner::open_existing_admitted(&scratch.0, [11; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("open owner");
        let mut owner = owner
            .reopen_existing_and_audit()
            .expect("reopen and audit under retained lock");
        let mut txn = owner
            .begin()
            .expect("known recovery owner remains writable");
        txn.put(b"known", b"usable".to_vec())
            .expect("write through recovered owner");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        assert_eq!(
            owner
                .read_view()
                .expect("known recovery read view")
                .get(b"known")
                .expect("read through recovered owner"),
            Some(b"usable".to_vec()),
        );
        drop(owner);

        assert!(matches!(
            NativeEngineOwner::open_existing_admitted(&scratch.0, [12; 16], || {
                Ok::<_, std::convert::Infallible>(())
            }),
            Err(NativeOwnerOpenError::Lock(
                NativeLockError::StoreInUse { .. }
            ))
        ));
        assert_ne!(
            std::fs::metadata(scratch.0.join(NATIVE_LOCK_FILE))
                .expect("lock metadata")
                .len(),
            0,
            "quarantine retains the nonempty owner marker",
        );
    }

    #[test]
    fn failed_recovery_reopen_never_recreates_and_remains_quarantined() {
        let scratch = Scratch::new("quarantine-missing");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let owner = NativeEngineOwner::open_existing_admitted(&scratch.0, [13; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("open owner");
        let engine_path = scratch.0.join(NATIVE_ENGINE_FILE);
        std::fs::remove_file(&engine_path).expect("remove engine");
        assert!(owner.reopen_existing_and_audit().is_err());
        assert!(
            !engine_path.exists(),
            "recovery must not recreate the engine"
        );
        assert!(matches!(
            NativeEngineOwner::open_existing_admitted(&scratch.0, [14; 16], || {
                Ok::<_, std::convert::Infallible>(())
            }),
            Err(NativeOwnerOpenError::Lock(
                NativeLockError::StoreInUse { .. }
            ))
        ));
    }

    #[test]
    fn failed_recovery_reopen_never_adopts_invalid_replacements() {
        for (tag, replacement) in [
            ("quarantine-empty", b"".as_slice()),
            ("quarantine-malformed", b"not redb"),
        ] {
            let scratch = Scratch::new(tag);
            NativeEngineOwner::provision(&scratch.0).expect("provision");
            let owner = NativeEngineOwner::open_existing_admitted(&scratch.0, [0x31; 16], || {
                Ok::<_, std::convert::Infallible>(())
            })
            .expect("open owner");
            let engine_path = scratch.0.join(NATIVE_ENGINE_FILE);
            std::fs::remove_file(&engine_path).expect("remove live engine path");
            std::fs::write(&engine_path, replacement).expect("install invalid replacement");

            assert!(owner.reopen_existing_and_audit().is_err());
            assert_eq!(
                std::fs::read(&engine_path).expect("read refused replacement"),
                replacement,
                "recovery must not rewrite or stamp an invalid replacement",
            );
            assert!(matches!(
                NativeEngineOwner::open_existing_admitted(&scratch.0, [0x32; 16], || {
                    Ok::<_, std::convert::Infallible>(())
                }),
                Err(NativeOwnerOpenError::Lock(
                    NativeLockError::StoreInUse { .. }
                ))
            ));
        }
    }

    struct VerdictTxn(CommitOutcome);

    impl ReadView for VerdictTxn {
        fn get(&self, _key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
            Ok(None)
        }

        fn scan_after(&self, _prefix: &[u8], _cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
            Ok(Vec::new())
        }
    }

    impl WriteTxn for VerdictTxn {
        fn put(&mut self, _key: &[u8], _value: Vec<u8>) -> Result<(), StoreError> {
            Ok(())
        }

        fn remove(&mut self, _key: &[u8]) -> Result<(), StoreError> {
            Ok(())
        }

        fn commit(self) -> CommitOutcome {
            self.0
        }
    }

    #[test]
    fn transaction_wrapper_latches_only_an_indeterminate_engine_outcome() {
        for (tag, outcome, quarantined) in [
            ("confirmed", CommitOutcome::Confirmed, false),
            ("aborted", CommitOutcome::Aborted, false),
            ("indeterminate", CommitOutcome::Indeterminate, true),
        ] {
            let scratch = Scratch::new(tag);
            NativeEngineOwner::provision(&scratch.0).expect("provision");
            let mut owner = NativeEngineOwner::open_existing_admitted(&scratch.0, [17; 16], || {
                Ok::<_, std::convert::Infallible>(())
            })
            .expect("open owner");
            assert_eq!(
                commit_and_latch(VerdictTxn(outcome), &mut owner.lock),
                outcome,
                "the transaction wrapper commits once and preserves the engine verdict",
            );
            drop(owner);
            let retry = NativeEngineOwner::open_existing_admitted(&scratch.0, [18; 16], || {
                Ok::<_, std::convert::Infallible>(())
            });
            assert_eq!(
                matches!(
                    retry,
                    Err(NativeOwnerOpenError::Lock(
                        NativeLockError::StoreInUse { .. }
                    ))
                ),
                quarantined,
                "only Indeterminate may retain exclusion",
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_is_the_only_release_after_quarantine() {
        let scratch = Scratch::new("quarantine-process-exit");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "native_owner::tests::quarantine_child_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("MARROW_NATIVE_OWNER_QUARANTINE_DIR", &scratch.0)
            .status()
            .expect("run quarantine child");
        assert!(status.success(), "quarantine child failed: {status}");

        NativeEngineOwner::open_existing_admitted(&scratch.0, [16; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("process exit releases the quarantined descriptor");
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "child-process helper for process-lifetime quarantine"]
    fn quarantine_child_helper() {
        let Ok(path) = std::env::var("MARROW_NATIVE_OWNER_QUARANTINE_DIR") else {
            return;
        };
        let owner = NativeEngineOwner::open_existing_admitted(Path::new(&path), [15; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("child opens owner");
        let owner = owner
            .reopen_existing_and_audit()
            .expect("child enters quarantine");
        drop(owner);
        assert!(matches!(
            NativeEngineOwner::open_existing_admitted(Path::new(&path), [16; 16], || {
                Ok::<_, std::convert::Infallible>(())
            }),
            Err(NativeOwnerOpenError::Lock(
                NativeLockError::StoreInUse { .. }
            ))
        ));
    }
}
