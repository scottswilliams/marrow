//! Data-attached discharge: classify each evolution obligation against the live
//! store snapshot, read-only.
//!
//! An obligation is a claim current source and the catalog proposal make about
//! durable data: a member must be present, a new index must be buildable, a retired
//! entry must be approved before its data is dropped, a transform must reshape
//! stored meaning. Discharge reads the snapshot through the typed store API and
//! decides, per obligation, one [`Verdict`] role. It never writes: the verdicts and
//! counts feed the witness a future apply consumes.
//!
//! Obligations come from two sources that the per-family helpers below keep
//! distinct: the members and indexes a [`CheckedSavedPlace`] resolves for each saved
//! root (what current source requires), and the accepted catalog entries that
//! current source no longer declares (a retire, or a dropped sparse field). Both read
//! the same catalog identity facts; neither re-derives identity or re-classifies a
//! store path.
//!
//! Records are streamed, never materialized: a single paged scan probes every
//! required leaf and derives every prospective unique-index key tuple, so the scan
//! retains only bounded per-obligation state.

use std::collections::{BTreeSet, HashMap, HashSet};

use marrow_project::{CatalogEntry, CatalogEntryKind, CatalogLifecycle};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{DataPathSegment, TreeStore};

use super::const_default::eval_const_default;
use super::transform_reads::{TransformReadMember, transform_read_members};
use super::witness::{ObligationVerdict, RepairReason, Verdict};
use crate::StoreLeafKind;
use crate::executable::{
    CheckedSavedIndex, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace,
    checked_saved_root_place,
};
use crate::facts::{StoreIndexKeySource, StoredValueMeaning};
use crate::program::{CheckedProgram, EvolveDefault, EvolveTransform};

/// The most failing-record keys a diagnostic names before summarizing the rest, so
/// a large gap does not produce an unbounded message.
const MAX_NAMED_RECORDS: usize = 16;

/// One fail-closed repair message keyed by the catalog id whose obligation it explains.
/// The witness verdicts that cross into apply stay prose-free; this is the preview-side
/// prose, carried with the identity it describes so a renderer pairs each message to its
/// `RepairRequired` verdict by catalog id rather than by iteration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairDiagnostic {
    pub catalog_id: CatalogId,
    pub message: String,
}

/// The result of discharging every obligation against a snapshot: the per-obligation
/// verdicts that cross into apply, the accumulated counts, the catalog ids the change
/// touches partitioned into data roots and indexes, and the fail-closed diagnostics
/// naming what blocks activation.
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
            .map(|transform| transform.catalog_id.clone())
            .collect(),
        renamed_member_ids(program),
        accepted_member_leaves(program),
        new_member_proposal_ids(program),
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
    for root in source_store_roots(program) {
        let Some(place) = checked_saved_root_place(program, &root, Default::default()) else {
            continue;
        };
        // A store whose catalog id is unbound has not been accepted, so it addresses
        // no durable snapshot to discharge against. Its members carry no obligation
        // until the proposal is accepted and their ids are minted.
        if place.store_catalog_id.is_empty() {
            continue;
        }
        // An identity-key shape change orphans every record addressed by the old key
        // bytes, which v0.1 cannot migrate, so it fails closed ahead of any per-record
        // scan. The old records are unreachable under the new key shape, so a per-member
        // presence scan would read them under a key arity they were never written at and
        // report meaningless verdicts; the store-level repair subsumes them, so the scan
        // is skipped for a re-keyed store.
        if classify_store_key_shape(program, &place, &accepted_key_shapes, &mut acc)? {
            continue;
        }
        let resource_path = qualified_resource_path(program, &place);
        discharge_root(store, &place, &resource_path, &enum_members, &mut acc)?;
    }
    classify_absent_source_entries(program, store, &mut acc)?;
    discharge_transforms(program, store, &enum_members, &mut acc)?;
    // The default-deny structural backstop runs last, after every targeted classifier has had
    // its say, so it fails closed only what nothing else claimed. It keeps the fail-closed
    // invariant total by construction: any member whose structural signature changed and still
    // carries no verdict is an unhandled transition, so it cannot silently activate.
    for root in source_store_roots(program) {
        let Some(place) = checked_saved_root_place(program, &root, Default::default()) else {
            continue;
        };
        if place.store_catalog_id.is_empty() {
            continue;
        }
        classify_structural_backstop(program, store, &place, &mut acc)?;
    }
    Ok(acc.into_discharge())
}

/// Classify every `evolve transform` obligation. A transform recomputes its target per
/// record from the members its body reads, so the target is excluded from the
/// presence scan (its value is being replaced) and discharged here directly. The
/// target becomes an applyable [`Verdict::Transform`] carrying the read-member ids,
/// guarded by a decodability proof: every record's stored bytes for each read member
/// must decode under that member's current type, since the transform's soundness rests
/// on reading those old bytes. A read member with an undecodable record fails closed
/// with a typed repair, and the target is not classified applyable while any read it
/// depends on cannot decode.
fn discharge_transforms(
    program: &CheckedProgram,
    store: &TreeStore,
    enum_members: &EnumMembers,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for transform in &program.catalog.evolve_transforms {
        // An unbound transform (no accepted catalog yet) addresses no snapshot, the
        // same way an unaccepted store does; its purity was still checked at lowering.
        if transform.catalog_id.is_empty() {
            continue;
        }
        let Some(place) = transform_place(program, transform) else {
            continue;
        };
        if place.store_catalog_id.is_empty() {
            continue;
        }
        let target_id = catalog_id(&transform.catalog_id)?;
        let reads = transform_read_members(&place, &transform.reads);
        let read_ids: Vec<CatalogId> = reads.iter().map(|read| read.catalog_id.clone()).collect();
        // One pass over the place proves decodability and counts the records, the way
        // `discharge_root` fuses its presence and index probes. The decodability
        // obligation lands on the transform target, not on the read member: a read member
        // is often a normal required member with its own presence verdict, so a second
        // verdict on its id would duplicate the obligation. The target is what cannot be
        // recomputed when a read it depends on cannot decode, so its verdict carries the
        // proof and the diagnostic names the failing record.
        let scan = scan_transform_records(store, &place, &reads, enum_members)?;
        let verdict = match &scan.undecodable {
            None => {
                // An applyable transform rewrites the target cell for every record under
                // the place, so the witness counts those records and apply re-counts the
                // staged writes against this total.
                acc.counts.records_to_transform += scan.records;
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
        acc.push(target_id, verdict);
    }
    Ok(())
}

/// The checked saved place that owns a transform's target member, found by the
/// resource the transform names.
fn transform_place(
    program: &CheckedProgram,
    transform: &EvolveTransform,
) -> Option<CheckedSavedPlace> {
    let root = program.modules.iter().find_map(|module| {
        module.stores.iter().find_map(|store| {
            let resource_path = crate::resource_type_name(&module.name, &store.resource);
            (resource_path == transform.resource).then(|| store.root.clone())
        })
    })?;
    checked_saved_root_place(program, &root, Default::default())
}

/// The result of the single transform scan: the record count the witness carries, and
/// the first record whose stored value for some read member does not decode under its
/// current leaf type. A record that simply lacks a read member places no decodability
/// obligation; the transform reads what is present.
struct TransformScan {
    records: usize,
    undecodable: Option<String>,
}

/// Prove a transform's reads decode and count its records in one pass over the place,
/// mirroring how `discharge_root` fuses its presence and index probes into a single
/// scan. Every record is counted; the first record (in scan order) carrying an
/// undecodable read value is captured for the repair diagnostic. The count is consumed
/// only when no read fails, so a blocked transform stages nothing.
fn scan_transform_records(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    reads: &[TransformReadMember],
    enum_members: &EnumMembers,
) -> Result<TransformScan, StoreError> {
    let store_id = catalog_id(&place.store_catalog_id)?;
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

/// The selectable member identities of each current enum, keyed by the checker-local
/// enum id a [`StoreLeafKind::Enum`] leaf carries. A stored enum value is valid only when
/// its decoded member identity is still one of these, so a member removed or moved out of
/// the enum since the data was written fails closed instead of decoding to a member the
/// current schema no longer has. Only selectable members are admitted: a value names a
/// concrete leaf, never a `category` or a member that has since gained children, so a stored
/// value naming a now-unselectable member is not a valid value of the current enum.
struct EnumMembers {
    by_enum: HashMap<crate::facts::EnumId, HashSet<String>>,
}

impl EnumMembers {
    fn collect(program: &CheckedProgram) -> Self {
        let mut by_enum: HashMap<crate::facts::EnumId, HashSet<String>> = HashMap::new();
        for member in program.facts.enum_members() {
            if member.catalog_id.is_empty() || !program.facts.enum_member_is_selectable(member.id) {
                continue;
            }
            by_enum
                .entry(member.enum_id)
                .or_default()
                .insert(member.catalog_id.clone());
        }
        Self { by_enum }
    }

    /// Whether `member_id` is a current member of the enum the leaf refers to. A leaf whose
    /// enum has no recorded members (an unbound first-run enum) cannot validate membership,
    /// so it admits the value: there is no accepted snapshot to contradict it yet.
    fn contains(&self, enum_id: crate::facts::EnumId, member_id: &str) -> bool {
        match self.by_enum.get(&enum_id) {
            Some(members) => members.contains(member_id),
            None => true,
        }
    }

    /// The current selectable member catalog ids of the enum, or an empty slice for an
    /// unbound first-run enum with none recorded.
    fn selectable(&self, enum_id: crate::facts::EnumId) -> Option<&HashSet<String>> {
        self.by_enum.get(&enum_id)
    }
}

/// The current enums whose selectable-member set shrank relative to the accepted snapshot,
/// by the checker-local enum id a [`StoreLeafKind::Enum`] leaf carries. An enum that dropped a
/// selectable member this cycle — removed it, marked it `category`, or gave it children —
/// keeps its stable identity, so the leaf token is unchanged and the change is not a retype;
/// but a stored value may name the now-gone member, so every leaf referencing such an enum
/// must be scanned for validity even when it is optional and otherwise unchanged. A required
/// enum leaf is always scanned, so it needs no entry here; this drives the optional case.
struct ShrunkEnums {
    enums: HashSet<crate::facts::EnumId>,
}

impl ShrunkEnums {
    /// Compare each enum's accepted selectable-member set against its current one. A member is
    /// selectable in the accepted snapshot exactly when it is a leaf of the accepted member-path
    /// tree (no other accepted member path extends it), which mirrors the source rule that a
    /// member is a category exactly when it has children. An accepted selectable member whose
    /// catalog id is no longer in the current selectable set means the enum shrank.
    fn collect(program: &CheckedProgram, current: &EnumMembers) -> Self {
        let enum_id_by_catalog: HashMap<&str, crate::facts::EnumId> = program
            .facts
            .enums()
            .iter()
            .filter(|enum_fact| !enum_fact.catalog_id.is_empty())
            .map(|enum_fact| (enum_fact.catalog_id.as_str(), enum_fact.id))
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

/// The selectable member catalog ids of each enum in the accepted snapshot, keyed by the
/// enum's stable catalog id. A member is selectable when it is a leaf of the accepted
/// member-path tree: no other accepted member of the same enum carries its path as a strict
/// prefix. The accepted catalog records the full member tree as paths, so accepted
/// selectability is read from the paths without a separate recorded flag.
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
        let has_child = members.iter().any(|other| {
            !std::ptr::eq(*other, *member) && is_member_path_of(&other.path, &member.path)
        });
        if !has_child {
            by_enum
                .entry((*enum_catalog_id).to_string())
                .or_default()
                .insert(member.stable_id.clone());
        }
    }
    by_enum
}

/// Whether `path` names a member strictly under `ancestor`: it starts with `ancestor::` and
/// adds at least one segment. Used to read the accepted enum member tree from paths alone.
fn is_member_path_of(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .and_then(|tail| tail.strip_prefix("::"))
        .is_some_and(|rest| !rest.is_empty())
}

/// Whether stored bytes are a valid value of a leaf's current type. A scalar decodes by
/// its type; an identity decodes to its key tuple at the referenced arity; an enum decodes
/// to a member identity that must still be a member of the current enum. The enum check is
/// what closes the redefinition hole: bytes that structurally decode but name a member the
/// current enum no longer has are not a valid value of the current type.
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

/// The stable ids of catalog entries the proposal changes relative to the accepted
/// snapshot, each tagged with its catalog entry kind: an entry that is new, retired, or
/// whose identity moved. These are the catalog ids the change touches, so a future
/// apply re-verifies exactly them; the kind lets the accumulator partition a store
/// index from a data root without re-classifying it.
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

/// The stable ids of resource members a rename moved this cycle, as their raw catalog
/// id strings. A rename preserves the member's stable id and carries its old path
/// forward as an alias, so a proposal `ResourceMember` entry whose alias set gained a
/// path the accepted entry did not carry is one this evolution renamed. The rename
/// moves catalog identity only — the cells stay under the same id — so discharge
/// classifies these as `CatalogOnly` instead of re-proving their data presence.
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

/// The accepted identity-aware leaf token recorded for each resource member in the accepted
/// snapshot, keyed by its raw catalog id, as `Some(token)` when the entry was a leaf and `None`
/// when it was a non-leaf (a group or keyed group records no leaf token). The token is derived
/// from the member's structural signature, the one durable field that records it. A member absent
/// from this map is brand-new (it has no accepted identity yet). The discharge compares this
/// against the declared token to detect a type change the new type's decoder might otherwise
/// silently reinterpret. Under clean-break v0.1 every accepted leaf member records its token, so a
/// leaf member carrying `None` cannot arise from normal use; treating it as a fail-closed retype is
/// a defensive guard against a hand-edited catalog, not a migration path for a legacy snapshot.
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

/// The accepted identity-aware structural signature recorded for each resource member, keyed
/// by its raw catalog id, only for members that record one. A member with no recorded
/// signature carries no baseline (accepted before signatures were recorded, or brand-new this
/// cycle), so the backstop never fires against it — it freezes the current signature forward so
/// a later change has a baseline, exactly as the store key shape does. The backstop fail-closes
/// only against a recorded baseline that the current source diverges from.
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

/// The proposal-minted stable id of each brand-new resource member, keyed by its
/// module-qualified catalog path. A member current source adds but the accepted catalog does
/// not yet carry has no bound facts id, so its checked place carries an empty `catalog_id` and
/// the presence scan cannot reach it by id alone. Its identity lives only in the proposal, so
/// discharge resolves it here by source path: a brand-new required member is then presence-
/// scanned against existing records, failing closed when they lack it and no default covers it
/// (the add-required-field obligation). A member whose stable id is already in the accepted
/// snapshot is not brand-new and is reached by its bound id as before, so it is omitted.
fn new_member_proposal_ids(program: &CheckedProgram) -> HashMap<String, String> {
    let Some(proposal) = &program.catalog.proposal else {
        return HashMap::new();
    };
    let accepted: HashSet<&str> = program
        .catalog
        .accepted_entries
        .iter()
        .map(|entry| entry.stable_id.as_str())
        .collect();
    proposal
        .entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::ResourceMember)
        .filter(|entry| entry.lifecycle == CatalogLifecycle::Active)
        .filter(|entry| !accepted.contains(entry.stable_id.as_str()))
        .map(|entry| (entry.path.clone(), entry.stable_id.clone()))
        .collect()
}

/// The accepted identity-key shape recorded for each store in the accepted snapshot, keyed
/// by its raw catalog id, as `Some(shape)` when the entry records one. A store with no
/// recorded shape (accepted before key shapes were recorded) is absent from the map: there
/// is no baseline to compare against, and the proposal freezes the current shape forward, so
/// the next cycle has one. The discharge fails closed only against a recorded baseline.
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
/// durable records were keyed under, returning whether such a re-key was detected. The
/// identity keys live in the saved path itself, so a record written under the old key bytes
/// is unreachable under the new shape — a different arity or any different key type addresses
/// no existing record. v0.1 has no graceful store-key migration, so the obligation is
/// `RepairRequired` rather than a silent activation that would orphan every record. A store
/// with no recorded accepted shape carries no baseline to compare, so it places no obligation;
/// the proposal records its current shape so a later re-key has a baseline.
fn classify_store_key_shape(
    program: &CheckedProgram,
    place: &CheckedSavedPlace,
    accepted_key_shapes: &HashMap<String, String>,
    acc: &mut Accumulator,
) -> Result<bool, StoreError> {
    let Some(accepted) = accepted_key_shapes.get(&place.store_catalog_id) else {
        return Ok(false);
    };
    let Some(declared) = program
        .catalog
        .declared_store_key_shapes
        .get(&place.store_catalog_id)
    else {
        return Ok(false);
    };
    if accepted == declared {
        return Ok(false);
    }
    let store_id = catalog_id(&place.store_catalog_id)?;
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
    );
    Ok(true)
}

/// One step on the record-rooted descent to a structural-backstop candidate. A plain member or
/// unkeyed group descends by member id alone; a keyed layer is paged per entry, so the populated
/// probe expands it into one branch per existing entry key. The descent is what makes the
/// backstop total over nesting depth: an interior member below any number of keyed layers is
/// reached by resolving each keyed step's entry keys at scan time, since the static path cannot
/// name them.
#[derive(Clone)]
enum DescentStep {
    Member(CatalogId),
    KeyedLayer(CatalogId),
}

/// One member the structural backstop must fail closed when its data is populated: the
/// record-rooted descent to its subtree, the member id its repair is keyed by, and the typed
/// reason and prose. The descent ends with the candidate's own member segment, always probed as
/// a subtree rather than paged into, so a re-keyed candidate layer is judged as one unit. A
/// candidate is collected only when the member's signature diverged and no targeted classifier
/// already claimed it; the populated check happens in a single record scan.
struct StructuralCandidate {
    member_id: CatalogId,
    descent: Vec<DescentStep>,
    reason: RepairReason,
    message: String,
}

/// The default-deny structural backstop: fail closed any durable member whose structural
/// signature changed, whose old data is still present, and which no targeted classifier
/// already judged. The signature records kind, key shape, and leaf token identity-aware, so a
/// keyed-layer re-key, a group<->keyed-group reshape, and any unforeseen structural transition
/// all read as a divergence. This is the catch-all that keeps the fail-closed invariant total
/// by construction: a transition v0.1 has no specific handler for cannot silently activate over
/// existing data, while the additive and identity-preserving changes the targeted classifiers
/// bless (an optional add, a rename, a retype, a reorder) keep their verdicts and are not
/// re-judged here.
fn classify_structural_backstop(
    program: &CheckedProgram,
    store: &TreeStore,
    place: &CheckedSavedPlace,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let resource_path = qualified_resource_path(program, place);
    let mut candidates = Vec::new();
    collect_structural_candidates(
        place,
        &resource_path,
        &place.root_members,
        &[],
        &[],
        acc,
        &mut candidates,
    )?;
    if candidates.is_empty() {
        return Ok(());
    }
    let store_id = catalog_id(&place.store_catalog_id)?;
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
            // No record holds data under the diverged member's old shape, so there is nothing to
            // orphan: an empty store reshapes freely. A new write under the new shape is governed
            // by the current schema, not this backstop.
            continue;
        }
        acc.diagnostic(candidate.member_id.clone(), candidate.message);
        acc.push(
            candidate.member_id,
            Verdict::RepairRequired {
                reason: candidate.reason,
            },
        );
    }
    Ok(())
}

/// Whether any concrete record-rooted path the descent names holds a subtree. A plain member
/// step extends the path by that member id; a keyed-layer step pages every existing entry under
/// the path so far and continues one branch per entry key, since the layer's interior is
/// addressed per entry by a key the static path cannot name. The candidate's own member segment
/// is the last step and is probed as a subtree, never paged into, so a re-keyed candidate layer
/// is judged whole. An empty layer prunes its branch — there is nothing below it to orphan.
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
            for entry_key in store.data_child_keys(store_id, identity, &layer_path)? {
                let mut entry_path = layer_path.clone();
                entry_path.push(DataPathSegment::Key(entry_key));
                if descend_path(store, store_id, identity, &entry_path, rest)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

/// Walk the member tree collecting a backstop candidate for each member whose structural
/// signature diverged and which no targeted classifier already claimed. The walk descends
/// through both unkeyed groups and keyed layers, recording one [`DescentStep`] per level so an
/// interior member below any number of keyed layers is reachable per entry — this is what keeps
/// the backstop total over nesting depth. A keyed-layer ancestor becomes a paged step; an
/// unkeyed group or plain layer becomes a plain member step. The candidate's own subtree is the
/// unit the backstop judges, so once a member fails closed the walk does not descend into its
/// interior: an enclosing layer's failure subsumes a deeper divergence, so a deeper required
/// leaf does not also emit a misleading data proof. A pure leaf carries no interior to walk.
fn collect_structural_candidates(
    place: &CheckedSavedPlace,
    resource_path: &str,
    members: &[CheckedSavedMember],
    descent: &[DescentStep],
    names: &[&str],
    acc: &Accumulator,
    candidates: &mut Vec<StructuralCandidate>,
) -> Result<(), StoreError> {
    for member in members {
        let mut member_names = names.to_vec();
        member_names.push(member.name.as_str());
        let Some(raw_id) =
            resolved_member_id(resource_path, member, &member_names, acc.new_members())
        else {
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
            resource_path,
            &member.group_members,
            &child_descent,
            &member_names,
            acc,
            candidates,
        )?;
    }
    Ok(())
}

/// The typed reason and prose for a structural divergence the backstop fails closed. A change
/// between two non-leaf shapes that involves a keyed layer — a keyed-layer re-key, or a
/// group<->keyed-group reshape — is the keyed-layer analogue of a store re-key, so it carries
/// [`RepairReason::KeyedLayerKeyShapeChange`]. Every other unhandled divergence carries the
/// general [`RepairReason::StructuralDivergence`].
fn structural_repair(
    place: &CheckedSavedPlace,
    member: &CheckedSavedMember,
    accepted: &str,
    declared: &str,
) -> (RepairReason, String) {
    let label = member_label(place, member);
    let leaf_involved = accepted.starts_with("leaf:") || declared.starts_with("leaf:");
    let keyed_involved =
        accepted.starts_with("keyed-group:") || declared.starts_with("keyed-group:");
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

/// The module-qualified catalog path of the resource a place addresses (`module::Resource`),
/// the prefix every member's catalog path shares. A brand-new member's proposal path is
/// `module::Resource::member...`, so this is how the presence scan reconstructs that path to
/// resolve the member's proposal id when its facts id is not yet bound.
fn qualified_resource_path(program: &CheckedProgram, place: &CheckedSavedPlace) -> String {
    let resource = program.facts.resource(place.resource_id);
    let module = &program.modules[resource.module.0 as usize];
    crate::catalog::resource_path(&module.name, &resource.name)
}

/// The saved-store roots current source declares, in declaration order.
fn source_store_roots(program: &CheckedProgram) -> Vec<String> {
    program
        .modules
        .iter()
        .flat_map(|module| module.stores.iter().map(|store| store.root.clone()))
        .collect()
}

/// Classify the member-presence and index obligations a single saved root carries.
/// A single streaming scan visits each record once, probing every required leaf and
/// deriving every prospective unique-index key tuple; the verdicts fall out of the
/// accumulated counts and key tuples after the scan.
fn discharge_root(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    resource_path: &str,
    enum_members: &EnumMembers,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let store_id = catalog_id(&place.store_catalog_id)?;
    let leaves = required_leaf_obligations(place, resource_path, acc)?;
    let UniqueIndexPlan {
        probes: unique_indexes,
        unprobeable,
    } = unique_index_plan(place)?;
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
                // A retype's old bytes may sit under a different shape than the current
                // member declares — a plain cell where the new shape pages keyed entries,
                // or keyed entries where the new shape reads a plain cell — so the cell read
                // at the current shape can miss them. Subtree existence at the member path
                // finds the old data wherever it physically sits, so a populated retype of any
                // shape fails closed.
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
        classify_leaf(leaf, state, acc);
    }
    let collisions: HashMap<CatalogId, IndexScan> = unique_indexes
        .into_iter()
        .map(|probe| probe.catalog_id)
        .zip(index_state)
        .collect();
    classify_indexes(place, &collisions, &unprobeable, acc)?;
    discharge_keyed_layers(store, place, resource_path, enum_members, acc)?;
    Ok(())
}

/// One leaf obligation the scan visits, of any leaf kind: the catalog id its data cells
/// use, the data path from the record node to the leaf cell, a human label, and the flags
/// that pick its verdict. A transform-targeted member is classified eagerly and excluded.
struct LeafObligation {
    catalog_id: CatalogId,
    raw_catalog_id: String,
    path: Vec<DataPathSegment>,
    label: String,
    /// The leaf kind whose bytes the scan validates, or `None` for a leaf position whose
    /// declared type is non-tokenizable (a `sequence`/`unknown`). A non-tokenizable leaf
    /// arises only as a retype: there is no current type to decode the old bytes under, so
    /// any present cell counts as populated and the retype check fails it closed.
    leaf: Option<StoreLeafKind>,
    /// Effective requiredness at the member's nesting. A required leaf that is missing or
    /// undecodable is a repair; an optional leaf's absence is harmless. An optional leaf
    /// becomes an obligation only when it is retyped, so the scan can learn whether it has
    /// bytes to reinterpret.
    required: bool,
    /// Set when a rename moved this member this cycle. A clean prove of a renamed leaf
    /// reports the operative change as catalog-only; a failed decode still repairs.
    renamed: bool,
    /// Set when this member's declared leaf type differs from the type its durable bytes
    /// were accepted as. A populated retyped leaf requires an explicit transform; an
    /// unpopulated one has no bytes to reinterpret and falls back to its presence verdict.
    /// The scan supplies the populated count so the decision composes uniformly with
    /// required and optional leaves at any nesting.
    retyped: bool,
}

/// The invariant context of a member-tree walk: the saved place being discharged and the
/// module-qualified resource path its members hang under. Bundling the two read-only refs
/// keeps each recursive walker to its varying arguments (the member slice, the data-path
/// prefix, and the name chain) rather than re-threading the invariants at every level.
struct LeafWalk<'a> {
    place: &'a CheckedSavedPlace,
    resource_path: &'a str,
}

/// The required-leaf obligations at the root and inside unkeyed groups: leaves whose
/// presence cell sits directly under the record node. A required leaf inside a keyed
/// layer is required per existing entry, not for the record, so it is scanned and
/// classified separately by [`discharge_keyed_layers`].
fn required_leaf_obligations(
    place: &CheckedSavedPlace,
    resource_path: &str,
    acc: &mut Accumulator,
) -> Result<Vec<LeafObligation>, StoreError> {
    let walk = LeafWalk {
        place,
        resource_path,
    };
    let mut obligations = Vec::new();
    collect_required_leaves(&walk, &place.root_members, &[], &[], &mut obligations, acc)?;
    Ok(obligations)
}

/// Walk the member tree, emitting a leaf obligation for each required leaf and each
/// retyped leaf — of any kind, scalar, enum, or identity — at the root or inside an unkeyed
/// group, building the nested data-path chain to its cell. `names` is the member name chain
/// from the resource down to here, used to resolve a brand-new member's proposal id. A
/// transform-targeted member is classified eagerly and not scanned. A keyed member is left to
/// the keyed-layer check.
fn collect_required_leaves(
    walk: &LeafWalk,
    members: &[CheckedSavedMember],
    prefix: &[DataPathSegment],
    names: &[&str],
    obligations: &mut Vec<LeafObligation>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for member in members {
        if !member.key_params.is_empty() {
            continue;
        }
        let mut member_names = names.to_vec();
        member_names.push(member.name.as_str());
        // A brand-new member current source adds has no bound facts id, so its checked
        // `catalog_id` is empty; resolve its proposal-minted id by source path so a new
        // required member is still presence-scanned against existing records. A member with
        // neither a bound id nor a proposal id (a pending first-run identity with no proposal)
        // anchors no obligation.
        let Some(raw_id) =
            resolved_member_id(walk.resource_path, member, &member_names, acc.new_members())
        else {
            continue;
        };
        let member_id = catalog_id(&raw_id)?;
        let mut path = prefix.to_vec();
        path.push(DataPathSegment::Member(member_id.clone()));
        match &member.kind {
            // A group at this position whose accepted snapshot recorded a leaf token is a
            // plain leaf that became a group: its old single-cell bytes still sit at the
            // member path the group now occupies. The subtree-existence probe at that path
            // steers a member whose old leaf cell holds bytes to a transform, the same
            // fail-closed path a non-leaf-becoming-leaf retype takes. But a record whose old
            // leaf cell was never populated has no bytes for that probe to find, so the new
            // group's brand-new required sub-members must ALSO be presence-scanned: a record
            // that exists without the old leaf value fails closed on the missing required
            // sub-member rather than activating over an unpopulated old cell. So the walk emits
            // the disappeared-leaf probe AND descends into the new group's members.
            CheckedSavedMemberKind::Group if acc.leaf_disappeared(&raw_id) => {
                emit_disappeared_leaf(
                    walk.place,
                    member,
                    &raw_id,
                    member_id,
                    path.clone(),
                    obligations,
                );
                collect_required_leaves(
                    walk,
                    &member.group_members,
                    &path,
                    &member_names,
                    obligations,
                    acc,
                )?;
            }
            // An unkeyed group whose own structural signature diverged is owned whole by the
            // backstop; descending into it would re-judge a deeper required leaf the enclosing
            // failure already subsumes, so its interior is left unwalked here.
            CheckedSavedMemberKind::Group if acc.prunes_interior(&raw_id, &member_id) => {}
            CheckedSavedMemberKind::Group => {
                collect_required_leaves(
                    walk,
                    &member.group_members,
                    &path,
                    &member_names,
                    obligations,
                    acc,
                )?;
            }
            CheckedSavedMemberKind::Field { .. } => {
                // The cell sits directly under the record node, so the obligation path
                // is the full nested member chain to it.
                emit_member_leaf(
                    walk.place,
                    member,
                    &raw_id,
                    member_id,
                    path,
                    obligations,
                    acc,
                );
            }
        }
    }
    Ok(())
}

/// The raw catalog id to scan a member under: its bound facts id when source already carries
/// one, else the proposal-minted id of a brand-new member at this source path. `None` when the
/// member has neither — a pending first-run identity with no proposal to mint from.
fn resolved_member_id(
    resource_path: &str,
    member: &CheckedSavedMember,
    names: &[&str],
    new_members: &HashMap<String, String>,
) -> Option<String> {
    if !member.catalog_id.is_empty() {
        return Some(member.catalog_id.clone());
    }
    let source_path = format!("{resource_path}::{}", names.join("::"));
    new_members.get(&source_path).cloned()
}

/// One leaf and the decision discharge makes about it before the scan, shared by the
/// unkeyed and keyed walkers so the rule lives in one place.
enum MemberLeafOutcome {
    /// A verdict known without scanning: a transform-targeted leaf, or an unchanged optional
    /// leaf whose absence is the sparse-absence contract.
    Eager(Verdict),
    /// A leaf with no cell to probe here: a member that resolved no storable leaf kind (a
    /// non-leaf member, or a type error already reported).
    Skip,
    /// A leaf the scan must visit, of any kind. The path stays with the caller, which alone
    /// knows whether the cell is reached directly or through a keyed entry. `renamed`
    /// marks a leaf a rename moved this cycle, so a clean prove reports the operative
    /// change as identity-only rather than a fresh data proof. `retyped` marks a leaf
    /// whose declared type changed across any leaf kind; the scan's populated count then
    /// decides whether it needs a transform.
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
/// uniform over leaf kind. A transform target resolves eagerly out of the scan. Otherwise
/// the leaf becomes a scan obligation whenever it is required (so its presence and
/// decodability are proven, an enum value's member-validity included) or retyped (so the
/// populated probe decides whether it needs a transform); a scalar, an enum, and an
/// identity are treated alike. An optional, non-retyped leaf places no obligation: its
/// absence is harmless, so it resolves eagerly to a catalog-only move under a rename or a
/// no-op. `required` reflects the effective requiredness at the member's nesting (a
/// keyed-layer leaf is required per existing entry).
fn classify_member_leaf(
    place: &CheckedSavedPlace,
    member: &CheckedSavedMember,
    raw_id: &str,
    required: bool,
    acc: &Accumulator,
) -> MemberLeafOutcome {
    // A transform recomputes this member per record, so its presence before the
    // evolution is irrelevant: skip it from the presence scan and let
    // `discharge_transforms` classify it from the decodability of the members it reads.
    if acc.is_transform(raw_id) {
        return MemberLeafOutcome::Skip;
    }
    let renamed = acc.is_renamed(raw_id);
    let retyped = acc.is_retyped(raw_id);
    let leaf = member.leaf.clone();
    // An enum leaf whose enum dropped a selectable member this cycle must be scanned for a
    // stored value naming the gone member, even when it is optional and otherwise unchanged.
    // The enum keeps its identity, so this is not a retype; the validity check during the scan
    // fails a now-invalid stored value closed. A non-enum leaf is never a shrunk-enum scan.
    let enum_shrank = acc.enum_shrank(leaf.as_ref());
    if leaf.is_none() && !retyped {
        // No storable leaf kind resolved and no retype: a non-tokenizable leaf position
        // (a `sequence`/`unknown`) that did not change type, or a non-leaf member. There is
        // no current cell to probe and no old bytes to reinterpret. A rename still moves
        // catalog identity only. A non-tokenizable leaf that DID change type falls through to
        // the obligation below so its populated old bytes fail the retype check closed. A
        // leaf with no kind cannot be an enum, so a shrunk enum never applies here.
        return if renamed {
            MemberLeafOutcome::Eager(Verdict::CatalogOnly)
        } else {
            MemberLeafOutcome::Skip
        };
    }
    if !required && !retyped && !enum_shrank {
        // An optional, unchanged leaf of any kind carries no obligation: its absence is the
        // sparse-absence contract. A rename still moves catalog identity only.
        return if renamed {
            MemberLeafOutcome::Eager(Verdict::CatalogOnly)
        } else {
            MemberLeafOutcome::Eager(Verdict::NoOp)
        };
    }
    // A required leaf of any kind keeps its presence obligation even under a rename, so its
    // bytes are proven present and valid under the current type. A retyped leaf — required
    // or optional — also becomes an obligation so the scan reports whether it is populated.
    // An optional enum leaf over a shrunk enum is scanned for stored validity only: its
    // absence is harmless, so `classify_leaf` must not treat a missing optional cell as a
    // repair; only a stored now-invalid value fails it closed. `classify_leaf` makes the call.
    MemberLeafOutcome::Obligation {
        raw_catalog_id: raw_id.to_string(),
        label: member_label(place, member),
        leaf,
        required,
        renamed,
        retyped,
    }
}

/// Apply the shared leaf decision for one `Field` member: push an eager verdict, skip
/// a non-scalar leaf, or record an obligation at `path`. `raw_id` is the member's bound or
/// proposal-resolved catalog id, and `member_id` its typed form; `path` is the data path to
/// the cell, which only the calling walker can build.
fn emit_member_leaf(
    place: &CheckedSavedPlace,
    member: &CheckedSavedMember,
    raw_id: &str,
    member_id: CatalogId,
    path: Vec<DataPathSegment>,
    obligations: &mut Vec<LeafObligation>,
    acc: &mut Accumulator,
) {
    let CheckedSavedMemberKind::Field { required } = &member.kind else {
        return;
    };
    match classify_member_leaf(place, member, raw_id, *required, acc) {
        MemberLeafOutcome::Eager(verdict) => {
            acc.push(member_id, verdict);
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
}

/// Emit the retype obligation for a member that was a plain leaf and is now a non-leaf (a
/// group or a keyed layer). The new shape declares no leaf cell, so there is no current type
/// to decode the old bytes under (`leaf: None`); the obligation is purely a retype probe. Its
/// presence is decided by subtree existence at the member path the now-non-leaf occupies, so a
/// populated member fails closed to a transform and an empty one passes. Requiredness is
/// irrelevant — the reshape hazard is the bytes' existence, not their requiredness — so the
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

/// Classify every required leaf inside a keyed layer. A keyed layer applies its
/// required-field checks per existing entry, so the obligation is "every entry that
/// exists carries this leaf", not "every record does". The scan descends each keyed
/// layer one entry at a time through the paged child cursor, holding only the current
/// entry's key path, and classifies each leaf from the accumulated per-entry counts
/// exactly as an unkeyed leaf is: proven, defaulted, or a fail-closed repair.
fn discharge_keyed_layers(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    resource_path: &str,
    enum_members: &EnumMembers,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let obligations = keyed_leaf_obligations(place, resource_path, acc)?;
    if obligations.is_empty() {
        return Ok(());
    }
    // A retype that also changed the keyed SHAPE — a leaf becoming a keyed layer, or a
    // keyed-leaf-map whose value type changed — leaves old data under a path the per-entry scan
    // at the new shape never visits, so it is probed by subtree existence at its static member
    // path and excluded from the per-entry scan. A retype that left the keyed shape unchanged (a
    // leaf nested inside a keyed group whose own type changed) carries no static path: its old
    // per-entry bytes sit exactly where the per-entry scan descends, so it stays in the scan and
    // its populated count comes from the per-entry reads. Every other keyed obligation is
    // required-presence, proven per existing entry.
    let (flat_retyped, per_entry): (Vec<_>, Vec<_>) = obligations
        .into_iter()
        .partition(|leaf| leaf.retyped && !leaf.path.is_empty());
    let store_id = catalog_id(&place.store_catalog_id)?;
    let mut scan = KeyedScan {
        store,
        store_id: &store_id,
        obligations: &per_entry,
        enum_members,
        resource_path,
        new_members: acc.new_members(),
        state: HashMap::new(),
    };
    let mut flat_state: Vec<LeafScan> = flat_retyped.iter().map(|_| LeafScan::default()).collect();
    store.for_each_record(&store_id, place.identity_keys.len(), &mut |identity| {
        scan.descend(identity, &place.root_members, &[], &[])?;
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
        classify_leaf(obligation, leaf_scan, acc);
    }
    for (obligation, leaf_scan) in flat_retyped.into_iter().zip(flat_state) {
        classify_leaf(obligation, leaf_scan, acc);
    }
    Ok(())
}

/// The read-only context of one keyed-layer scan: the store and its root id, the leaf
/// obligations to record, the current enum members the value-validity check consults, and
/// the inputs to resolve a brand-new member's id by source path. `state` accumulates the
/// per-leaf presence keyed by leaf catalog id. Bundling the invariants here keeps the
/// recursive descent to its varying arguments alone.
struct KeyedScan<'a> {
    store: &'a TreeStore,
    store_id: &'a CatalogId,
    obligations: &'a [LeafObligation],
    enum_members: &'a EnumMembers,
    resource_path: &'a str,
    new_members: &'a HashMap<String, String>,
    state: HashMap<CatalogId, LeafScan>,
}

impl KeyedScan<'_> {
    /// Descend the member tree of a record (or a keyed entry) collecting the per-entry
    /// presence of each keyed-layer leaf and keyed-leaf-map value. At a keyed-group member
    /// the scan pages every existing entry under the current data path, then recurses into
    /// the entry with its key appended; at a keyed-leaf-map member it pages every entry and
    /// records the value cell under each entry's key; an unkeyed group is descended in place;
    /// a top-level leaf with an obligation records its presence and value-validity directly.
    /// Each member's id is resolved bound-or-proposal by its source-path name chain, so a
    /// brand-new required leaf inside an existing keyed layer is reached at its proposal id
    /// and fails closed over the populated entries rather than escaping the bound-id scan.
    fn descend(
        &mut self,
        identity: &[SavedKey],
        members: &[CheckedSavedMember],
        prefix: &[DataPathSegment],
        names: &[&str],
    ) -> Result<(), StoreError> {
        for member in members {
            let mut member_names = names.to_vec();
            member_names.push(member.name.as_str());
            let Some(raw_id) =
                resolved_member_id(self.resource_path, member, &member_names, self.new_members)
            else {
                continue;
            };
            let member_id = catalog_id(&raw_id)?;
            let mut member_path = prefix.to_vec();
            member_path.push(DataPathSegment::Member(member_id.clone()));
            if !member.key_params.is_empty() {
                // A keyed layer: page each existing entry under the layer path. A keyed group
                // recurses into each entry to reach its sub-members; a keyed-leaf-map
                // (`map[K, V]`) holds its value directly under the entry key, so the value
                // cell at the key path is recorded against the map member's own obligation.
                for entry_key in
                    self.store
                        .data_child_keys(self.store_id, identity, &member_path)?
                {
                    let mut entry_path = member_path.clone();
                    entry_path.push(DataPathSegment::Key(entry_key));
                    match &member.kind {
                        CheckedSavedMemberKind::Group => {
                            self.descend(
                                identity,
                                &member.group_members,
                                &entry_path,
                                &member_names,
                            )?;
                        }
                        CheckedSavedMemberKind::Field { .. } => {
                            self.record_leaf(&member_id, identity, &entry_path)?;
                        }
                    }
                }
                continue;
            }
            match &member.kind {
                CheckedSavedMemberKind::Group => {
                    self.descend(identity, &member.group_members, &member_path, &member_names)?;
                }
                CheckedSavedMemberKind::Field { .. } => {
                    self.record_leaf(&member_id, identity, &member_path)?;
                }
            }
        }
        Ok(())
    }

    /// Record one keyed-layer leaf's or keyed-leaf-map value's per-entry presence: present
    /// when the cell holds a valid value under the current type, invalid when it holds bytes
    /// that are not, missing when no cell exists. A member with no obligation is skipped.
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

/// The leaf obligations that live inside a keyed layer, captured once for the scan: a
/// required leaf, or a retyped leaf — of any kind. A transform-targeted leaf is classified
/// eagerly and excluded; an unchanged optional leaf places no per-entry obligation and is
/// recorded as a no-op.
fn keyed_leaf_obligations(
    place: &CheckedSavedPlace,
    resource_path: &str,
    acc: &mut Accumulator,
) -> Result<Vec<LeafObligation>, StoreError> {
    let walk = LeafWalk {
        place,
        resource_path,
    };
    let mut obligations = Vec::new();
    collect_keyed_leaves(
        &walk,
        &place.root_members,
        false,
        &[],
        &[],
        &mut obligations,
        acc,
    )?;
    Ok(obligations)
}

/// Walk the member tree, emitting one keyed-leaf obligation per required leaf and per
/// retyped leaf — of any kind — that is reached through a keyed layer. `in_keyed` becomes true
/// once the walk has crossed a keyed layer, so a `Field` inside a keyed group is keyed; a
/// keyed-leaf-layer (`map[K, V]`) member is itself a keyed leaf, its own key params making it
/// keyed even at the root. Both are scanned per existing entry by [`KeyedScan`], which knows
/// each entry's key; here only the per-leaf classification inputs are captured. `prefix` is
/// the static data path to the layer node, built only while no keyed ancestor sits above it;
/// a keyed-leaf-map at the root or inside an unkeyed group carries its full member path so a
/// retype probe can find old data of a different shape, while a leaf below a keyed ancestor
/// carries none (its path contains an unknown entry key) and relies on the per-entry scan.
fn collect_keyed_leaves(
    walk: &LeafWalk,
    members: &[CheckedSavedMember],
    in_keyed: bool,
    prefix: &[DataPathSegment],
    names: &[&str],
    obligations: &mut Vec<LeafObligation>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for member in members {
        let mut member_names = names.to_vec();
        member_names.push(member.name.as_str());
        let Some(raw_id) =
            resolved_member_id(walk.resource_path, member, &member_names, acc.new_members())
        else {
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
            // A keyed group whose accepted snapshot recorded a leaf token is a plain leaf that
            // became a keyed layer: its old single-cell bytes sit at the member path the layer
            // now occupies, under no entry key. The subtree-existence probe at that static path
            // finds them and fails a populated member closed, the same as a leaf becoming an
            // unkeyed group. Its keyed sub-members are subsumed, so the scan does not descend.
            CheckedSavedMemberKind::Group if keyed_here && acc.leaf_disappeared(&raw_id) => {
                emit_disappeared_leaf(
                    walk.place,
                    member,
                    &raw_id,
                    member_id,
                    obligation_path,
                    obligations,
                );
            }
            // A keyed layer or group whose own structural signature diverged is owned whole by
            // the backstop; descending past it would emit a misleading per-entry data proof on a
            // deeper required leaf the enclosing failure already subsumes, so its interior is left
            // unwalked. This is the keyed analogue of the store-level re-key skip.
            CheckedSavedMemberKind::Group if acc.prunes_interior(&raw_id, &member_id) => {}
            CheckedSavedMemberKind::Group => {
                collect_keyed_leaves(
                    walk,
                    &member.group_members,
                    keyed_here,
                    &static_path,
                    &member_names,
                    obligations,
                    acc,
                )?;
            }
            CheckedSavedMemberKind::Field { .. } if keyed_here => {
                // A keyed-layer leaf or a keyed-leaf-map value. The per-entry scan reaches the
                // value cell through each entry's key path; the obligation carries a static
                // member path only when no keyed ancestor sits above it.
                emit_member_leaf(
                    walk.place,
                    member,
                    &raw_id,
                    member_id,
                    obligation_path,
                    obligations,
                    acc,
                );
            }
            CheckedSavedMemberKind::Field { .. } => {}
        }
    }
    Ok(())
}

/// The running presence state for one leaf: how many records lack it, how many carry
/// undecodable bytes, how many carry a stored cell at all, and a bounded sample of the
/// records missing or invalid for the diagnostic. `present_count` answers whether a
/// retyped leaf has any bytes to reinterpret, independent of decodability.
#[derive(Default)]
struct LeafScan {
    missing_count: usize,
    invalid_count: usize,
    present_count: usize,
    sample: Vec<Vec<SavedKey>>,
}

impl LeafScan {
    /// Fold one record's read of a leaf cell into the running state, the single owner of
    /// the present/invalid/missing decision both the unkeyed and keyed scans share. A cell
    /// holding bytes valid under the current leaf type is present; one holding bytes that
    /// are not is invalid (and still counts as populated). A leaf with no current type to
    /// decode under (a non-tokenizable retype) treats any stored cell as plainly present, so
    /// the retype check sees its populated old bytes; an absent cell is missing.
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

/// Classify one leaf from its scan state. A populated leaf whose declared type changed
/// is checked first and fails closed: its bytes were written under the old type, so the
/// new type's decoder would silently coerce them. An unpopulated retype has no bytes to
/// reinterpret and falls through to the presence verdict it otherwise carries.
///
/// Otherwise the leaf is proven when every record carries it, a constant default when an
/// `evolve default` supplies a typed fill, else a fail-closed repair. A leaf a rename
/// moved this cycle reports as the catalog-only identity move it is.
fn classify_leaf(leaf: LeafObligation, state: LeafScan, acc: &mut Accumulator) {
    let id = leaf.catalog_id;
    // A populated retype is steered to a transform ahead of every other classification.
    // An unpopulated retype has no bytes to reinterpret, so it falls through to the
    // presence verdict the member would otherwise carry.
    if leaf.retyped && state.present_count > 0 {
        acc.diagnostic(
            id.clone(),
            format!(
                "member `{}` changed leaf type with {} populated record(s); a leaf type change on stored data fails closed. Add a new member of the new type, populate it with an `evolve transform` computed from this one, then retire this member",
                leaf.label, state.present_count
            ),
        );
        acc.push(
            id,
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform,
            },
        );
        return;
    }
    // An optional leaf places no presence obligation: its absence is the sparse-absence
    // contract, not a repair. It reaches the scan only when retyped (handled above) or when
    // its enum dropped a selectable member, where a stored value naming the gone member is
    // invalid and fails closed; a missing optional cell stays harmless.
    if !leaf.required {
        if state.invalid_count > 0 {
            acc.counts.records_lacking_member += state.invalid_count;
            acc.diagnostic(
                id.clone(),
                format!(
                    "member `{}` has {} record(s) whose stored value is not valid under the current type (it names an enum member the current enum no longer has); repair before activating",
                    leaf.label, state.invalid_count
                ),
            );
            acc.push(
                id,
                Verdict::RepairRequired {
                    reason: RepairReason::InvalidStoredValue,
                },
            );
            return;
        }
        let verdict = if leaf.renamed {
            Verdict::CatalogOnly
        } else {
            Verdict::NoOp
        };
        acc.push(id, verdict);
        return;
    }
    if state.missing_count == 0 && state.invalid_count == 0 {
        let verdict = if leaf.renamed {
            Verdict::CatalogOnly
        } else {
            Verdict::DataProof
        };
        acc.push(id, verdict);
        return;
    }
    acc.counts.records_lacking_member += state.missing_count + state.invalid_count;
    let repair = Verdict::RepairRequired {
        reason: RepairReason::MissingRequiredMember,
    };
    if state.invalid_count > 0 {
        acc.diagnostic(
            id.clone(),
            format!(
                "required member `{}` has {} record(s) whose stored value is not valid under the current type (it does not decode, or names an enum member the current enum no longer has); repair before activating",
                leaf.label, state.invalid_count
            ),
        );
        acc.push(
            id,
            Verdict::RepairRequired {
                reason: RepairReason::InvalidStoredValue,
            },
        );
        return;
    }
    let verdict = match acc.default_value_for(&leaf.raw_catalog_id, leaf.leaf.as_ref()) {
        Some(Ok(value)) => {
            acc.counts.records_to_backfill += state.missing_count;
            Verdict::Default { value }
        }
        Some(Err(message)) => {
            acc.diagnostic(id.clone(), message);
            repair
        }
        None => {
            acc.diagnostic(
                id.clone(),
                missing_member_message(&leaf.label, state.missing_count, &state.sample),
            );
            repair
        }
    };
    acc.push(id, verdict);
}

/// One unique-index obligation to probe during the record scan: the index catalog id
/// and how to read each key column's value from a record. The collision state the scan
/// builds is keyed by `catalog_id`, which the per-index classification then looks up.
struct UniqueIndexProbe {
    catalog_id: CatalogId,
    columns: Vec<IndexKeyColumn>,
}

/// The unique-index plan for a place: the indexes the record scan can probe for
/// collisions, and the catalog ids of any unique index whose key shape the scan cannot
/// probe. An unprobeable unique index is not silently rebuilt unchecked; the discharge
/// fails it closed so a uniqueness guarantee is never published without verification.
struct UniqueIndexPlan {
    probes: Vec<UniqueIndexProbe>,
    unprobeable: Vec<CatalogId>,
}

/// How to read one index key column for a record: an identity key by its position in
/// the record's identity tuple, or a top-level member cell decoded by its meaning.
enum IndexKeyColumn {
    Identity {
        position: usize,
    },
    Member {
        path: DataPathSegment,
        meaning: StoredValueMeaning,
    },
}

/// Build the unique-index plan for a place. Each column resolves to an identity position
/// or a top-level member cell with the meaning to decode it; a unique index whose every
/// column resolves becomes a probe the record scan checks for collisions, while one with
/// any unresolvable column is recorded as unprobeable and fails closed rather than
/// rebuilding an unchecked uniqueness guarantee.
fn unique_index_plan(place: &CheckedSavedPlace) -> Result<UniqueIndexPlan, StoreError> {
    let mut probes = Vec::new();
    let mut unprobeable = Vec::new();
    for index in &place.indexes {
        if !index.unique {
            continue;
        }
        match index_key_columns(place, index)? {
            Some(columns) => probes.push(UniqueIndexProbe {
                catalog_id: catalog_id(&index.catalog_id)?,
                columns,
            }),
            None => unprobeable.push(catalog_id(&index.catalog_id)?),
        }
    }
    Ok(UniqueIndexPlan {
        probes,
        unprobeable,
    })
}

/// The key-column readers for one index, or `None` when a column resolves to neither
/// an identity key position nor a top-level plain field. Every v0.1 index key is a
/// single-segment top-level field or an identity key, so a unique index resolves here;
/// a future index over a nested or keyed-layer column would resolve to `None`, and a
/// unique index that does not resolve fails closed rather than rebuilding an unchecked
/// uniqueness guarantee.
fn index_key_columns(
    place: &CheckedSavedPlace,
    index: &CheckedSavedIndex,
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
                columns.push(IndexKeyColumn::Member {
                    path: DataPathSegment::Member(catalog_id(&member.catalog_id)?),
                    meaning: key.value_meaning.clone(),
                });
            }
        }
    }
    Ok(Some(columns))
}

/// The full prospective unique-index key tuple a record would publish, derived from
/// the record's identity and member values. `None` when any key column is absent, so
/// the record contributes no index entry and cannot collide.
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
            IndexKeyColumn::Member { path, meaning } => {
                let Some(bytes) =
                    store.read_data_value(store_id, identity, std::slice::from_ref(path))?
                else {
                    return Ok(None);
                };
                let Some(key) = meaning.stored_key(&bytes) else {
                    return Ok(None);
                };
                tuple.push(key);
            }
        }
    }
    Ok(Some(tuple))
}

/// The running collision state for one unique index, keyed by the canonical byte
/// encoding of each full key tuple (every tuple shares the index's arity, so the
/// encoding is an injective identity for the tuple). It tracks the tuples seen so far
/// and the distinct tuples more than one record claims.
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

/// Classify every index the place declares. Each index carries a derived-rebuild
/// obligation regardless of uniqueness, so apply rebuilds its entries from the records
/// it covers; a silently empty non-unique index is the symptom of skipping this. A
/// unique index whose prospective key tuples collide is upgraded to a fail-closed
/// repair, using the collision state the scan accumulated for it. A unique index the
/// scan could not probe — its key shape is one the collision scan does not resolve — is
/// also a fail-closed repair, never an unchecked rebuild, so a uniqueness guarantee is
/// never published without verification.
fn classify_indexes(
    place: &CheckedSavedPlace,
    collisions: &HashMap<CatalogId, IndexScan>,
    unprobeable: &HashSet<CatalogId>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for index in &place.indexes {
        let index_id = catalog_id(&index.catalog_id)?;
        let colliding = collisions
            .get(&index_id)
            .map(|state| state.collisions.len())
            .unwrap_or(0);
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
        acc.push_index(index_id, verdict);
    }
    Ok(())
}

/// Classify the accepted catalog entries current source no longer declares. A retire
/// intent marks the proposal entry `Removed`: dropping populated data is a
/// destructive decision that names the exact catalog id and count. A source-dropped
/// index deletes its derived cell subtree on apply. A member source merely stopped
/// declaring, with no retire and no dependent, is a dependency-free sparse-field drop:
/// a legal no-op whose data lingers. A dropped member an active index still reads
/// cannot be silently dropped; it needs an explicit retire intent that also removes or
/// rebinds the index.
fn classify_absent_source_entries(
    program: &CheckedProgram,
    store: &TreeStore,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let source_paths = crate::catalog::source_catalog_entries(program);
    let declared: HashSet<(CatalogEntryKind, &str)> = source_paths
        .iter()
        .map(|entry| (entry.kind, entry.path.as_str()))
        .collect();

    for entry in dropped_or_removed_entries(program) {
        if declared.contains(&(entry.kind, entry.path.as_str())) {
            continue;
        }
        let entry_id = catalog_id(&entry.stable_id)?;
        let is_index = entry.kind == CatalogEntryKind::StoreIndex;
        match entry.lifecycle {
            CatalogLifecycle::Removed if is_index => {
                // An index holds no per-record source data, so an explicit retire of one
                // is the same durable operation as a bare source-drop: delete the derived
                // index-cell subtree. Routing it through the per-record member-deletion
                // path would clear nothing and orphan the real index cells.
                acc.record(entry_id, Verdict::IndexDropped, true);
            }
            CatalogLifecycle::Removed => {
                if retired_member_is_nested(program, entry) {
                    acc.diagnostic(
                        entry_id.clone(),
                        format!(
                            "retiring `{}` drops a member nested under a group or keyed layer, which apply does not yet support; retire a top-level member instead",
                            entry.path
                        ),
                    );
                    acc.record(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::NestedRetireUnsupported,
                        },
                        is_index,
                    );
                } else {
                    let populated = populated_member_records(program, store, entry)?;
                    acc.record(
                        entry_id,
                        Verdict::DestructiveDecisionRequired { populated },
                        is_index,
                    );
                }
            }
            CatalogLifecycle::Active | CatalogLifecycle::Deprecated => {
                if is_index {
                    // A source-dropped index is a derived structure whose cells outlive
                    // their binding. Apply deletes the index-cell subtree under this id
                    // so the entries do not leak; no source data moves.
                    acc.record(entry_id, Verdict::IndexDropped, true);
                } else if let Some((index_name, index_id)) = index_depends_on(program, entry)? {
                    acc.diagnostic(
                        entry_id.clone(),
                        format!(
                            "dropped `{}` is still used by index `{index_name}`; retire it with an evolve intent",
                            entry.path
                        ),
                    );
                    acc.record(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::RetireRequired { index: index_id },
                        },
                        false,
                    );
                } else {
                    // A dependency-free dropped sparse field is a legal no-op: its data
                    // simply lingers under a stable id source no longer declares.
                    acc.record(entry_id, Verdict::NoOp, false);
                }
            }
        }
    }
    Ok(())
}

/// The catalog entries discharge must consider for a source drop: the proposal
/// entries when source proposed a change, else the accepted entries. The proposal
/// already carries any `Removed` lifecycle and the lingering still-active entries, so
/// it supersedes the accepted snapshot; when source proposed nothing, the accepted
/// entries are the snapshot to diff against.
fn dropped_or_removed_entries(program: &CheckedProgram) -> &[CatalogEntry] {
    match &program.catalog.proposal {
        Some(proposal) => &proposal.entries,
        None => &program.catalog.accepted_entries,
    }
}

/// Count records that carry a value for the dropped member identified by `entry`.
/// Only a resource-member entry holds per-record data; a store, index, or enum entry
/// has none to count. The records are streamed, never materialized.
fn populated_member_records(
    program: &CheckedProgram,
    store: &TreeStore,
    entry: &CatalogEntry,
) -> Result<usize, StoreError> {
    if entry.kind != CatalogEntryKind::ResourceMember {
        return Ok(0);
    }
    let Some((store_id, member_id)) = dropped_member_addresses(program, entry)? else {
        return Ok(0);
    };
    let path = [DataPathSegment::Member(member_id)];
    let mut populated = 0;
    store.for_each_record(
        &store_id,
        owning_root_arity(program, entry),
        &mut |identity| {
            if store.data_subtree_exists(&store_id, identity, &path)? {
                populated += 1;
            }
            Ok(())
        },
    )?;
    Ok(populated)
}

/// The store and member catalog ids for a dropped resource-member entry. The store id
/// comes from the owning resource's store; the member id is the entry's own stable id,
/// since a dropped member's cells were written under that id.
fn dropped_member_addresses(
    program: &CheckedProgram,
    entry: &CatalogEntry,
) -> Result<Option<(CatalogId, CatalogId)>, StoreError> {
    let Some(root) = owning_root(program, entry) else {
        return Ok(None);
    };
    let Some(place) = checked_saved_root_place(program, &root, Default::default()) else {
        return Ok(None);
    };
    let store_id = catalog_id(&place.store_catalog_id)?;
    let member_id = catalog_id(&entry.stable_id)?;
    Ok(Some((store_id, member_id)))
}

/// The store root whose resource owns the dropped member, found by matching the
/// member path's resource prefix against a source store's resource. A member path is
/// `module::Resource::field...`; its resource prefix is the source resource path.
fn owning_root(program: &CheckedProgram, entry: &CatalogEntry) -> Option<String> {
    let resource_prefix = entry.path.rsplit_once("::").map(|(head, _)| head)?;
    program.modules.iter().find_map(|module| {
        module.stores.iter().find_map(|store| {
            let resource_path = crate::catalog::resource_path(&module.name, &store.resource);
            (resource_path == resource_prefix).then(|| store.root.clone())
        })
    })
}

/// Whether a retired resource-member entry names a member nested under an unkeyed group
/// or a keyed layer rather than a top-level member of the record. The member chain is
/// everything after the owning resource path; a top-level member is a single segment,
/// while a nested member carries the group or layer segments before its own. A retired
/// member is gone from current source, so its nesting is read from its catalog path
/// against the owning source resource, not from the live member tree.
fn retired_member_is_nested(program: &CheckedProgram, entry: &CatalogEntry) -> bool {
    if entry.kind != CatalogEntryKind::ResourceMember {
        return false;
    }
    program.modules.iter().any(|module| {
        module.stores.iter().any(|store| {
            let resource_path = crate::catalog::resource_path(&module.name, &store.resource);
            entry
                .path
                .strip_prefix(&resource_path)
                .and_then(|tail| tail.strip_prefix("::"))
                .is_some_and(|member_chain| member_chain.contains("::"))
        })
    })
}

/// The identity arity of the store that owns the dropped member, or `1` when it
/// cannot be resolved (the common single-key store).
fn owning_root_arity(program: &CheckedProgram, entry: &CatalogEntry) -> usize {
    owning_root(program, entry)
        .and_then(|root| checked_saved_root_place(program, &root, Default::default()))
        .map(|place| place.identity_keys.len())
        .unwrap_or(1)
}

/// An active source index that reads the dropped member, as its developer-facing name
/// and its catalog identity. A dropped member an index still needs cannot be silently
/// deprecated. The name is for the diagnostic; the catalog id is the typed identity the
/// verdict carries across into apply. The index is matched on its source-declared key
/// columns, which still name the dropped member, and its stable id is read from the
/// catalog entry for the index path.
fn index_depends_on(
    program: &CheckedProgram,
    entry: &CatalogEntry,
) -> Result<Option<(String, CatalogId)>, StoreError> {
    if entry.kind != CatalogEntryKind::ResourceMember {
        return Ok(None);
    }
    let Some(member_name) = entry.path.rsplit_once("::").map(|(_, tail)| tail) else {
        return Ok(None);
    };
    let found = program.modules.iter().find_map(|module| {
        module.stores.iter().find_map(|store| {
            store
                .indexes
                .iter()
                .find(|index| index.args.iter().any(|arg| arg == member_name))
                .map(|index| {
                    (
                        index.name.clone(),
                        crate::catalog::store_index_path(&module.name, &store.root, &index.name),
                    )
                })
        })
    });
    let Some((index_name, index_path)) = found else {
        return Ok(None);
    };
    let Some(stable_id) = index_stable_id(program, &index_path) else {
        return Ok(None);
    };
    Ok(Some((index_name, catalog_id(&stable_id)?)))
}

/// The stable id of the store-index catalog entry at `path`, from the proposal when
/// source proposed a change, else the accepted snapshot. Both carry the index entry;
/// the proposal supersedes the accepted snapshot the same way the dropped-entry scan
/// chooses its source.
fn index_stable_id(program: &CheckedProgram, path: &str) -> Option<String> {
    dropped_or_removed_entries(program)
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::StoreIndex && entry.path == path)
        .map(|entry| entry.stable_id.clone())
}

fn catalog_id(raw: &str) -> Result<CatalogId, StoreError> {
    CatalogId::new(raw).map_err(|_| StoreError::Corruption {
        message: format!("evolution discharge saw an invalid catalog id `{raw}`"),
    })
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
/// resolves `evolve default` fills. Affected ids are typed [`CatalogId`]s validated once
/// on insertion and partitioned at classify time into data roots and store indexes, so
/// apply never re-classifies them from current source.
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
    new_members: HashMap<String, String>,
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
        new_members: HashMap<String, String>,
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
            new_members,
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

    /// Install the accepted and current structural signatures the default-deny backstop
    /// compares. Set after construction alongside the shrunk-enum set, so the constructor stays
    /// to the obligation inputs the per-family classifiers consume.
    fn set_member_structs(
        &mut self,
        accepted_structs: HashMap<String, String>,
        declared_structs: HashMap<String, String>,
    ) {
        self.accepted_structs = accepted_structs;
        self.declared_structs = declared_structs;
    }

    /// Whether the enum a leaf refers to lost a selectable member this cycle. An enum-typed
    /// leaf over a shrunk enum must be scanned for stored values naming the gone member, even
    /// when it is optional and otherwise unchanged.
    fn enum_shrank(&self, leaf: Option<&StoreLeafKind>) -> bool {
        matches!(leaf, Some(StoreLeafKind::Enum { enum_id }) if self.shrunk_enums.shrank(*enum_id))
    }

    /// The proposal-minted stable id of a brand-new member at `source_path`, or `None` when no
    /// member current source adds anew lives there. A brand-new member has no bound facts id,
    /// so the presence scan resolves its identity here to reach it; an already-accepted member
    /// is reached by its bound id and is absent from this map.
    fn new_members(&self) -> &HashMap<String, String> {
        &self.new_members
    }

    fn is_transform(&self, catalog_id: &str) -> bool {
        self.transforms.contains(catalog_id)
    }

    fn is_renamed(&self, catalog_id: &str) -> bool {
        self.renamed.contains(catalog_id)
    }

    /// Whether a member's declared leaf type differs from the type its durable bytes were
    /// accepted as, fail-closed and total over leaf kind. The comparison is between two
    /// identity-aware leaf tokens: a scalar by name, an enum by its referent's stable catalog
    /// id, a store identity by its referent's stable catalog id and arity. A change between
    /// any scalar, enum, or identity leaf, or from one enum or store to a different one, is a
    /// retype; a pure enum or store rename leaves the token unchanged and is not.
    ///
    /// A member not currently a plain leaf field (a group or keyed layer) has no declared
    /// token and is never a leaf retype. A member with no accepted identity is brand-new and
    /// not a retype. The `Some(None)` arm is the non-leaf-to-leaf transition: the member was a
    /// group or keyed layer in the accepted snapshot, which records no leaf token, and current
    /// source makes it a plain leaf field. Its old multi-cell subtree would be reread as a
    /// single leaf cell, so it fails closed the same way a scalar retype does; this is the
    /// appearance half of a leaf retype, symmetric to [`Self::leaf_disappeared`], and the
    /// populated probe steers it to a transform rather than silently reinterpreting the bytes.
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

    /// The identity-aware leaf token current source declares for member `catalog_id`, derived
    /// from its structural signature: a leaf member records `leaf:<token>`, so the token is the
    /// signature with that prefix stripped. The structural signature is the single in-memory
    /// source for the leaf token, mirroring the durable [`CatalogEntry::accepted_leaf_token`] on
    /// the accepted side. `None` for a non-leaf member (a group or keyed group records no leaf
    /// token) and for a member with no recorded signature (a pending first-run referent).
    fn declared_leaf_token(&self, catalog_id: &str) -> Option<&str> {
        self.declared_structs
            .get(catalog_id)
            .and_then(|signature| signature.strip_prefix("leaf:"))
    }

    /// Whether a member that WAS a plain leaf has become a non-leaf — a group or a keyed
    /// layer — so its current declaration produces no leaf token. The accepted snapshot
    /// recorded a leaf token; current source records none, so its old single-cell bytes live
    /// under the member position the now-group/now-layer occupies and would be orphaned. This
    /// is the disappearance half of a leaf retype, symmetric to a non-leaf becoming a leaf, and
    /// fails closed the same way: a subtree-existence probe at the member path steers a
    /// populated member to a transform rather than silently reshaping over the old bytes.
    fn leaf_disappeared(&self, catalog_id: &str) -> bool {
        matches!(self.accepted_leaves.get(catalog_id), Some(Some(_)))
            && self.declared_leaf_token(catalog_id).is_none()
    }

    /// The typed constant fill for a defaulted member, or an error message when the
    /// default value is not a constant the checker can evaluate against the leaf
    /// type. `None` when no `evolve default` targets the member. A non-scalar leaf — an
    /// enum, an identity, or a non-tokenizable position with no leaf kind — cannot take a
    /// constant default; a computed fill is a transform.
    fn default_value_for(
        &self,
        raw_catalog_id: &str,
        leaf: Option<&StoreLeafKind>,
    ) -> Option<Result<super::witness::DefaultValue, String>> {
        let value = self.defaults.get(raw_catalog_id)?;
        let Some(StoreLeafKind::Scalar(scalar)) = leaf else {
            return Some(Err(
                "evolve default targets a non-scalar member; use a transform for computed values"
                    .to_string(),
            ));
        };
        Some(eval_const_default(value, *scalar).map_err(|error| error.message()))
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

    /// Record a verdict for a data-root obligation (a member, store, resource, or
    /// enum). Its catalog id joins the changed-root partition.
    fn push(&mut self, id: CatalogId, verdict: Verdict) {
        self.record(id, verdict, false);
    }

    /// Record a verdict for a store-index obligation. Its catalog id joins the
    /// changed-index partition, so apply stamps it as an index rather than a root.
    fn push_index(&mut self, id: CatalogId, verdict: Verdict) {
        self.record(id, verdict, true);
    }

    fn record(&mut self, id: CatalogId, verdict: Verdict, is_index: bool) {
        self.changed_set(is_index).insert(id.clone());
        self.classified.insert(id.clone());
        self.verdicts.push(ObligationVerdict {
            catalog_id: id,
            verdict,
        });
    }

    /// Whether obligation `id` already carries a verdict from a targeted classifier. The
    /// structural backstop fires only on an unclaimed id, so a retype, reshape, rename,
    /// default, transform, or retire that already classified the member is not double-judged.
    fn is_classified(&self, id: &CatalogId) -> bool {
        self.classified.contains(id)
    }

    /// The accepted and current structural signatures of member `raw_id` when both are
    /// recorded and they differ — the divergence the backstop fails closed. `None` when the
    /// member has no recorded accepted signature (no baseline to compare), no current
    /// signature (a pending referent), or the two match (no structural change).
    fn struct_divergence(&self, raw_id: &str) -> Option<(&str, &str)> {
        let accepted = self.accepted_structs.get(raw_id)?;
        let declared = self.declared_structs.get(raw_id)?;
        (accepted != declared).then_some((accepted.as_str(), declared.as_str()))
    }

    /// Whether a non-leaf member's interior must be left unwalked by the leaf scans because the
    /// backstop owns it as a whole. A keyed layer or unkeyed group whose own structural signature
    /// diverged fails closed as one unit; descending into it would re-judge its interior and emit
    /// a misleading data proof on a deeper required leaf the enclosing failure already subsumes.
    /// This mirrors the store-level re-key skip one level down, and applies only to a pure
    /// structural divergence: a leaf that became a non-leaf (`leaf_disappeared`) is steered by the
    /// leaf path's own retype probe, which descends deliberately, so it is not pruned here.
    fn prunes_interior(&self, raw_id: &str, member_id: &CatalogId) -> bool {
        self.struct_divergence(raw_id).is_some()
            && !self.is_classified(member_id)
            && !self.leaf_disappeared(raw_id)
    }

    /// Record the fail-closed prose for the obligation `id`, carried with that identity
    /// so a renderer matches it to the obligation's `RepairRequired` verdict by catalog
    /// id, not by position.
    fn diagnostic(&mut self, id: CatalogId, message: String) {
        self.diagnostics.push(RepairDiagnostic {
            catalog_id: id,
            message,
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executable::{CheckedSavedIndexKey, CheckedSavedKeyParam, CheckedSavedTerminal};
    use crate::facts::{ResourceId, ResourceMemberId, StoreId, StoreIndexId};
    use marrow_store::value::ScalarType;

    fn unique_index(name: &str, catalog_id: &str, key_name: &str) -> CheckedSavedIndex {
        CheckedSavedIndex {
            id: StoreIndexId(0),
            name: name.to_string(),
            catalog_id: catalog_id.to_string(),
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
            resource_id: ResourceId(0),
            store_catalog_id: "cat_00000000000000aa".to_string(),
            resource_name: "Book".to_string(),
            root_members: vec![CheckedSavedMember {
                id: Some(ResourceMemberId(0)),
                name: "isbn".to_string(),
                key_params: Vec::new(),
                kind: CheckedSavedMemberKind::Field { required: true },
                catalog_id: "cat_00000000000000bb".to_string(),
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
            index_count: 0,
            next_id_shape: String::new(),
            layers: Vec::new(),
            terminal: CheckedSavedTerminal::Record,
            span: marrow_syntax::SourceSpan::default(),
        }
    }

    fn empty_accumulator() -> Accumulator {
        Accumulator::new(
            Vec::new(),
            BTreeSet::new(),
            HashSet::new(),
            HashMap::new(),
            HashMap::new(),
        )
    }

    // A unique index whose key resolves to a top-level plain field is probeable; one whose
    // key names a member the place does not declare cannot be probed for collisions, so the
    // plan must route it to `unprobeable` rather than silently treat it as a clean rebuild.
    #[test]
    fn unique_index_with_unresolvable_key_is_unprobeable() {
        let place = place_with_indexes(vec![
            unique_index("byIsbn", "cat_00000000000000c1", "isbn"),
            unique_index("byGhost", "cat_00000000000000c2", "ghost"),
        ]);

        let plan = unique_index_plan(&place).expect("plan");

        let probed: Vec<&str> = plan
            .probes
            .iter()
            .map(|probe| probe.catalog_id.as_str())
            .collect();
        let unprobeable: Vec<&str> = plan.unprobeable.iter().map(CatalogId::as_str).collect();
        assert_eq!(
            probed,
            ["cat_00000000000000c1"],
            "probed {probed:?} unprobeable {unprobeable:?}"
        );
        assert_eq!(
            unprobeable,
            ["cat_00000000000000c2"],
            "probed {probed:?} unprobeable {unprobeable:?}"
        );
    }

    // An unprobeable unique index must fail closed: its uniqueness cannot be verified from
    // the snapshot, so the discharge blocks activation rather than rebuilding an unchecked
    // guarantee. A probeable index with no collisions still discharges to a derived rebuild.
    #[test]
    fn unprobeable_unique_index_fails_closed() {
        let place = place_with_indexes(vec![
            unique_index("byIsbn", "cat_00000000000000c1", "isbn"),
            unique_index("byGhost", "cat_00000000000000c2", "ghost"),
        ]);
        let unprobeable: HashSet<CatalogId> = [catalog_id("cat_00000000000000c2").unwrap()]
            .into_iter()
            .collect();
        let mut acc = empty_accumulator();

        classify_indexes(&place, &HashMap::new(), &unprobeable, &mut acc).expect("classify");

        let ghost = acc
            .verdicts
            .iter()
            .find(|v| v.catalog_id.as_str() == "cat_00000000000000c2")
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
            .find(|v| v.catalog_id.as_str() == "cat_00000000000000c1")
            .expect("isbn verdict");
        assert!(
            matches!(isbn.verdict, Verdict::DerivedRebuild),
            "a probeable collision-free unique index rebuilds, got {:?}",
            isbn.verdict
        );
    }
}
