//! Data-attached discharge: classify each evolution obligation against the live
//! store snapshot, read-only. Obligations map to [`Verdict`] roles; discharge never
//! writes — the verdicts and counts feed the witness a future apply consumes.
//!
//! Obligations come from two sources: the members and indexes a [`CheckedSavedPlace`]
//! resolves for each saved root, and the accepted catalog entries current source no
//! longer declares. Both read the same catalog identity facts.
//!
//! Records are streamed, never materialized: a single paged scan probes every
//! required leaf and derives every prospective unique-index key tuple, so the scan
//! retains only bounded per-obligation state.

mod absent_source;

use std::collections::{BTreeSet, HashMap, HashSet};

use marrow_project::{CatalogEntry, CatalogEntryKind, StructuralSignature};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{DataPathSegment, TreeStore};

use super::const_default::default_value_for_leaf;
use super::transform_reads::{TransformReadMember, transform_read_members};
use super::witness::{DefaultValue, ObligationVerdict, RejectedDefault, RepairReason, Verdict};
use crate::StoreLeafKind;
use crate::executable::{
    CheckedSavedIndex, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace,
    checked_activation_root_places,
};
use crate::facts::{StoreIndexKeySource, StoredValueMeaning};
use crate::program::{CheckedProgram, EvolveDefault, EvolveTransform};

/// Cap on failing-record keys a diagnostic names before summarizing the rest, so a
/// large gap stays bounded.
const MAX_NAMED_RECORDS: usize = 16;

/// A fail-closed repair message paired to the obligation it explains by catalog id, so
/// a renderer matches it to a `RepairRequired` verdict by identity, not iteration order.
/// The witness verdicts that cross into apply stay prose-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairDiagnostic {
    pub catalog_id: CatalogId,
    pub message: String,
}

/// The discharge result: per-obligation verdicts, accumulated counts, the touched
/// catalog ids partitioned into data roots and indexes, and the fail-closed diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Discharge {
    pub(crate) verdicts: Vec<ObligationVerdict>,
    pub(crate) counts: super::witness::DischargeCounts,
    pub(crate) changed_root_catalog_ids: Vec<CatalogId>,
    pub(crate) changed_index_catalog_ids: Vec<CatalogId>,
    pub(crate) diagnostics: Vec<RepairDiagnostic>,
}

/// Discharge every evolution obligation `program` implies against `store`,
/// read-only.
pub(crate) fn discharge(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<Discharge, StoreError> {
    let mut acc = Accumulator::new(
        program.catalog.evolve_defaults.clone(),
        program
            .catalog
            .evolve_transforms
            .iter()
            .filter_map(|transform| transform.catalog_id.clone())
            .collect(),
        renamed_member_ids(program),
        accepted_member_leaves(program),
    );
    let enum_members = EnumMembers::collect(program);
    acc.set_shrunk_enums(ShrunkEnums::collect(program, &enum_members));
    acc.set_member_structs(
        accepted_member_structs(program),
        program.catalog.declared_member_structs.clone(),
    );
    let accepted_key_shapes = accepted_store_key_shapes(program);
    for (id, kind) in proposal_changed_catalog_ids(program) {
        acc.insert_affected(&id, kind)?;
    }
    let places = checked_activation_root_places(program);
    for place in &places {
        // A re-keyed store fails closed ahead of the per-record scan: its old records are
        // unreachable under the new key shape, and the store-level repair subsumes them.
        if classify_store_key_shape(program, place, &accepted_key_shapes, &mut acc)? {
            continue;
        }
        discharge_root(store, place, &enum_members, &mut acc)?;
    }
    absent_source::classify_absent_source_entries(program, store, &mut acc)?;
    discharge_transforms(program, store, &places, &enum_members, &mut acc)?;
    // The default-deny backstop runs last, so it fails closed only what no targeted classifier
    // claimed. Any member whose signature changed and still carries no verdict is an unhandled
    // transition, so it cannot silently activate — this keeps the fail-closed invariant total.
    for place in &places {
        classify_structural_backstop(store, place, &mut acc)?;
    }
    Ok(acc.into_discharge())
}

/// Classify every `evolve transform` obligation. A transform recomputes its target per
/// record from the members it reads, so the target is excluded from the presence scan and
/// discharged here as an applyable [`Verdict::Transform`] carrying the read-member ids.
/// Soundness rests on reading the old bytes, so the target is guarded by a decodability
/// proof: every record's stored bytes for each read member must decode under that member's
/// current type, or the target fails closed instead of being classified applyable.
fn discharge_transforms(
    program: &CheckedProgram,
    store: &TreeStore,
    places: &[CheckedSavedPlace],
    enum_members: &EnumMembers,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for transform in &program.catalog.evolve_transforms {
        // The type pass already reported an unresolved target; the lowered body still
        // had its purity checked, but there is no catalog obligation to discharge.
        let Some(target_raw_id) = transform.catalog_id.as_deref() else {
            continue;
        };
        let target_places = transform_places(program, places, transform);
        if target_places.is_empty() {
            // No accepted/proposal activation place uses this resource, so there is no
            // store snapshot for the transform to read.
            continue;
        }
        let target_id = catalog_id(target_raw_id)?;
        let mut read_ids = None;
        let mut records = 0usize;
        let mut undecodable = None;
        for place in target_places {
            let reads = transform_read_members(place, &transform.reads);
            let place_read_ids: Vec<CatalogId> =
                reads.iter().map(|read| read.catalog_id.clone()).collect();
            match &read_ids {
                Some(expected) if expected != &place_read_ids => {
                    return Err(StoreError::Corruption {
                        message: format!(
                            "transform `{}` resolved different read members across stores of the same resource",
                            transform.resource
                        ),
                    });
                }
                Some(_) => {}
                None => read_ids = Some(place_read_ids),
            }
            // The decodability obligation lands on the target, not the read member: a read
            // member often has its own presence verdict, so a second verdict on its id would
            // duplicate it. The target is what cannot be recomputed when a read cannot decode.
            let scan = scan_transform_records(store, place, &reads, enum_members)?;
            records += scan.records;
            if undecodable.is_none() {
                undecodable = scan.undecodable;
            }
        }
        let read_ids = read_ids.unwrap_or_default();
        let verdict = match &undecodable {
            None => {
                acc.counts.records_to_transform += records;
                Verdict::Transform { reads: read_ids }
            }
            Some(sample) => {
                acc.diagnostic(
                    target_id.clone(),
                    format!(
                        "transform `{}` reads a member whose stored value does not decode under its current type (record {sample}); repair that data before activating",
                        transform.resource
                    ),
                );
                Verdict::RepairRequired {
                    reason: RepairReason::UndecodableTransformInput,
                }
            }
        };
        acc.push(target_id, verdict)?;
    }
    Ok(())
}

/// The checked saved places that own a transform's target member, found by the
/// resource the transform names.
fn transform_places<'a>(
    program: &CheckedProgram,
    places: &'a [CheckedSavedPlace],
    transform: &EvolveTransform,
) -> Vec<&'a CheckedSavedPlace> {
    let roots: HashSet<&str> = program
        .modules
        .iter()
        .flat_map(|module| {
            module.stores.iter().filter_map(|store| {
                let resource_path = crate::catalog::resource_path(&module.name, &store.resource);
                (resource_path == transform.resource).then_some(store.root.as_str())
            })
        })
        .collect();
    places
        .iter()
        .filter(|place| roots.contains(place.root.as_str()))
        .collect()
}

/// One transform scan: total record count, and the first record whose stored value for
/// some read member does not decode under its current leaf type. A record that simply
/// lacks a read member places no decodability obligation.
struct TransformScan {
    records: usize,
    undecodable: Option<String>,
}

/// Scan one place's records, counting total and capturing the first undecodable read in
/// scan order for the repair diagnostic.
fn scan_transform_records(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    reads: &[TransformReadMember],
    enum_members: &EnumMembers,
) -> Result<TransformScan, StoreError> {
    let store_id = required_catalog_id(&place.store_catalog_id)?;
    let mut records = 0usize;
    let mut undecodable = None;
    store.for_each_record(&store_id, place.identity_keys.len(), &mut |identity| {
        records += 1;
        if undecodable.is_none() {
            for read in reads {
                let path = [DataPathSegment::Member(read.catalog_id.clone())];
                if let Some(bytes) = store.read_data_value(&store_id, identity, &path)?
                    && !leaf_value_valid(&read.leaf, &bytes, enum_members)
                {
                    undecodable = Some(format_identity(identity));
                    break;
                }
            }
        }
        Ok(())
    })?;
    Ok(TransformScan {
        records,
        undecodable,
    })
}

/// Selectable member identities of each current enum, keyed by the enum id a
/// [`StoreLeafKind::Enum`] leaf carries. A stored value is valid only when its decoded
/// member is still selectable here, so a value naming a member the enum removed, marked
/// `category`, or gave children since the write fails closed.
struct EnumMembers {
    by_enum: HashMap<crate::facts::EnumId, HashSet<String>>,
}

impl EnumMembers {
    fn collect(program: &CheckedProgram) -> Self {
        let mut by_enum: HashMap<crate::facts::EnumId, HashSet<String>> = HashMap::new();
        for member in program.facts.enum_members() {
            let Some(catalog_id) = member.catalog_id.as_ref() else {
                continue;
            };
            if !program.facts.enum_member_is_selectable(member.id) {
                continue;
            }
            by_enum
                .entry(member.enum_id)
                .or_default()
                .insert(catalog_id.clone());
        }
        Self { by_enum }
    }

    /// Whether `member_id` is a current member of the enum. An enum with no recorded members
    /// (unbound first-run) admits any value: there is no accepted snapshot to contradict it.
    fn contains(&self, enum_id: crate::facts::EnumId, member_id: &str) -> bool {
        match self.by_enum.get(&enum_id) {
            Some(members) => members.contains(member_id),
            None => true,
        }
    }

    fn selectable(&self, enum_id: crate::facts::EnumId) -> Option<&HashSet<String>> {
        self.by_enum.get(&enum_id)
    }
}

/// Enum ids whose selectable-member set shrank since acceptance. Such an enum keeps its
/// stable identity, so the leaf token is unchanged and the change is not a retype; but a
/// stored value may name the now-gone member, so optional leaves referencing it must still
/// be scanned for validity. Required enum leaves are always scanned, so this drives only the
/// optional case.
struct ShrunkEnums {
    enums: HashSet<crate::facts::EnumId>,
}

impl ShrunkEnums {
    fn collect(program: &CheckedProgram, current: &EnumMembers) -> Self {
        let enum_id_by_catalog: HashMap<&str, crate::facts::EnumId> = program
            .facts
            .enums()
            .iter()
            .filter_map(|enum_fact| {
                enum_fact
                    .catalog_id
                    .as_deref()
                    .map(|catalog_id| (catalog_id, enum_fact.id))
            })
            .collect();
        let mut enums = HashSet::new();
        for (enum_catalog_id, accepted_ids) in accepted_selectable_enum_members(program) {
            let Some(&enum_id) = enum_id_by_catalog.get(enum_catalog_id.as_str()) else {
                continue;
            };
            let empty = HashSet::new();
            let current_ids = current.selectable(enum_id).unwrap_or(&empty);
            if accepted_ids.iter().any(|id| !current_ids.contains(id)) {
                enums.insert(enum_id);
            }
        }
        Self { enums }
    }

    fn shrank(&self, enum_id: crate::facts::EnumId) -> bool {
        self.enums.contains(&enum_id)
    }
}

/// Selectable members of each accepted enum, keyed by its stable catalog id. The accepted
/// catalog records the member tree only as paths, so selectability is read structurally: a
/// member is selectable iff no other member's path extends it.
fn accepted_selectable_enum_members(program: &CheckedProgram) -> HashMap<String, HashSet<String>> {
    let enum_paths: Vec<(&str, &str)> = program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::Enum)
        .map(|entry| (entry.path.as_str(), entry.stable_id.as_str()))
        .collect();
    let members: Vec<&CatalogEntry> = program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::EnumMember)
        .collect();
    let mut by_enum: HashMap<String, HashSet<String>> = HashMap::new();
    for member in &members {
        let Some((_, enum_catalog_id)) = enum_paths
            .iter()
            .find(|(enum_path, _)| is_member_path_of(&member.path, enum_path))
        else {
            continue;
        };
        if accepted_member_is_selectable(member, &members) {
            by_enum
                .entry((*enum_catalog_id).to_string())
                .or_default()
                .insert(member.stable_id.clone());
        }
    }
    by_enum
}

/// Whether an accepted member is a leaf of the member-path tree — no other member's path
/// extends it. This mirrors the source rule that a member is a category iff it has children,
/// and is the one home for the accepted-side selectability derivation.
fn accepted_member_is_selectable(member: &CatalogEntry, members: &[&CatalogEntry]) -> bool {
    !members
        .iter()
        .any(|other| !std::ptr::eq(*other, member) && is_member_path_of(&other.path, &member.path))
}

/// Whether `path` starts with `ancestor::` and adds at least one segment.
fn is_member_path_of(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .and_then(|tail| tail.strip_prefix("::"))
        .is_some_and(|rest| !rest.is_empty())
}

/// Whether stored bytes are a valid value of a leaf's current type. The enum arm closes the
/// redefinition hole: bytes that structurally decode but name a member the current enum no
/// longer has are not a valid value, so they fail closed rather than decode silently.
fn leaf_value_valid(leaf: &StoreLeafKind, bytes: &[u8], enum_members: &EnumMembers) -> bool {
    match leaf {
        StoreLeafKind::Scalar(scalar) => {
            marrow_store::value::decode_value(bytes, *scalar).is_some()
        }
        StoreLeafKind::Enum { enum_id } => marrow_store::tree::decode_tree_enum_member(bytes)
            .is_ok_and(|member| enum_members.contains(*enum_id, member.member_id().as_str())),
        StoreLeafKind::Identity { arity, .. } => {
            marrow_store::key::decode_identity_payload_arity(bytes, *arity).is_some()
        }
    }
}

/// Stable ids of catalog entries the proposal changes (new, retired, or moved), each
/// tagged with its kind so the accumulator partitions an index from a data root without
/// re-classifying it.
fn proposal_changed_catalog_ids(program: &CheckedProgram) -> Vec<(String, CatalogEntryKind)> {
    let Some(proposal) = &program.catalog.proposal else {
        return Vec::new();
    };
    let accepted: HashMap<(CatalogEntryKind, &str), &CatalogEntry> = program
        .catalog
        .accepted_entries
        .iter()
        .map(|entry| ((entry.kind, entry.path.as_str()), entry))
        .collect();
    proposal
        .entries
        .iter()
        .filter(
            |entry| match accepted.get(&(entry.kind, entry.path.as_str())) {
                Some(prior) => {
                    prior.stable_id != entry.stable_id || prior.lifecycle != entry.lifecycle
                }
                None => true,
            },
        )
        .map(|entry| (entry.stable_id.clone(), entry.kind))
        .collect()
}

/// Raw catalog ids of resource members a rename moved this cycle, detected by a proposal
/// `ResourceMember` whose alias set gained a path the accepted entry lacked. A rename moves
/// catalog identity only — the cells stay under the same id — so these classify as
/// `CatalogOnly` rather than re-proving data presence.
fn renamed_member_ids(program: &CheckedProgram) -> HashSet<String> {
    let Some(proposal) = &program.catalog.proposal else {
        return HashSet::new();
    };
    let accepted_aliases: HashMap<&str, &[String]> = program
        .catalog
        .accepted_entries
        .iter()
        .map(|entry| (entry.stable_id.as_str(), entry.aliases.as_slice()))
        .collect();
    proposal
        .entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::ResourceMember)
        .filter(|entry| {
            let accepted = accepted_aliases
                .get(entry.stable_id.as_str())
                .copied()
                .unwrap_or(&[]);
            entry.aliases.iter().any(|alias| !accepted.contains(alias))
        })
        .map(|entry| entry.stable_id.clone())
        .collect()
}

/// Accepted identity-aware leaf token for each resource member, keyed by raw catalog id:
/// `Some(token)` when the entry was a leaf, `None` when it was a non-leaf. A member absent
/// from the map is brand-new. Discharge compares this against the declared token to catch a
/// leaf type change the new decoder might otherwise reinterpret silently.
fn accepted_member_leaves(program: &CheckedProgram) -> HashMap<String, Option<String>> {
    program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::ResourceMember)
        .map(|entry| {
            (
                entry.stable_id.clone(),
                entry.accepted_leaf_token().map(str::to_string),
            )
        })
        .collect()
}

/// Accepted structural signature for each resource member that records one, keyed by raw
/// catalog id. A member with no recorded signature carries no baseline, so the backstop never
/// fires against it; the proposal freezes the current signature forward so a later change has
/// one. The backstop fails closed only against a recorded baseline the current source diverges
/// from.
fn accepted_member_structs(program: &CheckedProgram) -> HashMap<String, String> {
    program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::ResourceMember)
        .filter_map(|entry| {
            entry
                .accepted_struct
                .clone()
                .map(|signature| (entry.stable_id.clone(), signature))
        })
        .collect()
}

/// Accepted identity-key shape for each store that records one, keyed by raw catalog id. A
/// store with no recorded shape is absent: there is no baseline, and the proposal freezes the
/// current shape forward so the next cycle has one.
fn accepted_store_key_shapes(program: &CheckedProgram) -> HashMap<String, String> {
    program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::Store)
        .filter_map(|entry| {
            entry
                .accepted_key_shape
                .clone()
                .map(|shape| (entry.stable_id.clone(), shape))
        })
        .collect()
}

/// Fail closed when a store's declared identity-key shape no longer matches the shape its
/// records were keyed under, returning whether such a re-key was detected. Identity keys live
/// in the saved path itself, so a record under the old key bytes is unreachable under the new
/// shape. v0.1 has no graceful store-key migration, so this is `RepairRequired` rather than a
/// silent activation that would orphan every record.
fn classify_store_key_shape(
    program: &CheckedProgram,
    place: &CheckedSavedPlace,
    accepted_key_shapes: &HashMap<String, String>,
    acc: &mut Accumulator,
) -> Result<bool, StoreError> {
    let Some(store_catalog_id) = place.store_catalog_id.as_deref() else {
        return Ok(false);
    };
    let Some(accepted) = accepted_key_shapes.get(store_catalog_id) else {
        return Ok(false);
    };
    let Some(declared) = program
        .catalog
        .declared_store_key_shapes
        .get(store_catalog_id)
    else {
        return Ok(false);
    };
    if accepted == declared {
        return Ok(false);
    }
    let store_id = required_catalog_id(&place.store_catalog_id)?;
    acc.diagnostic(
        store_id.clone(),
        format!(
            "store `^{}` changed its identity key shape from `{accepted}` to `{declared}`; v0.1 does not support migrating an identity key shape over saved data, so this fails closed. Existing records are keyed by the old shape and cannot be addressed by the new one — model a new store and migrate with maintenance code instead",
            place.root
        ),
    );
    acc.push(
        store_id,
        Verdict::RepairRequired {
            reason: RepairReason::StoreKeyShapeChange,
        },
    )?;
    Ok(true)
}

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
fn classify_structural_backstop(
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
    store.for_each_record(&store_id, place.identity_keys.len(), &mut |identity| {
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
fn for_each_entry_key(
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
    let shapes = [accepted, declared].map(marrow_project::structural_signature);
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

/// Classify the member-presence and index obligations one saved root carries. A single
/// streaming scan visits each record once, probing every required leaf and deriving every
/// prospective unique-index key tuple; the verdicts fall out of the accumulated state.
fn discharge_root(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    enum_members: &EnumMembers,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let store_id = required_catalog_id(&place.store_catalog_id)?;
    let leaves = required_leaf_obligations(place, acc)?;
    let UniqueIndexPlan {
        probes: unique_indexes,
        unprobeable,
    } = unique_index_plan(place, acc)?;
    let unprobeable: HashSet<CatalogId> = unprobeable.into_iter().collect();

    let mut leaf_state: Vec<LeafScan> = leaves.iter().map(|_| LeafScan::default()).collect();
    let mut index_state: Vec<IndexScan> = unique_indexes
        .iter()
        .map(|_| IndexScan::default())
        .collect();
    let mut scanned = 0usize;

    store.for_each_record(&store_id, place.identity_keys.len(), &mut |identity| {
        scanned += 1;
        for (leaf, state) in leaves.iter().zip(leaf_state.iter_mut()) {
            if leaf.retyped {
                // A retype's old bytes may sit under a different shape than the current member
                // declares, so a cell read at the new shape can miss them. Subtree existence at
                // the member path finds the old data wherever it sits, failing a populated
                // retype of any shape closed.
                if store.data_subtree_exists(&store_id, identity, &leaf.path)? {
                    state.record_present();
                }
                continue;
            }
            let bytes = store.read_data_value(&store_id, identity, &leaf.path)?;
            state.record_read(bytes.as_deref(), leaf.leaf.as_ref(), enum_members, identity);
        }
        for (probe, state) in unique_indexes.iter().zip(index_state.iter_mut()) {
            if let Some(key) = prospective_index_key(store, &store_id, probe, identity)? {
                state.observe(key);
            }
        }
        Ok(())
    })?;

    acc.counts.scanned_records += scanned;

    for (leaf, state) in leaves.into_iter().zip(leaf_state) {
        classify_leaf(leaf, state, acc)?;
    }
    let collisions: HashMap<CatalogId, IndexScan> = unique_indexes
        .into_iter()
        .map(|probe| probe.catalog_id)
        .zip(index_state)
        .collect();
    classify_indexes(place, &collisions, &unprobeable, acc)?;
    discharge_keyed_layers(store, place, enum_members, acc)?;
    Ok(())
}

/// One leaf obligation the scan visits, of any leaf kind. A transform-targeted member is
/// classified eagerly and excluded.
struct LeafObligation {
    catalog_id: CatalogId,
    raw_catalog_id: String,
    path: Vec<DataPathSegment>,
    label: String,
    /// The leaf kind whose bytes the scan validates, or `None` for a non-tokenizable position
    /// (`sequence`/`unknown`). Such a leaf arises only as a retype, so any present cell counts
    /// as populated and the retype check fails it closed.
    leaf: Option<StoreLeafKind>,
    /// Effective requiredness at the member's nesting. An optional leaf becomes an obligation
    /// only when retyped or over a shrunk enum, so the scan can learn whether it has bytes.
    required: bool,
    /// Set when a rename moved this member this cycle: a clean prove reports as catalog-only.
    renamed: bool,
    /// Set when the declared leaf type differs from the accepted one. A populated retyped leaf
    /// requires a transform; an unpopulated one falls back to its presence verdict.
    retyped: bool,
}

/// Leaf obligations at the root and inside unkeyed groups, whose presence cell sits directly
/// under the record node. Keyed-layer leaves are required per entry, so [`discharge_keyed_layers`]
/// scans them separately.
fn required_leaf_obligations(
    place: &CheckedSavedPlace,
    acc: &mut Accumulator,
) -> Result<Vec<LeafObligation>, StoreError> {
    let mut obligations = Vec::new();
    collect_required_leaves(place, &place.root_members, &[], &mut obligations, acc)?;
    Ok(obligations)
}

/// Walk the member tree, emitting leaf obligations for required and retyped leaves of any kind
/// at the root or inside an unkeyed group. Transforms are classified eagerly; keyed members are
/// left to the keyed-layer check.
fn collect_required_leaves(
    place: &CheckedSavedPlace,
    members: &[CheckedSavedMember],
    prefix: &[DataPathSegment],
    obligations: &mut Vec<LeafObligation>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for member in members {
        if !member.key_params.is_empty() {
            continue;
        }
        let Some(raw_id) = member.catalog_id.clone() else {
            continue;
        };
        let member_id = catalog_id(&raw_id)?;
        let mut path = prefix.to_vec();
        path.push(DataPathSegment::Member(member_id.clone()));
        match &member.kind {
            // A leaf that became a group: its old single-cell bytes sit at the member path, so
            // the disappeared-leaf probe steers a populated cell to a transform. But a record
            // with no old leaf value must still presence-scan the group's new required
            // sub-members, so the walk emits the probe AND descends.
            CheckedSavedMemberKind::Group if acc.leaf_disappeared(&raw_id) => {
                emit_disappeared_leaf(place, member, &raw_id, member_id, path.clone(), obligations);
                collect_required_leaves(place, &member.group_members, &path, obligations, acc)?;
            }
            // An unkeyed group whose own signature diverged is owned whole by the backstop;
            // descending would re-judge a deeper leaf the enclosing failure already subsumes.
            CheckedSavedMemberKind::Group if acc.prunes_interior(&raw_id, &member_id) => {}
            CheckedSavedMemberKind::Group => {
                collect_required_leaves(place, &member.group_members, &path, obligations, acc)?;
            }
            CheckedSavedMemberKind::Field { .. } => {
                emit_member_leaf(place, member, &raw_id, member_id, path, obligations, acc)?;
            }
        }
    }
    Ok(())
}

/// The decision discharge makes about one leaf before the scan, shared by the unkeyed and keyed
/// walkers so the rule lives in one place. The obligation's path stays with the caller, which
/// alone knows whether the cell is reached directly or through a keyed entry.
enum MemberLeafOutcome {
    /// A verdict known without scanning: a transform target, or an unchanged optional leaf.
    Eager(Verdict),
    /// No cell to probe here: a non-leaf member or a type error already reported.
    Skip,
    /// A leaf the scan must visit.
    Obligation {
        raw_catalog_id: String,
        label: String,
        leaf: Option<StoreLeafKind>,
        required: bool,
        renamed: bool,
        retyped: bool,
    },
}

/// Classify one `Field` member into the single leaf decision both walkers share, total and
/// uniform over leaf kind. A transform target resolves eagerly out of the scan; otherwise the
/// leaf becomes an obligation when required, retyped, or over a shrunk enum, and an unchanged
/// optional leaf resolves eagerly to a catalog-only move under a rename or a no-op. `required`
/// reflects the effective requiredness at the member's nesting.
fn classify_member_leaf(
    place: &CheckedSavedPlace,
    member: &CheckedSavedMember,
    raw_id: &str,
    required: bool,
    acc: &Accumulator,
) -> MemberLeafOutcome {
    // A transform recomputes this member per record; `discharge_transforms` classifies it.
    if acc.is_transform(raw_id) {
        return MemberLeafOutcome::Skip;
    }
    let renamed = acc.is_renamed(raw_id);
    let retyped = acc.is_retyped(raw_id);
    let leaf = member.leaf.clone();
    // An enum that dropped a selectable member keeps its identity (not a retype), but a stored
    // value may name the gone member, so even an optional unchanged enum leaf must be scanned.
    let enum_shrank = acc.enum_shrank(leaf.as_ref());
    if leaf.is_none() && !retyped {
        // No storable leaf kind and no retype: a non-tokenizable position that did not change
        // type, or a non-leaf member. Nothing to probe; a rename still moves catalog identity.
        return if renamed {
            MemberLeafOutcome::Eager(Verdict::CatalogOnly)
        } else {
            MemberLeafOutcome::Skip
        };
    }
    if !required && !retyped && !enum_shrank {
        // An optional, unchanged leaf carries no obligation: its absence is the sparse-absence
        // contract. A rename still moves catalog identity only.
        return if renamed {
            MemberLeafOutcome::Eager(Verdict::CatalogOnly)
        } else {
            MemberLeafOutcome::Eager(Verdict::NoOp)
        };
    }
    // A required leaf keeps its presence obligation even under a rename; a retyped leaf reports
    // its populated count; an optional shrunk-enum leaf is scanned for stored validity only.
    // `classify_leaf` makes the call from the scan state.
    MemberLeafOutcome::Obligation {
        raw_catalog_id: raw_id.to_string(),
        label: member_label(place, member),
        leaf,
        required,
        renamed,
        retyped,
    }
}

/// Apply the shared leaf decision for one `Field` member: push an eager verdict, skip, or
/// record an obligation at `path`, the data path only the calling walker can build.
fn emit_member_leaf(
    place: &CheckedSavedPlace,
    member: &CheckedSavedMember,
    raw_id: &str,
    member_id: CatalogId,
    path: Vec<DataPathSegment>,
    obligations: &mut Vec<LeafObligation>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let CheckedSavedMemberKind::Field { required } = &member.kind else {
        return Ok(());
    };
    match classify_member_leaf(place, member, raw_id, *required, acc) {
        MemberLeafOutcome::Eager(verdict) => {
            acc.push_leaf(member_id, verdict)?;
        }
        MemberLeafOutcome::Skip => {}
        MemberLeafOutcome::Obligation {
            raw_catalog_id,
            label,
            leaf,
            required,
            renamed,
            retyped,
        } => obligations.push(LeafObligation {
            catalog_id: member_id,
            raw_catalog_id,
            path,
            label,
            leaf,
            required,
            renamed,
            retyped,
        }),
    }
    Ok(())
}

/// Emit the retype obligation for a member that was a plain leaf and is now a non-leaf. The new
/// shape declares no leaf cell (`leaf: None`), so the obligation is a pure retype probe decided
/// by subtree existence at the member path: a populated member fails closed to a transform, an
/// empty one passes. The reshape hazard is the bytes' existence, not requiredness, so the
/// obligation is optional and carries the retype flag alone.
fn emit_disappeared_leaf(
    place: &CheckedSavedPlace,
    member: &CheckedSavedMember,
    raw_id: &str,
    member_id: CatalogId,
    path: Vec<DataPathSegment>,
    obligations: &mut Vec<LeafObligation>,
) {
    obligations.push(LeafObligation {
        catalog_id: member_id,
        raw_catalog_id: raw_id.to_string(),
        path,
        label: member_label(place, member),
        leaf: None,
        required: false,
        renamed: false,
        retyped: true,
    });
}

/// Classify every required leaf inside a keyed layer. A keyed layer applies required-field
/// checks per existing entry, so the obligation is "every entry carries this leaf". The scan
/// pages each layer one entry at a time, holding only the current entry's key path, then
/// classifies each leaf from the accumulated per-entry counts exactly as an unkeyed leaf is.
fn discharge_keyed_layers(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    enum_members: &EnumMembers,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let obligations = keyed_leaf_obligations(place, acc)?;
    if obligations.is_empty() {
        return Ok(());
    }
    // A retype that also changed the keyed SHAPE leaves old data under a path the per-entry
    // scan never visits, so it is probed by subtree existence at its static member path and
    // split out here. A retype that left the keyed shape unchanged carries no static path: its
    // old bytes sit where the per-entry scan descends, so it stays in the scan.
    let (flat_retyped, per_entry): (Vec<_>, Vec<_>) = obligations
        .into_iter()
        .partition(|leaf| leaf.retyped && !leaf.path.is_empty());
    let store_id = required_catalog_id(&place.store_catalog_id)?;
    let mut scan = KeyedScan {
        store,
        store_id: &store_id,
        obligations: &per_entry,
        enum_members,
        state: HashMap::new(),
    };
    let mut flat_state: Vec<LeafScan> = flat_retyped.iter().map(|_| LeafScan::default()).collect();
    store.for_each_record(&store_id, place.identity_keys.len(), &mut |identity| {
        scan.descend(identity, &place.root_members, &[])?;
        for (leaf, state) in flat_retyped.iter().zip(flat_state.iter_mut()) {
            if store.data_subtree_exists(&store_id, identity, &leaf.path)? {
                state.record_present();
            }
        }
        Ok(())
    })?;
    let mut state = scan.state;
    for obligation in per_entry {
        let leaf_scan = state.remove(&obligation.catalog_id).unwrap_or_default();
        classify_leaf(obligation, leaf_scan, acc)?;
    }
    for (obligation, leaf_scan) in flat_retyped.into_iter().zip(flat_state) {
        classify_leaf(obligation, leaf_scan, acc)?;
    }
    Ok(())
}

/// The read-only context of one keyed-layer scan. `state` accumulates per-leaf presence keyed
/// by leaf catalog id, while the descent carries only the varying path arguments.
struct KeyedScan<'a> {
    store: &'a TreeStore,
    store_id: &'a CatalogId,
    obligations: &'a [LeafObligation],
    enum_members: &'a EnumMembers,
    state: HashMap<CatalogId, LeafScan>,
}

impl KeyedScan<'_> {
    /// Descend the member tree of a record or keyed entry, collecting per-entry presence of
    /// each keyed-layer leaf and keyed-leaf-map value. A keyed group pages every entry and
    /// recurses with its key appended; a keyed-leaf-map records the value cell under each entry
    /// key; an unkeyed group descends in place; a top-level leaf records directly.
    fn descend(
        &mut self,
        identity: &[SavedKey],
        members: &[CheckedSavedMember],
        prefix: &[DataPathSegment],
    ) -> Result<(), StoreError> {
        for member in members {
            let Some(raw_id) = member.catalog_id.clone() else {
                continue;
            };
            let member_id = catalog_id(&raw_id)?;
            let mut member_path = prefix.to_vec();
            member_path.push(DataPathSegment::Member(member_id.clone()));
            if !member.key_params.is_empty() {
                // A keyed layer: page each existing entry under the layer path. A keyed group
                // recurses into each entry; a keyed-leaf-map (`map[K, V]`) holds its value
                // directly under the entry key, recorded against the map member's obligation.
                let (store, store_id) = (self.store, self.store_id);
                for_each_entry_key(store, store_id, identity, &member_path, |entry_key| {
                    let mut entry_path = member_path.clone();
                    entry_path.push(DataPathSegment::Key(entry_key.clone()));
                    match &member.kind {
                        CheckedSavedMemberKind::Group => {
                            self.descend(identity, &member.group_members, &entry_path)?;
                        }
                        CheckedSavedMemberKind::Field { .. } => {
                            self.record_leaf(&member_id, identity, &entry_path)?;
                        }
                    }
                    Ok(false)
                })?;
                continue;
            }
            match &member.kind {
                CheckedSavedMemberKind::Group => {
                    self.descend(identity, &member.group_members, &member_path)?;
                }
                CheckedSavedMemberKind::Field { .. } => {
                    self.record_leaf(&member_id, identity, &member_path)?;
                }
            }
        }
        Ok(())
    }

    /// Record one keyed leaf's per-entry presence. A member with no obligation is skipped.
    fn record_leaf(
        &mut self,
        member_id: &CatalogId,
        identity: &[SavedKey],
        member_path: &[DataPathSegment],
    ) -> Result<(), StoreError> {
        let Some(obligation) = self
            .obligations
            .iter()
            .find(|obligation| &obligation.catalog_id == member_id)
        else {
            return Ok(());
        };
        let leaf = obligation.leaf.clone();
        let entry = self.state.entry(member_id.clone()).or_default();
        let bytes = self
            .store
            .read_data_value(self.store_id, identity, member_path)?;
        entry.record_read(bytes.as_deref(), leaf.as_ref(), self.enum_members, identity);
        Ok(())
    }
}

/// The required and retyped leaf obligations that live inside a keyed layer, captured once for
/// the scan. A transform target is classified eagerly and excluded.
fn keyed_leaf_obligations(
    place: &CheckedSavedPlace,
    acc: &mut Accumulator,
) -> Result<Vec<LeafObligation>, StoreError> {
    let mut obligations = Vec::new();
    collect_keyed_leaves(
        place,
        &place.root_members,
        false,
        &[],
        &mut obligations,
        acc,
    )?;
    Ok(obligations)
}

/// Walk the member tree, emitting a keyed-leaf obligation per required or retyped leaf reached
/// through a keyed layer. `in_keyed` becomes true once the walk crosses a keyed layer; a
/// `map[K, V]` member is itself keyed by its own key params. The obligation `path` is the
/// static member path only above the first keyed layer, where a retype probe can find old data
/// of a different shape; a leaf below a keyed ancestor carries none and relies on the per-entry
/// scan, which knows each entry's key.
fn collect_keyed_leaves(
    place: &CheckedSavedPlace,
    members: &[CheckedSavedMember],
    in_keyed: bool,
    prefix: &[DataPathSegment],
    obligations: &mut Vec<LeafObligation>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for member in members {
        let Some(raw_id) = member.catalog_id.clone() else {
            continue;
        };
        let member_id = catalog_id(&raw_id)?;
        let keyed_here = in_keyed || !member.key_params.is_empty();
        // The record-rooted path stays static only above the first keyed layer; below one it
        // would need an unknown entry key. A leaf reached through a keyed ancestor carries no
        // static path: the per-entry scan finds its cell by descending each entry's key, so its
        // obligation path is empty and a retype of it is probed per entry, not by a flat subtree
        // check that would look under the wrong shape. A keyed-leaf-map or keyed group at the
        // root or inside an unkeyed group does carry its full member path, so a retype probe
        // finds old data a shape change left under a different path.
        let mut static_path = prefix.to_vec();
        static_path.push(DataPathSegment::Member(member_id.clone()));
        let obligation_path = if in_keyed {
            Vec::new()
        } else {
            static_path.clone()
        };
        match &member.kind {
            // A leaf that became a keyed layer: its old single-cell bytes sit at the static
            // member path under no entry key. The subtree probe there fails a populated member
            // closed; its keyed sub-members are subsumed, so the scan does not descend.
            CheckedSavedMemberKind::Group if keyed_here && acc.leaf_disappeared(&raw_id) => {
                emit_disappeared_leaf(
                    place,
                    member,
                    &raw_id,
                    member_id,
                    obligation_path,
                    obligations,
                );
            }
            // A keyed layer or group whose own signature diverged is owned whole by the
            // backstop; descending would emit a misleading per-entry proof on a deeper leaf.
            CheckedSavedMemberKind::Group if acc.prunes_interior(&raw_id, &member_id) => {}
            CheckedSavedMemberKind::Group => {
                collect_keyed_leaves(
                    place,
                    &member.group_members,
                    keyed_here,
                    &static_path,
                    obligations,
                    acc,
                )?;
            }
            CheckedSavedMemberKind::Field { .. } if keyed_here => {
                emit_member_leaf(
                    place,
                    member,
                    &raw_id,
                    member_id,
                    obligation_path,
                    obligations,
                    acc,
                )?;
            }
            CheckedSavedMemberKind::Field { .. } => {}
        }
    }
    Ok(())
}

/// Running presence state for one leaf, with a bounded sample of missing/invalid records for
/// the diagnostic. `present_count` answers whether a retyped leaf has any bytes to reinterpret,
/// independent of decodability.
#[derive(Default)]
struct LeafScan {
    missing_count: usize,
    invalid_count: usize,
    present_count: usize,
    sample: Vec<Vec<SavedKey>>,
}

impl LeafScan {
    /// Fold one record's read into the running state, the single owner of the
    /// present/invalid/missing decision both scans share. Bytes valid under the current type are
    /// present; bytes that are not are invalid (and still populated); a non-tokenizable retype
    /// treats any stored cell as present so the retype check sees its old bytes; absent is missing.
    fn record_read(
        &mut self,
        bytes: Option<&[u8]>,
        leaf: Option<&StoreLeafKind>,
        enum_members: &EnumMembers,
        identity: &[SavedKey],
    ) {
        match (bytes, leaf) {
            (None, _) => self.record_missing(identity),
            (Some(bytes), Some(leaf)) if leaf_value_valid(leaf, bytes, enum_members) => {
                self.record_present()
            }
            (Some(_), Some(_)) => self.record_invalid(identity),
            (Some(_), None) => self.record_present(),
        }
    }

    fn record_present(&mut self) {
        self.present_count += 1;
    }

    fn record_missing(&mut self, identity: &[SavedKey]) {
        self.missing_count += 1;
        if self.sample.len() < MAX_NAMED_RECORDS {
            self.sample.push(identity.to_vec());
        }
    }

    fn record_invalid(&mut self, identity: &[SavedKey]) {
        self.invalid_count += 1;
        self.present_count += 1;
        if self.sample.len() < MAX_NAMED_RECORDS {
            self.sample.push(identity.to_vec());
        }
    }
}

/// Classify one leaf from its scan state. A populated retype is checked first and fails closed:
/// its bytes were written under the old type, so the new decoder would silently coerce them; an
/// unpopulated retype falls through to its presence verdict. Otherwise the leaf is proven when
/// every record carries it, a constant default when an `evolve default` supplies a typed fill,
/// else a fail-closed repair. A renamed leaf reports as the catalog-only move it is.
fn classify_leaf(
    leaf: LeafObligation,
    state: LeafScan,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let id = leaf.catalog_id;
    if leaf.retyped && state.present_count > 0 {
        acc.diagnostic(
            id.clone(),
            format!(
                "member `{}` changed leaf type with {} populated record(s); a leaf type change on stored data fails closed. Add a new member of the new type, populate it with an `evolve transform` computed from this one, then retire this member",
                leaf.label, state.present_count
            ),
        );
        acc.push_leaf(
            id,
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform,
            },
        )?;
        return Ok(());
    }
    // An optional leaf places no presence obligation; only a stored now-invalid value (a
    // shrunk-enum scan) fails it closed. A missing optional cell stays harmless.
    if !leaf.required {
        if state.invalid_count > 0 {
            acc.counts.records_lacking_member += state.invalid_count;
            return acc.invalid_stored_value(
                id,
                format!(
                    "member `{}` has {} record(s) whose stored value is not valid under the current type (it names an enum member the current enum no longer has); repair before activating",
                    leaf.label, state.invalid_count
                ),
            );
        }
        let verdict = if leaf.renamed {
            Verdict::CatalogOnly
        } else {
            Verdict::NoOp
        };
        acc.push_leaf(id, verdict)?;
        return Ok(());
    }
    if state.missing_count == 0 && state.invalid_count == 0 {
        let verdict = if leaf.renamed {
            Verdict::CatalogOnly
        } else {
            Verdict::DataProof
        };
        acc.push_leaf(id, verdict)?;
        return Ok(());
    }
    acc.counts.records_lacking_member += state.missing_count + state.invalid_count;
    if state.invalid_count > 0 {
        return acc.invalid_stored_value(
            id,
            format!(
                "required member `{}` has {} record(s) whose stored value is not valid under the current type (it does not decode, or names an enum member the current enum no longer has); repair before activating",
                leaf.label, state.invalid_count
            ),
        );
    }
    let verdict = match acc.default_value_for(&leaf.raw_catalog_id, leaf.leaf.as_ref()) {
        Some(Ok(value)) => {
            acc.counts.records_to_backfill += state.missing_count;
            Verdict::Default { value }
        }
        // The member declares a default the checker cannot encode: a distinct obligation from
        // no default at all, so the verdict names the rejected default by its typed cause.
        Some(Err(reason)) => {
            acc.diagnostic(id.clone(), reason.message().to_string());
            Verdict::RepairRequired {
                reason: RepairReason::DefaultRejected { reason },
            }
        }
        None => {
            acc.diagnostic(
                id.clone(),
                missing_member_message(&leaf.label, state.missing_count, &state.sample),
            );
            Verdict::RepairRequired {
                reason: RepairReason::MissingRequiredMember,
            }
        }
    };
    acc.push_leaf(id, verdict)?;
    Ok(())
}

/// One unique-index obligation to probe during the record scan: the index catalog id and how to
/// read each key column. The scan keys its collision state by `catalog_id`.
struct UniqueIndexProbe {
    catalog_id: CatalogId,
    columns: Vec<IndexKeyColumn>,
}

/// A place's unique-index plan: the indexes the scan can probe for collisions, and the ids of
/// any whose key shape it cannot. An unprobeable unique index fails closed rather than rebuild
/// unchecked, so a uniqueness guarantee is never published without verification.
struct UniqueIndexPlan {
    probes: Vec<UniqueIndexProbe>,
    unprobeable: Vec<CatalogId>,
}

/// How to read one index key column: an identity key by its tuple position, or a top-level
/// member cell decoded by its meaning.
enum IndexKeyColumn {
    Identity {
        position: usize,
    },
    Member {
        path: DataPathSegment,
        meaning: StoredValueMeaning,
        default: Option<DefaultValue>,
    },
}

/// Build the unique-index plan: a unique index whose every column resolves becomes a probe; one
/// with any unresolvable column is recorded unprobeable and fails closed.
fn unique_index_plan(
    place: &CheckedSavedPlace,
    acc: &Accumulator,
) -> Result<UniqueIndexPlan, StoreError> {
    let mut probes = Vec::new();
    let mut unprobeable = Vec::new();
    for index in &place.indexes {
        if !index.unique {
            continue;
        }
        let Some(index_catalog_id) = index.catalog_id.as_deref() else {
            continue;
        };
        let index_id = catalog_id(index_catalog_id)?;
        match index_key_columns(place, index, acc)? {
            Some(columns) => probes.push(UniqueIndexProbe {
                catalog_id: index_id,
                columns,
            }),
            None => unprobeable.push(index_id),
        }
    }
    Ok(UniqueIndexPlan {
        probes,
        unprobeable,
    })
}

/// The key-column readers for one index, or `None` when any column resolves to neither an
/// identity key position nor a top-level plain field. Every v0.1 index key resolves here; a
/// future index over a nested or keyed-layer column would resolve to `None` and fail closed.
fn index_key_columns(
    place: &CheckedSavedPlace,
    index: &CheckedSavedIndex,
    acc: &Accumulator,
) -> Result<Option<Vec<IndexKeyColumn>>, StoreError> {
    let mut columns = Vec::with_capacity(index.keys.len());
    for key in &index.keys {
        match key.source {
            StoreIndexKeySource::IdentityKey => {
                let Some(position) = place
                    .identity_keys
                    .iter()
                    .position(|identity_key| identity_key.name == key.name)
                else {
                    return Ok(None);
                };
                columns.push(IndexKeyColumn::Identity { position });
            }
            StoreIndexKeySource::ResourceMember(_) => {
                let Some(member) = place
                    .root_members
                    .iter()
                    .find(|member| member.name == key.name && member.is_plain_field())
                else {
                    return Ok(None);
                };
                let Some(member_catalog_id) = member.catalog_id.as_deref() else {
                    return Ok(None);
                };
                if acc.is_transform(member_catalog_id) {
                    return Ok(None);
                }
                let default = acc
                    .default_value_for(member_catalog_id, member.leaf.as_ref())
                    .and_then(Result::ok);
                columns.push(IndexKeyColumn::Member {
                    path: DataPathSegment::Member(catalog_id(member_catalog_id)?),
                    meaning: key.value_meaning.clone(),
                    default,
                });
            }
        }
    }
    Ok(Some(columns))
}

/// The full prospective unique-index key tuple a record would publish, or `None` when any
/// column is absent, so the record contributes no entry and cannot collide.
fn prospective_index_key(
    store: &TreeStore,
    store_id: &CatalogId,
    probe: &UniqueIndexProbe,
    identity: &[SavedKey],
) -> Result<Option<Vec<SavedKey>>, StoreError> {
    let mut tuple = Vec::with_capacity(probe.columns.len());
    for column in &probe.columns {
        match column {
            IndexKeyColumn::Identity { position } => {
                let Some(key) = identity.get(*position) else {
                    return Ok(None);
                };
                tuple.push(key.clone());
            }
            IndexKeyColumn::Member {
                path,
                meaning,
                default,
            } => {
                let stored =
                    store.read_data_value(store_id, identity, std::slice::from_ref(path))?;
                let Some(bytes) = stored
                    .as_deref()
                    .or_else(|| default.as_ref().map(|value| value.encoded.as_slice()))
                else {
                    return Ok(None);
                };
                let Some(key) = meaning.stored_key(bytes) else {
                    return Ok(None);
                };
                tuple.push(key);
            }
        }
    }
    Ok(Some(tuple))
}

/// Running collision state for one unique index, keyed by the canonical byte encoding of each
/// key tuple. Every tuple shares the index's arity, so the encoding is an injective identity for
/// the tuple; `collisions` holds the distinct tuples more than one record claims.
#[derive(Default)]
struct IndexScan {
    seen: HashSet<Vec<u8>>,
    collisions: HashSet<Vec<u8>>,
}

impl IndexScan {
    fn observe(&mut self, key: Vec<SavedKey>) {
        let encoded = encode_identity_payload(&key);
        if !self.seen.insert(encoded.clone()) {
            self.collisions.insert(encoded);
        }
    }
}

/// Classify every index the place declares. Each carries a derived-rebuild obligation
/// regardless of uniqueness, so apply rebuilds its entries from the records it covers. A unique
/// index whose prospective tuples collide, or one the scan could not probe, upgrades to a
/// fail-closed repair so a uniqueness guarantee is never published without verification.
fn classify_indexes(
    place: &CheckedSavedPlace,
    collisions: &HashMap<CatalogId, IndexScan>,
    unprobeable: &HashSet<CatalogId>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for index in &place.indexes {
        let Some(index_catalog_id) = index.catalog_id.as_deref() else {
            continue;
        };
        let index_id = catalog_id(index_catalog_id)?;
        let colliding = collisions
            .get(&index_id)
            .map_or(0, |state| state.collisions.len());
        let verdict = if index.unique && colliding > 0 {
            acc.counts.index_collisions += colliding;
            acc.diagnostic(
                index_id.clone(),
                format!(
                    "unique index `{}` has {colliding} colliding key tuple(s); resolve duplicates before activating",
                    index.name
                ),
            );
            Verdict::RepairRequired {
                reason: RepairReason::UniqueIndexCollision,
            }
        } else if unprobeable.contains(&index_id) {
            acc.diagnostic(
                index_id.clone(),
                format!(
                    "unique index `{}` has a key shape the uniqueness scan cannot probe; its collisions cannot be verified, so the change fails closed",
                    index.name
                ),
            );
            Verdict::RepairRequired {
                reason: RepairReason::UniqueIndexUnprobeable,
            }
        } else {
            Verdict::DerivedRebuild
        };
        acc.push_index(index_id, verdict)?;
    }
    Ok(())
}

fn catalog_id(raw: &str) -> Result<CatalogId, StoreError> {
    CatalogId::new(raw).map_err(|_| StoreError::Corruption {
        message: format!("evolution discharge saw an invalid catalog id `{raw}`"),
    })
}

fn required_catalog_id(raw: &Option<String>) -> Result<CatalogId, StoreError> {
    match raw.as_deref() {
        Some(raw) => catalog_id(raw),
        None => Err(StoreError::Corruption {
            message: "evolution discharge required an accepted catalog id".to_string(),
        }),
    }
}

/// A human label for a member obligation: the resource-qualified member name.
fn member_label(place: &CheckedSavedPlace, member: &CheckedSavedMember) -> String {
    format!("{}.{}", place.resource_name, member.name)
}

fn missing_member_message(member: &str, missing: usize, sample: &[Vec<SavedKey>]) -> String {
    let named: Vec<String> = sample
        .iter()
        .map(|identity| format_identity(identity))
        .collect();
    let suffix = if missing > sample.len() {
        format!(" and {} more", missing - sample.len())
    } else {
        String::new()
    };
    format!(
        "required member `{member}` has no value and no default in record(s) {}{suffix}; activation cannot attach data",
        named.join(", ")
    )
}

fn format_identity(identity: &[SavedKey]) -> String {
    let parts: Vec<String> = identity.iter().map(format_key).collect();
    parts.join("/")
}

fn format_key(key: &SavedKey) -> String {
    match key {
        SavedKey::Int(value) => value.to_string(),
        SavedKey::Bool(value) => value.to_string(),
        SavedKey::Str(value) => value.clone(),
        SavedKey::Date(value) => format!("date({value})"),
        SavedKey::Duration(value) => format!("duration({value})"),
        SavedKey::Instant(value) => format!("instant({value})"),
        SavedKey::Bytes(value) => format!("bytes[{}]", value.len()),
    }
}

/// Accumulates verdicts, counts, affected ids, and diagnostics across the families, and
/// resolves `evolve default` fills. Affected ids are typed [`CatalogId`]s validated once on
/// insertion and partitioned into data roots and store indexes, so apply never re-classifies
/// them from current source.
struct Accumulator {
    verdicts: Vec<ObligationVerdict>,
    counts: super::witness::DischargeCounts,
    changed_roots: BTreeSet<CatalogId>,
    changed_indexes: BTreeSet<CatalogId>,
    diagnostics: Vec<RepairDiagnostic>,
    defaults: HashMap<String, marrow_syntax::Expression>,
    transforms: BTreeSet<String>,
    renamed: HashSet<String>,
    accepted_leaves: HashMap<String, Option<String>>,
    accepted_structs: HashMap<String, String>,
    declared_structs: HashMap<String, String>,
    classified: HashSet<CatalogId>,
    shrunk_enums: ShrunkEnums,
}

impl Accumulator {
    fn new(
        defaults: Vec<EvolveDefault>,
        transforms: BTreeSet<String>,
        renamed: HashSet<String>,
        accepted_leaves: HashMap<String, Option<String>>,
    ) -> Self {
        Self {
            verdicts: Vec::new(),
            counts: super::witness::DischargeCounts::default(),
            changed_roots: BTreeSet::new(),
            changed_indexes: BTreeSet::new(),
            diagnostics: Vec::new(),
            defaults: defaults
                .into_iter()
                .map(|default| (default.catalog_id, default.value))
                .collect(),
            transforms,
            renamed,
            accepted_leaves,
            accepted_structs: HashMap::new(),
            declared_structs: HashMap::new(),
            classified: HashSet::new(),
            shrunk_enums: ShrunkEnums {
                enums: HashSet::new(),
            },
        }
    }

    fn set_shrunk_enums(&mut self, shrunk_enums: ShrunkEnums) {
        self.shrunk_enums = shrunk_enums;
    }

    /// Install the accepted and current structural signatures the backstop compares.
    fn set_member_structs(
        &mut self,
        accepted_structs: HashMap<String, String>,
        declared_structs: HashMap<String, String>,
    ) {
        self.accepted_structs = accepted_structs;
        self.declared_structs = declared_structs;
    }

    /// Whether the enum a leaf refers to lost a selectable member this cycle, so even an
    /// optional unchanged leaf over it must be scanned for stored values naming the gone member.
    fn enum_shrank(&self, leaf: Option<&StoreLeafKind>) -> bool {
        matches!(leaf, Some(StoreLeafKind::Enum { enum_id }) if self.shrunk_enums.shrank(*enum_id))
    }

    fn is_transform(&self, catalog_id: &str) -> bool {
        self.transforms.contains(catalog_id)
    }

    fn is_renamed(&self, catalog_id: &str) -> bool {
        self.renamed.contains(catalog_id)
    }

    /// Whether a member's declared leaf type differs from its accepted type, by comparing two
    /// identity-aware tokens (scalar by name, enum/identity by referent stable id and arity), so
    /// a pure enum or store rename is not a retype. A non-leaf member has no declared token and
    /// is never a retype; a member with no accepted identity is brand-new.
    ///
    /// The `Some(None)` arm is the non-leaf-to-leaf transition: an old multi-cell subtree would
    /// be reread as a single leaf cell, so it fails closed the same way a scalar retype does.
    /// This is the appearance half of a leaf retype, symmetric to [`Self::leaf_disappeared`].
    fn is_retyped(&self, catalog_id: &str) -> bool {
        let Some(declared) = self.declared_leaf_token(catalog_id) else {
            return false;
        };
        match self.accepted_leaves.get(catalog_id) {
            None => false,
            Some(None) => true,
            Some(Some(accepted)) => accepted != declared,
        }
    }

    /// The identity-aware leaf token current source declares for member `catalog_id`, decoded
    /// through the signature's single owner so the declared and accepted sides read tokens the
    /// same way. `None` for a non-leaf member or one with no recorded signature.
    fn declared_leaf_token(&self, catalog_id: &str) -> Option<&str> {
        self.declared_structs
            .get(catalog_id)
            .map(String::as_str)
            .and_then(marrow_project::structural_signature_leaf_token)
    }

    /// Whether a member that was a plain leaf has become a non-leaf, leaving its old single-cell
    /// bytes under the now-group/now-layer position where they would be orphaned. This is the
    /// disappearance half of a leaf retype, symmetric to [`Self::is_retyped`]'s `Some(None)`
    /// arm; the subtree-existence probe steers a populated member to a transform.
    fn leaf_disappeared(&self, catalog_id: &str) -> bool {
        matches!(self.accepted_leaves.get(catalog_id), Some(Some(_)))
            && self.declared_leaf_token(catalog_id).is_none()
    }

    /// The typed constant fill for a defaulted member, the typed rejection cause when the
    /// default is not a constant the checker can evaluate, or `None` when no `evolve default`
    /// targets the member. A non-scalar leaf cannot take a constant default; use a transform.
    fn default_value_for(
        &self,
        raw_catalog_id: &str,
        leaf: Option<&StoreLeafKind>,
    ) -> Option<Result<super::witness::DefaultValue, RejectedDefault>> {
        let value = self.defaults.get(raw_catalog_id)?;
        Some(default_value_for_leaf(value, leaf))
    }

    fn insert_affected(
        &mut self,
        raw_catalog_id: &str,
        kind: CatalogEntryKind,
    ) -> Result<(), StoreError> {
        let id = catalog_id(raw_catalog_id)?;
        self.changed_set(kind == CatalogEntryKind::StoreIndex)
            .insert(id);
        Ok(())
    }

    /// Record a verdict for a data-root obligation. Its catalog id joins the changed-root
    /// partition.
    fn push(&mut self, id: CatalogId, verdict: Verdict) -> Result<(), StoreError> {
        self.record(id, verdict, false)
    }

    /// Record or merge a resource-member leaf verdict. A resource shape may be stored by several
    /// roots, so a single member id can be discharged once per root; the counts have already
    /// accumulated per root, while the witness carries one merged catalog-id verdict for apply.
    fn push_leaf(&mut self, id: CatalogId, verdict: Verdict) -> Result<(), StoreError> {
        self.changed_roots.insert(id.clone());
        self.classified.insert(id.clone());
        if let Some(existing) = self
            .verdicts
            .iter_mut()
            .find(|existing| existing.catalog_id == id)
        {
            merge_leaf_verdict(&id, &mut existing.verdict, verdict)?;
            return Ok(());
        }
        self.verdicts.push(ObligationVerdict {
            catalog_id: id,
            verdict,
        });
        Ok(())
    }

    /// Record a verdict for a store-index obligation. Its catalog id joins the changed-index
    /// partition, so apply stamps it as an index rather than a root.
    fn push_index(&mut self, id: CatalogId, verdict: Verdict) -> Result<(), StoreError> {
        self.record(id, verdict, true)
    }

    fn record(
        &mut self,
        id: CatalogId,
        verdict: Verdict,
        is_index: bool,
    ) -> Result<(), StoreError> {
        self.changed_set(is_index).insert(id.clone());
        self.classified.insert(id.clone());
        if self
            .verdicts
            .iter()
            .any(|existing| existing.catalog_id == id)
        {
            return Err(StoreError::Corruption {
                message: format!(
                    "evolution discharge produced duplicate non-leaf verdicts for catalog id `{}`",
                    id.as_str()
                ),
            });
        }
        self.verdicts.push(ObligationVerdict {
            catalog_id: id,
            verdict,
        });
        Ok(())
    }

    /// Whether obligation `id` already carries a verdict from a targeted classifier, so the
    /// backstop fires only on an unclaimed id and never double-judges a member.
    fn is_classified(&self, id: &CatalogId) -> bool {
        self.classified.contains(id)
    }

    /// The accepted and current structural signatures of member `raw_id` when both are recorded
    /// and differ — the divergence the backstop fails closed. `None` when either is absent or
    /// the two match.
    fn struct_divergence(&self, raw_id: &str) -> Option<(&str, &str)> {
        let accepted = self.accepted_structs.get(raw_id)?;
        let declared = self.declared_structs.get(raw_id)?;
        (accepted != declared).then_some((accepted.as_str(), declared.as_str()))
    }

    /// Whether a non-leaf member's interior is owned whole by the backstop and must not be
    /// descended, since a deeper data proof would mislead under an enclosing failure. Applies
    /// only to a pure divergence: a `leaf_disappeared` member is steered by its own retype probe.
    fn prunes_interior(&self, raw_id: &str, member_id: &CatalogId) -> bool {
        self.struct_divergence(raw_id).is_some()
            && !self.is_classified(member_id)
            && !self.leaf_disappeared(raw_id)
    }

    /// Record fail-closed prose keyed by catalog id, so a renderer matches it to the
    /// obligation's `RepairRequired` verdict by identity, not position.
    fn diagnostic(&mut self, id: CatalogId, message: String) {
        self.diagnostics.push(RepairDiagnostic {
            catalog_id: id,
            message,
        });
    }

    /// Fail a leaf closed because a record's stored value is not valid under its current type.
    /// The required and optional arms share this so the verdict construction lives in one place.
    fn invalid_stored_value(&mut self, id: CatalogId, message: String) -> Result<(), StoreError> {
        self.diagnostic(id.clone(), message);
        self.push_leaf(
            id,
            Verdict::RepairRequired {
                reason: RepairReason::InvalidStoredValue,
            },
        )
    }

    fn changed_set(&mut self, is_index: bool) -> &mut BTreeSet<CatalogId> {
        if is_index {
            &mut self.changed_indexes
        } else {
            &mut self.changed_roots
        }
    }

    fn into_discharge(self) -> Discharge {
        Discharge {
            verdicts: self.verdicts,
            counts: self.counts,
            changed_root_catalog_ids: self.changed_roots.into_iter().collect(),
            changed_index_catalog_ids: self.changed_indexes.into_iter().collect(),
            diagnostics: self.diagnostics,
        }
    }
}

fn merge_leaf_verdict(
    id: &CatalogId,
    existing: &mut Verdict,
    incoming: Verdict,
) -> Result<(), StoreError> {
    if *existing == incoming {
        return Ok(());
    }
    match (existing, incoming) {
        (Verdict::RepairRequired { .. }, _) => {}
        (slot, Verdict::RepairRequired { reason }) => {
            *slot = Verdict::RepairRequired { reason };
        }
        (Verdict::Default { .. }, Verdict::DataProof | Verdict::CatalogOnly | Verdict::NoOp) => {}
        (
            slot @ (Verdict::DataProof | Verdict::CatalogOnly | Verdict::NoOp),
            Verdict::Default { value },
        ) => {
            *slot = Verdict::Default { value };
        }
        (Verdict::CatalogOnly, Verdict::DataProof)
        | (Verdict::CatalogOnly, Verdict::NoOp)
        | (Verdict::DataProof, Verdict::CatalogOnly)
        | (Verdict::DataProof, Verdict::NoOp)
        | (Verdict::NoOp, Verdict::CatalogOnly)
        | (Verdict::NoOp, Verdict::DataProof) => {}
        (slot, incoming) => {
            return Err(StoreError::Corruption {
                message: format!(
                    "evolution discharge produced incompatible leaf verdicts for catalog id `{}`: existing `{:?}`, incoming `{:?}`",
                    id.as_str(),
                    slot,
                    incoming
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};

    use super::{Accumulator, catalog_id, classify_indexes, unique_index_plan};
    use crate::StoreLeafKind;
    use crate::evolution::{RepairReason, Verdict};
    use crate::executable::{
        CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedKeyParam, CheckedSavedMember,
        CheckedSavedMemberKind, CheckedSavedPlace, CheckedSavedTerminal,
    };
    use crate::facts::{
        ResourceMemberId, StoreId, StoreIndexId, StoreIndexKeySource, StoredValueMeaning,
    };
    use marrow_store::cell::CatalogId;
    use marrow_store::value::ScalarType;

    fn unique_index(name: &str, catalog_id: &str, key_name: &str) -> CheckedSavedIndex {
        CheckedSavedIndex {
            id: StoreIndexId(0),
            name: name.to_string(),
            catalog_id: Some(catalog_id.to_string()),
            unique: true,
            keys: vec![CheckedSavedIndexKey {
                name: key_name.to_string(),
                source: StoreIndexKeySource::ResourceMember(ResourceMemberId(0)),
                value_meaning: StoredValueMeaning::Scalar(ScalarType::Str),
            }],
        }
    }

    fn place_with_indexes(indexes: Vec<CheckedSavedIndex>) -> CheckedSavedPlace {
        CheckedSavedPlace {
            root: "books".to_string(),
            store_id: StoreId(0),
            store_catalog_id: Some("cat_000000000000000000000000000000aa".to_string()),
            resource_name: "Book".to_string(),
            root_members: vec![CheckedSavedMember {
                id: Some(ResourceMemberId(0)),
                name: "isbn".to_string(),
                key_params: Vec::new(),
                kind: CheckedSavedMemberKind::Field { required: true },
                catalog_id: Some("cat_000000000000000000000000000000bb".to_string()),
                leaf: Some(StoreLeafKind::Scalar(ScalarType::Str)),
                group_members: Vec::new(),
            }],
            members: Vec::new(),
            indexes,
            identity_args: Vec::new(),
            identity_keys: vec![CheckedSavedKeyParam {
                name: "id".to_string(),
                scalar: Some(ScalarType::Int),
            }],
            next_id_shape: String::new(),
            layers: Vec::new(),
            terminal: CheckedSavedTerminal::Record,
            span: marrow_syntax::SourceSpan::default(),
        }
    }

    fn empty_accumulator() -> Accumulator {
        Accumulator::new(Vec::new(), BTreeSet::new(), HashSet::new(), HashMap::new())
    }

    // A unique index whose key resolves to a top-level plain field is probeable; one whose
    // key names a member the place does not declare cannot be probed for collisions, so the
    // plan must route it to `unprobeable` rather than silently treat it as a clean rebuild.
    #[test]
    fn unique_index_with_unresolvable_key_is_unprobeable() {
        let place = place_with_indexes(vec![
            unique_index("byIsbn", "cat_000000000000000000000000000000c1", "isbn"),
            unique_index("byGhost", "cat_000000000000000000000000000000c2", "ghost"),
        ]);

        let acc = empty_accumulator();
        let plan = unique_index_plan(&place, &acc).expect("plan");

        let probed: Vec<&str> = plan
            .probes
            .iter()
            .map(|probe| probe.catalog_id.as_str())
            .collect();
        let unprobeable: Vec<&str> = plan.unprobeable.iter().map(CatalogId::as_str).collect();
        assert_eq!(
            probed,
            ["cat_000000000000000000000000000000c1"],
            "probed {probed:?} unprobeable {unprobeable:?}"
        );
        assert_eq!(
            unprobeable,
            ["cat_000000000000000000000000000000c2"],
            "probed {probed:?} unprobeable {unprobeable:?}"
        );
    }

    // An unprobeable unique index must fail closed: its uniqueness cannot be verified from
    // the snapshot, so the discharge blocks activation rather than rebuilding an unchecked
    // guarantee. A probeable index with no collisions still discharges to a derived rebuild.
    #[test]
    fn unprobeable_unique_index_fails_closed() {
        let place = place_with_indexes(vec![
            unique_index("byIsbn", "cat_000000000000000000000000000000c1", "isbn"),
            unique_index("byGhost", "cat_000000000000000000000000000000c2", "ghost"),
        ]);
        let unprobeable: HashSet<CatalogId> =
            [catalog_id("cat_000000000000000000000000000000c2").unwrap()]
                .into_iter()
                .collect();
        let mut acc = empty_accumulator();

        classify_indexes(&place, &HashMap::new(), &unprobeable, &mut acc).expect("classify");

        let ghost = acc
            .verdicts
            .iter()
            .find(|v| v.catalog_id.as_str() == "cat_000000000000000000000000000000c2")
            .expect("ghost verdict");
        assert!(
            matches!(
                ghost.verdict,
                Verdict::RepairRequired {
                    reason: RepairReason::UniqueIndexUnprobeable
                }
            ),
            "an unprobeable unique index must fail closed, got {:?}",
            ghost.verdict
        );
        let isbn = acc
            .verdicts
            .iter()
            .find(|v| v.catalog_id.as_str() == "cat_000000000000000000000000000000c1")
            .expect("isbn verdict");
        assert!(
            matches!(isbn.verdict, Verdict::DerivedRebuild),
            "a probeable collision-free unique index rebuilds, got {:?}",
            isbn.verdict
        );
    }
}
