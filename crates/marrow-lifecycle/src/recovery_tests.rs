//! Explicit recovery over real directories: which prefixes it adopts, what it preserves,
//! and where an interrupted recovery stops.

use super::*;
use crate::actor::{AdmissionRefusal, attach_observed};
use crate::provision::provision_observed;
use crate::seam::Event;
use crate::store_dir::{AdmittedStoreDir, Body};
use crate::test_support::{
    SOURCE, Scratch, compile_bytes, cut, cut_parent_sync, mutate_at, populate_counter, request,
};
use crate::{LogicalHead, active_binding, prepare, provision};

/// One deferred change to the store, run at the step a test arms it for.
type Mutation = Box<dyn FnOnce(&AdmittedStoreDir) + Send>;

/// Replace `artifact` with `bytes` under the retained descriptor and sync the directory,
/// as a writer racing the sequence would.
fn replace_synced(store: &Path, dir: &AdmittedStoreDir, artifact: Artifact, bytes: &[u8]) {
    dir.replace(artifact, bytes).expect("replace artifact");
    crate::durable_fs::sync_dir(store).expect("sync directory");
}

/// A store left at the published-but-unconfirmed prefix: provisioned, with its parent
/// barrier cut, so ordinary attach demands activation.
fn provision_unconfirmed(
    store: &Path,
    image: &marrow_verify::VerifiedImage,
    instance: StoreInstanceId,
) {
    assert!(matches!(
        provision_observed(store, request(image, instance), cut_parent_sync()),
        Err(crate::ProvisionError {
            fault: crate::ProvisionFault::PublicationUncertain { .. },
            ..
        })
    ));
}

#[test]
fn logical_corruption_refuses_recovery_before_preserving_debris_or_activating() {
    let populated = Scratch::new("recovery");
    let target = Scratch::new("recovery");
    let integer_image = marrow_verify::verify(&compile_bytes(&format!(
        "{SOURCE}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n"
    ))).expect("verify");
    let boolean_image = marrow_verify::verify(&compile_bytes(
        "resource Counter { required value: bool }\nstore ^counters[id: int]: Counter\npub fn readValue(n: int): bool { return ^counters[n].value ?? false }\n",
    )).expect("verify");
    provision(
        &populated.store(),
        request(&integer_image, StoreInstanceId::draw().expect("instance")),
    )
    .expect("integer store");
    populate_counter(&populated.store(), &integer_image);
    provision(
        &target.store(),
        request(&boolean_image, StoreInstanceId::draw().expect("instance")),
    )
    .expect("boolean store");
    std::fs::copy(
        populated.store().join(crate::ENGINE_FILE),
        target.store().join(crate::ENGINE_FILE),
    )
    .expect("physically valid engine with incompatible saved values");
    let head = std::fs::read(target.store().join(crate::HEAD_FILE)).expect("head");
    let envelope = std::fs::read(target.store().join(crate::ENVELOPE_FILE)).expect("envelope");
    std::fs::write(target.store().join("envelope.replacing"), b"unexplained").expect("debris");
    let error = recover(&target.store(), prepare(boolean_image)).expect_err("logical corruption");
    assert!(error.preserved.is_empty());
    assert_eq!(error.code(), Code::StoreCorruption);
    let RecoveryFault::Logical(report) = error.fault else {
        panic!("physical validation must reach the logical refusal");
    };
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == Code::StoreAuditUndecodable)
    );
    assert_eq!(
        std::fs::read(target.store().join(crate::HEAD_FILE)).expect("head unchanged"),
        head
    );
    assert_eq!(
        std::fs::read(target.store().join(crate::ENVELOPE_FILE)).expect("no activation"),
        envelope
    );
    assert_eq!(
        std::fs::read(target.store().join("envelope.replacing")).expect("debris unmoved"),
        b"unexplained"
    );
}

#[test]
fn recovery_activates_an_actual_uncertain_publication_without_replaying_the_head() {
    let scratch = Scratch::new("recovery");
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    let instance = StoreInstanceId::draw().expect("instance");
    provision_unconfirmed(&scratch.store(), &image, instance);
    let head_before = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
    assert!(matches!(
        crate::attach(&scratch.store(), prepare(image.clone())),
        Err(crate::LifecycleError::Open(
            OpenError::ActivationRequired { .. }
        ))
    ));
    let receipt =
        recover(&scratch.store(), prepare(image.clone())).expect("recover published store");
    assert_eq!(
        receipt,
        RecoveredStore {
            instance,
            image_id: image.image_id(),
            preserved: Vec::new(),
        }
    );
    assert_eq!(
        std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head"),
        head_before
    );
    assert!(
        crate::audit(&scratch.store(), prepare(image))
            .expect("ordinary audit after activation")
            .is_clean()
    );
}

#[test]
fn failed_replacement_prefixes_preserve_pending_authority_and_actual_bytes() {
    for point in [
        Step::Append(Body::Replacement(Artifact::Envelope)),
        Step::FileSync(Body::Replacement(Artifact::Envelope)),
        Step::Install(Artifact::Envelope),
    ] {
        a_failed_replacement_prefix_preserves_authority_and_bytes(point);
    }
}

/// After a failed replacement the authoritative envelope and head are exactly the
/// bytes the interrupted publication left, the partial replacement stands as written,
/// and the store still refuses to open without activation.
fn the_pending_authority_survives(
    store: &std::path::Path,
    image: &marrow_verify::VerifiedImage,
    envelope: &[u8],
    head: &[u8],
    replacement: &[u8],
) {
    assert_eq!(
        std::fs::read(store.join(crate::ENVELOPE_FILE)).expect("authoritative envelope"),
        envelope
    );
    assert_eq!(
        std::fs::read(store.join(crate::HEAD_FILE)).expect("authoritative head"),
        head
    );
    assert_eq!(
        std::fs::read(store.join("envelope.replacing")).expect("actual failed replacement"),
        replacement
    );
    assert!(matches!(
        crate::attach(store, prepare(image.clone())),
        Err(crate::LifecycleError::Open(
            OpenError::ActivationRequired { .. }
        ))
    ));
}

/// One failed-replacement prefix: the authoritative envelope and head are untouched, the
/// partial replacement is kept as written, and a fresh recovery preserves it in turn.
fn a_failed_replacement_prefix_preserves_authority_and_bytes(point: Step) {
    let scratch = std::mem::ManuallyDrop::new(Scratch::new("recovery"));
    eprintln!("replacement-prefix fixture: {}", scratch.base().display());
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    let instance = StoreInstanceId::draw().expect("instance");
    provision_unconfirmed(&scratch.store(), &image, instance);
    let envelope =
        std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("pending envelope");
    let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
    let mut active = EnvelopeRecord::decode(&envelope).expect("pending record");
    assert!(matches!(active.state, EnvelopeState::Provision { .. }));
    active.state = EnvelopeState::Active;
    let mut active_bytes = active.encode().expect("expected active record");
    // A cut append leaves the prefix the failed write reached: half the record.
    let seam = if let Step::Append(_) = point {
        active_bytes.truncate(active_bytes.len() / 2);
        let partial = scratch.store().join("envelope.replacing");
        let prefix = active_bytes.clone();
        crate::test_support::once(
            move |event| matches!(event, Event::Step { step, .. } if *step == point),
            move |_| {
                std::fs::write(&partial, &prefix).expect("partial append");
                Err(crate::test_support::io_fault(
                    marrow_fs_journal::CustodyOp::Append,
                ))
            },
        )
        .0
    } else {
        cut(point)
    };
    std::fs::write(
        scratch.store().join("head.replacing"),
        b"prior interrupted head",
    )
    .expect("prior debris");
    let error = recover_observed(&scratch.store(), prepare(image.clone()), seam)
        .expect_err("unsynced replacement cannot finish activation");
    assert_eq!(error.code(), Code::StoreActivationUncertain);
    assert!(
        matches!(error.fault, RecoveryFault::Completion { instance: found, .. } if found == instance)
    );
    assert_eq!(error.preserved.len(), 1);
    let prior = scratch.store().join(&error.preserved[0]);
    assert_eq!(
        std::fs::read(&prior).expect("earlier preservation retained"),
        b"prior interrupted head"
    );
    the_pending_authority_survives(&scratch.store(), &image, &envelope, &head, &active_bytes);
    let receipt =
        recover(&scratch.store(), prepare(image.clone())).expect("fresh explicit recovery");
    assert_eq!(receipt.instance, instance);
    assert_eq!(receipt.image_id, image.image_id());
    assert_eq!(receipt.preserved.len(), 1);
    assert_ne!(receipt.preserved, error.preserved);
    assert_eq!(
        std::fs::read(scratch.store().join(&receipt.preserved[0]))
            .expect("preserved actual replacement"),
        active_bytes
    );
    assert_eq!(
        std::fs::read(&prior).expect("earlier preservation unchanged"),
        b"prior interrupted head"
    );
    assert_eq!(
        std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head not replayed"),
        head
    );
    assert!(!scratch.store().join("envelope.replacing").exists());
    assert!(
        crate::audit(&scratch.store(), prepare(image))
            .expect("ordinary audit after recovery")
            .is_clean()
    );
    drop(std::mem::ManuallyDrop::into_inner(scratch));
}

#[test]
fn recovery_preserves_occupied_replacements_before_activation() {
    let scratch = Scratch::new("recovery");
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    provision(
        &scratch.store(),
        request(&image, StoreInstanceId::draw().expect("instance")),
    )
    .expect("provision");
    std::fs::write(
        scratch.store().join("envelope.replacing"),
        b"partial envelope",
    )
    .expect("envelope debris");
    std::fs::write(scratch.store().join("head.replacing"), b"partial head").expect("head debris");
    let receipt = match recover(&scratch.store(), prepare(image)) {
        Ok(receipt) => receipt,
        Err(error) => {
            let original = scratch.base().to_path_buf();
            std::mem::forget(scratch);
            panic!(
                "eligible recovery did not preserve replacement files: {error}; preserve {}",
                original.display()
            );
        }
    };
    assert_eq!(receipt.preserved.len(), 2);
    for (name, bytes) in receipt
        .preserved
        .iter()
        .zip([b"partial envelope".as_slice(), b"partial head".as_slice()])
    {
        assert_eq!(
            std::fs::read(scratch.store().join(name)).expect("preserved bytes"),
            bytes
        );
    }
    assert!(!scratch.store().join("envelope.replacing").exists());
    assert!(!scratch.store().join("head.replacing").exists());
}

#[cfg(unix)]
#[test]
fn preservation_keeps_file_identity_permissions_and_uninterpreted_bytes() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    for slot in ["envelope.replacing", "head.replacing"] {
        for bytes in [b"".as_slice(), b"partial", &[0xff, 0, 0xfe, 0x80]] {
            let scratch = Scratch::new("recovery");
            provision(
                &scratch.store(),
                request(&image, StoreInstanceId::draw().expect("instance")),
            )
            .expect("provision");
            let path = scratch.store().join(slot);
            std::fs::write(&path, bytes).expect("debris");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
                .expect("permissions");
            let before = std::fs::symlink_metadata(&path).expect("original metadata");
            let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
            let receipt = recover(&scratch.store(), prepare(image.clone())).expect("recover");
            assert_eq!(receipt.preserved.len(), 1);
            let moved = scratch.store().join(&receipt.preserved[0]);
            let after = std::fs::symlink_metadata(&moved).expect("preserved metadata");
            assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
            assert_eq!(after.mode(), before.mode());
            assert_eq!(after.nlink(), 1);
            assert_eq!(std::fs::read(&moved).expect("preserved bytes"), bytes);
            assert!(!path.exists());
            assert_eq!(
                std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head unchanged"),
                head
            );
            let repeated =
                recover(&scratch.store(), prepare(image.clone())).expect("repeat recovery");
            assert!(repeated.preserved.is_empty());
            assert_eq!(
                std::fs::read(&moved).expect("prior preservation retained"),
                bytes
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn recovery_refuses_nonexclusive_replacement_shapes_without_moving_them() {
    use std::os::unix::fs::MetadataExt;

    #[derive(Clone, Copy)]
    enum Shape {
        Directory,
        Symlink,
        Hardlink,
    }
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    for slot in ["envelope.replacing", "head.replacing"] {
        for shape in [Shape::Directory, Shape::Symlink, Shape::Hardlink] {
            let scratch = Scratch::new("recovery");
            provision(
                &scratch.store(),
                request(&image, StoreInstanceId::draw().expect("instance")),
            )
            .expect("provision");
            let path = scratch.store().join(slot);
            let peer = scratch.base().join("peer");
            std::fs::write(&peer, b"peer bytes").expect("peer");
            match shape {
                Shape::Directory => std::fs::create_dir(&path).expect("directory"),
                Shape::Symlink => std::os::unix::fs::symlink(&peer, &path).expect("symlink"),
                Shape::Hardlink => std::fs::hard_link(&peer, &path).expect("hard link"),
            }
            let before = std::fs::symlink_metadata(&path).expect("original metadata");
            let envelope =
                std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
            let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
            let error = recover(&scratch.store(), prepare(image.clone())).expect_err("refuse");
            assert!(error.preserved.is_empty());
            let RecoveryFault::Metadata(error) = error.fault else {
                panic!("expected metadata refusal");
            };
            match shape {
                Shape::Directory => assert!(matches!(
                    error.fault,
                    crate::AdmissionFault::Custody(crate::CustodyError::WrongNodeKind { .. })
                )),
                Shape::Symlink => assert!(matches!(
                    error.fault,
                    crate::AdmissionFault::Custody(crate::CustodyError::SymlinkRefused { .. })
                )),
                Shape::Hardlink => assert!(matches!(
                    error.fault,
                    crate::AdmissionFault::MultiplyLinked { links: 2 }
                )),
            }
            let after = std::fs::symlink_metadata(&path).expect("refused object retained");
            assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
            assert_eq!(after.mode(), before.mode());
            assert_eq!(after.nlink(), before.nlink());
            assert_eq!(std::fs::read(&peer).expect("peer unchanged"), b"peer bytes");
            assert_eq!(
                std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("no activation"),
                envelope
            );
            assert_eq!(
                std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head unchanged"),
                head
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn a_later_preservation_failure_reports_the_earlier_move() {
    let scratch = Scratch::new("recovery");
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    provision(
        &scratch.store(),
        request(&image, StoreInstanceId::draw().expect("instance")),
    )
    .expect("provision");
    let envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
    std::fs::write(scratch.store().join("envelope.replacing"), b"first partial")
        .expect("first slot");
    let peer = scratch.store().join("peer");
    std::fs::write(&peer, b"outside bytes").expect("peer");
    std::os::unix::fs::symlink(&peer, scratch.store().join("head.replacing"))
        .expect("refused slot");
    let error = recover(&scratch.store(), prepare(image)).expect_err("second slot refuses");
    assert!(matches!(
        error.fault,
        RecoveryFault::Metadata(AdmissionError {
            fault: crate::AdmissionFault::Custody(crate::CustodyError::SymlinkRefused { .. }),
            ..
        })
    ));
    assert_eq!(error.preserved.len(), 1);
    assert_eq!(
        std::fs::read(scratch.store().join(&error.preserved[0])).expect("known move"),
        b"first partial"
    );
    assert!(!scratch.store().join("envelope.replacing").exists());
    assert_eq!(
        std::fs::read_link(scratch.store().join("head.replacing")).expect("link retained"),
        peer
    );
    assert_eq!(
        std::fs::read(&peer).expect("peer unchanged"),
        b"outside bytes"
    );
    assert_eq!(
        std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("no activation"),
        envelope
    );
}

#[test]
fn a_failed_preservation_barrier_reports_the_move_without_creating_a_replacement() {
    let scratch = Scratch::new("recovery");
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    provision(
        &scratch.store(),
        request(&image, StoreInstanceId::draw().expect("instance")),
    )
    .expect("provision");
    let before = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
    std::fs::write(scratch.store().join("envelope.replacing"), b"partial").expect("debris");
    let error = recover_observed(&scratch.store(), prepare(image), cut(Step::Preservation))
        .expect_err("move barrier failed");
    assert!(matches!(
        error.fault,
        RecoveryFault::Metadata(AdmissionError {
            fault: crate::AdmissionFault::Custody(crate::CustodyError::Io {
                op: marrow_fs_journal::CustodyOp::Sync,
                ..
            }),
            ..
        })
    ));
    assert_eq!(error.preserved.len(), 1);
    assert_eq!(
        std::fs::read(scratch.store().join(&error.preserved[0])).expect("known move"),
        b"partial"
    );
    assert!(!scratch.store().join("envelope.replacing").exists());
    assert_eq!(
        std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("no activation write"),
        before
    );
}

#[test]
fn failed_rebind_activation_reports_uncertainty_after_the_new_head_is_visible() {
    let scratch = Scratch::new("recovery");
    let old = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    let new =
        marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
    let instance = StoreInstanceId::draw().expect("instance");
    provision(&scratch.store(), request(&old, instance)).expect("provision");
    let error = match attach_observed(
        &scratch.store(),
        prepare(new.clone()),
        cut(Step::RebindActive),
    ) {
        Err(error) => error,
        Ok(_) => panic!("failed activation must not return an attachment"),
    };
    let head = crate::LogicalHead::decode(
        &std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("actual head"),
    )
    .expect("decode head");
    assert_eq!(head.binding, active_binding(&new));
    if error.code() != Code::StoreActivationUncertain {
        let original = scratch.base().to_path_buf();
        std::mem::forget(scratch);
        panic!(
            "visible new binding lost activation uncertainty: {error:?}; preserve {}",
            original.display()
        );
    }
    assert!(matches!(
        error,
        crate::LifecycleError::ActivationUncertain { instance: found, .. } if found == instance
    ));
}

#[test]
fn interrupted_rebind_adopts_actual_head_and_preserves_populated_data() {
    let source = format!(
        "{SOURCE}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n"
    );
    let old = marrow_verify::verify(&compile_bytes(&source)).expect("verify");
    let new =
        marrow_verify::verify(&compile_bytes(&source.replace("?? 0", "?? 1"))).expect("verify");
    for point in [
        Step::RebindPending,
        Step::FileSync(Body::Replacement(Artifact::Head)),
        Step::RebindHead,
    ] {
        an_interrupted_rebind_adopts_the_actual_head(point, &old, &new);
    }
}

/// One interrupted-rebind prefix: recovery adopts whichever head actually reached the
/// disk and the populated data survives either way.
fn an_interrupted_rebind_adopts_the_actual_head(
    point: Step,
    old: &marrow_verify::VerifiedImage,
    new: &marrow_verify::VerifiedImage,
) {
    let scratch = std::mem::ManuallyDrop::new(Scratch::new("recovery"));
    eprintln!("rebind-prefix fixture: {}", scratch.base().display());
    let instance = StoreInstanceId::draw().expect("instance");
    provision(&scratch.store(), request(old, instance)).expect("provision");
    populate_counter(&scratch.store(), old);
    let before = crate::audit(&scratch.store(), prepare(old.clone())).expect("before");
    assert_eq!(before.summary.entries, 1);
    let error = match attach_observed(&scratch.store(), prepare(new.clone()), cut(point)) {
        Err(error) => error,
        Ok(_) => panic!("failed barrier must not return an attachment"),
    };
    assert!(matches!(error, crate::LifecycleError::Metadata(_)));
    let record = EnvelopeRecord::decode(
        &std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("pending"),
    )
    .expect("record");
    assert!(matches!(record.state, EnvelopeState::Rebind { .. }));
    let replacement = if point == Step::FileSync(Body::Replacement(Artifact::Head)) {
        let bytes = std::fs::read(scratch.store().join("head.replacing"))
            .expect("unsynced replacement retained");
        assert_eq!(
            LogicalHead::decode(&bytes)
                .expect("real replacement head")
                .binding,
            active_binding(new)
        );
        Some(bytes)
    } else {
        None
    };
    let (actual_image, other_image) = if point == Step::RebindHead {
        (new, old)
    } else {
        (old, new)
    };
    let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("actual head");
    assert_eq!(
        LogicalHead::decode(&head).expect("head").binding,
        active_binding(actual_image)
    );
    assert!(matches!(
        crate::attach(&scratch.store(), prepare(actual_image.clone())),
        Err(crate::LifecycleError::Open(
            OpenError::ActivationRequired { .. }
        ))
    ));
    let rejected = recover(&scratch.store(), prepare(other_image.clone()))
        .expect_err("recovery cannot replay a different head");
    assert!(matches!(
        rejected.fault,
        RecoveryFault::Validation(AuditError::Refused(AdmissionRefusal::NotActive))
    ));
    assert!(rejected.preserved.is_empty());
    if let Some(bytes) = &replacement {
        assert_eq!(
            &std::fs::read(scratch.store().join("head.replacing"))
                .expect("rejected recovery leaves replacement"),
            bytes
        );
    }
    the_actual_head_recovers_intact(
        &scratch.store(),
        actual_image,
        instance,
        replacement.as_deref(),
        &head,
        before.digest,
    );
    drop(std::mem::ManuallyDrop::into_inner(scratch));
}

/// A fresh recovery over the head that actually reached the disk: it preserves the
/// unsynced replacement rather than replaying it, leaves the authoritative head
/// alone, and reopens the store on the same data it held before the interruption.
fn the_actual_head_recovers_intact(
    store: &std::path::Path,
    image: &marrow_verify::VerifiedImage,
    instance: StoreInstanceId,
    replacement: Option<&[u8]>,
    head: &[u8],
    digest_before: marrow_image::StoreDataDigest,
) {
    let receipt = recover(store, prepare(image.clone())).expect("recover");
    assert_eq!(receipt.instance, instance);
    assert_eq!(receipt.image_id, image.image_id());
    match replacement {
        Some(bytes) => {
            assert_eq!(receipt.preserved.len(), 1);
            assert_eq!(
                std::fs::read(store.join(&receipt.preserved[0]))
                    .expect("fresh recovery preserves actual head bytes"),
                bytes
            );
        }
        None => assert!(receipt.preserved.is_empty()),
    }
    assert_eq!(
        std::fs::read(store.join(crate::HEAD_FILE)).expect("head not replayed"),
        head
    );
    let after = crate::audit(store, prepare(image.clone())).expect("after");
    assert!(after.is_clean());
    assert_eq!(after.summary.entries, 1);
    assert_eq!(after.digest, digest_before);
    assert!(matches!(
        crate::attach(store, prepare(image.clone())).expect("recovered"),
        crate::AttachOutcome::AlreadyActive(_)
    ));
}

#[test]
fn interrupted_recovery_preserves_known_moves_and_requires_fresh_completion() {
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    for point in [
        Step::RecoveryArtifacts,
        Step::RecoveryParent,
        Step::RecoveryActive,
    ] {
        an_interrupted_recovery_preserves_its_known_moves(point, &image);
    }
}

/// The typed fault an interrupted recovery reports depends on where the barrier cut
/// it: past the active write it is an uncertain activation; before that it is I/O.
fn the_barrier_point_names_its_fault(
    point: Step,
    error: &crate::RecoveryError,
    instance: StoreInstanceId,
) {
    match point {
        Step::RecoveryActive => {
            assert!(
                matches!(error.fault, RecoveryFault::Completion { instance: found, .. } if found == instance)
            );
            assert_eq!(error.code(), Code::StoreActivationUncertain);
        }
        Step::RecoveryParent => {
            assert!(matches!(error.fault, RecoveryFault::Io(_)));
            assert_eq!(error.code(), Code::StoreIo);
        }
        _ => {
            assert!(matches!(error.fault, RecoveryFault::Metadata(_)));
            assert_eq!(error.code(), Code::StoreIo);
        }
    }
}

/// The envelope carries the state the interrupted recovery actually reached, and a
/// store short of the active write still refuses to open.
fn the_envelope_stopped_where_the_barrier_did(
    point: Step,
    store: &std::path::Path,
    image: &marrow_verify::VerifiedImage,
    head: &[u8],
) {
    let record = EnvelopeRecord::decode(
        &std::fs::read(store.join(crate::ENVELOPE_FILE)).expect("actual envelope"),
    )
    .expect("record");
    let digest = LogicalHead::decode_with_digest(head).expect("head").1;
    assert_eq!(
        record.state,
        if point == Step::RecoveryActive {
            EnvelopeState::Active
        } else {
            EnvelopeState::Provision { head: digest }
        }
    );
    if point != Step::RecoveryActive {
        assert!(matches!(
            crate::attach(store, prepare(image.clone())),
            Err(crate::LifecycleError::Open(
                OpenError::ActivationRequired { .. }
            ))
        ));
    }
}

/// One interrupted-recovery prefix: the moves already made stay preserved, the store
/// still demands activation, and only a fresh recovery completes it.
fn an_interrupted_recovery_preserves_its_known_moves(
    point: Step,
    image: &marrow_verify::VerifiedImage,
) {
    let scratch = Scratch::new("recovery");
    let instance = StoreInstanceId::draw().expect("instance");
    provision_unconfirmed(&scratch.store(), image, instance);
    let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
    std::fs::write(
        scratch.store().join("envelope.replacing"),
        b"first interrupted bytes",
    )
    .expect("first debris");
    let error = recover_observed(&scratch.store(), prepare(image.clone()), cut(point))
        .expect_err("barrier failure cannot return recovery success");
    the_barrier_point_names_its_fault(point, &error, instance);
    assert_eq!(error.preserved.len(), 1);
    let first = scratch.store().join(&error.preserved[0]);
    assert_eq!(
        std::fs::read(&first).expect("known first move"),
        b"first interrupted bytes"
    );
    the_envelope_stopped_where_the_barrier_did(point, &scratch.store(), image, &head);
    std::fs::write(
        scratch.store().join("envelope.replacing"),
        b"second interrupted bytes",
    )
    .expect("new debris before fresh attempt");
    let receipt = recover(&scratch.store(), prepare(image.clone())).expect("fresh recovery");
    assert_eq!(receipt.instance, instance);
    assert_eq!(receipt.image_id, image.image_id());
    assert_eq!(receipt.preserved.len(), 1);
    assert_ne!(receipt.preserved, error.preserved);
    assert_eq!(
        std::fs::read(&first).expect("first preserved bytes retained"),
        b"first interrupted bytes"
    );
    assert_eq!(
        std::fs::read(scratch.store().join(&receipt.preserved[0])).expect("second preserved bytes"),
        b"second interrupted bytes"
    );
    assert_eq!(
        std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head never replayed"),
        head
    );
}

#[test]
fn recovery_refuses_a_held_owner_before_engine_access_or_preservation() {
    let scratch = Scratch::new("recovery");
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    provision(
        &scratch.store(),
        request(&image, StoreInstanceId::draw().expect("instance")),
    )
    .expect("provision");
    let held =
        LockedStore::acquire(&scratch.store(), Seam::NONE).expect("hold recovery admission owner");
    let envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
    std::fs::write(
        scratch.store().join(crate::ENGINE_FILE),
        b"invalid engine control",
    )
    .expect("engine control");
    std::fs::write(scratch.store().join("envelope.replacing"), b"unexplained")
        .expect("debris control");
    let error = recover(&scratch.store(), prepare(image)).expect_err("live owner refuses recovery");
    assert!(matches!(
        error.fault,
        RecoveryFault::Validation(AuditError::Open(OpenError::Lock(
            marrow_kernel::durable::NativeLockError::StoreInUse { .. }
        )))
    ));
    assert_eq!(error.code(), Code::StoreLocked);
    assert!(error.preserved.is_empty());
    assert_eq!(
        std::fs::read(scratch.store().join(crate::ENGINE_FILE)).expect("engine unchanged"),
        b"invalid engine control"
    );
    assert_eq!(
        std::fs::read(scratch.store().join("envelope.replacing")).expect("debris unmoved"),
        b"unexplained"
    );
    assert_eq!(
        std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("no activation"),
        envelope
    );
    drop(held);
}

#[test]
fn final_admission_rereads_both_metadata_files_after_activation() {
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    let edited =
        marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
    for artifact in [Artifact::Envelope, Artifact::Head] {
        let scratch = Scratch::new("recovery");
        let instance = StoreInstanceId::draw().expect("instance");
        let req = request(&image, instance);
        let mut changed_metadata = req.envelope.clone();
        changed_metadata.writer_toolchain = "changed before final read".into();
        let replacement = match artifact {
            Artifact::Envelope => EnvelopeRecord {
                metadata: changed_metadata,
                state: EnvelopeState::Active,
            }
            .encode()
            .expect("valid changed envelope"),
            Artifact::Head => request(&edited, instance).head.encode(),
        };
        provision(&scratch.store(), req).expect("provision");
        std::fs::write(
            scratch.store().join("envelope.replacing"),
            b"interrupted bytes",
        )
        .expect("debris");
        let store = scratch.store();
        let bytes = replacement.clone();
        let (seam, reached) = mutate_at(Step::FinalRead, move |dir| {
            replace_synced(&store, dir, artifact, &bytes)
        });
        let error = recover_observed(&scratch.store(), prepare(image.clone()), seam)
            .expect_err("cached admission cannot certify changed metadata");
        reached.assert();
        assert_eq!(error.code(), Code::StoreActivationUncertain);
        assert!(matches!(error.fault,
            RecoveryFault::Completion {
                instance: found,
                source: AuditError::Open(OpenError::Corruption { .. }),
            } if found == instance));
        assert_eq!(error.preserved.len(), 1);
        assert_eq!(
            std::fs::read(scratch.store().join(&error.preserved[0])).expect("known move"),
            b"interrupted bytes"
        );
        let name = match artifact {
            Artifact::Envelope => crate::ENVELOPE_FILE,
            Artifact::Head => crate::HEAD_FILE,
        };
        assert_eq!(
            std::fs::read(scratch.store().join(name)).expect("actual changed metadata"),
            replacement
        );
    }
}

#[test]
fn binding_preparation_refuses_a_replaced_engine_before_metadata_writes() {
    use marrow_kernel::durable::{StoreError, StoreOp};
    let scratch = Scratch::new("recovery");
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    let edited =
        marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
    let instance = StoreInstanceId::draw().expect("instance");
    provision(&scratch.store(), request(&image, instance)).expect("provision");
    let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
    let envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
    let engine = scratch.store().join(crate::ENGINE_FILE);
    let displaced = scratch.base().join("admitted-engine");
    let (seam, reached) = mutate_at(Step::Admitted, move |_| {
        std::fs::rename(&engine, &displaced).expect("displace engine");
        std::fs::copy(&displaced, &engine).expect("replace engine node");
    });
    let result = attach_observed(&scratch.store(), prepare(edited), seam);
    reached.assert();
    assert!(matches!(
        result,
        Err(crate::LifecycleError::Open(OpenError::Store(
            StoreError::Io {
                op: StoreOp::ServicePreparation,
                ..
            }
        )))
    ));
    assert_eq!(
        std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head"),
        head
    );
    assert_eq!(
        std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope"),
        envelope
    );
    assert!(!scratch.store().join(crate::LOCK_FILE).exists());
    assert_eq!(
        std::fs::read_dir(scratch.store()).expect("members").count(),
        3
    );
}

#[test]
fn binding_rechecks_old_metadata_and_location_after_service_preparation() {
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    let edited =
        marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
    for move_directory in [false, true] {
        let scratch = Scratch::new("recovery");
        let instance = StoreInstanceId::draw().expect("instance");
        let req = request(&image, instance);
        let old_head = req.head.encode();
        provision(&scratch.store(), req).expect("provision");
        let old_envelope =
            std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
        let changed_head = request(&edited, instance).head.encode();
        let store = scratch.store();
        let (mutation, retained, expected_head): (Mutation, _, _) = if move_directory {
            let moved = scratch.base().join("moved");
            let target = moved.clone();
            (
                Box::new(move |_: &AdmittedStoreDir| {
                    std::fs::rename(&store, &target).expect("move store");
                }),
                moved,
                old_head,
            )
        } else {
            let bytes = changed_head.clone();
            (
                Box::new(move |dir: &AdmittedStoreDir| {
                    replace_synced(&store, dir, Artifact::Head, &bytes)
                }),
                scratch.store(),
                changed_head,
            )
        };
        let (seam, reached) = mutate_at(Step::Prepared, mutation);
        let result = attach_observed(&scratch.store(), prepare(edited.clone()), seam);
        reached.assert();
        assert!(matches!(
            result,
            Err(crate::LifecycleError::Audit(AuditError::Open(_)))
        ));
        assert_eq!(
            std::fs::read(retained.join(crate::HEAD_FILE)).expect("retained head"),
            expected_head
        );
        assert_eq!(
            std::fs::read(retained.join(crate::ENVELOPE_FILE)).expect("retained envelope"),
            old_envelope
        );
        assert!(!retained.join("head.replacing").exists());
        assert!(!retained.join("envelope.replacing").exists());
    }
}

#[test]
fn binding_final_verification_refuses_changed_metadata_with_instance() {
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    let edited =
        marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
    for artifact in [Artifact::Head, Artifact::Envelope] {
        let scratch = Scratch::new("recovery");
        let instance = StoreInstanceId::draw().expect("instance");
        let req = request(&image, instance);
        let replacement = match artifact {
            Artifact::Head => req.head.encode(),
            Artifact::Envelope => {
                let mut metadata = req.envelope.clone();
                metadata.writer_toolchain = "changed during activation".into();
                EnvelopeRecord {
                    metadata,
                    state: EnvelopeState::Active,
                }
                .encode()
                .expect("envelope")
            }
        };
        provision(&scratch.store(), req).expect("provision");
        let store = scratch.store();
        let bytes = replacement.clone();
        let (seam, reached) = mutate_at(Step::Activated, move |dir| {
            replace_synced(&store, dir, artifact, &bytes)
        });
        let result = attach_observed(&scratch.store(), prepare(edited.clone()), seam);
        reached.assert();
        assert!(
            matches!(result, Err(crate::LifecycleError::ActivationUncertain {
            instance: found,
            source: AuditError::Open(OpenError::Corruption { .. }),
        }) if found == instance)
        );
        let name = match artifact {
            Artifact::Head => crate::HEAD_FILE,
            Artifact::Envelope => crate::ENVELOPE_FILE,
        };
        assert_eq!(
            std::fs::read(scratch.store().join(name)).expect("retained changed metadata"),
            replacement
        );
    }
}

#[test]
fn recovery_refuses_a_different_image_before_opening_the_engine() {
    let scratch = Scratch::new("recovery");
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    let edited =
        marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
    let instance = StoreInstanceId::draw().expect("instance");
    provision(&scratch.store(), request(&image, instance)).expect("provision");
    let engine = scratch.store().join(crate::ENGINE_FILE);
    std::fs::write(&engine, b"not an engine").expect("engine refusal control");
    let envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
    assert!(matches!(
        recover(&scratch.store(), prepare(edited)).map_err(|error| error.fault),
        Err(RecoveryFault::Validation(AuditError::Refused(
            AdmissionRefusal::NotActive
        )))
    ));
    assert_eq!(
        std::fs::read(&engine).expect("engine unchanged"),
        b"not an engine"
    );
    assert_eq!(
        std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope unchanged"),
        envelope
    );
}

#[test]
fn unsupported_stamp_refuses_admission_before_engine_or_preservation() {
    let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
    for pending in [false, true] {
        let scratch = Scratch::new("recovery");
        let req = request(&image, StoreInstanceId::draw().expect("instance"));
        let digest = req.head.encode_with_digest().1;
        let mut metadata = req.envelope.clone();
        provision(&scratch.store(), req).expect("current provision");
        metadata.engine_format_version = u32::MAX;
        let record = EnvelopeRecord {
            metadata,
            state: if pending {
                EnvelopeState::Provision { head: digest }
            } else {
                EnvelopeState::Active
            },
        };
        let bytes = record.encode().expect("resealed unsupported stamp");
        std::fs::write(scratch.store().join(crate::ENVELOPE_FILE), &bytes).expect("envelope");
        std::fs::write(scratch.store().join(crate::ENGINE_FILE), b"invalid engine")
            .expect("engine control");
        std::fs::write(scratch.store().join("envelope.replacing"), b"unexplained")
            .expect("debris control");
        let error =
            recover(&scratch.store(), prepare(image.clone())).expect_err("unsupported stamp");
        assert!(error.preserved.is_empty());
        assert!(matches!(
            error.fault,
            RecoveryFault::Validation(AuditError::Open(OpenError::Store(
                marrow_kernel::durable::StoreError::FormatVersion {
                    found: u32::MAX,
                    ..
                }
            )))
        ));
        assert!(matches!(
            crate::attach(&scratch.store(), prepare(image.clone())),
            Err(crate::LifecycleError::Open(OpenError::Store(
                marrow_kernel::durable::StoreError::FormatVersion {
                    found: u32::MAX,
                    ..
                }
            )))
        ));
        assert_eq!(
            std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope unchanged"),
            bytes
        );
        assert_eq!(
            std::fs::read(scratch.store().join(crate::ENGINE_FILE)).expect("engine unchanged"),
            b"invalid engine"
        );
        assert_eq!(
            std::fs::read(scratch.store().join("envelope.replacing")).expect("debris unmoved"),
            b"unexplained"
        );
    }
}

#[test]
fn rebind_recovery_adopts_only_the_exact_recorded_old_or_new_head() {
    let images = [
        marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify"),
        marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify"),
        marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 2"))).expect("verify"),
    ];
    let instance = StoreInstanceId::draw().expect("instance");
    let old = request(&images[0], instance).head.encode_with_digest().1;
    let new = request(&images[1], instance).head.encode_with_digest().1;
    for (index, image) in images.into_iter().enumerate() {
        let scratch = Scratch::new("recovery");
        let req = request(&image, instance);
        let record = EnvelopeRecord {
            metadata: req.envelope.clone(),
            state: EnvelopeState::Rebind { old, new },
        };
        provision(&scratch.store(), req).expect("provision");
        let envelope = record.encode().expect("pending");
        std::fs::write(scratch.store().join(crate::ENVELOPE_FILE), &envelope).expect("pending");
        let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
        let result = recover(&scratch.store(), prepare(image.clone())).map_err(|error| error.fault);
        if index < 2 {
            assert_eq!(
                result.expect("recorded head is eligible").image_id,
                image.image_id()
            );
        } else {
            assert!(matches!(result, Err(RecoveryFault::HeadMismatch)));
            assert_eq!(
                std::fs::read(scratch.store().join(crate::ENVELOPE_FILE))
                    .expect("pending unchanged"),
                envelope
            );
        }
        assert_eq!(
            std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head unchanged"),
            head
        );
    }
}
