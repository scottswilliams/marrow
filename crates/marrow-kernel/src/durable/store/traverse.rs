//! Bounded forward traversal over one entry family's marker and own-payload range.

use marrow_store::ReadView;

use super::super::physical::{self, CellKind};
use super::super::{AuthorizedSite, BoundedKeys, BoundedLimit, KernelFault, NextKey, Presence};
use super::address::take_columns;
use crate::codec::key::KeyScalar;
use crate::codec::value::{ScalarKind, scalar_key_matches_type};

/// Where a forward layer walk begins.
enum LayerSeek {
    /// At the layer's first child.
    Start,
    /// Strictly after `key`'s own payload (an exclusive resume).
    After(KeyScalar),
    /// At the first child whose key is `>= from` (an inclusive lower bound).
    From(KeyScalar),
}

/// One bounded scan for the next marker key in a family. Other families, including
/// children of absent entries, lie outside the prefix. Own payload encountered where
/// a marker should be is corruption. Marker values are checked by logical inspection.
fn layer_step<V: ReadView>(
    cells: &V,
    layer: &physical::Layer,
    key_kind: ScalarKind,
    seek: LayerSeek,
) -> Result<NextKey, KernelFault> {
    let cursor = match seek {
        LayerSeek::Start => layer.prefix().to_vec(),
        LayerSeek::After(key) => layer.child_cursor(&key),
        LayerSeek::From(from) => {
            if !scalar_key_matches_type(&from, key_kind) {
                return Err(KernelFault::Corruption);
            }
            layer.seek_from(&from)
        }
    };
    let page = cells
        .scan_after(layer.prefix(), &cursor)
        .map_err(KernelFault::Engine)?;
    let Some((cell_key, _)) = page.into_iter().next() else {
        return Ok(NextKey::End);
    };
    match layer.classify(&cell_key) {
        CellKind::Marker(key) if scalar_key_matches_type(&key, key_kind) => Ok(NextKey::Next(key)),
        CellKind::Marker(_) | CellKind::Orphan => Err(KernelFault::Corruption),
        CellKind::Foreign => Ok(NextKey::End),
    }
}

/// The durable layer the whole-entry `site` traverses, resolving its parent entry from
/// `ancestor_keys`: a root (`WholePayload`) site traverses the root's entry family with
/// no ancestor key; a branch site traverses its branch family beneath the parent entry
/// named by the concatenated declared key columns of the root and parent hops above
/// the traversed branch. The single owner of the site-to-traversed-layer mapping.
/// The verifier proves the ancestor arity and each key's scalar kind against the site's
/// declared root and hop kinds, but this is the trust boundary the independently verified image crosses into
/// the kernel, so a mismatch faults [`KernelFault::Corruption`] — matching [`node_stem`]'s
/// hard backstop — rather than mis-layering the traversal to a shallower or wrong parent
/// node.
fn layer_of(
    site: &AuthorizedSite,
    ancestor_keys: &[KeyScalar],
) -> Result<(physical::Layer, ScalarKind), KernelFault> {
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
            Ok((physical::Layer::new(site.root_number, &[]), site.key[0]))
        }
        Some((traversed, parent_hops)) => {
            // The traversed branch layer must be single-column (composite-keyed layers are
            // parked before traversal); its ancestor key-path locates its parent entry —
            // the root's key columns then each parent hop's key columns.
            if traversed.key.len() != 1 {
                return Err(KernelFault::Corruption);
            }
            let mut cols = ancestor_keys;
            take_columns(&mut cols, &site.key)?;
            for hop in parent_hops {
                take_columns(&mut cols, &hop.key)?;
            }
            if !cols.is_empty() {
                return Err(KernelFault::Corruption);
            }
            Ok((
                physical::Layer::new(traversed.number, ancestor_keys),
                traversed.key[0],
            ))
        }
    }
}

/// Freeze the first `limit` immediate keys of the layer `site` traverses and report
/// whether a further key existed. Acquires at most `limit + 1` distinct present keys —
/// the frozen set plus one existence probe — through at most `limit + 1` bounded scans.
/// Each scan can copy a page of own payload cells as well as marker keys; the count
/// does not bound backend seek time or retained cache bytes.
/// The frozen keys are captured before any caller runs a loop
/// body, so writes a body performs cannot change the set.
pub(super) fn op_iterate_bounded<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    ancestor_keys: &[KeyScalar],
    from: Option<KeyScalar>,
    limit: BoundedLimit,
) -> Result<BoundedKeys, KernelFault> {
    let (layer, key_kind) = layer_of(site, ancestor_keys)?;
    // Cap the initial reservation. The result length stays within `limit`, while
    // Vec capacity can exceed its length. The cursor, extra key and returned scan
    // page add temporary storage. The VM checks its aggregate byte ceiling after
    // these keys materialize as a List[K]; that is not a peak-memory bound here.
    let mut keys: Vec<KeyScalar> = Vec::with_capacity(limit.get().min(1024));
    // The first step honors an inclusive `from`; each later step resumes strictly after
    // the last frozen key.
    let mut seek = match from {
        Some(from) => LayerSeek::From(from),
        None => LayerSeek::Start,
    };
    loop {
        match layer_step(cells, &layer, key_kind, seek)? {
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
/// yields `Present`; a family without its own markers yields `Absent` unless the scan
/// encounters orphan payload. One bounded scan establishes no per-key presence fact.
pub(super) fn op_family_populated<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    ancestor_keys: &[KeyScalar],
) -> Result<Presence, KernelFault> {
    let (layer, key_kind) = layer_of(site, ancestor_keys)?;
    Ok(
        match layer_step(cells, &layer, key_kind, LayerSeek::Start)? {
            NextKey::Next(_) => Presence::Present,
            NextKey::End => Presence::Absent,
        },
    )
}
