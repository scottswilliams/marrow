//! The owner lock: single-writer exclusion over a store directory, naming the live owner.
//!
//! A store admits exactly one live owner. [`OwnerLock::acquire`] takes an exclusive advisory
//! lock over `<dir>/lock` with the standard library's [`std::fs::File::try_lock`] (an
//! `flock`-style lock released when the file handle drops, including on process crash). A
//! second acquirer whose attempt would block reads the lock body and returns
//! [`LockError::StoreInUse`] naming the current owner — its process id, the store instance,
//! and when it acquired — so the contention is actionable, never opaque.
//!
//! The lock file is permanent: it is never unlinked (which would race the advisory lock and
//! break exclusion), so its *body* carries the clean/unclean signal. A clean drop truncates
//! the body to empty; an unclean shutdown (crash) leaves the owner descriptor behind. On the
//! next acquire, a non-empty prior body therefore means the previous owner did not shut down
//! cleanly, which the open path uses to decide whether to run an integrity audit.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use marrow_codes::Code;

use crate::durable_fs::sync_dir;
use crate::instance::StoreInstanceId;
use crate::store_dir;

/// The lock-body magic: "MWSL" (Marrow Store Lock).
const MAGIC: &[u8; 4] = b"MWSL";
/// The lock-body format version.
const LOCK_VERSION: u8 = 0x00;
/// The fixed size of an owner descriptor: magic(4) + version(1) + pid(4) + instance(16) +
/// acquired_unix_secs(8).
const OWNER_BYTES: usize = 4 + 1 + 4 + 16 + 8;

/// The live owner recorded in a held lock: which process holds the store, over which store
/// instance, and when it acquired the lock. Rendered to a caller so a contention names the
/// blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockOwner {
    /// The owning process id.
    pub pid: u32,
    /// The store instance the owner holds.
    pub instance: StoreInstanceId,
    /// The wall-clock second (Unix epoch) at which the owner acquired the lock. Forensic
    /// only — the lock's authority is the advisory `flock`, never this timestamp.
    pub acquired_unix_secs: u64,
}

impl LockOwner {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(OWNER_BYTES);
        out.extend_from_slice(MAGIC);
        out.push(LOCK_VERSION);
        out.extend_from_slice(&self.pid.to_be_bytes());
        out.extend_from_slice(self.instance.bytes());
        out.extend_from_slice(&self.acquired_unix_secs.to_be_bytes());
        out
    }

    /// Decode an owner descriptor, or `None` when the bytes are not a well-formed descriptor
    /// (an empty body after a clean shutdown, or a body a holder has not finished writing).
    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != OWNER_BYTES || &bytes[0..4] != MAGIC || bytes[4] != LOCK_VERSION {
            return None;
        }
        let pid = u32::from_be_bytes(bytes[5..9].try_into().ok()?);
        let instance = StoreInstanceId::from_bytes(bytes[9..25].try_into().ok()?);
        let acquired_unix_secs = u64::from_be_bytes(bytes[25..33].try_into().ok()?);
        Some(Self {
            pid,
            instance,
            acquired_unix_secs,
        })
    }
}

/// A held owner lock. Dropping it normally releases the advisory lock; it truncates the lock
/// body to empty (the clean-shutdown signal) *only* once [`OwnerLock::mark_clean`] has recorded
/// that a healthy open completed. A lock dropped before that — an open that acquired the lock
/// but then failed its integrity audit or engine open — leaves the descriptor in place, so the
/// uncleanness persists and the next open audits again rather than skipping the check. Losing
/// an indeterminate-commit recovery fact instead retires the lock until process exit, because
/// no later session in that process may proceed without the affine fact.
pub(crate) struct OwnerLock {
    file: Option<File>,
    drop_disposition: DropDisposition,
}

/// What dropping an owner lock may do with its descriptor and advisory lock.
///
/// A failed ordinary open preserves the nonempty crash marker but releases exclusion so a
/// later audited open can retry. A healthy owner clears the marker and releases exclusion.
/// Once indeterminate-commit recovery begins, neither unwinding nor an early return may admit
/// another session in the same process, so the descriptor is quarantined until process exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropDisposition {
    PreserveUnclean,
    Clean,
    Quarantine,
}

/// The result of acquiring an owner lock: the held lock and whether the prior shutdown was
/// unclean (the lock body carried a stale owner descriptor, meaning a previous owner crashed
/// without truncating it). The open path runs an integrity audit when the prior shutdown was
/// unclean.
pub(crate) struct Acquired {
    /// The held lock.
    pub(crate) lock: OwnerLock,
    /// Whether a previous owner left a stale owner descriptor (an unclean prior shutdown).
    pub(crate) prior_unclean: bool,
}

/// Why acquiring an owner lock failed.
#[derive(Debug)]
pub enum LockError {
    /// The store is already held by another live owner. Names the owner where the lock body
    /// is readable; `owner` is `None` only in the narrow window where a holder has taken the
    /// lock but not yet written its descriptor.
    StoreInUse { owner: Option<LockOwner> },
    /// An I/O operation on the lock file failed.
    Io(std::io::Error),
}

impl LockError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            LockError::StoreInUse { .. } => Code::StoreLocked.as_str(),
            LockError::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::StoreInUse { owner: Some(owner) } => write!(
                f,
                "the store is already open by process {} (store instance {}); close it, then \
                 retry",
                owner.pid,
                owner.instance.to_hex(),
            ),
            LockError::StoreInUse { owner: None } => write!(
                f,
                "the store is already open by another process; close it, then retry",
            ),
            LockError::Io(error) => write!(f, "the store lock could not be taken: {error}"),
        }
    }
}

impl std::error::Error for LockError {}

impl OwnerLock {
    /// Acquire the exclusive owner lock over the store at `dir` on behalf of store `instance`
    /// held by this process. Returns the held lock and whether the prior shutdown was
    /// unclean, or [`LockError::StoreInUse`] naming the live owner. The lock file is created
    /// if absent and never unlinked; the advisory lock — not the file's presence — is the
    /// exclusion.
    pub(crate) fn acquire(dir: &Path, instance: StoreInstanceId) -> Result<Acquired, LockError> {
        let path = store_dir::lock_path(dir);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(LockError::Io)?;

        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                // Held by another live owner: read its descriptor to name it. A short body is
                // the narrow pre-write window, reported as an unnamed owner.
                let owner = read_owner(&mut file).map_err(LockError::Io)?;
                return Err(LockError::StoreInUse { owner });
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(LockError::Io(error)),
        }

        // We hold the lock. Body presence, not successful descriptor decoding, is the
        // durable clean/unclean bit: a crash during descriptor overwrite may leave fixed-
        // length malformed bytes, and those must still force the next integrity audit.
        let prior_unclean = file.metadata().map_err(LockError::Io)?.len() != 0;

        // Record this owner: truncate, write the descriptor, and flush to disk.
        let owner = LockOwner {
            pid: std::process::id(),
            instance,
            acquired_unix_secs: now_unix_secs(),
        };
        write_owner(&mut file, &owner).map_err(LockError::Io)?;
        // The descriptor's fsync does not make creation of `<dir>/lock` durable. Sync the
        // parent directory before admitting the owner so a machine failure cannot erase the
        // only persisted unclean marker. Doing this for an existing lock file is harmless
        // and keeps the acquisition path independent of a racy pre-open existence check.
        sync_dir(dir).map_err(LockError::Io)?;

        Ok(Acquired {
            lock: OwnerLock {
                file: Some(file),
                drop_disposition: DropDisposition::PreserveUnclean,
            },
            prior_unclean,
        })
    }

    /// Record that a healthy open completed under this lock, so a subsequent clean drop
    /// truncates the lock body to the clean-shutdown signal. Called on the open success path
    /// after any unclean-open integrity audit has passed. Until it is called, dropping the
    /// lock preserves the owner descriptor, so an open that fails after acquiring the lock
    /// leaves the store unclean and the next open re-runs the audit.
    pub(crate) fn mark_clean(&mut self) {
        self.drop_disposition = DropDisposition::Clean;
    }

    /// Enter fail-stop quarantine before indeterminate-commit recovery performs any
    /// fallible work. The already-flushed owner descriptor remains in the lock body, and
    /// dropping this value — including during unwinding — retains the advisory lock until
    /// process exit. Only a successful classification may call [`Self::mark_clean`] later.
    pub(crate) fn mark_unclean(&mut self) {
        self.drop_disposition = DropDisposition::Quarantine;
    }

    /// Retain this advisory lock until the operating system closes the process's file
    /// descriptors. This is the fail-stop path for a lost affine recovery fact: the owner
    /// descriptor remains nonempty, and neither this process nor another can start a later
    /// session over the unclassified store. The attached runner exits immediately after this
    /// path; retaining the one descriptor is deliberate process-lifetime quarantine, not a
    /// recovery ledger.
    pub(crate) fn quarantine_until_process_exit(&mut self) {
        self.drop_disposition = DropDisposition::Quarantine;
    }
}

impl Drop for OwnerLock {
    fn drop(&mut self) {
        match self.drop_disposition {
            DropDisposition::PreserveUnclean => {
                // Closing releases exclusion but leaves the descriptor nonempty, so the next
                // ordinary owner must audit before it can mark the store clean.
            }
            DropDisposition::Clean => {
                // Truncation is best-effort: failure degrades to an extra audit, never to lost
                // exclusion. The file closes normally after this arm.
                if let Some(file) = &self.file {
                    let _ = file.set_len(0);
                    let _ = file.sync_all();
                }
            }
            DropDisposition::Quarantine => {
                // Leaking this one descriptor is deliberate: the operating system closes it at
                // process exit. Taking it before `forget` makes a second Drop path impossible.
                if let Some(file) = self.file.take() {
                    std::mem::forget(file);
                }
            }
        }
    }
}

/// Read and decode the fixed owner descriptor, or `None` when the body is empty, partial,
/// overlong, or malformed. The raw nonempty bit is read separately for crash detection; this
/// function is only the best-effort live-owner naming path and never allocates from file size.
fn read_owner(file: &mut File) -> std::io::Result<Option<LockOwner>> {
    if file.metadata()?.len() != OWNER_BYTES as u64 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = [0u8; OWNER_BYTES];
    file.read_exact(&mut bytes)?;
    Ok(LockOwner::decode(&bytes))
}

/// Reserve the full nonempty body before overwriting it, then flush the descriptor. A crash
/// before or during the write therefore leaves either the previous nonempty body or a
/// fixed-length partial body; it never creates the empty clean signal. Only clean `Drop`
/// truncates to zero.
fn write_owner(file: &mut File, owner: &LockOwner) -> std::io::Result<()> {
    file.set_len(OWNER_BYTES as u64)?;
    // Persist the nonempty reservation before overwriting its bytes. A process or machine
    // failure after this point can leave malformed contents, but not the empty clean marker.
    file.sync_all()?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&owner.encode())?;
    file.sync_all()
}

/// The current Unix-epoch second. Forensic only; a clock skew never affects exclusion, which
/// the advisory lock alone provides.
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("marrow-lock-{tag}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// A lock dropped WITHOUT `mark_clean` — an open that acquired the lock but then failed
    /// (its integrity audit or engine open) — leaves the owner descriptor in place, so the
    /// next acquire still reports `prior_unclean`. Only a `mark_clean` drop truncates to the
    /// clean signal. Without this, one failed unclean-open audit would erase the crash marker
    /// and the next open would skip the audit over a still-corrupt store.
    #[test]
    fn an_unmarked_drop_preserves_uncleanness_and_mark_clean_clears_it() {
        let dir = scratch_dir("latch");
        let instance = StoreInstanceId::from_bytes([0x07; 16]);

        // A fresh lock: clean prior. Drop it WITHOUT marking clean (a failed open).
        let acquired = OwnerLock::acquire(&dir, instance).expect("acquire");
        assert!(!acquired.prior_unclean, "a fresh store is clean");
        drop(acquired.lock);

        // The next acquire sees the preserved descriptor: still unclean.
        let again = OwnerLock::acquire(&dir, instance).expect("reacquire");
        assert!(
            again.prior_unclean,
            "a lock dropped without mark_clean must leave the store unclean",
        );

        // A healthy open marks clean; its drop truncates the body to the clean signal.
        let mut lock = again.lock;
        lock.mark_clean();
        drop(lock);
        let clean = OwnerLock::acquire(&dir, instance).expect("acquire after clean");
        assert!(
            !clean.prior_unclean,
            "a mark_clean drop truncates to the clean signal",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn owner_descriptor_byte_layout_is_frozen() {
        let owner = LockOwner {
            pid: 0x01020304,
            instance: StoreInstanceId::from_bytes([0xAB; 16]),
            acquired_unix_secs: 0x00000000_DEADBEEF,
        };
        let bytes = owner.encode();
        assert_eq!(bytes.len(), OWNER_BYTES);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"MWSL");
        expected.push(0x00);
        expected.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]); // pid, big-endian
        expected.extend_from_slice(&[0xAB; 16]); // instance
        expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF]); // secs
        assert_eq!(bytes, expected);

        // Round trip, and reject a truncated or foreign body.
        assert_eq!(LockOwner::decode(&bytes), Some(owner));
        assert_eq!(
            LockOwner::decode(&[]),
            None,
            "an empty body is a clean prior"
        );
        assert_eq!(LockOwner::decode(&bytes[..OWNER_BYTES - 1]), None);
        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert_eq!(LockOwner::decode(&bad_magic), None);
    }

    #[test]
    fn every_nonempty_or_partial_prior_body_is_unclean() {
        for (tag, bytes) in [
            ("one-byte", vec![0xA5]),
            ("reserved", vec![0; OWNER_BYTES]),
            ("overlong", vec![0x5A; OWNER_BYTES + 1]),
        ] {
            let dir = scratch_dir(tag);
            std::fs::write(store_dir::lock_path(&dir), bytes).expect("seed malformed lock body");
            let acquired = OwnerLock::acquire(&dir, StoreInstanceId::from_bytes([0x19; 16]))
                .expect("acquire malformed prior");
            assert!(
                acquired.prior_unclean,
                "a {tag} nonempty body must force the integrity audit",
            );
            let mut lock = acquired.lock;
            assert!(
                read_owner(
                    lock.file
                        .as_mut()
                        .expect("a live owner lock retains its descriptor"),
                )
                .expect("read current owner")
                .is_some(),
                "acquisition must replace the partial body with a complete descriptor",
            );
            lock.mark_clean();
            drop(lock);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[cfg(unix)]
    #[test]
    fn quarantined_owner_blocks_same_process_reopen_until_exit() {
        let dir = scratch_dir("quarantined-owner");
        let instance = StoreInstanceId::from_bytes([0x2A; 16]);
        let mut lock = OwnerLock::acquire(&dir, instance)
            .expect("first owner")
            .lock;
        lock.mark_clean();
        lock.mark_unclean();
        drop(lock);

        assert!(matches!(
            OwnerLock::acquire(&dir, instance),
            Err(LockError::StoreInUse { .. })
        ));
        assert_ne!(
            std::fs::metadata(store_dir::lock_path(&dir))
                .expect("quarantined descriptor")
                .len(),
            0,
            "quarantine must preserve the durable unclean marker",
        );

        // Unix permits unlinking the path while the deliberately leaked descriptor retains
        // its lock on the now-unlinked inode; the OS closes that descriptor at process exit.
        let _ = std::fs::remove_dir_all(&dir);
    }
}
