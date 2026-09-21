//! The flat-executable classification and the sealed branch, group, and index projections
//! of a decoded root.
//!
//! `seal_branch_run` recurses to `MAX_DURABLE_DEPTH`, which `VERIFY_STACK_BYTES` budgets.

use super::super::model::DecodedRoot;
use super::super::reject;
use crate::reject::{Projection, RejectionKind as Kind, VerifyPhase, VerifyRejection};
use crate::sealed::{
    SealedBranch, SealedGroup, SealedIndex, SealedIndexComponent, SealedRecordType,
};
use marrow_image::{
    DurableIndexComponent, DurableMemberView, DurableMemberViewKind, DurableMemberViews,
    DurableProductGraph, ImageType, LedgerIdBytes, RootId,
};
use std::collections::HashMap;
use std::rc::Rc;

/// Whether a decoded root is the flat keyed root the kernel executes: at least one key
/// column and a member tree of top-level storable-value fields (scalar or widened) and
/// keyed branches of the same shape (no group). The key may be single-column or a composite
/// tuple, at the root and at every branch. Re-derived from the decoded graph, so the
/// flat/parked classification never trusts a compiler summary.
pub(crate) fn is_flat_executable_root(root: &DecodedRoot) -> bool {
    !root.keys.is_empty() && root.members.iter().all(member_flat_at_root)
}

/// Whether one member is a field-only keyed branch, at every level below it — the branch
/// shape the kernel executes at any depth. Its key is one or more columns and every direct
/// member itself keeps flat: a field (scalar or widened composite), or a nested branch that
/// is itself a simple branch. A static `group` breaks it. The rule admits an arbitrarily
/// deep chain of field-only branches with composite keys, which the recursive physical
/// layout and profile serve.
///
/// The descent is an explicit stack of member runs, not recursion: nesting depth is a
/// property of the rows the image stated, and this predicate's own stack use must not be.
fn is_simple_branch(member: DurableMemberView<'_>) -> bool {
    let DurableMemberViewKind::Branch(branch) = member.kind() else {
        return false;
    };
    if branch.keys().is_empty() {
        return false;
    }
    let mut stack = vec![member.members()];
    while let Some(run) = stack.last_mut() {
        let Some(inner) = run.next() else {
            stack.pop();
            continue;
        };
        match inner.kind() {
            DurableMemberViewKind::Field(_) => {}
            DurableMemberViewKind::Group(_) => return false,
            DurableMemberViewKind::Branch(nested) => {
                if nested.keys().is_empty() {
                    return false;
                }
                stack.push(inner.members());
            }
        }
    }
    true
}

/// Whether a root's *direct* member keeps the root flat-executable. It admits one more
/// shape than [`is_simple_branch`]'s inner rule: a root-level unkeyed `group` whose own
/// members are all storable-value fields (a scalar or widened composite). A group is a
/// value unit of the root entry, executable at the root level, but a group nested in a
/// branch or in another group still parks — the branch rule keeps `Group => false`, so a
/// group below the root's direct members never makes its enclosing branch flat.
pub(crate) fn member_flat_at_root(member: DurableMemberView<'_>) -> bool {
    match member.kind() {
        DurableMemberViewKind::Field(_) => true,
        DurableMemberViewKind::Group(_) => member
            .members()
            .all(|inner| matches!(inner.kind(), DurableMemberViewKind::Field(_))),
        DurableMemberViewKind::Branch(_) => is_simple_branch(member),
    }
}

/// Seal a member tree's keyed branches into the recursive [`SealedBranch`] tree, in
/// declaration order, so a [`SealedSiteTarget::BranchEntry`] branch path indexes it level
/// by level. Called only for a flat-executable root, so every branch is a scalar-field
/// keyed branch (its `keys` are its ordered key columns) and its own members recurse
/// through the same rule.
pub(crate) fn seal_branches(
    members: &DurableProductGraph,
    strings: &[Rc<str>],
) -> Vec<SealedBranch> {
    seal_branch_run(members.iter(), strings)
}

/// Seal one run of member rows into its branch list, descending through each branch's own
/// members. Nesting is bounded by the decoded member depth the table phase already
/// enforced.
fn seal_branch_run(members: DurableMemberViews<'_>, strings: &[Rc<str>]) -> Vec<SealedBranch> {
    members
        .filter_map(|member| match member.kind() {
            DurableMemberViewKind::Branch(branch) => Some(SealedBranch {
                name: strings[branch.name().index() as usize].clone(),
                keys: branch.keys().iter().map(|key| key.scalar).collect(),
                record: branch.record(),
                branches: seal_branch_run(member.members(), strings),
            }),
            _ => None,
        })
        .collect()
}

/// Seal a flat-executable root's root-level unkeyed groups into [`SealedGroup`]s, in
/// declaration order, so a [`SealedSiteTarget::GroupEntry`] group index selects one.
/// Each group's name and materialized record come from the root's own record: the
/// verifier's record↔member tie (validated in the table phase) places one trailing
/// group slot per `Group` member, after the leading scalar/widened field slots, in
/// declaration order — so the group slot at `field_count + ordinal` is exactly this
/// group's slot. Called only for a flat-executable root, whose groups are all
/// storable-value-field groups.
pub(crate) fn seal_groups(root: &DecodedRoot, types: &[SealedRecordType]) -> Vec<SealedGroup> {
    let record = &types[root.record as usize];
    let field_count = root
        .members
        .iter()
        .filter(|member| matches!(member.kind(), DurableMemberViewKind::Field(_)))
        .count();
    root.members
        .iter()
        .filter(|member| matches!(member.kind(), DurableMemberViewKind::Group(_)))
        .enumerate()
        .map(|(ordinal, _group)| {
            let slot = &record.fields[field_count + ordinal];
            let record = match slot.ty {
                ImageType::Record { idx, .. } => idx,
                _ => unreachable!("the record↔member tie places a Record slot per group member"),
            };
            SealedGroup {
                name: slot.name.clone(),
                record,
            }
        })
        .collect()
}

/// Seal every managed index of the root occurrence at DURABLE-table index
/// `root_index`, resolving each ledger-id projection to the record/key positions the
/// path kernel maintains.
///
/// A managed index is declared by one root occurrence and its projection is resolved
/// against that occurrence and no other: a field component names its position in the
/// Product declaration's member order, while a key component names its column in *this
/// occurrence's* key tuple, which two roots over one Product may spell differently.
/// Sealing the whole set at once is what makes that pairing structural, so no caller can
/// pair an index with a neighbouring root.
///
/// Every component already resolved to a real leaf during decode, so a miss here is an
/// internal inconsistency the verifier refuses rather than mis-addressing a cell.
pub(crate) fn seal_root_indexes(
    root_index: u16,
    root: &DecodedRoot,
) -> Result<Vec<SealedIndex>, VerifyRejection> {
    // The occurrence's two leaf position tables, built once for the whole set rather than
    // rescanned per component: a top-level field's index into the materialized record
    // (their orders are tied during root decode), and a key column's position in this
    // occurrence's own key tuple.
    let mut fields: HashMap<LedgerIdBytes, u16> = HashMap::new();
    for (position, field) in root
        .members
        .iter()
        .filter_map(|member| match member.kind() {
            DurableMemberViewKind::Field(field) => Some(field),
            _ => None,
        })
        .enumerate()
    {
        fields.entry(field.id()).or_insert(position as u16);
    }
    let mut columns: HashMap<LedgerIdBytes, u16> = HashMap::with_capacity(root.keys.len());
    for (column, (_, id)) in root.keys.iter().enumerate() {
        columns.entry(*id).or_insert(column as u16);
    }
    root.indexes
        .iter()
        .map(|index| {
            let projection = index
                .components
                .iter()
                .map(|component| match component {
                    DurableIndexComponent::Field(id) => fields
                        .get(id)
                        .copied()
                        .map(SealedIndexComponent::Field)
                        .ok_or(reject(
                            VerifyPhase::Table,
                            Kind::IndexProjection(Projection::FieldUnknown),
                        )),
                    DurableIndexComponent::Key(id) => columns
                        .get(id)
                        .copied()
                        .map(SealedIndexComponent::Key)
                        .ok_or(reject(
                            VerifyPhase::Table,
                            Kind::IndexProjection(Projection::KeyUnknown),
                        )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SealedIndex {
                id: index.id,
                root: RootId::from_index(root_index),
                unique: index.unique,
                components: index.components.clone(),
                projection,
            })
        })
        .collect()
}
