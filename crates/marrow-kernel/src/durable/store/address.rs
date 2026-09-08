//! Shared physical addressing and record-shape glue over resolved sites: marker-stem
//! derivation, column consumption, and the numbered record/group projections the sessions
//! and read ops build on. Every stem derives from cell-key numbers, never source spelling.

use marrow_store::ReadView;

use super::super::physical;
use super::super::{AuthTarget, AuthorizedSite, KernelFault, ResolvedField, ResolvedGroup};
use crate::codec::key::KeyScalar;
use crate::codec::value::{ScalarKind, scalar_key_matches_type};

/// Resolve an addressed entry's static family and validate its full key path before
/// encoding it once. Root and branch entries use the same marker constructor. Exact
/// column exhaustion and declared scalar domains defend the independently verified
/// image boundary before any operation accesses the engine.
pub(super) fn node_stem(site: &AuthorizedSite, keys: &[KeyScalar]) -> Result<Vec<u8>, KernelFault> {
    let mut cols = keys;
    take_columns(&mut cols, &site.key)?;
    let mut family = site.root_number;
    for hop in &site.branch {
        take_columns(&mut cols, &hop.key)?;
        family = hop.number;
    }
    // Every operand column must be consumed by a node in the branch path; a leftover
    // column is a key-path/schema arity disagreement (a forged image), faulted rather
    // than silently ignored.
    if cols.is_empty() {
        Ok(physical::marker_key(family, keys))
    } else {
        Err(KernelFault::Corruption)
    }
}

/// Take the next `kinds.len()` columns off the front of `cols`, checking each column's
/// scalar kind and supported domain, and advance `cols` past them. A short
/// key-path or a per-column mismatch faults [`KernelFault::Corruption`] — the trust
/// boundary the verifier's arity/kind proof stands on, defended in depth here so a forged
/// image can never mis-split a composite key-path across nodes.
pub(super) fn take_columns(
    cols: &mut &[KeyScalar],
    kinds: &[ScalarKind],
) -> Result<(), KernelFault> {
    if cols.len() < kinds.len() {
        return Err(KernelFault::Corruption);
    }
    let (head, tail) = cols.split_at(kinds.len());
    for (column, kind) in head.iter().zip(kinds) {
        if !scalar_key_matches_type(column, *kind) {
            return Err(KernelFault::Corruption);
        }
    }
    *cols = tail;
    Ok(())
}

/// The numbered record whose fields a site addresses: the entry's own record for a
/// whole-entry site, the containing node's record for a field site. Index maintenance reads
/// projected leaves from it.
pub(super) fn site_record(site: &AuthorizedSite) -> &[ResolvedField] {
    match &site.target {
        AuthTarget::Entry { fields, .. } => fields,
        AuthTarget::Field { payload } => &payload.record,
        AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
            unreachable!("verifier proved a node op targets a node site")
        }
    }
}

/// The position of a field site's field within its containing record, by cell-key number.
pub(super) fn field_index_in_record(site: &AuthorizedSite, record: &[ResolvedField]) -> usize {
    let AuthTarget::Field { payload } = &site.target else {
        unreachable!("a field op targets a field site")
    };
    record
        .iter()
        .position(|field| field.number == payload.number)
        .expect("a field site names a record field")
}

/// The cell-key number and required flag of a field-target site. The flag is the
/// token's own, read here so an erase of a required field is refused from the site the
/// kernel resolved rather than from a caller assertion.
pub(super) fn field_target(site: &AuthorizedSite) -> (physical::NodeNumber, bool) {
    match &site.target {
        AuthTarget::Field { payload } => (payload.number, payload.required),
        AuthTarget::Entry { .. } | AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
            unreachable!("verifier proved a field-target site")
        }
    }
}

/// The addressed node's own numbered record fields and groups for a whole-entry op — the
/// whole payload footprint the consequence planner enumerates. The verifier proves a
/// whole-entry opcode targets an entry site, so a field target here is unreachable. A branch
/// node carries no group, so its group slice is empty.
pub(super) fn node_shape(site: &AuthorizedSite) -> (&[ResolvedField], &[ResolvedGroup]) {
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

/// The group's cell-key number and its own numbered record fields a group site addresses.
/// The verifier proves a whole-group op targets a group site, so any other target here is a
/// forged image.
pub(super) fn group_target(site: &AuthorizedSite) -> (physical::NodeNumber, &[ResolvedField]) {
    match &site.target {
        AuthTarget::Group { number, fields } => (*number, fields),
        AuthTarget::Entry { .. } | AuthTarget::Field { .. } | AuthTarget::Index { .. } => {
            unreachable!("verifier proved a whole-group op targets a group site")
        }
    }
}
