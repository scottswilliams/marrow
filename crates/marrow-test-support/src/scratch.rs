//! The one temporary-directory fixture every test binary mints its files under.

use std::fs;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One private scratch directory under the system temporary root, removed on drop.
///
/// The tag, the process id, a clock nonce and a process-local counter together keep
/// concurrent cases — in this binary and in a sibling one — off each other's paths. The
/// base directory exists; the store path it hands out does not, so a caller can provision
/// or refuse to provision a store at it. A case that panics keeps its directory and prints
/// the path: the files beside a failure are the evidence for why it failed.
pub struct Scratch {
    root: PathBuf,
    store: PathBuf,
}

/// The manifest a scaffolded project carries: the sole supported edition, nothing else.
const MANIFEST: &str = "edition = \"2026\"\n";

impl Scratch {
    /// A fresh base directory tagged for the case that owns it.
    pub fn new(tag: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "marrow-{tag}-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).expect("create the scratch directory");
        let store = root.join("store");
        Self { root, store }
    }

    /// A directory scaffolded as a capturable project: a manifest and `src/main.mw`
    /// carrying `main`.
    pub fn project(tag: &str, main: &str) -> Self {
        let dir = Self::new(tag);
        dir.write("marrow.toml", MANIFEST);
        dir.write("src/main.mw", main);
        dir
    }

    /// The base directory itself.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// The conventional single-store path under this base. It does not exist.
    pub fn store(&self) -> &Path {
        &self.store
    }

    /// Write `contents` at `relative`, creating the parent directories.
    pub fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create the parent directory");
        }
        fs::write(path, contents).expect("write the fixture file");
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.root
    }
}

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("failed test fixture retained at {}", self.root.display());
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The `file://` URI of `dir`.
pub fn file_uri(dir: &Path) -> String {
    let mut uri = String::from("file://");
    for component in dir.components() {
        if let Component::Normal(part) = component {
            uri.push('/');
            uri.push_str(part.to_str().expect("a UTF-8 scratch path"));
        }
    }
    uri
}
