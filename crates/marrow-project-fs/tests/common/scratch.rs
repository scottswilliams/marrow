//! The one temporary-directory fixture this crate's tests share, in-crate and
//! integration alike. The in-crate suites reach this file through
//! `#[path = "../tests/common/scratch.rs"]`, so no case mints its own.
//!
//! Nothing here names a Marrow type: `tests/consumer.rs` is the external-consumer
//! test and imports only `marrow_project_fs` and the standard library.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One temporary directory holding one or more project trees, removed on drop.
///
/// The tag, the process id, a clock nonce and a process-local counter together keep
/// concurrent cases — in this binary and in a sibling one — off each other's paths.
pub struct TempDir {
    root: PathBuf,
}

/// The manifest a scaffolded project carries.
const MANIFEST: &[u8] = b"edition = \"2026\"\n";

impl TempDir {
    /// An empty directory.
    pub fn new(tag: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "marrow-project-fs-{tag}-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).expect("create temp dir");
        Self { root }
    }

    /// A directory scaffolded as the smallest capturable project: a manifest and an
    /// empty `src/main.mw`.
    pub fn project(tag: &str) -> Self {
        let dir = Self::new(tag);
        fs::create_dir_all(dir.root.join("src")).expect("create the source directory");
        dir.write("marrow.toml", MANIFEST);
        dir.write("src/main.mw", b"");
        dir
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Write `contents` at `relative`, creating the parent directories.
    pub fn write(&self, relative: &str, contents: &[u8]) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write fixture");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}
