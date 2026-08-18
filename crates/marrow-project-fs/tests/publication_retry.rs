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

mod common;

use common::Project;
use marrow_project_fs::{IdsPublication, IdsPublishOutcome, IdsRefusal, ProjectMetadataWriteGuard};

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
