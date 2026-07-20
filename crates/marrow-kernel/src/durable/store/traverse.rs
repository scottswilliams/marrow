//! Bounded forward traversal over a durable layer: the marker walk that skips
//! descendant-only children, and the bounded key acquisition and family-populated probe
//! built on it.

use marrow_store::ReadView;

use super::super::physical::{self, CellKind};
use super::super::{AuthorizedSite, BoundedKeys, BoundedLimit, KernelFault, NextKey, Presence};
use super::address::take_columns;
use crate::codec::key::KeyScalar;

/// Where a forward layer walk begins.
enum LayerSeek {
    /// At the layer's first child.
    Start,
    /// Strictly after `key`'s whole subtree (an exclusive resume).
    After(KeyScalar),
    /// At the first child whose key is `>= from` (an inclusive lower bound).
    From(KeyScalar),
}

/// One forward step over a durable `layer`: the first present (payload-bearing) child
/// at or after `seek`, or [`NextKey::End`]. A descendant-only child — branch children
/// but no payload marker — is skipped with one prefix-successor seek past its subtree,
/// which passes its whole subtree regardless of branch fan-out. The single owner of the
/// forward marker walk: the bounded layer acquisition steps through it for both the
/// root and branch layers, so they walk identically.
fn layer_step<V: ReadView>(
    cells: &V,
    layer: &physical::Layer,
    seek: LayerSeek,
) -> Result<NextKey, KernelFault> {
    let mut cursor = match seek {
        LayerSeek::Start => layer.prefix().to_vec(),
        LayerSeek::After(key) => layer.child_cursor(&key),
        LayerSeek::From(from) => layer.seek_from(&from),
    };
    loop {
        let page = cells
            .scan_after(layer.prefix(), &cursor)
            .map_err(KernelFault::Engine)?;
        let Some((cell_key, _)) = page.into_iter().next() else {
            return Ok(NextKey::End);
        };
        match layer.classify(&cell_key) {
            CellKind::Marker(key) => return Ok(NextKey::Next(key)),
            CellKind::Descendant(key) => cursor = layer.child_cursor(&key),
            CellKind::Orphan => return Err(KernelFault::Corruption),
            CellKind::Foreign => return Ok(NextKey::End),
        }
    }
}

/// The durable layer the whole-entry `site` traverses, resolving its parent entry from
/// `ancestor_keys`: a root (`WholePayload`) site traverses the root's entry family with
/// no ancestor key; a branch site traverses its branch family beneath the parent entry
/// the ancestor key-path names, one ancestor key per parent hop above the traversed
/// branch. The single owner of the site-to-traversed-layer mapping. The verifier proves
/// the ancestor arity and each key's scalar kind against the site's declared root and hop
/// kinds, but this is the trust boundary the independently verified image crosses into
/// the kernel, so a mismatch faults [`KernelFault::Corruption`] — matching [`node_stem`]'s
/// hard backstop — rather than mis-layering the traversal to a shallower or wrong parent
/// node.
fn layer_of(
    site: &AuthorizedSite,
    ancestor_keys: &[KeyScalar],
) -> Result<physical::Layer, KernelFault> {
    match site.branch.split_last() {
        None => {
            if !ancestor_keys.is_empty() {
                return Err(KernelFault::Corruption);
            }
            // A traversable layer is single-column; a composite-keyed root layer is not
            // traversed (the verifier parks it), so a multi-column root here is a forged
            // image reaching an untraversable shape.
            if site.key.len() != 1 {
                return Err(KernelFault::Corruption);
            }
            Ok(physical::Layer::root(&site.root))
        }
        Some((traversed, parent_hops)) => {
            // The traversed branch layer must be single-column (composite-keyed layers are
            // parked before traversal); its ancestor key-path locates its parent entry —
            // the root's key columns then each parent hop's key columns.
            if traversed.key.len() != 1 {
                return Err(KernelFault::Corruption);
            }
            let mut cols = ancestor_keys;
            let root_cols = take_columns(&mut cols, &site.key)?;
            let mut stem = physical::marker_key(&site.root, root_cols);
            for hop in parent_hops {
                let hop_cols = take_columns(&mut cols, &hop.key)?;
                stem = physical::branch_child_stem(&stem, &hop.name, hop_cols);
            }
            if !cols.is_empty() {
                return Err(KernelFault::Corruption);
            }
            Ok(physical::Layer::branch(&stem, &traversed.name))
        }
    }
}

/// Freeze the first `limit` immediate keys of the layer `site` traverses and report
/// whether a further key existed. Acquires at most `limit + 1` distinct present keys —
/// the frozen set plus one existence probe — through the bounded [`layer_step`] walk,
/// costing `O(limit + 1 + d)` seeks, where `d` is the count of descendant-only siblings
/// interspersed among them (each skipped by one prefix-successor seek without its
/// fan-out being read). The frozen keys are captured before any caller runs a loop
/// body, so writes a body performs cannot change the set.
pub(super) fn op_iterate_bounded<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    ancestor_keys: &[KeyScalar],
    from: Option<KeyScalar>,
    limit: BoundedLimit,
) -> Result<BoundedKeys, KernelFault> {
    let layer = layer_of(site, ancestor_keys)?;
    // Reserve a bounded spine rather than the full `limit`: a sparse layer freezes far
    // fewer keys than a large `at most N` permits, so the eager reservation is capped
    // and the Vec grows on demand within `limit`. Peak freeze memory is the frozen key
    // count times the maximum key size; the exact aggregate ceiling is enforced by the
    // VM's one collection owner (`MAX_AGGREGATE_BYTES`) once the keys materialize as a
    // `List[K]`.
    let mut keys: Vec<KeyScalar> = Vec::with_capacity(limit.get().min(1024));
    // The first step honors an inclusive `from`; each later step resumes strictly after
    // the last frozen key.
    let mut seek = match from {
        Some(from) => LayerSeek::From(from),
        None => LayerSeek::Start,
    };
    loop {
        match layer_step(cells, &layer, seek)? {
            NextKey::End => return Ok(BoundedKeys { keys, more: false }),
            NextKey::Next(key) => {
                if keys.len() == limit.get() {
                    // A present key exists beyond the frozen `limit`: the `on more` bit.
                    // Its existence is recorded but the key itself is not frozen or run.
                    return Ok(BoundedKeys { keys, more: true });
                }
                seek = LayerSeek::After(key.clone());
                keys.push(key);
            }
        }
    }
}

/// Whether the layer the whole-entry `site` names has at least one payload-bearing
/// immediate child: one forward [`layer_step`] from the layer's start. A present child
/// yields `Present`; an empty or purely descendant-only layer yields `Absent`. Reads at
/// most one payload child key (descendant-only children are skipped by one seek each) and
/// establishes no per-key presence fact — the bounded family-populated probe.
pub(super) fn op_family_populated<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    ancestor_keys: &[KeyScalar],
) -> Result<Presence, KernelFault> {
    let layer = layer_of(site, ancestor_keys)?;
    Ok(match layer_step(cells, &layer, LayerSeek::Start)? {
        NextKey::Next(_) => Presence::Present,
        NextKey::End => Presence::Absent,
    })
}
