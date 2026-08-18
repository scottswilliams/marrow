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

use marrow_codes::Code;
mod common;

use common::Project;
use marrow_project_fs::{
    IdsPublication, IdsPublishOutcome, IdsRefusal, OverlaySnapshot, ProjectMetadataWriteGuard,
    capture_project,
};

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
