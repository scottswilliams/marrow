//! The temporary project fixture the publication quarantine binary uses.
//!
//! That binary exists because the publication capability it drives is
//! process-wide: a durable claim dropped in one test is observable by every
//! later test in the same process, so it runs alone.

use std::path::{Path, PathBuf};

use marrow_project::{
    DurableIdentityId, IdentityAnchor, IdentityKind, LedgerPublicationPlan, META_DIR,
};
use marrow_project_fs::{OverlaySnapshot, capture_project};

use marrow_test_support::Scratch;

/// A temporary project root, over the crate's one scratch fixture.
pub struct Project {
    dir: Scratch,
}

impl Project {
    pub fn new(tag: &str) -> Self {
        Self {
            dir: Scratch::project(tag, ""),
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn meta(&self) -> PathBuf {
        self.dir.path().join(META_DIR)
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
