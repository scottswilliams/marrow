use marrow_catalog::StructuralSignature;
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::evolution::witness::{RepairReason, Verdict};
use crate::executable::{CheckedSavedMember, CheckedSavedPlace, for_each_place_record};

use super::{Accumulator, catalog_id, member_label, required_catalog_id};

/// One step on the record-rooted descent to a backstop candidate: a plain member, or a keyed
/// layer paged per entry. Paging each keyed step's entry keys at scan time is what makes the
/// descent total over nesting depth, since the static path cannot name them.
#[derive(Clone)]
enum DescentStep {
    Member(CatalogId),
    KeyedLayer(CatalogId),
}

/// A backstop candidate: the record-rooted descent to its subtree, the member id its repair is
/// keyed by, and the typed reason and prose. The descent ends with the candidate's own member
/// segment, always probed as a subtree rather than paged into, so a re-keyed candidate layer is
/// judged as one unit.
struct StructuralCandidate {
    member_id: CatalogId,
    descent: Vec<DescentStep>,
    reason: RepairReason,
    message: String,
}

/// The default-deny structural backstop: fail closed any member whose structural signature
/// diverged, whose old data is still present, and which no targeted classifier already judged.
/// The signature is identity-aware over kind, key shape, and leaf token, so a keyed-layer
/// re-key, a group<->keyed-group reshape, and any unforeseen transition all read as divergence.
/// This catch-all keeps the fail-closed invariant total: a transition v0.1 has no handler for
/// cannot silently activate over existing data.
pub(super) fn classify_structural_backstop(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let mut candidates = Vec::new();
    collect_structural_candidates(place, &place.root_members, &[], acc, &mut candidates)?;
    if candidates.is_empty() {
        return Ok(());
    }
    let store_id = required_catalog_id(&place.store_catalog_id)?;
    let mut populated = vec![false; candidates.len()];
    for_each_place_record(store, place, &mut |identity| {
        for (candidate, present) in candidates.iter().zip(populated.iter_mut()) {
            if !*present && descent_subtree_exists(store, &store_id, identity, &candidate.descent)?
            {
                *present = true;
            }
        }
        Ok(())
    })?;
    for (candidate, present) in candidates.into_iter().zip(populated) {
        if !present {
            // No record holds data under the diverged member's old shape, so nothing is
            // orphaned: an empty store reshapes freely under the current schema.
            continue;
        }
        acc.diagnostic(candidate.member_id.clone(), candidate.message);
        acc.push(
            candidate.member_id,
            Verdict::RepairRequired {
                reason: candidate.reason,
            },
        )?;
    }
    Ok(())
}

/// Whether any record-rooted path the descent names holds a subtree. Plain steps extend the
/// path; a keyed-layer step pages every entry and continues one branch per entry key. An empty
/// layer prunes its branch — nothing below it to orphan.
fn descent_subtree_exists(
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    steps: &[DescentStep],
) -> Result<bool, StoreError> {
    descend_path(store, store_id, identity, &[], steps)
}

fn descend_path(
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    prefix: &[DataPathSegment],
    steps: &[DescentStep],
) -> Result<bool, StoreError> {
    let Some((step, rest)) = steps.split_first() else {
        return store.data_subtree_exists(store_id, identity, prefix);
    };
    match step {
        DescentStep::Member(member_id) => {
            let mut path = prefix.to_vec();
            path.push(DataPathSegment::Member(member_id.clone()));
            descend_path(store, store_id, identity, &path, rest)
        }
        DescentStep::KeyedLayer(layer_id) => {
            let mut layer_path = prefix.to_vec();
            layer_path.push(DataPathSegment::Member(layer_id.clone()));
            for_each_entry_key(store, store_id, identity, &layer_path, |entry_key| {
                let mut entry_path = layer_path.clone();
                entry_path.push(DataPathSegment::Key(entry_key.clone()));
                descend_path(store, store_id, identity, &entry_path, rest)
            })
        }
    }
}

/// Page every existing entry key under `layer_path` in key order, calling `visit` once per
/// entry; `visit` returns `true` to stop early. The loop holds only the current entry key, so
/// an arbitrarily wide layer is paged without materializing its keys.
pub(super) fn for_each_entry_key(
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    layer_path: &[DataPathSegment],
    mut visit: impl FnMut(&SavedKey) -> Result<bool, StoreError>,
) -> Result<bool, StoreError> {
    let mut next = store.data_first_child(store_id, identity, layer_path)?;
    while let Some(entry_key) = next {
        if visit(&entry_key)? {
            return Ok(true);
        }
        next = store.data_next_child(store_id, identity, layer_path, &entry_key)?;
    }
    Ok(false)
}

/// Walk the member tree collecting a backstop candidate for each member whose signature
/// diverged and which no targeted classifier already claimed, recording one [`DescentStep`] per
/// level (keyed ancestors paged, unkeyed groups plain) so interior members stay reachable. Once
/// a member is collected the walk stops descending into it: an enclosing failure subsumes a
/// deeper divergence, so a deeper required leaf does not also emit a misleading data proof.
fn collect_structural_candidates(
    place: &CheckedSavedPlace,
    members: &[CheckedSavedMember],
    descent: &[DescentStep],
    acc: &Accumulator,
    candidates: &mut Vec<StructuralCandidate>,
) -> Result<(), StoreError> {
    for member in members {
        let Some(raw_id) = member.catalog_id.clone() else {
            continue;
        };
        let member_id = catalog_id(&raw_id)?;
        if let Some((accepted, declared)) = acc.struct_divergence(&raw_id)
            && !acc.is_classified(&member_id)
        {
            let (reason, message) = structural_repair(place, member, accepted, declared);
            let mut candidate_descent = descent.to_vec();
            candidate_descent.push(DescentStep::Member(member_id.clone()));
            candidates.push(StructuralCandidate {
                member_id,
                descent: candidate_descent,
                reason,
                message,
            });
            continue;
        }
        if member.is_field() {
            continue;
        }
        let mut child_descent = descent.to_vec();
        child_descent.push(if member.key_params.is_empty() {
            DescentStep::Member(member_id)
        } else {
            DescentStep::KeyedLayer(member_id)
        });
        collect_structural_candidates(
            place,
            &member.group_members,
            &child_descent,
            acc,
            candidates,
        )?;
    }
    Ok(())
}

/// The typed reason and prose for a structural divergence. A change between two non-leaf shapes
/// involving a keyed layer is the keyed-layer analogue of a store re-key, so it carries
/// [`RepairReason::KeyedLayerKeyShapeChange`]; every other divergence carries the general
/// [`RepairReason::StructuralDivergence`].
fn structural_repair(
    place: &CheckedSavedPlace,
    member: &CheckedSavedMember,
    accepted: &str,
    declared: &str,
) -> (RepairReason, String) {
    let label = member_label(place, member);
    let shapes = [accepted, declared].map(marrow_catalog::structural_signature);
    let leaf_involved = shapes
        .iter()
        .any(|shape| matches!(shape, Some(StructuralSignature::Leaf(_))));
    let keyed_involved = shapes
        .iter()
        .any(|shape| matches!(shape, Some(StructuralSignature::KeyedGroup(_))));
    if !leaf_involved && keyed_involved {
        (
            RepairReason::KeyedLayerKeyShapeChange,
            format!(
                "keyed layer `{label}` changed its shape from `{accepted}` to `{declared}`; v0.1 cannot migrate a keyed-layer key shape over saved entries, so this fails closed. Existing entries are keyed by the old shape and the new one addresses none of them — model a new layer and migrate with maintenance code instead"
            ),
        )
    } else {
        (
            RepairReason::StructuralDivergence,
            format!(
                "member `{label}` changed its durable shape from `{accepted}` to `{declared}`; this structural transition has no v0.1 evolution path over saved data, so it fails closed. Model a new member of the new shape and migrate the old data with maintenance code"
            ),
        )
    }
}
