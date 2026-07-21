//! The on-disk layout of a provisioned store directory.
//!
//! A provisioned store is a private owner-only directory holding four files:
//!
//! ```text
//! <dir>/store.redb   the ordered-byte engine database
//! <dir>/envelope     the StoreEnvelope bytes (store instance + writer/engine provenance)
//! <dir>/head         the LogicalHead bytes (active binding + reserved slots + head map)
//! <dir>/lock         the owner lock (advisory; its body names the live owner)
//! ```
//!
//! A store is COMPLETE only when the directory exists and all three of `store.redb`,
//! `envelope`, and `head` are present; the lock is transient (held while a process owns the
//! store, left behind after an unclean shutdown). This module owns the file-name constants
//! and the path helpers only — no I/O policy, which lives in [`crate::provision`].

use std::path::{Path, PathBuf};

/// The engine database file name within a store directory.
pub const ENGINE_FILE: &str = "store.redb";
/// The persisted envelope file name within a store directory.
pub const ENVELOPE_FILE: &str = "envelope";
/// The logical-head file name within a store directory.
pub const HEAD_FILE: &str = "head";
/// The owner lock file name within a store directory.
pub const LOCK_FILE: &str = "lock";

/// The engine database path within `dir`.
pub fn engine_path(dir: &Path) -> PathBuf {
    dir.join(ENGINE_FILE)
}

/// The envelope file path within `dir`.
pub fn envelope_path(dir: &Path) -> PathBuf {
    dir.join(ENVELOPE_FILE)
}

/// The logical-head file path within `dir`.
pub fn head_path(dir: &Path) -> PathBuf {
    dir.join(HEAD_FILE)
}

/// The owner lock file path within `dir`.
pub fn lock_path(dir: &Path) -> PathBuf {
    dir.join(LOCK_FILE)
}

/// Whether all three durable artifacts (engine, envelope, head) are present in `dir`. A
/// store missing any one is incomplete — never published as complete — so a crash mid-build
/// (which leaves a temp directory, never a partial destination) can never be mistaken for a
/// finished store. The lock is deliberately excluded: it is transient, not a completeness
/// signal.
pub fn artifacts_present(dir: &Path) -> bool {
    engine_path(dir).is_file() && envelope_path(dir).is_file() && head_path(dir).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_directory_file_names_are_frozen() {
        // The store-directory layout is a durability contract; these names are frozen.
        assert_eq!(ENGINE_FILE, "store.redb");
        assert_eq!(ENVELOPE_FILE, "envelope");
        assert_eq!(HEAD_FILE, "head");
        assert_eq!(LOCK_FILE, "lock");

        let dir = Path::new("/stores/app");
        assert_eq!(engine_path(dir), Path::new("/stores/app/store.redb"));
        assert_eq!(envelope_path(dir), Path::new("/stores/app/envelope"));
        assert_eq!(head_path(dir), Path::new("/stores/app/head"));
        assert_eq!(lock_path(dir), Path::new("/stores/app/lock"));
    }
}
