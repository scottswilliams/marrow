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
use super::witness::{ObligationVerdict, RepairReason, Verdict};
use crate::StoreLeafKind;
use crate::executable::{
    CheckedSavedIndex, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace,
    checked_saved_root_place,
};
use crate::facts::{StoreIndexKeySource, StoredValueMeaning};
use crate::program::{CheckedProgram, EvolveDefault};

/// The most failing-record keys a diagnostic names before summarizing the rest, so
/// a large gap does not produce an unbounded message.
const MAX_NAMED_RECORDS: usize = 16;

/// The human label for one obligation, kept on the discharge result for preview and
/// diagnostics. The witness verdicts are prose-free; this is the only place a
/// resource-qualified place name lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DischargeLabel {
    pub(crate) catalog_id: CatalogId,
    pub(crate) place: String,
}

/// The result of discharging every obligation against a snapshot: the per-obligation
/// verdicts that cross into apply, the human labels for preview, the accumulated
/// counts, the catalog ids the change touches partitioned into data roots and indexes,
/// and the fail-closed diagnostics naming what blocks activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Discharge {
    pub(crate) verdicts: Vec<ObligationVerdict>,
    pub(crate) labels: Vec<DischargeLabel>,
    pub(crate) counts: super::witness::DischargeCounts,
    pub(crate) changed_root_catalog_ids: Vec<CatalogId>,
    pub(crate) changed_index_catalog_ids: Vec<CatalogId>,
    pub(crate) diagnostics: Vec<String>,
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
    );
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
        discharge_root(store, &place, &mut acc)?;
    }
    classify_absent_source_entries(program, store, &mut acc)?;
    Ok(acc.into_discharge())
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
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let store_id = catalog_id(&place.store_catalog_id)?;
    let leaves = required_leaf_obligations(place, acc)?;
    let unique_indexes = unique_index_probes(place)?;

    let mut leaf_state: Vec<LeafScan> = leaves.iter().map(|_| LeafScan::default()).collect();
    let mut index_state: Vec<IndexScan> = unique_indexes
        .iter()
        .map(|_| IndexScan::default())
        .collect();
    let mut scanned = 0usize;

    for_each_record(
        store,
        &store_id,
        place.identity_keys.len(),
        &mut |identity| {
            scanned += 1;
            for (leaf, state) in leaves.iter().zip(leaf_state.iter_mut()) {
                if !store.data_subtree_exists(&store_id, identity, &leaf.path)? {
                    state.record_missing(identity);
                }
            }
            for (probe, state) in unique_indexes.iter().zip(index_state.iter_mut()) {
                if let Some(key) = prospective_index_key(store, &store_id, probe, identity)? {
                    state.observe(key);
                }
            }
            Ok(())
        },
    )?;

    acc.counts.scanned_records += scanned;

    for (leaf, state) in leaves.into_iter().zip(leaf_state) {
        classify_leaf(leaf, state, acc);
    }
    let collisions: HashMap<CatalogId, IndexScan> = unique_indexes
        .into_iter()
        .map(|probe| probe.catalog_id)
        .zip(index_state)
        .collect();
    classify_indexes(place, &collisions, acc)?;
    discharge_keyed_layers(store, place, acc)?;
    Ok(())
}

/// One required-leaf obligation: the catalog id its data cells use, the data path
/// from the record node to the leaf cell, whether an `evolve default` supplies a
/// fill, and a human label. A transform-targeted member is classified eagerly and
/// excluded from the scan.
struct LeafObligation {
    catalog_id: CatalogId,
    raw_catalog_id: String,
    path: Vec<DataPathSegment>,
    label: String,
    leaf: StoreLeafKind,
}

/// The required-leaf obligations at the root and inside unkeyed groups: leaves whose
/// presence cell sits directly under the record node. A required leaf inside a keyed
/// layer is required per existing entry, not for the record, so it is scanned and
/// classified separately by [`discharge_keyed_layers`].
fn required_leaf_obligations(
    place: &CheckedSavedPlace,
    acc: &mut Accumulator,
) -> Result<Vec<LeafObligation>, StoreError> {
    let mut obligations = Vec::new();
    collect_required_leaves(place, &place.root_members, &[], &mut obligations, acc)?;
    Ok(obligations)
}

/// Walk the member tree, emitting a required-leaf obligation for each required scalar
/// field at the root or inside an unkeyed group and building the nested data-path
/// chain to its cell. A transform-targeted member is classified eagerly and not
/// scanned. A keyed member is left to the keyed-layer check.
fn collect_required_leaves(
    place: &CheckedSavedPlace,
    members: &[CheckedSavedMember],
    prefix: &[DataPathSegment],
    obligations: &mut Vec<LeafObligation>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for member in members {
        if !member.key_params.is_empty() || member.catalog_id.is_empty() {
            continue;
        }
        let member_id = catalog_id(&member.catalog_id)?;
        let mut path = prefix.to_vec();
        path.push(DataPathSegment::Member(member_id.clone()));
        match &member.kind {
            CheckedSavedMemberKind::Group => {
                collect_required_leaves(place, &member.group_members, &path, obligations, acc)?;
            }
            CheckedSavedMemberKind::Field { .. } => {
                // The cell sits directly under the record node, so the obligation path
                // is the full nested member chain to it.
                emit_member_leaf(place, member, member_id, path, obligations, acc);
            }
        }
    }
    Ok(())
}

/// One required scalar leaf and the decision discharge makes about it before the scan,
/// shared by the unkeyed and keyed walkers so the rule lives in one place.
enum MemberLeafOutcome {
    /// A verdict known without scanning: a transform-targeted leaf or an optional one.
    Eager(Verdict),
    /// A non-scalar leaf, handled by its own narrowing facts, with no presence cell to
    /// probe.
    Skip,
    /// A required scalar leaf whose presence the scan must prove. The path stays with
    /// the caller, which alone knows whether the cell is reached directly or through a
    /// keyed entry.
    Obligation {
        raw_catalog_id: String,
        label: String,
        leaf: StoreLeafKind,
    },
}

/// Classify one `Field` member into the single leaf decision both walkers share: a
/// transform target and an optional member resolve eagerly, a non-scalar leaf is
/// skipped, and a required scalar leaf becomes an obligation the caller positions with
/// its own data path. `required` reflects the effective requiredness at the member's
/// nesting (a keyed-layer leaf is required per existing entry).
fn classify_member_leaf(
    place: &CheckedSavedPlace,
    member: &CheckedSavedMember,
    required: bool,
    acc: &Accumulator,
) -> MemberLeafOutcome {
    if acc.is_transform(&member.catalog_id) {
        return MemberLeafOutcome::Eager(Verdict::TypedTransformRequired);
    }
    let Some(StoreLeafKind::Scalar(_)) = member.leaf else {
        return MemberLeafOutcome::Skip;
    };
    if !required {
        return MemberLeafOutcome::Eager(Verdict::NoOp);
    }
    MemberLeafOutcome::Obligation {
        raw_catalog_id: member.catalog_id.clone(),
        label: member_label(place, member),
        leaf: member.leaf.clone().expect("scalar leaf checked above"),
    }
}

/// Apply the shared leaf decision for one `Field` member: push an eager verdict, skip
/// a non-scalar leaf, or record an obligation at `path`. `path` is the data path to
/// the cell, which only the calling walker can build.
fn emit_member_leaf(
    place: &CheckedSavedPlace,
    member: &CheckedSavedMember,
    member_id: CatalogId,
    path: Vec<DataPathSegment>,
    obligations: &mut Vec<LeafObligation>,
    acc: &mut Accumulator,
) {
    let CheckedSavedMemberKind::Field { required } = &member.kind else {
        return;
    };
    match classify_member_leaf(place, member, *required, acc) {
        MemberLeafOutcome::Eager(verdict) => {
            acc.push(member_id, member_label(place, member), verdict);
        }
        MemberLeafOutcome::Skip => {}
        MemberLeafOutcome::Obligation {
            raw_catalog_id,
            label,
            leaf,
        } => obligations.push(LeafObligation {
            catalog_id: member_id,
            raw_catalog_id,
            path,
            label,
            leaf,
        }),
    }
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
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let obligations = keyed_leaf_obligations(place, acc)?;
    if obligations.is_empty() {
        return Ok(());
    }
    let store_id = catalog_id(&place.store_catalog_id)?;
    let mut state: HashMap<CatalogId, LeafScan> = HashMap::new();
    for_each_record(
        store,
        &store_id,
        place.identity_keys.len(),
        &mut |identity| {
            scan_keyed_entries(
                store,
                &store_id,
                identity,
                &place.root_members,
                &[],
                &obligations,
                &mut state,
            )
        },
    )?;
    for obligation in obligations {
        let scan = state.remove(&obligation.catalog_id).unwrap_or_default();
        classify_leaf(obligation, scan, acc);
    }
    Ok(())
}

/// Descend the member tree of a record (or a keyed entry) collecting the per-entry
/// presence of each required keyed-layer leaf. At a keyed-group member the scan pages
/// every existing entry under the current data path, then recurses into the entry with
/// its key appended; an unkeyed group is descended in place; a required scalar leaf
/// records its presence into the per-obligation scan keyed by the leaf catalog id.
fn scan_keyed_entries(
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    members: &[CheckedSavedMember],
    prefix: &[DataPathSegment],
    obligations: &[LeafObligation],
    state: &mut HashMap<CatalogId, LeafScan>,
) -> Result<(), StoreError> {
    for member in members {
        if member.catalog_id.is_empty() {
            continue;
        }
        let member_id = catalog_id(&member.catalog_id)?;
        let mut member_path = prefix.to_vec();
        member_path.push(DataPathSegment::Member(member_id.clone()));
        if !member.key_params.is_empty() {
            // A keyed layer: page each existing entry and recurse with its key path.
            // Only a keyed group nests members worth descending; a keyed leaf
            // (`map[K, V]`) carries no required sub-leaf to prove.
            if matches!(member.kind, CheckedSavedMemberKind::Group) {
                for entry_key in store.data_child_keys(store_id, identity, &member_path)? {
                    let mut entry_path = member_path.clone();
                    entry_path.push(DataPathSegment::Key(entry_key));
                    scan_keyed_entries(
                        store,
                        store_id,
                        identity,
                        &member.group_members,
                        &entry_path,
                        obligations,
                        state,
                    )?;
                }
            }
            continue;
        }
        match &member.kind {
            CheckedSavedMemberKind::Group => {
                scan_keyed_entries(
                    store,
                    store_id,
                    identity,
                    &member.group_members,
                    &member_path,
                    obligations,
                    state,
                )?;
            }
            CheckedSavedMemberKind::Field { .. } => {
                let is_obligation = obligations
                    .iter()
                    .any(|obligation| obligation.catalog_id == member_id);
                if is_obligation && !store.data_subtree_exists(store_id, identity, &member_path)? {
                    state
                        .entry(member_id.clone())
                        .or_default()
                        .record_missing(identity);
                }
            }
        }
    }
    Ok(())
}

/// The required scalar-leaf obligations that live inside a keyed layer, captured once
/// for the scan. A transform-targeted leaf is classified eagerly and excluded; an
/// optional leaf places no per-entry obligation and is recorded as a no-op.
fn keyed_leaf_obligations(
    place: &CheckedSavedPlace,
    acc: &mut Accumulator,
) -> Result<Vec<LeafObligation>, StoreError> {
    let mut obligations = Vec::new();
    collect_keyed_leaves(place, &place.root_members, false, &mut obligations, acc)?;
    Ok(obligations)
}

/// Walk the member tree, emitting one keyed-leaf obligation per required scalar leaf
/// that sits inside a keyed layer. `in_keyed` becomes true once the walk has crossed a
/// keyed layer, so a leaf is "keyed" exactly when an ancestor is keyed. The data path
/// stays on [`scan_keyed_entries`], which knows each entry's key; here only the
/// per-leaf classification inputs are captured.
fn collect_keyed_leaves(
    place: &CheckedSavedPlace,
    members: &[CheckedSavedMember],
    in_keyed: bool,
    obligations: &mut Vec<LeafObligation>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for member in members {
        if member.catalog_id.is_empty() {
            continue;
        }
        let keyed_here = in_keyed || !member.key_params.is_empty();
        match &member.kind {
            CheckedSavedMemberKind::Group => {
                collect_keyed_leaves(place, &member.group_members, keyed_here, obligations, acc)?;
            }
            CheckedSavedMemberKind::Field { .. } if in_keyed => {
                // The keyed scan reaches the cell through each entry's key path, so the
                // obligation carries no path of its own.
                let member_id = catalog_id(&member.catalog_id)?;
                emit_member_leaf(place, member, member_id, Vec::new(), obligations, acc);
            }
            CheckedSavedMemberKind::Field { .. } => {}
        }
    }
    Ok(())
}

/// The running presence state for one required leaf: how many records lack it and a
/// bounded sample of those records for the diagnostic.
#[derive(Default)]
struct LeafScan {
    missing_count: usize,
    sample: Vec<Vec<SavedKey>>,
}

impl LeafScan {
    fn record_missing(&mut self, identity: &[SavedKey]) {
        self.missing_count += 1;
        if self.sample.len() < MAX_NAMED_RECORDS {
            self.sample.push(identity.to_vec());
        }
    }
}

/// Classify one required leaf from its scan state: proven when every record carries
/// it, a constant default when an `evolve default` supplies a typed fill, else a
/// fail-closed repair. A default whose value is not a constant the checker can
/// evaluate is itself a repair, with a diagnostic steering the developer to a
/// transform.
fn classify_leaf(leaf: LeafObligation, state: LeafScan, acc: &mut Accumulator) {
    if state.missing_count == 0 {
        acc.push(leaf.catalog_id, leaf.label, Verdict::DataProof);
        return;
    }
    acc.counts.records_lacking_member += state.missing_count;
    let verdict = match acc.default_value_for(&leaf.raw_catalog_id, &leaf.leaf) {
        Some(Ok(value)) => {
            acc.counts.records_to_backfill += state.missing_count;
            Verdict::Default { value }
        }
        Some(Err(message)) => {
            acc.diagnostics.push(message);
            Verdict::RepairRequired {
                reason: RepairReason::MissingRequiredMember,
            }
        }
        None => {
            acc.diagnostics.push(missing_member_message(
                &leaf.label,
                state.missing_count,
                &state.sample,
            ));
            Verdict::RepairRequired {
                reason: RepairReason::MissingRequiredMember,
            }
        }
    };
    acc.push(leaf.catalog_id, leaf.label, verdict);
}

/// One unique-index obligation to probe during the record scan: the index catalog id
/// and how to read each key column's value from a record. The collision state the scan
/// builds is keyed by `catalog_id`, which the per-index classification then looks up.
struct UniqueIndexProbe {
    catalog_id: CatalogId,
    columns: Vec<IndexKeyColumn>,
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

/// Build a key-column reader for every unique index the place declares. Each column
/// resolves to an identity position or a top-level member cell with the meaning to
/// decode it; a column that resolves to neither leaves the index unprobeable and the
/// index falls to a plain derived rebuild.
fn unique_index_probes(place: &CheckedSavedPlace) -> Result<Vec<UniqueIndexProbe>, StoreError> {
    let mut probes = Vec::new();
    for index in &place.indexes {
        if !index.unique {
            continue;
        }
        let Some(columns) = index_key_columns(place, index)? else {
            continue;
        };
        probes.push(UniqueIndexProbe {
            catalog_id: catalog_id(&index.catalog_id)?,
            columns,
        });
    }
    Ok(probes)
}

/// The key-column readers for one index, or `None` when a column resolves to neither
/// an identity key position nor a top-level plain field. `None` makes the index
/// unprobeable, so it falls to a plain derived rebuild without a collision check. This
/// is sound only because v0.1 index keys are single-segment top-level fields or
/// identity keys; a future index over a nested or keyed-layer column would resolve to
/// `None` here and silently skip its uniqueness check, so widening index key shapes
/// must revisit this resolution rather than leave the fall-through in place.
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
/// repair instead, using the collision state the scan accumulated for it. Every v0.1
/// unique index is probed -- its keys are single-segment top-level fields or identity
/// keys (see `index_key_columns`) -- so a clean unique index rebuilds and a colliding
/// one repairs. A future index over a nested or keyed-layer column would not resolve to
/// a probe; that path must gain a fail-closed branch here before it can rebuild an
/// unchecked unique index.
fn classify_indexes(
    place: &CheckedSavedPlace,
    collisions: &HashMap<CatalogId, IndexScan>,
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
            acc.diagnostics.push(format!(
                "unique index `{}` has {colliding} colliding key tuple(s); resolve duplicates before activating",
                index.name
            ));
            Verdict::RepairRequired {
                reason: RepairReason::UniqueIndexCollision,
            }
        } else {
            Verdict::DerivedRebuild
        };
        acc.push_index(index_id, format!("index {}", index.name), verdict);
    }
    Ok(())
}

/// Classify the accepted catalog entries current source no longer declares. A retire
/// intent marks the proposal entry `Removed`: dropping populated data is a
/// destructive decision that names the exact catalog id and count. An entry source
/// merely stopped declaring, with no retire and no dependent, is a deprecation:
/// dropping a sparse field is a legal no-op and its data lingers. A dropped member an
/// active index still reads cannot be silently dropped; it needs an explicit retire
/// intent that also removes or rebinds the index.
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
            CatalogLifecycle::Removed => {
                if retired_member_is_nested(program, entry) {
                    acc.diagnostics.push(format!(
                        "retiring `{}` drops a member nested under a group or keyed layer, which apply does not yet support; retire a top-level member instead",
                        entry.path
                    ));
                    acc.record(
                        entry_id,
                        format!("retire {}", entry.path),
                        Verdict::RepairRequired {
                            reason: RepairReason::NestedRetireUnsupported,
                        },
                        is_index,
                    );
                } else {
                    let populated = populated_member_records(program, store, entry)?;
                    acc.record(
                        entry_id,
                        format!("retire {}", entry.path),
                        Verdict::DestructiveDecisionRequired { populated },
                        is_index,
                    );
                }
            }
            CatalogLifecycle::Active | CatalogLifecycle::Deprecated => {
                if let Some((index_name, index_id)) = index_depends_on(program, entry)? {
                    acc.diagnostics.push(format!(
                        "dropped `{}` is still used by index `{index_name}`; retire it with an evolve intent",
                        entry.path
                    ));
                    acc.record(
                        entry_id,
                        format!("drop {}", entry.path),
                        Verdict::RepairRequired {
                            reason: RepairReason::RetireRequired { index: index_id },
                        },
                        is_index,
                    );
                } else {
                    acc.record(
                        entry_id,
                        format!("deprecate {}", entry.path),
                        Verdict::Deprecated,
                        is_index,
                    );
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
    for_each_record(
        store,
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

/// Visit every record identity under `store_id`, descending `arity` key levels and
/// invoking `visit` with each full identity tuple. The descent reads one key at a
/// time through the paged child cursor, so the scan never materializes the whole
/// store; only the current identity path is held.
fn for_each_record(
    store: &TreeStore,
    store_id: &CatalogId,
    arity: usize,
    visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut identity = Vec::new();
    descend_records(store, store_id, arity.max(1), &mut identity, visit)
}

fn descend_records(
    store: &TreeStore,
    store_id: &CatalogId,
    remaining: usize,
    identity: &mut Vec<SavedKey>,
    visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut next = store.record_first_child(store_id, identity)?;
    while let Some(key) = next {
        identity.push(key.clone());
        if remaining == 1 {
            visit(identity)?;
        } else {
            descend_records(store, store_id, remaining - 1, identity, visit)?;
        }
        identity.pop();
        next = store.record_next_child(store_id, identity, &key)?;
    }
    Ok(())
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

/// Accumulates verdicts, labels, counts, affected ids, and diagnostics across the
/// families, and resolves `evolve default` fills. Affected ids are typed
/// [`CatalogId`]s validated once on insertion and partitioned at classify time into
/// data roots and store indexes, so apply never re-classifies them from current source.
struct Accumulator {
    verdicts: Vec<ObligationVerdict>,
    labels: Vec<DischargeLabel>,
    counts: super::witness::DischargeCounts,
    changed_roots: BTreeSet<CatalogId>,
    changed_indexes: BTreeSet<CatalogId>,
    diagnostics: Vec<String>,
    defaults: HashMap<String, marrow_syntax::Expression>,
    transforms: BTreeSet<String>,
}

impl Accumulator {
    fn new(defaults: Vec<EvolveDefault>, transforms: BTreeSet<String>) -> Self {
        Self {
            verdicts: Vec::new(),
            labels: Vec::new(),
            counts: super::witness::DischargeCounts::default(),
            changed_roots: BTreeSet::new(),
            changed_indexes: BTreeSet::new(),
            diagnostics: Vec::new(),
            defaults: defaults
                .into_iter()
                .map(|default| (default.catalog_id, default.value))
                .collect(),
            transforms,
        }
    }

    fn is_transform(&self, catalog_id: &str) -> bool {
        self.transforms.contains(catalog_id)
    }

    /// The typed constant fill for a defaulted member, or an error message when the
    /// default value is not a constant the checker can evaluate against the leaf
    /// type. `None` when no `evolve default` targets the member.
    fn default_value_for(
        &self,
        raw_catalog_id: &str,
        leaf: &StoreLeafKind,
    ) -> Option<Result<super::witness::DefaultValue, String>> {
        let value = self.defaults.get(raw_catalog_id)?;
        let StoreLeafKind::Scalar(scalar) = leaf else {
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
    fn push(&mut self, id: CatalogId, place: String, verdict: Verdict) {
        self.record(id, place, verdict, false);
    }

    /// Record a verdict for a store-index obligation. Its catalog id joins the
    /// changed-index partition, so apply stamps it as an index rather than a root.
    fn push_index(&mut self, id: CatalogId, place: String, verdict: Verdict) {
        self.record(id, place, verdict, true);
    }

    fn record(&mut self, id: CatalogId, place: String, verdict: Verdict, is_index: bool) {
        self.changed_set(is_index).insert(id.clone());
        self.labels.push(DischargeLabel {
            catalog_id: id.clone(),
            place,
        });
        self.verdicts.push(ObligationVerdict {
            catalog_id: id,
            verdict,
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
            labels: self.labels,
            counts: self.counts,
            changed_root_catalog_ids: self.changed_roots.into_iter().collect(),
            changed_index_catalog_ids: self.changed_indexes.into_iter().collect(),
            diagnostics: self.diagnostics,
        }
    }
}
