//! The one temporary-directory fixture this crate's tests share, in-crate and
//! integration alike. The in-crate suites reach this file through
//! `#[path = "../tests/common/scratch.rs"]`, so no case mints its own.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One private scratch directory, removed on drop.
///
/// The base directory and a `store` child under it both exist; the store child is the
/// directory a native engine owner is provisioned into, so a case can assert about the
/// engine file the owner would create without the owner having run.
pub struct Scratch {
    base: PathBuf,
}

impl Scratch {
    /// A fresh base directory tagged for the case that owns it. The tag, the process id,
    /// a clock nonce and a process-local counter together keep concurrent cases — in this
    /// binary and in a sibling one — off each other's paths.
    pub fn new(tag: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir().join(format!(
            "marrow-kernel-{tag}-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(base.join("store")).expect("create the scratch store directory");
        Self { base }
    }

    /// The base directory itself.
    pub fn path(&self) -> &Path {
        &self.base
    }

    /// The store directory under this base.
    pub fn store(&self) -> PathBuf {
        self.base.join("store")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}
