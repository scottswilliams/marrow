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

/// A held owner lock. Dropping it releases the advisory lock; it truncates the lock body to
/// empty (the clean-shutdown signal) *only* once [`OwnerLock::mark_clean`] has recorded that a
/// healthy open completed. A lock dropped before that — an open that acquired the lock but
/// then failed its integrity audit or engine open — leaves the descriptor in place, so the
/// uncleanness persists and the next open audits again rather than skipping the check.
pub struct OwnerLock {
    file: File,
    /// Whether a healthy open completed under this lock. Set by [`OwnerLock::mark_clean`] on
    /// the open success path; only then does [`Drop`] truncate the body to the clean signal.
    clean_on_drop: bool,
}

/// The result of acquiring an owner lock: the held lock and whether the prior shutdown was
/// unclean (the lock body carried a stale owner descriptor, meaning a previous owner crashed
/// without truncating it). The open path runs an integrity audit when the prior shutdown was
/// unclean.
pub struct Acquired {
    /// The held lock.
    pub lock: OwnerLock,
    /// Whether a previous owner left a stale owner descriptor (an unclean prior shutdown).
    pub prior_unclean: bool,
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
    pub fn acquire(dir: &Path, instance: StoreInstanceId) -> Result<Acquired, LockError> {
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

        // We hold the lock. A non-empty prior body means the previous owner crashed without
        // truncating it — an unclean prior shutdown.
        let prior = read_owner(&mut file).map_err(LockError::Io)?;
        let prior_unclean = prior.is_some();

        // Record this owner: truncate, write the descriptor, and flush to disk.
        let owner = LockOwner {
            pid: std::process::id(),
            instance,
            acquired_unix_secs: now_unix_secs(),
        };
        write_owner(&mut file, &owner).map_err(LockError::Io)?;

        Ok(Acquired {
            lock: OwnerLock {
                file,
                clean_on_drop: false,
            },
            prior_unclean,
        })
    }

    /// Record that a healthy open completed under this lock, so a subsequent clean drop
    /// truncates the lock body to the clean-shutdown signal. Called on the open success path
    /// after any unclean-open integrity audit has passed. Until it is called, dropping the
    /// lock preserves the owner descriptor, so an open that fails after acquiring the lock
    /// leaves the store unclean and the next open re-runs the audit.
    pub fn mark_clean(&mut self) {
        self.clean_on_drop = true;
    }

    /// The owner descriptor currently written in the held lock body (this process).
    pub fn owner(&mut self) -> std::io::Result<Option<LockOwner>> {
        read_owner(&mut self.file)
    }
}

impl Drop for OwnerLock {
    fn drop(&mut self) {
        // Release the advisory lock by closing the file. Truncate the body to the clean signal
        // only when a healthy open completed (`mark_clean`); otherwise the owner descriptor
        // stays so the next acquirer still sees an unclean prior shutdown and audits. Truncate
        // is best-effort — a failure degrades only to an extra audit next time, never to lost
        // exclusion.
        if self.clean_on_drop {
            let _ = self.file.set_len(0);
            let _ = self.file.sync_all();
        }
    }
}

/// Read and decode the owner descriptor from the lock body, or `None` when the body is empty
/// or not a well-formed descriptor.
fn read_owner(file: &mut File) -> std::io::Result<Option<LockOwner>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(LockOwner::decode(&bytes))
}

/// Truncate the lock body and write `owner`'s descriptor, flushing to disk.
fn write_owner(file: &mut File, owner: &LockOwner) -> std::io::Result<()> {
    file.set_len(0)?;
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
}
