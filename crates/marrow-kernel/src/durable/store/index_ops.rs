//! Managed-index reads: the nonunique progressive-prefix scan and the unique complete-key
//! lookup over a maintained index cell family, with the per-column kind checks the trust
//! boundary rests on.

use marrow_store::ReadView;

use super::super::physical;
use super::super::{AuthorizedSite, BoundedKeys, BoundedLimit, KernelFault};
use crate::codec::key::KeyScalar;
use crate::codec::value::ScalarKind;

/// The resolved index-read target a site addresses: its cell-family identity, unique
/// flag, and ordered projection component kinds. A non-index site reaching an index op
/// is a forged image routing a read to the wrong site kind — faulted rather than trusted.
fn index_target(site: &AuthorizedSite) -> Result<(&[u8; 16], bool, &[ScalarKind]), KernelFault> {
    match site.index_read() {
        Some(parts) => Ok(parts),
        None => Err(KernelFault::Corruption),
    }
}

/// Freeze the first `limit` distinct values of a nonunique index's next projected
/// component, holding the leading `prefix`, and report whether a further distinct value
/// existed. Acquires at most `limit + 1` distinct component values through the index cell
/// family, costing `O(limit + 1)` seeks: one prefix-successor seek past each yielded
/// value passes its whole run of rows regardless of fan-out (the index traversal-skip
/// law). An index scan reads only the derived index and establishes no source presence.
pub(super) fn op_index_scan<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    prefix: &[KeyScalar],
    from: Option<KeyScalar>,
    limit: BoundedLimit,
) -> Result<BoundedKeys, KernelFault> {
    let (id, unique, projection) = index_target(site)?;
    // A scan is the nonunique read; a unique index admits only the complete-key lookup.
    // The verifier proves the read kind matches the index, so a mismatch here is a forged
    // image.
    if unique {
        return Err(KernelFault::Corruption);
    }
    // The held prefix must be a strict prefix of the projection — at least one component
    // remains to enumerate — and each held column must match its projected kind.
    let held = prefix.len();
    if held >= projection.len() {
        return Err(KernelFault::Corruption);
    }
    check_kinds(prefix, &projection[..held])?;
    let next_kind = projection[held];
    if let Some(from) = &from
        && from.scalar_kind() != next_kind
    {
        return Err(KernelFault::Corruption);
    }

    let layer = physical::IndexLayer::new(site.root_number, id, prefix);
    let mut keys: Vec<KeyScalar> = Vec::with_capacity(limit.get().min(1024));
    // An inclusive `from` seeks to `prefix ++ enc(from)`; a bare forward scan then
    // excludes an equal cursor, which misses the `from` row only when `from` completes
    // the projection (its cell equals that key exactly). One probe of that exact key
    // resolves the boundary without a second seek per value.
    let mut cursor = match &from {
        Some(from) => {
            let seek = layer.seek_from(from);
            if cells.get(&seek).map_err(KernelFault::Engine)?.is_some() {
                keys.push(from.clone());
            }
            seek
        }
        None => layer.prefix().to_vec(),
    };
    loop {
        let page = cells
            .scan_after(layer.prefix(), &cursor)
            .map_err(KernelFault::Engine)?;
        let Some((cell_key, _)) = page.into_iter().next() else {
            return Ok(BoundedKeys { keys, more: false });
        };
        let Some(component) = layer.next_component(&cell_key) else {
            return Err(KernelFault::Corruption);
        };
        // The decoded component's kind must match the projection: an index cell whose
        // next column decodes as a different scalar kind is a corrupt or forged cell.
        if component.scalar_kind() != next_kind {
            return Err(KernelFault::Corruption);
        }
        if keys.len() == limit.get() {
            // A further distinct value exists beyond the frozen `limit`: the `on more`
            // bit. Its existence is recorded but the value itself is not frozen.
            return Ok(BoundedKeys { keys, more: true });
        }
        cursor = layer.skip_cursor(&component);
        keys.push(component);
    }
}

/// Look up the single source key tuple a unique index maps the complete projection `key`
/// to, or [`None`]. One exact probe of the index cell family; the stored value decodes as
/// the root's key tuple (`site.key.len()` columns). An index cell whose value does not
/// decode as exactly that many key columns is corruption.
pub(super) fn op_index_lookup<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    key: &[KeyScalar],
) -> Result<Option<Vec<KeyScalar>>, KernelFault> {
    let (id, unique, projection) = index_target(site)?;
    // A complete-key lookup is the unique read; a nonunique index admits only the
    // progressive scan. The verifier proves the match, so a mismatch is a forged image.
    if !unique {
        return Err(KernelFault::Corruption);
    }
    if key.len() != projection.len() {
        return Err(KernelFault::Corruption);
    }
    check_kinds(key, projection)?;
    let cell_key = physical::index_cell_key(site.root_number, id, key);
    match cells.get(&cell_key).map_err(KernelFault::Engine)? {
        None => Ok(None),
        Some(bytes) => match physical::decode_index_source_key(&bytes, site.key.len()) {
            Some(source) => Ok(Some(source)),
            None => Err(KernelFault::Corruption),
        },
    }
}

/// Check that each value's scalar kind matches the expected column kind. A per-column
/// mismatch is the trust boundary the verifier's operand-kind proof stands on, defended
/// in depth so a forged image can never read an index at the wrong component type.
fn check_kinds(values: &[KeyScalar], kinds: &[ScalarKind]) -> Result<(), KernelFault> {
    for (value, kind) in values.iter().zip(kinds) {
        if value.scalar_kind() != *kind {
            return Err(KernelFault::Corruption);
        }
    }
    Ok(())
}
