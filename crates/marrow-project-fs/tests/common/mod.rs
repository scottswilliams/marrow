//! The temporary project fixture the publication integration binaries share.
//!
//! Each binary here exists because the publication capability it drives is
//! process-wide: a durable claim reached in one test is observable by every
//! later test in the same process. The fixture is the same in both, so it lives
//! once.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_project::{
    DurableIdentityId, IdentityAnchor, IdentityKind, LedgerPublicationPlan, META_DIR,
};
use marrow_project_fs::{OverlaySnapshot, capture_project};

const MANIFEST: &[u8] = b"edition = \"2026\"\n";

/// A temporary project root removed on drop.
pub struct Project {
    root: PathBuf,
}

impl Project {
    pub fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "marrow-idpub01-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create temp project");
        fs::write(root.join("marrow.toml"), MANIFEST).expect("write manifest");
        fs::write(root.join("src/main.mw"), b"").expect("write source");
        Self { root }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn meta(&self) -> PathBuf {
        self.root.join(META_DIR)
    }

    /// One publication plan minting `anchor`, admitted against whatever the
    /// project currently carries.
    pub fn plan(&self, anchor: &str, id: u8) -> LedgerPublicationPlan {
        let input = capture_project(self.path(), OverlaySnapshot::empty())
            .expect("the fixture project captures");
        input
            .admit_identity_mints_with(
                IdentityAnchor::new(IdentityKind::Product, anchor),
                Vec::new(),
                |count| {
                    Ok::<_, std::convert::Infallible>(
                        (0..count)
                            .map(|index| {
                                let mut bytes = [0u8; 16];
                                bytes[0] = id;
                                bytes[15] = u8::try_from(index).expect("one candidate");
                                DurableIdentityId::from_bytes(bytes)
                            })
                            .collect(),
                    )
                },
            )
            .expect("the mint is admitted")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}
