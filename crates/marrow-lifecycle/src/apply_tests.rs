//! Explicit sparse apply against a populated store: what it refuses, what it preserves,
//! and what a store looks like after an interrupted publication.

use std::path::Path;

use super::{ApplyError, UnsupportedChange, apply, apply_observed};
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::recovery::{RecoveryFault, recover};
use crate::seam::Step;
use crate::store_dir::{Artifact, Body};
use crate::test_support::{IDS, SOURCE, compile_bytes, cut, mutate_at, populate_counter, request};
use crate::{
    AdmissionRefusal, AuditError, LifecycleError, LogicalHead, StoreInstanceId, accepted_ceiling,
    active_binding, head_map, prepare, provision,
};
use marrow_test_support::Scratch;

/// A head whose accepted-ceiling payload does not decode is store corruption to the
/// compatible gate (attach) and the exact gate (apply) alike, decided before any engine call.
#[test]
fn a_corrupt_accepted_ceiling_is_refused_as_corruption_before_the_engine_opens() {
    let (old, new) = sparse_images();
    let old = marrow_verify::verify(&old).expect("old image");
    let new = marrow_verify::verify(&new).expect("new image");
    let scratch = Scratch::new("apply");
    provision(
        scratch.store(),
        request(&old, StoreInstanceId::draw().expect("instance")),
    )
    .expect("provision");
    let corrupt = LogicalHead::provision(
        active_binding(&old),
        vec![0xff; 3],
        head_map(&old).expect("head map"),
    );
    std::fs::write(scratch.store().join(crate::HEAD_FILE), corrupt.encode()).expect("head");
    std::fs::write(scratch.store().join(crate::ENGINE_FILE), b"not an engine")
        .expect("engine control");
    let refused = match crate::attach(scratch.store(), prepare(old.clone())) {
        Err(error) => error,
        Ok(_) => panic!("a corrupt ceiling cannot attach"),
    };
    assert!(matches!(
        refused,
        LifecycleError::Refused(AdmissionRefusal::CeilingCorrupt)
    ));
    assert_eq!(refused.code(), marrow_codes::Code::StoreCorruption);
    let refused = apply(scratch.store(), prepare(old), prepare(new), None)
        .expect_err("a corrupt ceiling cannot apply");
    assert!(matches!(
        refused,
        ApplyError::Lifecycle(LifecycleError::Audit(AuditError::Refused(
            AdmissionRefusal::CeilingCorrupt
        )))
    ));
    assert_eq!(refused.code(), marrow_codes::Code::StoreCorruption);
    assert_eq!(
        std::fs::read(scratch.store().join(crate::ENGINE_FILE)).expect("engine"),
        b"not an engine"
    );
}

fn sparse_images() -> (Vec<u8>, Vec<u8>) {
    let source = format!(
        "{SOURCE}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n"
    );
    let old = compile_bytes(&source);
    let ids = IDS.replace(
        "high-water",
        "id field Counter.extra 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\nhigh-water",
    );
    let source = SOURCE.replace("required value", "extra: int\nrequired value")
        + "\npub fn readExtra(n: int): int { return ^counters[n].extra ?? -1 }\n";
    let new = marrow_test_programs::program::compile_bytes(&source, &ids);
    (old, new)
}

fn store_bytes(dir: &Path) -> std::collections::BTreeMap<std::ffi::OsString, (Vec<u8>, u32)> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::read_dir(dir)
        .expect("store members")
        .map(|entry| {
            let entry = entry.expect("member");
            let metadata = std::fs::symlink_metadata(entry.path()).expect("metadata");
            assert!(
                metadata.is_file(),
                "fixture has only regular store artifacts"
            );
            (
                entry.file_name(),
                (
                    std::fs::read(entry.path()).expect("artifact"),
                    metadata.permissions().mode(),
                ),
            )
        })
        .collect()
}

#[test]
fn sparse_apply_refusals_preserve_populated_store_bytes() {
    use ApplyError;
    use marrow_image::{CeilingDescriptor, CeilingId};
    let (old, new) = sparse_images();
    let old = marrow_verify::verify(&old).expect("old image");
    let new = marrow_verify::verify(&new).expect("new image");
    let scratch = Scratch::new("apply");
    provision(
        scratch.store(),
        request(&old, StoreInstanceId::draw().expect("instance")),
    )
    .expect("provision");
    populate_counter(scratch.store(), &old);
    let before = store_bytes(scratch.store());
    let ceiling = CeilingDescriptor::from_payload(&accepted_ceiling(&old)).expect("old ceiling");
    let proposed = ceiling
        .expanded(
            &new.demand_union(),
            crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
        )
        .expect("bounded union");
    assert_ne!(ceiling.ceiling_id(), proposed.ceiling_id());
    let image_ceiling = CeilingDescriptor::from_demand_union(new.demand_union());
    assert_ne!(image_ceiling.ceiling_id(), proposed.ceiling_id());
    for accepted in [
        None,
        Some(CeilingId::from_bytes([0; 32])),
        Some(image_ceiling.ceiling_id()),
    ] {
        let error = apply(
            scratch.store(),
            prepare(old.clone()),
            prepare(new.clone()),
            accepted,
        )
        .expect_err("explicit expansion required");
        let ApplyError::CeilingUnaccepted {
            old: prior,
            proposed: found,
            added,
        } = error
        else {
            panic!("wrong refusal: {error:?}")
        };
        assert_eq!(prior, ceiling.ceiling_id());
        assert_eq!(found, proposed.ceiling_id());
        assert!(
            added
                .iter()
                .any(|effect| effect.path.as_deref() == Some("^counters.extra"))
        );
        assert_eq!(store_bytes(scratch.store()), before);
    }
    let error = apply(
        scratch.store(),
        prepare(old.clone()),
        prepare(old.clone()),
        Some(CeilingId::from_bytes([0; 32])),
    )
    .expect_err("wrong ID even without expansion");
    assert!(
        matches!(error, ApplyError::CeilingUnaccepted { old: prior, proposed, added } if prior == ceiling.ceiling_id() && proposed == prior && added.is_empty())
    );
    assert_eq!(store_bytes(scratch.store()), before);
    let changed = marrow_verify::verify(&compile_bytes(&SOURCE.replace("required ", "")))
        .expect("changed requiredness");
    assert!(matches!(
        apply(
            scratch.store(),
            prepare(old.clone()),
            prepare(changed),
            None
        ),
        Err(ApplyError::Unsupported(UnsupportedChange::MemberChanged))
    ));
    assert_eq!(store_bytes(scratch.store()), before);
    let receipt = apply(
        scratch.store(),
        prepare(old.clone()),
        prepare(new.clone()),
        Some(proposed.ceiling_id()),
    )
    .expect("exact union accepted");
    assert_eq!(receipt.old_ceiling, ceiling.ceiling_id());
    assert_eq!(receipt.ceiling, proposed.ceiling_id());
    let head = LogicalHead::decode(
        &std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("applied Head"),
    )
    .expect("Head");
    assert_eq!(head.accepted_ceiling, proposed.atom_set_payload());
    let applied = store_bytes(scratch.store());
    assert!(matches!(
        apply(
            scratch.store(),
            prepare(old),
            prepare(new),
            Some(proposed.ceiling_id())
        ),
        Err(ApplyError::Lifecycle(LifecycleError::Audit(
            AuditError::Refused(AdmissionRefusal::ContractChanged(_))
        )))
    ));
    assert_eq!(store_bytes(scratch.store()), applied);
}

#[test]
fn sparse_apply_rejects_incompatible_graphs_without_store_changes() {
    let (old, _) = sparse_images();
    let old = marrow_verify::verify(&old).expect("old image");
    let scratch = Scratch::new("apply");
    provision(
        scratch.store(),
        request(&old, StoreInstanceId::draw().expect("instance")),
    )
    .expect("provision");
    populate_counter(scratch.store(), &old);
    let before = store_bytes(scratch.store());
    let ids = IDS.replace(
        "high-water",
        "id field Counter.extra 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\nid index counters.byValue 10101010101010101010101010101010\nid sum Option[int] 11111111111111111111111111111111\nid member Option[int].none 12121212121212121212121212121212\nid member Option[int].some 13131313131313131313131313131313\nhigh-water",
    );
    use UnsupportedChange::{Index, MemberAdded, MemberChanged, MemberRemoved, Root, StoredValue};
    for (label, fields, key, suffix, reason) in [
        (
            "changed value",
            "required value: bool",
            "int",
            "",
            StoredValue,
        ),
        ("removed field", "", "int", "", MemberRemoved),
        (
            "changed requiredness",
            "value: int",
            "int",
            "",
            MemberChanged,
        ),
        (
            "new required field",
            "required value: int\nrequired extra: int",
            "int",
            "",
            MemberAdded,
        ),
        (
            "new composite field",
            "required value: int\nextra: Option<int>",
            "int",
            "",
            MemberAdded,
        ),
        ("changed key", "required value: int", "string", "", Root),
        (
            "new index",
            "required value: int",
            "int",
            "{ index byValue[value] unique }",
            Index,
        ),
    ] {
        let source = format!(
            "resource Counter {{ {fields} }}\nstore ^counters[id: {key}]: Counter {suffix}\npub fn bootstrap(): int {{ return 0 }}\n"
        );
        let new =
            marrow_verify::verify(&marrow_test_programs::program::compile_bytes(&source, &ids))
                .unwrap_or_else(|error| panic!("{label}: {error:?}"));
        let refused = apply(scratch.store(), prepare(old.clone()), prepare(new), None);
        assert!(
            matches!(refused, Err(ApplyError::Unsupported(found)) if found == reason),
            "{label}: {refused:?}"
        );
        assert_eq!(store_bytes(scratch.store()), before, "{label}");
    }
}

#[test]
fn sparse_apply_rejects_changed_index_meaning_without_store_changes() {
    let source = format!(
        "{}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n",
        SOURCE.replace(
            ": Counter\n",
            ": Counter { index byValue[value, id] unique }\n"
        )
    );
    let ids = IDS.replace(
        "high-water",
        "id index counters.byValue 10101010101010101010101010101010\nhigh-water",
    );
    let old = marrow_verify::verify(&marrow_test_programs::program::compile_bytes(&source, &ids))
        .expect("indexed image");
    let scratch = Scratch::new("apply");
    provision(
        scratch.store(),
        request(&old, StoreInstanceId::draw().expect("instance")),
    )
    .expect("provision");
    populate_counter(scratch.store(), &old);
    assert!(
        crate::audit(scratch.store(), prepare(old.clone()))
            .expect("old audit")
            .is_clean()
    );
    let before = store_bytes(scratch.store());
    for replacement in [
        "",
        "index byValue[id, value] unique",
        "index byValue[value, id]",
    ] {
        let changed = source.replace("index byValue[value, id] unique", replacement);
        let new = marrow_verify::verify(&marrow_test_programs::program::compile_bytes(
            &changed, &ids,
        ))
        .expect("changed index");
        assert!(
            matches!(
                apply(scratch.store(), prepare(old.clone()), prepare(new), None),
                Err(ApplyError::Unsupported(UnsupportedChange::Index))
            ),
            "{replacement}"
        );
        assert_eq!(store_bytes(scratch.store()), before, "{replacement}");
    }
}

#[test]
fn sparse_apply_preserves_populated_irregular_addresses_and_refuses_exhaustion() {
    let (old, new) = sparse_images();
    let old = marrow_verify::verify(&old).expect("old image");
    let new = marrow_verify::verify(&new).expect("new image");
    let ceiling = marrow_image::CeilingDescriptor::from_payload(&accepted_ceiling(&old))
        .expect("old ceiling")
        .expanded(
            &new.demand_union(),
            crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
        )
        .expect("bounded union")
        .ceiling_id();
    for high_water in [90, u32::MAX] {
        let scratch = Scratch::new("apply");
        let mut request = request(&old, StoreInstanceId::draw().expect("instance"));
        let mut encoded = Vec::new();
        request.head.head_map.encode(&mut encoded);
        assert_eq!(request.head.head_map.len(), 2);
        encoded[..4].copy_from_slice(&high_water.to_be_bytes());
        let (entries, remainder) = encoded[8..].as_chunks_mut::<20>();
        assert!(remainder.is_empty());
        for (index, entry) in entries.iter_mut().enumerate() {
            entry[16..].copy_from_slice(&(17 - index as u32 * 5).to_be_bytes());
        }
        request.head.head_map = crate::HeadMap::decode(&mut crate::codec::Reader::new(&encoded))
            .expect("valid irregular map before population");
        let old_map = request.head.head_map.clone();
        provision(scratch.store(), request).expect("provision");
        populate_counter(scratch.store(), &old);
        let before = crate::audit(scratch.store(), prepare(old.clone())).expect("populated old");
        let bytes = store_bytes(scratch.store());
        let result = apply(
            scratch.store(),
            prepare(old.clone()),
            prepare(new.clone()),
            Some(ceiling),
        );
        if high_water == u32::MAX {
            assert!(matches!(
                result,
                Err(ApplyError::HeadMap(crate::FormatError::LengthOverflow {
                    field: crate::FormatField::HeadMapLifetimeNumbers
                }))
            ));
            assert_eq!(result.unwrap_err().code(), marrow_codes::Code::StoreLimit);
            assert_eq!(store_bytes(scratch.store()), bytes);
            continue;
        }
        result.expect("apply at actual high-water");
        let head = LogicalHead::decode(
            &std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("Head"),
        )
        .expect("decode");
        for entry in old_map.entries() {
            assert_eq!(
                head.head_map.number_of(&entry.ledger_id),
                Some(entry.number)
            );
        }
        assert_eq!(head.head_map.len(), old_map.len() + 1);
        assert_eq!(head.head_map.next_number(), high_water + 1);
        let new_entry = head
            .head_map
            .entries()
            .iter()
            .find(|entry| old_map.number_of(&entry.ledger_id).is_none())
            .expect("added field");
        assert_eq!(new_entry.number, high_water);
        let after = crate::audit(scratch.store(), prepare(new.clone())).expect("new layout");
        assert!(after.is_clean());
        assert_eq!(after.summary.entries, 1);
        assert_eq!(after.digest, before.digest);
    }
}

#[test]
fn interrupted_sparse_apply_recovers_only_the_actual_head() {
    let (old, new) = sparse_images();
    let old = marrow_verify::verify(&old).expect("old image");
    let new = marrow_verify::verify(&new).expect("new image");
    let ceiling = marrow_image::CeilingDescriptor::from_payload(&accepted_ceiling(&old))
        .expect("old ceiling")
        .expanded(
            &new.demand_union(),
            crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
        )
        .expect("bounded union")
        .ceiling_id();
    for point in [
        Step::RebindPending,
        Step::FileSync(Body::Replacement(Artifact::Head)),
        Step::RebindHead,
        Step::RebindActive,
    ] {
        an_interrupted_sparse_apply_recovers_its_actual_head(point, &old, &new, ceiling);
    }
}

/// The envelope state and typed error an interrupted apply reports depend on where the
/// barrier cut it: past the active write the envelope is already active and the failure
/// is an uncertain activation; before it the envelope is still rebinding.
fn the_barrier_point_names_its_state(
    point: Step,
    record: &EnvelopeRecord,
    error: &ApplyError,
    instance: StoreInstanceId,
) {
    if point == Step::RebindActive {
        assert_eq!(record.state, EnvelopeState::Active);
        assert!(
            matches!(error, ApplyError::Lifecycle(LifecycleError::ActivationUncertain { instance: found, .. }) if *found == instance)
        );
    } else {
        assert!(matches!(record.state, EnvelopeState::Rebind { .. }));
        assert!(matches!(
            error,
            ApplyError::Lifecycle(LifecycleError::Metadata(_))
        ));
    }
}

/// One interrupted-apply prefix: the envelope stops where the barrier cut it, recovery
/// refuses the image the head does not name, and the head that actually reached the disk
/// recovers with its sparse data intact.
fn an_interrupted_sparse_apply_recovers_its_actual_head(
    point: Step,
    old: &marrow_verify::VerifiedImage,
    new: &marrow_verify::VerifiedImage,
    ceiling: marrow_image::CeilingId,
) {
    let scratch = Scratch::new("apply");
    let instance = StoreInstanceId::draw().expect("instance");
    provision(scratch.store(), request(old, instance)).expect("provision");
    populate_counter(scratch.store(), old);
    let before = crate::audit(scratch.store(), prepare(old.clone())).expect("old logical contents");
    let error = apply_observed(
        scratch.store(),
        prepare(old.clone()),
        prepare(new.clone()),
        Some(ceiling),
        cut(point),
    )
    .expect_err("publication barrier fails");
    let record = EnvelopeRecord::decode(
        &std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("actual envelope"),
    )
    .expect("envelope");
    the_barrier_point_names_its_state(point, &record, &error, instance);
    let (selected, other) = if matches!(
        point,
        Step::RebindPending | Step::FileSync(Body::Replacement(Artifact::Head))
    ) {
        (old, new)
    } else {
        (new, old)
    };
    let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("actual Head");
    let replacement = if point == Step::FileSync(Body::Replacement(Artifact::Head)) {
        let bytes =
            std::fs::read(scratch.store().join("head.replacing")).expect("retained replacement");
        assert_eq!(
            LogicalHead::decode(&bytes)
                .expect("replacement Head")
                .binding,
            active_binding(new)
        );
        Some(bytes)
    } else {
        None
    };
    assert_eq!(
        LogicalHead::decode(&head).expect("Head").binding,
        active_binding(selected)
    );
    let rejected =
        recover(scratch.store(), prepare(other.clone())).expect_err("wrong selected image");
    assert!(matches!(
        rejected.fault,
        RecoveryFault::Validation(AuditError::Refused(AdmissionRefusal::ContractChanged(_)))
    ));
    assert!(rejected.preserved.is_empty());
    if let Some(bytes) = &replacement {
        assert_eq!(
            &std::fs::read(scratch.store().join("head.replacing"))
                .expect("wrong image preserves replacement"),
            bytes
        );
    }
    let recovered =
        recover(scratch.store(), prepare(selected.clone())).expect("recover actual image");
    assert_eq!(recovered.instance, instance);
    assert_eq!(recovered.image_id, selected.image_id());
    if let Some(bytes) = replacement {
        assert_eq!(recovered.preserved.len(), 1);
        assert_eq!(
            std::fs::read(scratch.store().join(&recovered.preserved[0]))
                .expect("preserved replacement"),
            bytes
        );
    } else {
        assert!(recovered.preserved.is_empty());
    }
    assert_eq!(
        std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("Head not replayed"),
        head
    );
    let after =
        crate::audit(scratch.store(), prepare(selected.clone())).expect("recovered contents");
    assert!(after.is_clean());
    assert_eq!(after.summary.entries, 1);
    assert_eq!(after.digest, before.digest);
}

#[test]
fn sparse_apply_final_verification_preserves_uncertainty_and_instance() {
    let (old, new) = sparse_images();
    let old = marrow_verify::verify(&old).expect("old image");
    let new = marrow_verify::verify(&new).expect("new image");
    let ceiling = marrow_image::CeilingDescriptor::from_payload(&accepted_ceiling(&old))
        .expect("old ceiling")
        .expanded(
            &new.demand_union(),
            crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
        )
        .expect("union")
        .ceiling_id();
    for artifact in [Artifact::Head, Artifact::Envelope] {
        let scratch = Scratch::new("apply");
        let instance = StoreInstanceId::draw().expect("instance");
        let req = request(&old, instance);
        let replacement = match artifact {
            Artifact::Head => req.head.encode(),
            Artifact::Envelope => {
                let mut metadata = req.envelope.clone();
                metadata.writer_toolchain = "substituted after publication".into();
                EnvelopeRecord {
                    metadata,
                    state: EnvelopeState::Active,
                }
                .encode()
                .expect("valid envelope")
            }
        };
        provision(scratch.store(), req).expect("provision");
        populate_counter(scratch.store(), &old);
        let store = scratch.store().to_path_buf();
        let bytes = replacement.clone();
        let (seam, reached) = mutate_at(Step::Activated, move |dir| {
            dir.replace(artifact, &bytes).expect("replace artifact");
            crate::durable_fs::sync_dir(&store).expect("sync directory");
        });
        let error = apply_observed(
            scratch.store(),
            prepare(old.clone()),
            prepare(new.clone()),
            Some(ceiling),
            seam,
        )
        .expect_err("changed final metadata cannot yield a receipt");
        reached.assert();
        assert!(
            matches!(error, ApplyError::Lifecycle(LifecycleError::ActivationUncertain { instance: found, .. }) if found == instance)
        );
        let name = match artifact {
            Artifact::Head => crate::HEAD_FILE,
            Artifact::Envelope => crate::ENVELOPE_FILE,
        };
        assert_eq!(
            std::fs::read(scratch.store().join(name)).expect("actual changed metadata"),
            replacement
        );
    }
}

#[test]
fn sparse_apply_refuses_logical_corruption_before_publication() {
    let (integer, _) = sparse_images();
    let integer = marrow_verify::verify(&integer).expect("integer image");
    let source = "resource Counter { required value: bool }\nstore ^counters[id: int]: Counter\npub fn readValue(n: int): bool { return ^counters[n].value ?? false }\n";
    let old = marrow_verify::verify(&compile_bytes(source)).expect("boolean image");
    let ids = IDS.replace(
        "high-water",
        "id field Counter.extra 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\nhigh-water",
    );
    let new_source = source.replace("required value", "extra: int\nrequired value")
        + "\npub fn readExtra(n: int): int { return ^counters[n].extra ?? -1 }\n";
    let new = marrow_verify::verify(&marrow_test_programs::program::compile_bytes(
        &new_source,
        &ids,
    ))
    .expect("new boolean image");
    let populated = Scratch::new("apply");
    let target = Scratch::new("apply");
    provision(
        populated.store(),
        request(&integer, StoreInstanceId::draw().expect("instance")),
    )
    .expect("integer store");
    populate_counter(populated.store(), &integer);
    provision(
        target.store(),
        request(&old, StoreInstanceId::draw().expect("instance")),
    )
    .expect("boolean store");
    std::fs::copy(
        populated.store().join(crate::ENGINE_FILE),
        target.store().join(crate::ENGINE_FILE),
    )
    .expect("physical store with wrong logical values");
    let before = store_bytes(target.store());
    let ceiling = marrow_image::CeilingDescriptor::from_payload(&accepted_ceiling(&old))
        .expect("old ceiling")
        .expanded(
            &new.demand_union(),
            crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
        )
        .expect("union")
        .ceiling_id();
    let error = apply(target.store(), prepare(old), prepare(new), Some(ceiling))
        .expect_err("invalid OLD data");
    let ApplyError::Lifecycle(LifecycleError::Invalid(report)) = error else {
        panic!("logical audit must refuse: {error:?}")
    };
    assert!(!report.is_clean());
    assert_eq!(store_bytes(target.store()), before);
}

/// A store whose envelope is not Active is mid-publication. Apply refuses it before opening
/// the engine, so an unfinished transition is never overlaid with a second one.
#[test]
fn sparse_apply_refuses_a_store_awaiting_activation() {
    let (old, new) = sparse_images();
    let old = marrow_verify::verify(&old).expect("old image");
    let new = marrow_verify::verify(&new).expect("new image");
    let ceiling = marrow_image::CeilingDescriptor::from_payload(&accepted_ceiling(&old))
        .expect("old ceiling")
        .expanded(
            &new.demand_union(),
            crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
        )
        .expect("bounded union")
        .ceiling_id();
    let scratch = Scratch::new("apply");
    let instance = StoreInstanceId::draw().expect("instance");
    provision(scratch.store(), request(&old, instance)).expect("provision");
    populate_counter(scratch.store(), &old);
    apply_observed(
        scratch.store(),
        prepare(old.clone()),
        prepare(new.clone()),
        Some(ceiling),
        cut(Step::RebindPending),
    )
    .expect_err("publication barrier fails");
    let record = EnvelopeRecord::decode(
        &std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("actual envelope"),
    )
    .expect("envelope");
    assert!(matches!(record.state, EnvelopeState::Rebind { .. }));
    let before = store_bytes(scratch.store());
    let error = apply(scratch.store(), prepare(old), prepare(new), Some(ceiling))
        .expect_err("a store awaiting activation admits no apply");
    assert!(
        matches!(
            error,
            ApplyError::Lifecycle(LifecycleError::Audit(AuditError::Open(
                crate::OpenError::ActivationRequired { instance: found }
            ))) if found == instance
        ),
        "{error:?}"
    );
    assert_eq!(store_bytes(scratch.store()), before);
}
