//! The one temporary-directory fixture this crate's tests share, in-crate and
//! integration alike. The in-crate suites reach this file through
//! `#[path = "../tests/common/scratch.rs"]`, so no case mints its own.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One private scratch directory, removed on drop.
///
/// The tag, the process id, a clock nonce and a process-local counter together keep
/// concurrent cases — in this binary and in a sibling one — off each other's paths.
pub struct Scratch {
    root: PathBuf,
}

impl Scratch {
    pub fn new(tag: &str) -> Self {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "marrow-fs-journal-{tag}-{}-{nonce}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("create scratch directory");
        Self { root }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
