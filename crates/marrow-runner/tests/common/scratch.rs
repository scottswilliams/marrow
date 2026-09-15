//! Temporary store locations for the runner suites.
//!
//! A store path must be unique across concurrently running test binaries and across repeated
//! calls inside one: the process id separates binaries, a monotonic clock reading separates
//! runs of the same binary, and a process-local counter separates calls that land in the same
//! clock tick.

// Not every suite needs the owning form.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// An unused path under the system temporary directory, named after `tag`. The directory is
/// not created.
pub fn path(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "marrow-{tag}-{}-{nonce}-{counter}",
        std::process::id()
    ))
}

/// A created temporary directory that is removed when the handle drops.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        let dir = path(tag);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir)
    }

    pub fn dir(&self) -> &Path {
        &self.0
    }

    /// The conventional store location inside this directory.
    pub fn store(&self) -> PathBuf {
        self.0.join("store")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
