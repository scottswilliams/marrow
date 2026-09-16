//! The one temporary-directory fixture this crate's tests share.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One private scratch directory, removed on drop.
///
/// The tag, the process id, a clock nonce and a process-local counter together keep
/// concurrent cases off each other's paths: the process id separates test binaries,
/// the clock separates runs of one binary, and the counter separates calls that land
/// in the same clock tick.
pub(crate) struct Scratch {
    root: PathBuf,
}

impl Scratch {
    pub(crate) fn new(tag: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "marrow-store-{tag}-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&root).expect("create the scratch directory");
        Self { root }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A failed case keeps its directory: the engine file, the lock marker and any
        // debris beside them are the evidence for why it failed.
        if std::thread::panicking() {
            eprintln!("failed store fixture retained at {}", self.root.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
