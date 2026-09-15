//! The one temporary-directory fixture the lifecycle crate's suites share, in-crate
//! and integration alike.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One private scratch directory, removed on drop.
///
/// The base directory exists; the paths it hands out do not, so a caller can
/// provision or refuse to provision a store at one.
pub struct Scratch {
    base: PathBuf,
    store: PathBuf,
}

impl Scratch {
    /// A fresh base directory tagged for the case that owns it.
    pub fn new(tag: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir().join(format!(
            "marrow-lifecycle-{tag}-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&base).expect("create the scratch base");
        let store = base.join("store");
        Self { base, store }
    }

    /// The base directory itself.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// The conventional single-store path under this base. It does not exist.
    pub fn dir(&self) -> &Path {
        &self.store
    }

    /// The same path, owned.
    pub fn store(&self) -> PathBuf {
        self.store.clone()
    }

    /// A named store path under this base. It does not exist.
    pub fn named_store(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "failed lifecycle fixture retained at {}",
                self.base.display()
            );
            return;
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}
