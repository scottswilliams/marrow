//! The provision report and approval over a real compiled durable image: the report names
//! the destination and the roots and renders no identity hash, an approval binds the exact
//! image and destination spelling it was accepted for, and an accepted provision round-trips
//! through open.

use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_lifecycle::{
    AttachOutcome, PreparedImage, ProvisionApproval, ProvisionImageError, ProvisionReport,
    StoreInstanceId, accepted_ceiling, attach, prepare, provision_image,
};
use marrow_verify::VerifiedImage;

use crate::support::ceiling::{image as compile, source_broadened, source_read_only};
use marrow_test_support::Scratch;

fn prepared(image: &VerifiedImage) -> PreparedImage {
    let prepared = prepare(image.clone());
    assert!(prepared.projection().is_some(), "flat-executable");
    prepared
}

/// The report carries the destination and the durable roots as typed values, and the
/// effects it presents are the image's demand: the read-only program reads and does not
/// write, the broadened one does both.
#[test]
fn the_report_names_the_destination_the_roots_and_the_effects() {
    let dest = Path::new("/tmp/notes-store");

    let read_only = compile(&source_read_only());
    let report = ProvisionReport::new(dest, &prepared(&read_only)).expect("report");
    assert_eq!(report.destination(), dest);
    assert_eq!(report.roots(), ["counters"]);
    assert!(report.reads());
    assert!(!report.writes());

    let broadened = compile(&source_broadened());
    let report = ProvisionReport::new(dest, &prepared(&broadened)).expect("report");
    assert_eq!(report.roots(), ["counters"]);
    assert!(report.reads());
    assert!(report.writes());
}

/// The absence gate for the "never a raw hash a human would retype" rule: nothing the report
/// renders is a 32- or 64-character hex identity string.
#[test]
fn the_report_renders_no_identity_hash() {
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

/// Asserts a typed provision refusal that published nothing at any of `destinations`.
fn assert_unapproved(
    refused: Result<marrow_lifecycle::Provisioned, ProvisionImageError>,
    destinations: &[&Path],
) {
    match refused {
        Err(error @ ProvisionImageError::Unapproved) => {
            assert_eq!(error.code(), Code::StoreProvisionUnapproved);
            assert_eq!(error.code().as_str(), "store.provision_unapproved");
        }
        Err(other) => panic!("expected an unapproved refusal, got {other:?}"),
        Ok(_) => panic!("a mismatched approval provisioned a store"),
    }
    for dest in destinations {
        assert!(
            !dest.exists(),
            "a refused provision writes no store at {dest:?}"
        );
    }
}

/// An approval binds the exact image it was accepted for. Each pair renders a byte-identical
/// report at the same destination, yet the images differ: in body only, and in which field
/// inside the same root the program writes (a different accepted ceiling). The approval for
/// A is refused for B, and no store is published.
#[test]
fn an_approval_for_one_image_is_refused_for_another_with_the_same_report() {
    let read_only = source_read_only();
    let body_b = read_only.replace("?? 0", "?? 1");
    let broadened = source_broadened();
    let effects_b = broadened.replace("slot.label = \"seen\"", "slot.value = 7");
    assert_ne!(
        read_only, body_b,
        "the body variant differs from its source"
    );
    assert_ne!(
        broadened, effects_b,
        "the effects variant differs from its source"
    );

    for (case, a_source, b_source) in [
        ("body", &read_only, &body_b),
        ("effects-within-roots", &broadened, &effects_b),
    ] {
        let scratch = Scratch::new("provision-approval");
        let (a_image, b_image) = (compile(a_source), compile(b_source));
        let (a, b) = (prepared(&a_image), prepared(&b_image));
        let report_a = ProvisionReport::new(scratch.store(), &a).expect("report A");
        let report_b = ProvisionReport::new(scratch.store(), &b).expect("report B");
        assert_eq!(
            report_a.render(),
            report_b.render(),
            "{case}: the reports read alike"
        );
        assert_ne!(
            a_image.image_id(),
            b_image.image_id(),
            "{case}: the images differ"
        );
        if case == "effects-within-roots" {
            assert_ne!(
                accepted_ceiling(&a_image),
                accepted_ceiling(&b_image),
                "{case}: the accepted ceilings differ",
            );
        }

        let approval_a = ProvisionApproval::accept(&report_a);
        assert_unapproved(
            provision_image(scratch.store(), &b, &approval_a),
            &[scratch.store()],
        );
    }
}

/// An approval binds the exact destination spelling it was accepted for, compared byte for
/// byte. The sibling row refuses another directory; the trailing-separator row kills a
/// component-normalized (`Path`) comparison; the non-UTF-8 row kills a lossy comparison
/// through a display or UTF-8 spelling, under which the two destinations read alike.
#[test]
fn an_approval_is_refused_at_any_other_destination_spelling() {
    let image = compile(&source_read_only());
    let prepared = prepared(&image);
    let scratch = Scratch::new("provision-approval");

    let mut trailing = scratch.store().as_os_str().to_os_string();
    trailing.push("/");
    let mut rows: Vec<(PathBuf, PathBuf)> = vec![
        (scratch.path().join("a"), scratch.path().join("b")),
        (scratch.store().to_path_buf(), PathBuf::from(trailing)),
    ];
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        rows.push((
            scratch
                .path()
                .join(OsStr::from_bytes(b"\xff"))
                .join("store"),
            scratch
                .path()
                .join(OsStr::from_bytes(b"\xfe"))
                .join("store"),
        ));
    }

    for (accepted_at, presented_at) in &rows {
        let report = ProvisionReport::new(accepted_at, &prepared).expect("report");
        let approval = ProvisionApproval::accept(&report);
        assert_unapproved(
            provision_image(presented_at, &prepared, &approval),
            &[accepted_at, presented_at],
        );
    }
}

/// An accepted provision publishes the store and round-trips: attaching the same image reads
/// back the same store instance and active binding the image derives.
#[test]
fn an_accepted_provision_round_trips_through_attach() {
    let image = compile(&source_read_only());
    let scratch = Scratch::new("provision-approval");

    let prepared = prepared(&image);
    let report = ProvisionReport::new(scratch.store(), &prepared).expect("report");
    let approval = ProvisionApproval::accept(&report);
    let provisioned = provision_image(scratch.store(), &prepared, &approval).expect("provision");

    let attachment = match attach(scratch.store(), prepared).expect("attach") {
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
