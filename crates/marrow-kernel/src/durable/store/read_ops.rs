//! Stateless read operations over a `ReadView`: whole-entry slot classification and the
//! presence, field, entry, and group reads both sessions delegate to.

use std::collections::HashMap;

use marrow_store::ReadView;

use super::super::physical;
use super::super::{AuthTarget, AuthorizedSite, EntryValue, FieldSchema, KernelFault, Presence};
use super::address::{group_target, node_shape, node_stem, read_raw};
use crate::codec::key::KeyScalar;
use crate::codec::value::decode_domain;
use crate::equality::ValueDomain;

/// The four-state classification of a whole-entry slot the bounded prefix probe
/// yields.
pub(super) enum SlotClass {
    /// The payload marker is present: the entry has a payload.
    Present,
    /// No marker, but a branch descendant exists — a descendant-only node (children,
    /// no payload). It reads as payload-absent; a create gives it a payload without
    /// disturbing the descendants.
    DescendantOnly,
    /// No marker, but an own field leaf exists — a marker/field mismatch. A persisted
    /// orphan is corruption; a sparse field staged earlier in the same transaction is
    /// reconcile-pending, so a mutating session tolerates it (see [`op_read_entry`]).
    Orphan,
    /// No marker and nothing beneath: the slot is absent.
    Absent,
}

/// One bounded prefix probe over an entry's marker `stem`: a point read of the
/// marker plus, when the marker is absent, one bounded scan for the first cell
/// beneath it. This is the single owner of whole-entry slot classification —
/// separating an absent slot from a descendant-only node and from a marker/field
/// mismatch — so create/read/replace/erase share one marker-first precedence rather
/// than each re-deriving presence. The scan reads the node's own cells in key order,
/// and own field leaves sort ahead of branch descendants, so the first cell decides
/// (an orphan own-leaf takes precedence over a descendant, surfacing corruption).
pub(super) fn probe_slot<V: ReadView>(cells: &V, stem: &[u8]) -> Result<SlotClass, KernelFault> {
    if read_raw(cells, stem)?.is_some() {
        return Ok(SlotClass::Present);
    }
    let page = cells.scan_after(stem, stem).map_err(KernelFault::Engine)?;
    Ok(match page.first() {
        None => SlotClass::Absent,
        Some((cell_key, _)) => match physical::below_marker(stem, cell_key) {
            // An own field leaf or a group leaf (both the node's own payload) below a
            // markerless stem is a marker/payload mismatch — the orphan case.
            physical::BelowMarker::OwnField | physical::BelowMarker::OwnGroup => SlotClass::Orphan,
            physical::BelowMarker::BranchDescendant => SlotClass::DescendantOnly,
            // An unrecognized structural tag is a shape the layout never writes: fail
            // closed with corruption in every session, never tolerated as staging.
            physical::BelowMarker::Corrupt => return Err(KernelFault::Corruption),
            physical::BelowMarker::Foreign => SlotClass::Absent,
        },
    })
}

pub(super) fn op_presence<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    keys: &[KeyScalar],
) -> Result<Presence, KernelFault> {
    let stem = node_stem(site, keys)?;
    let physical_key = match &site.target {
        AuthTarget::Entry { .. } => stem,
        AuthTarget::Field { name, .. } => physical::stem_field_leaf(&stem, name),
        AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
            unreachable!("verifier proved a presence op targets a node site")
        }
    };
    Ok(match read_raw(cells, &physical_key)? {
        Some(_) => Presence::Present,
        None => Presence::Absent,
    })
}

pub(super) fn op_read_field<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    keys: &[KeyScalar],
) -> Result<Option<ValueDomain>, KernelFault> {
    let AuthTarget::Field { name, shape, .. } = &site.target else {
        unreachable!("verifier proved a field read targets a field site")
    };
    let leaf = physical::stem_field_leaf(&node_stem(site, keys)?, name);
    match read_raw(cells, &leaf)? {
        None => Ok(None),
        Some(bytes) => decode_domain(&bytes, shape)
            .map(Some)
            .ok_or(KernelFault::Corruption),
    }
}

pub(super) fn op_read_entry<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    keys: &[KeyScalar],
    tolerate_pending: bool,
) -> Result<Option<EntryValue>, KernelFault> {
    let stem = node_stem(site, keys)?;
    let (fields, groups) = node_shape(site);
    // Marker-first precedence through the one bounded prefix probe. A node with no
    // payload marker reads as payload-absent whether it is empty or a descendant-only
    // node (branch children, no payload). A markerless slot carrying an own field
    // leaf is a marker/field mismatch: in a committed read session it is a persisted
    // orphan (corruption); inside a transaction it may be a sparse field staged for
    // reconcile at commit, so a mutating session tolerates it as payload-absent.
    match probe_slot(cells, &stem)? {
        SlotClass::DescendantOnly | SlotClass::Absent => return Ok(None),
        SlotClass::Orphan => {
            return if tolerate_pending {
                Ok(None)
            } else {
                Err(KernelFault::Corruption)
            };
        }
        SlotClass::Present => {}
    }
    let values = read_record_leaves(cells, &stem, fields)?;
    // A present entry materializes each of its groups (its own payload) under the group
    // prefix, in schema order — a group's presence is the entry's, so a present entry
    // always yields every group sub-record. A present entry missing a required group leaf
    // is the same marker/payload mismatch as a missing required top-level field.
    let mut group_values = Vec::with_capacity(groups.len());
    for group in groups {
        let group_stem = physical::group_stem(&stem, &group.name);
        group_values.push(EntryValue {
            fields: read_record_leaves(cells, &group_stem, &group.fields)?,
            groups: Vec::new(),
        });
    }
    Ok(Some(EntryValue {
        fields: values,
        groups: group_values,
    }))
}

/// Read the ordered field-leaf values of the record whose leaves namespace under `stem`
/// (an entry marker stem or a group stem): one slot per field, decoded to its shape. A
/// present required leaf is mandatory — its absence under a present container is a
/// marker/payload mismatch ([`KernelFault::Corruption`]) — while an absent sparse leaf
/// reads vacant. The shared owner of top-level-field and group-leaf materialization, so
/// both decode and enforce required-completeness identically.
///
/// Engine work is proportional to the *present* leaf count, not the declared field
/// width: a structural-tag-bounded range scan over the node's own contiguous field-leaf
/// cells ([`physical::field_leaf_range`]) visits only present leaves (`O(populated + 1)`
/// reads), where a per-declared-field probe would read a vacant cell per absent field. A
/// declared-field-to-position map resolves each scanned leaf in constant time and carries
/// the required-completeness check; building it touches the declared fields but performs
/// no engine work.
fn read_record_leaves<V: ReadView>(
    cells: &V,
    stem: &[u8],
    fields: &[FieldSchema],
) -> Result<Vec<Option<ValueDomain>>, KernelFault> {
    let mut position: HashMap<Vec<u8>, usize> = HashMap::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        position.insert(physical::stem_field_leaf(stem, &field.name), index);
    }
    let mut values: Vec<Option<ValueDomain>> = (0..fields.len()).map(|_| None).collect();

    let range = physical::field_leaf_range(stem);
    let mut cursor = range.clone();
    loop {
        let page = cells
            .scan_after(&range, &cursor)
            .map_err(KernelFault::Engine)?;
        let Some(last_key) = page.last().map(|(key, _)| key.clone()) else {
            break;
        };
        for (key, bytes) in &page {
            // A present own-field leaf under this node that resolves to no declared
            // field is a forged or orphaned cell: fail closed as corruption rather than
            // silently dropping it. Evolution constraint: a field drop or rename must
            // migrate or delete the retired field's stored leaves before a whole-entry
            // read; a stale leaf under a shrunk schema is indistinguishable from
            // corruption at this fail-closed check.
            let index = *position.get(key).ok_or(KernelFault::Corruption)?;
            values[index] =
                Some(decode_domain(bytes, &fields[index].shape).ok_or(KernelFault::Corruption)?);
        }
        cursor = last_key;
    }

    // A present container missing a required field leaf is the same marker/payload
    // mismatch a missing required top-level field is.
    for (index, field) in fields.iter().enumerate() {
        if field.required && values[index].is_none() {
            return Err(KernelFault::Corruption);
        }
    }
    Ok(values)
}

/// Materialize one group's record from the entry `keys` addresses: one slot per group
/// field, present or vacant. A group's presence is its containing entry's presence, so
/// this probes the entry marker exactly as [`op_read_entry`] does — a markerless slot is
/// payload-absent (or, for a persisted own-payload leaf with no marker, corruption in a
/// committed read and pending inside a transaction). A present entry then reads the
/// group's own leaves under the group prefix; a present entry missing a `required` group
/// leaf is a marker/payload mismatch (corruption), and an absent sparse leaf reads vacant.
pub(super) fn op_read_group<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    keys: &[KeyScalar],
    tolerate_pending: bool,
) -> Result<Option<EntryValue>, KernelFault> {
    let stem = node_stem(site, keys)?;
    let (name, fields) = group_target(site);
    match probe_slot(cells, &stem)? {
        SlotClass::DescendantOnly | SlotClass::Absent => return Ok(None),
        SlotClass::Orphan => {
            return if tolerate_pending {
                Ok(None)
            } else {
                Err(KernelFault::Corruption)
            };
        }
        SlotClass::Present => {}
    }
    let group_stem = physical::group_stem(&stem, name);
    Ok(Some(EntryValue {
        fields: read_record_leaves(cells, &group_stem, fields)?,
        groups: Vec::new(),
    }))
}
