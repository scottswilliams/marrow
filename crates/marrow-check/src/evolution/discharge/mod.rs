//! Data-attached discharge: classify each evolution obligation against the live
//! store snapshot, read-only. Obligations map to [`Verdict`] roles; discharge never
//! writes — the verdicts and counts feed the witness a future apply consumes.
//!
//! Obligations come from two sources: the members and indexes a [`CheckedSavedPlace`]
//! resolves for each saved root, and the accepted catalog entries current source no
//! longer declares. Both read the same catalog identity facts.
//!
//! Records are streamed, never materialized: a single paged scan probes every
//! required leaf and derives every prospective unique-index key tuple. Required-leaf
//! state is bounded; each unique-index probe retains a seen set proportional to the
//! number of distinct populated key tuples so it can fail closed on collisions.

mod absent_source;
mod accepted_state;
mod enum_shrink;
mod index;
mod leaf_obligations;
mod structural_backstop;
mod transforms;

use std::collections::{BTreeSet, HashMap, HashSet};

use marrow_catalog::CatalogEntryKind;
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

use super::const_default::default_value_for_leaf;
use super::witness::{ObligationVerdict, RejectedDefault, RepairReason, Verdict};
use crate::StoreLeafKind;
use crate::executable::{CheckedSavedMember, CheckedSavedPlace, checked_activation_root_places};
use crate::program::{CheckedProgram, EvolveDefault};

use accepted_state::{
    accepted_member_leaves, accepted_member_structs, accepted_store_key_shapes,
    classify_store_key_shape, proposal_changed_catalog_ids, renamed_member_ids,
};
use enum_shrink::{EnumMembers, ShrunkEnums};
use leaf_obligations::discharge_root;
use structural_backstop::classify_structural_backstop;
use transforms::discharge_transforms;

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
    pub guidance: RepairGuidance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairGuidance {
    None,
    Retire { target: String },
    RenameOrRetire { from: String, to: String },
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
        transforms::pending_transform_ids(program),
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
        discharge_root(program, store, place, &enum_members, &mut acc)?;
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

/// Whether the program declares an `evolve transform` not yet applied against the current
/// source. The activation fence cannot see a shape-neutral in-place transform — it moves no
/// source digest or epoch — so the run path consults this to honor the pending-evolution run
/// blocker, and a transform already discharged (its target records the transform's own identity)
/// no longer reads as pending.
pub fn has_pending_transform(program: &CheckedProgram) -> bool {
    !transforms::pending_transform_ids(program).is_empty()
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

    fn is_changed_index(&self, id: &CatalogId) -> bool {
        self.changed_indexes.contains(id)
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
            .and_then(marrow_catalog::structural_signature_leaf_token)
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
        error_code: bool,
    ) -> Option<Result<super::witness::DefaultValue, RejectedDefault>> {
        let value = self.defaults.get(raw_catalog_id)?;
        Some(default_value_for_leaf(value, leaf, error_code))
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
        self.diagnostic_with_guidance(id, message, RepairGuidance::None);
    }

    fn diagnostic_with_guidance(
        &mut self,
        id: CatalogId,
        message: String,
        guidance: RepairGuidance,
    ) {
        self.diagnostics.push(RepairDiagnostic {
            catalog_id: id,
            message,
            guidance,
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
