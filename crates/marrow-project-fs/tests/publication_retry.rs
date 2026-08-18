//! The retry a pending publication offers reconciles before it classifies.
//!
//! A cleanup can move its object to the quarantine name and then fail before
//! the unlink, and the value handed back is what the CLI consumes to try again.
//! That retry is a fresh classification of the filesystem, so it must reconcile
//! exactly as a fresh process does; reading the map across an interrupted
//! removal would see the moved object's name absent and call a removal still
//! owed a removal already finished.
//!
//! This is a binary of its own for the same reason the quarantine kat is: a
//! durable claim reached here would be observed by any later test in the same
//! process. Nothing here drops one — the retry settles.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_project::{
    DurableIdentityId, IdentityAnchor, IdentityKind, LedgerPublicationPlan, META_DIR,
};
use marrow_project_fs::{
    IdsPublication, IdsPublishOutcome, IdsRefusal, OverlaySnapshot, ProjectMetadataWriteGuard,
    capture_project,
};

const MANIFEST: &[u8] = b"edition = \"2026\"\n";

/// A temporary project root removed on drop.
struct Project {
    root: PathBuf,
}

impl Project {
    fn new(tag: &str) -> Self {
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

    fn path(&self) -> &Path {
        &self.root
    }

    fn meta(&self) -> PathBuf {
        self.root.join(META_DIR)
    }

    /// One publication plan minting `anchor`, admitted against whatever the
    /// project currently carries.
    fn plan(&self, anchor: &str, id: u8) -> LedgerPublicationPlan {
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

/// A claim this process abandons costs it the capability to publish again.
///
/// The retry reconciles the quarantine before it reads the artifact map.
///
/// A durable claim is reached the way the quarantine kat reaches one — a
/// foreign second link makes the reading off-map after the claim — and the
/// interrupted removal is then planted under it: the staged successor at the
/// quarantine name with its own name absent. The retry must put that object
/// back and settle, not read the absent stage as a cleanup already done.
#[test]
fn a_retried_pending_publication_reconciles_the_quarantine_before_it_classifies() {
    let project = Project::new("retry-reconciles");
    let first = project.plan("Book", 1);
    let mut guard =
        ProjectMetadataWriteGuard::acquire(project.path()).expect("the first write owner");
    assert!(matches!(
        guard
            .publish_ids(first)
            .expect("the first publication runs"),
        IdsPublishOutcome::Settled(IdsPublication::Published)
    ));

    let second = project.plan("Shelf", 2);
    let alias = project.meta().join("ids.alias");
    fs::hard_link(project.meta().join("ids"), &alias).expect("plant the second link");

    let pending = match guard
        .publish_ids(second)
        .expect("the claim is durable, so the interruption is not an ordinary refusal")
    {
        IdsPublishOutcome::Pending(pending) => pending,
        IdsPublishOutcome::Settled(settled) => {
            panic!("an off-map reading after the claim must be reported as pending: {settled:?}")
        }
    };
    assert_eq!(pending.cause().refusal(), IdsRefusal::Corrupt);

    // The off-map second link goes, and the state an interrupted removal
    // leaves is planted in its place.
    fs::remove_file(&alias).expect("drop the foreign second link");
    fs::rename(
        project.meta().join("ids.publish.stage"),
        project.meta().join("ids.publish.quarantine"),
    )
    .expect("an interrupted removal left the successor at the quarantine name");

    let settled = pending.recover().expect("the retry settles");
    assert_eq!(settled, IdsPublication::Published);
    assert!(
        !project.meta().join("ids.publish.quarantine").exists(),
        "the retry reconciled the quarantine name"
    );
    assert!(
        !project.meta().join("ids.publish.stage").exists(),
        "the successor it put back was then installed and cleaned"
    );
    assert!(
        !project.meta().join("ids.pending").exists(),
        "the publication finished"
    );
}
