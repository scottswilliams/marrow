use std::collections::{HashMap, HashSet};

use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::StoreLeafKind;
use crate::evolution::witness::{RepairReason, Verdict};
use crate::executable::{
    CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, for_each_place_record,
};
use crate::program::CheckedProgram;

use super::enum_shrink::{EnumMembers, leaf_value_valid};
use super::index::{
    IndexScan, ProspectiveIndexKey, UniqueIndexPlan, classify_indexes, prospective_index_key,
    unique_index_plan,
};
use super::structural_backstop::for_each_entry_path;
use super::{
    Accumulator, MAX_NAMED_RECORDS, catalog_id, member_label, missing_member_message,
    required_catalog_id,
};

/// Classify the member-presence and index obligations one saved root carries. A single
/// streaming scan visits each record once, probing every required leaf and deriving every
/// prospective unique-index key tuple; the verdicts fall out of the accumulated state.
pub(super) fn discharge_root(
    program: &CheckedProgram,
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
    let mut invalid_index_values = vec![0usize; unique_indexes.len()];
    let mut scanned = 0usize;

    for_each_place_record(store, place, &mut |identity| {
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
            state.record_read(
                program,
                bytes.as_deref(),
                leaf.leaf.as_ref(),
                enum_members,
                identity,
            );
        }
        for ((probe, state), invalid_count) in unique_indexes
            .iter()
            .zip(index_state.iter_mut())
            .zip(invalid_index_values.iter_mut())
        {
            match prospective_index_key(store, &store_id, probe, identity)? {
                ProspectiveIndexKey::Present(key) => state.observe(key),
                ProspectiveIndexKey::Absent => {}
                ProspectiveIndexKey::Invalid => *invalid_count += 1,
            }
        }
        Ok(())
    })?;

    acc.counts.scanned_records += scanned;

    for (leaf, state) in leaves.into_iter().zip(leaf_state) {
        classify_leaf(leaf, state, acc)?;
    }
    let invalid_values: HashMap<CatalogId, usize> = unique_indexes
        .iter()
        .map(|probe| probe.catalog_id.clone())
        .zip(invalid_index_values)
        .filter(|(_, count)| *count > 0)
        .collect();
    let collisions: HashMap<CatalogId, IndexScan> = unique_indexes
        .into_iter()
        .map(|probe| probe.catalog_id)
        .zip(index_state)
        .collect();
    classify_indexes(place, &collisions, &unprobeable, &invalid_values, acc)?;
    discharge_keyed_layers(store, program, place, enum_members, acc)?;
    Ok(())
}

/// One leaf obligation the scan visits, of any leaf kind. A transform-targeted member is
/// classified eagerly and excluded.
struct LeafObligation {
    catalog_id: CatalogId,
    path: Vec<DataPathSegment>,
    label: String,
    /// The leaf kind whose bytes the scan validates, or `None` for a non-tokenizable position
    /// (`sequence`/`unknown`). Such a leaf arises only as a retype, so any present cell counts
    /// as populated and the retype check fails it closed.
    leaf: Option<StoreLeafKind>,
    /// The leaf was declared `ErrorCode`, so a constant default backfilled into it must
    /// satisfy the dotted-lowercase grammar; its `Str` leaf kind cannot carry the spelling.
    error_code: bool,
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
                emit_disappeared_leaf(place, member, member_id, path.clone(), obligations);
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
        label: String,
        leaf: Option<StoreLeafKind>,
        error_code: bool,
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
        label: member_label(place, member),
        leaf,
        error_code: member.error_code,
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
            label,
            leaf,
            error_code,
            required,
            renamed,
            retyped,
        } => obligations.push(LeafObligation {
            catalog_id: member_id,
            path,
            label,
            leaf,
            error_code,
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
    member_id: CatalogId,
    path: Vec<DataPathSegment>,
    obligations: &mut Vec<LeafObligation>,
) {
    obligations.push(LeafObligation {
        catalog_id: member_id,
        path,
        label: member_label(place, member),
        leaf: None,
        error_code: false,
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
    program: &CheckedProgram,
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
        program,
        state: HashMap::new(),
    };
    let mut flat_state: Vec<LeafScan> = flat_retyped.iter().map(|_| LeafScan::default()).collect();
    for_each_place_record(store, place, &mut |identity| {
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
    program: &'a CheckedProgram,
    state: HashMap<CatalogId, LeafScan>,
}

impl KeyedScan<'_> {
    /// Descend the member tree of a record or keyed entry, collecting per-entry presence of
    /// each keyed leaf. A keyed group pages every entry and recurses with its key appended;
    /// a keyed leaf records the value cell under each entry key; an unkeyed group descends
    /// in place; a top-level leaf records directly.
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
                // recurses into each entry; a keyed leaf holds its value directly under the
                // entry key, recorded against the member's obligation.
                let (store, store_id) = (self.store, self.store_id);
                let key_scalars: Vec<_> =
                    member.key_params.iter().map(|param| param.scalar).collect();
                for_each_entry_path(
                    store,
                    store_id,
                    identity,
                    &member_path,
                    &key_scalars,
                    |entry_path| {
                        match &member.kind {
                            CheckedSavedMemberKind::Group => {
                                self.descend(identity, &member.group_members, entry_path)?;
                            }
                            CheckedSavedMemberKind::Field { .. } => {
                                self.record_leaf(&member_id, identity, entry_path)?;
                            }
                        }
                        Ok(false)
                    },
                )?;
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
        entry.record_read(
            self.program,
            bytes.as_deref(),
            leaf.as_ref(),
            self.enum_members,
            identity,
        );
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
/// through a keyed layer. `in_keyed` becomes true once the walk crosses a keyed layer. The obligation `path` is the
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
        // check that would look under the wrong shape. A keyed leaf or keyed group at the
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
                emit_disappeared_leaf(place, member, member_id, obligation_path, obligations);
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
        program: &CheckedProgram,
        bytes: Option<&[u8]>,
        leaf: Option<&StoreLeafKind>,
        enum_members: &EnumMembers,
        identity: &[SavedKey],
    ) {
        match (bytes, leaf) {
            (None, _) => self.record_missing(identity),
            (Some(bytes), Some(leaf)) if leaf_value_valid(program, leaf, bytes, enum_members) => {
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
    let verdict = match acc.default_value_for(id.as_str(), leaf.leaf.as_ref(), leaf.error_code) {
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
