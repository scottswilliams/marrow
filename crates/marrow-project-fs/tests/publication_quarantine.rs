//! The publication capability this process drops when a durable claim is
//! abandoned.
//!
//! A dropped [`IdsPublicationPending`] leaves a durable marker whose live
//! handles are gone, so the process publishes nothing further for the rest of
//! its life. That capability drop is process-wide by construction, which is why
//! it is proven in a test binary of its own: a second test here would run after
//! the quarantine and observe it rather than its own subject.
//!
//! [`IdsPublicationPending`]: marrow_project_fs::IdsPublicationPending

use std::fs;
use std::path::{Path, PathBuf};

use marrow_codes::Code;
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
/// The interruption is built from a state the protocol cannot have produced and
/// cannot repair: a second live link to the committed ledger, which no phase of
/// the map admits. It appears after the plan is admitted, so the claim is
/// already durable when the map is read — exactly the affine case the pending
/// publication exists for. Dropping the pending value instead of recovering it
/// then refuses every later acquisition in this process, and the retained
/// marker keeps gating capture until a fresh process settles it.
#[test]
fn dropping_a_durable_claim_quarantines_publication_in_this_process() {
    let project = Project::new("quarantine");
    let first = project.plan("Book", 1);
    let mut guard =
        ProjectMetadataWriteGuard::acquire(project.path()).expect("the first write owner");
    assert!(matches!(
        guard
            .publish_ids(first)
            .expect("the first publication runs"),
        IdsPublishOutcome::Settled(IdsPublication::Published)
    ));
    let published = fs::read(project.meta().join("ids")).expect("the published ledger");

    // Admitted against the one-link ledger the plan captured.
    let second = project.plan("Shelf", 2);
    // A foreign second link to the committed ledger: the map admits the bound
    // generation at one link only, so the reading is off-map by the time the
    // claim is durable.
    fs::hard_link(project.meta().join("ids"), project.meta().join("ids.alias"))
        .expect("plant the second link");

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
    assert!(
        project.meta().join("ids.pending").exists(),
        "the durable marker is what makes the interruption affine"
    );

    // Abandoning it rather than recovering it drops this process's capability.
    drop(pending);
    drop(guard);

    let refusal = ProjectMetadataWriteGuard::acquire(project.path())
        .expect_err("a quarantined process acquires no write owner");
    assert_eq!(refusal.refusal(), IdsRefusal::Quarantined);
    assert_eq!(refusal.code(), Code::ProjectIdsPublicationPending);
    assert!(
        refusal.to_string().contains("publishes no more"),
        "the refusal names the dropped capability: {refusal}"
    );

    // Every byte is retained: the ledger is the generation that was committed,
    // and the marker still gates the front doors for a fresh process.
    assert_eq!(
        fs::read(project.meta().join("ids")).expect("the ledger"),
        published,
        "an abandoned claim installs nothing"
    );
    assert!(project.meta().join("ids.pending").exists());
    assert!(
        capture_project(project.path(), OverlaySnapshot::empty()).is_err(),
        "the retained marker keeps gating capture"
    );
}
