//! Shared helpers for the store integration tests.
#[cfg(feature = "native")]
use std::path::{Path, PathBuf};
#[cfg(feature = "native")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "native")]
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;

#[cfg(feature = "native")]
static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "native")]
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

#[cfg(feature = "native")]
impl TempDir {
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        let base = std::env::temp_dir();
        let process = std::process::id();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for attempt in 0..128u64 {
            let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("{prefix}-{process}-{nonce}-{counter}-{attempt}"));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique temp dir",
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(feature = "native")]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The catalog id with `hex` zero-padded into the canonical 32-character body, the
/// one construction convention the store tests build ids by.
pub fn catalog_id(hex: &str) -> CatalogId {
    CatalogId::new(format!("cat_{hex:0>32}")).unwrap()
}

/// Walk a child layer from `first` to exhaustion, following `next` from each child,
/// and collect the children in cursor order. The four store child layers differ
/// only in which first/next cursor methods they call, so they share this walk.
pub fn collect_children(
    first: impl FnOnce() -> Result<Option<SavedKey>, StoreError>,
    next: impl Fn(&SavedKey) -> Result<Option<SavedKey>, StoreError>,
) -> Vec<SavedKey> {
    let mut children = Vec::new();
    let mut cursor = first().expect("first child");
    while let Some(child) = cursor {
        cursor = next(&child).expect("next child");
        children.push(child);
    }
    children
}
