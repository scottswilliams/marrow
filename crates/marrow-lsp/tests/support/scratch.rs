//! The one temporary-directory fixture this crate's tests share, in-crate and
//! integration alike. The in-crate suites reach this file through
//! `#[path = "../tests/support/scratch.rs"]`, so no case mints its own.

use std::fs;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One private scratch directory, removed on drop — including through a failing
/// assertion, so a failed run leaves nothing for the next one to trip over.
///
/// The tag, the process id, a clock nonce and a process-local counter together keep
/// concurrent cases — in this binary and in a sibling one — off each other's paths.
pub struct TempDir {
    root: PathBuf,
}

impl TempDir {
    /// An empty directory.
    pub fn new(tag: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "marrow-lsp-{tag}-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).expect("create the scratch directory");
        Self { root }
    }

    /// A directory scaffolded as a capturable project: a manifest and `src/main.mw`
    /// carrying `main`.
    pub fn project(tag: &str, main: &str) -> Self {
        let dir = Self::new(tag);
        fs::create_dir_all(dir.root.join("src")).expect("create the source directory");
        dir.write("marrow.toml", "edition = \"2026\"\n");
        dir.write("src/main.mw", main);
        dir
    }

    /// Write `contents` at `relative`, creating the parent directories.
    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write fixture");
    }
}

/// The `file://` URI of `dir`.
pub fn uri_of(dir: &Path) -> String {
    let mut uri = String::from("file://");
    for component in dir.components() {
        if let Component::Normal(part) = component {
            uri.push('/');
            uri.push_str(part.to_str().expect("a UTF-8 scratch path"));
        }
    }
    uri
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}
