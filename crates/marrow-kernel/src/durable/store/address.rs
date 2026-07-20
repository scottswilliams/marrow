//! Shared physical addressing and record-shape glue over resolved sites: marker-stem
//! derivation, column consumption, and the record/group projections the sessions and
//! read ops build on.

use marrow_store::ReadView;

use super::super::physical;
use super::super::{AuthTarget, AuthorizedSite, FieldSchema, GroupSchema, KernelFault};
use crate::codec::key::KeyScalar;
use crate::codec::value::ScalarKind;

/// The physical marker stem of the node `site` addresses at key-path `keys`: the root
/// marker followed by one branch-child stem per branch hop. The single owner of
/// key-path-to-node-stem resolution, so a root and a branch node derive their stem the
/// same way. The verifier proves the key-path arity and each element's scalar kind
/// against the site's declared root and hop kinds, but this is the trust boundary the
/// independently verified image crosses into the kernel, so a mismatch faults
/// [`KernelFault::Corruption`] in release rather than dropping a hop and mis-addressing
/// the write to a shallower node.
pub(super) fn node_stem(site: &AuthorizedSite, keys: &[KeyScalar]) -> Result<Vec<u8>, KernelFault> {
    let mut cols = keys;
    let root_cols = take_columns(&mut cols, &site.key)?;
    let mut stem = physical::marker_key(&site.root, root_cols);
    for hop in &site.branch {
        let hop_cols = take_columns(&mut cols, &hop.key)?;
        stem = physical::branch_child_stem(&stem, &hop.name, hop_cols);
    }
    // Every operand column must be consumed by a node in the branch path; a leftover
    // column is a key-path/schema arity disagreement (a forged image), faulted rather
    // than silently ignored.
    if cols.is_empty() {
        Ok(stem)
    } else {
        Err(KernelFault::Corruption)
    }
}

/// Take the next `kinds.len()` columns off the front of `cols`, checking each column's
/// scalar kind matches the expected column kind, and advance `cols` past them. A short
/// key-path or a per-column kind mismatch faults [`KernelFault::Corruption`] — the trust
/// boundary the verifier's arity/kind proof stands on, defended in depth here so a forged
/// image can never mis-split a composite key-path across nodes.
pub(super) fn take_columns<'a>(
    cols: &mut &'a [KeyScalar],
    kinds: &[ScalarKind],
) -> Result<&'a [KeyScalar], KernelFault> {
    if cols.len() < kinds.len() {
        return Err(KernelFault::Corruption);
    }
    let (head, tail) = cols.split_at(kinds.len());
    for (column, kind) in head.iter().zip(kinds) {
        if column.scalar_kind() != *kind {
            return Err(KernelFault::Corruption);
        }
    }
    *cols = tail;
    Ok(head)
}

/// The record whose fields a site addresses: the entry's own record for a whole-entry
/// site, the containing node's record for a field site. Index maintenance reads projected
/// leaves from it.
pub(super) fn site_record(site: &AuthorizedSite) -> &[FieldSchema] {
    match &site.target {
        AuthTarget::Entry { fields, .. } => fields,
        AuthTarget::Field { record, .. } => record,
        AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
            unreachable!("verifier proved a node op targets a node site")
        }
    }
}

/// The position of a field site's field within its containing record.
pub(super) fn field_index_in_record(site: &AuthorizedSite, record: &[FieldSchema]) -> usize {
    let AuthTarget::Field { name, .. } = &site.target else {
        unreachable!("a field op targets a field site")
    };
    record
        .iter()
        .position(|field| &field.name == name)
        .expect("a field site names a record field")
}

/// The field name of a field-target site, checking the required flag matches the
/// operation. The verifier already restricts required vs sparse ops to the right
/// site target; this reads the token's own flag as defense-in-depth over the trust
/// boundary rather than trusting a caller assertion.
pub(super) fn field_name(site: &AuthorizedSite, want_required: bool) -> &str {
    match &site.target {
        AuthTarget::Field { name, required, .. } => {
            debug_assert_eq!(
                *required, want_required,
                "site required-ness must match the operation the verifier admitted"
            );
            name
        }
        AuthTarget::Entry { .. } | AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
            unreachable!("verifier proved a field-target site")
        }
    }
}

/// The addressed node's own record fields and groups for a whole-entry op — the whole
/// payload footprint the consequence planner enumerates. The verifier proves a whole-entry
/// opcode targets an entry site, so a field target here is unreachable. A branch node
/// carries no group, so its group slice is empty.
pub(super) fn node_shape(site: &AuthorizedSite) -> (&[FieldSchema], &[GroupSchema]) {
    match &site.target {
        AuthTarget::Entry { fields, groups } => (fields, groups),
        AuthTarget::Field { .. } | AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
            unreachable!("verifier proved a whole-entry op targets an entry site")
        }
    }
}

pub(super) fn read_raw<V: ReadView>(cells: &V, key: &[u8]) -> Result<Option<Vec<u8>>, KernelFault> {
    cells.get(key).map_err(KernelFault::Engine)
}

/// The group name and its own record fields a group site addresses. The verifier proves
/// a whole-group op targets a group site, so any other target here is a forged image.
pub(super) fn group_target(site: &AuthorizedSite) -> (&str, &[FieldSchema]) {
    match &site.target {
        AuthTarget::Group { name, fields } => (name, fields),
        AuthTarget::Entry { .. } | AuthTarget::Field { .. } | AuthTarget::Index { .. } => {
            unreachable!("verifier proved a whole-group op targets a group site")
        }
    }
}
