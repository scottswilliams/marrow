//! The provision report and approval over a real compiled durable image: the report carries
//! no identity hash, provision refuses without a matching approval, and an accepted
//! provision round-trips through open.

use std::path::Path;

use marrow_lifecycle::{
    AttachOutcome, PreparedImage, ProvisionApproval, ProvisionImageError, ProvisionReport,
    StoreInstanceId, attach, prepare, provision_image,
};
use marrow_verify::VerifiedImage;

use crate::support::Scratch;
use crate::support::ceiling::{image as compile, source_read_only};

fn prepared(image: &VerifiedImage) -> PreparedImage {
    let prepared = prepare(image.clone());
    assert!(prepared.projection().is_some(), "flat-executable");
    prepared
}

/// The absence gate for the "never a raw hash a human would retype" rule: nothing the report
/// renders is a 32- or 64-character hex identity string.
#[test]
fn the_report_carries_no_identity_hash() {
    let image = compile(&source_read_only());
    let dest = Path::new("/tmp/notes-store");
    let report = ProvisionReport::new(dest, &prepared(&image)).expect("report");
    let rendered = report.render();

    // No run of 32+ hex characters (a 16- or 32-byte identity spelled out).
    let mut run = 0usize;
    for ch in rendered.chars() {
        if ch.is_ascii_hexdigit() {
            run += 1;
            assert!(
                run < 32,
                "the report must not contain an identity hash: {rendered}"
            );
        } else {
            run = 0;
        }
    }
}

/// Provision refuses when the approval token does not match the report it would write — a
/// store is never provisioned without an auditable acceptance of the exact report.
#[test]
fn provision_refuses_without_a_matching_approval() {
    let image = compile(&source_read_only());
    let scratch = Scratch::new("provision-approval");

    let wrong = ProvisionApproval::from_token("not-the-right-token");
    let refused = provision_image(&scratch.store(), &prepared(&image), &wrong);
    assert!(
        matches!(refused, Err(ProvisionImageError::Unapproved)),
        "a mismatched approval is refused",
    );
    // Nothing was published.
    assert!(
        !scratch.store().exists(),
        "a refused provision writes no store"
    );
}

/// An accepted provision publishes the store and round-trips: attaching the same image reads
/// back the same store instance and active binding the image derives.
#[test]
fn an_accepted_provision_round_trips_through_attach() {
    let image = compile(&source_read_only());
    let scratch = Scratch::new("provision-approval");

    let prepared = prepared(&image);
    let report = ProvisionReport::new(&scratch.store(), &prepared).expect("report");
    let approval = ProvisionApproval::accept(&report);
    let provisioned = provision_image(&scratch.store(), &prepared, &approval).expect("provision");

    let attachment = match attach(&scratch.store(), prepared).expect("attach") {
        AttachOutcome::AlreadyActive(attachment) => attachment,
        AttachOutcome::Rebound { .. } => panic!("the provisioned image is already active"),
    };
    assert_eq!(
        attachment.envelope().instance,
        provisioned.instance,
        "the opened store carries the provisioned instance",
    );
    assert_eq!(
        attachment.head().binding,
        marrow_lifecycle::active_binding(&image),
        "the head records the image's active binding",
    );
    // The instance is a well-formed 32-hex spelling.
    assert_eq!(
        provisioned.instance.to_hex().len(),
        32,
        "the instance renders as 32 hex characters",
    );
    let _ = StoreInstanceId::from_bytes(*provisioned.instance.bytes());
}
