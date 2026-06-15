use std::collections::HashSet;

use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::evolution::transform_reads::{TransformReadMember, transform_read_members};
use crate::evolution::witness::{RepairReason, Verdict};
use crate::executable::{CheckedSavedPlace, for_each_place_record};
use crate::program::{CheckedProgram, EvolveTransform};

use super::enum_shrink::{EnumMembers, leaf_value_valid};
use super::{Accumulator, catalog_id, format_identity, required_catalog_id};

/// Classify every `evolve transform` obligation. A transform recomputes its target per
/// record from the members it reads, so the target is excluded from the presence scan and
/// discharged here as an applyable [`Verdict::Transform`] carrying the read-member ids.
/// Soundness rests on reading the old bytes, so the target is guarded by a decodability
/// proof: every record's stored bytes for each read member must decode under that member's
/// current type, or the target fails closed instead of being classified applyable.
pub(super) fn discharge_transforms(
    program: &CheckedProgram,
    store: &TreeStore,
    places: &[CheckedSavedPlace],
    enum_members: &EnumMembers,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for transform in &program.catalog.evolve_transforms {
        // The type pass already reported an unresolved target; the lowered body still
        // had its purity checked, but there is no catalog obligation to discharge.
        let Some(target_raw_id) = transform.catalog_id.as_deref() else {
            continue;
        };
        let target_places = transform_places(program, places, transform);
        if target_places.is_empty() {
            // No accepted/proposal activation place uses this resource, so there is no
            // store snapshot for the transform to read.
            continue;
        }
        let target_id = catalog_id(target_raw_id)?;
        let mut read_ids = None;
        let mut records = 0usize;
        let mut undecodable = None;
        for place in target_places {
            let reads = transform_read_members(place, &transform.reads);
            let place_read_ids: Vec<CatalogId> =
                reads.iter().map(|read| read.catalog_id.clone()).collect();
            match &read_ids {
                Some(expected) if expected != &place_read_ids => {
                    return Err(StoreError::Corruption {
                        message: format!(
                            "transform `{}` resolved different read members across stores of the same resource",
                            transform.resource
                        ),
                    });
                }
                Some(_) => {}
                None => read_ids = Some(place_read_ids),
            }
            // The decodability obligation lands on the target, not the read member: a read
            // member often has its own presence verdict, so a second verdict on its id would
            // duplicate it. The target is what cannot be recomputed when a read cannot decode.
            let scan = scan_transform_records(store, program, place, &reads, enum_members)?;
            records += scan.records;
            if undecodable.is_none() {
                undecodable = scan.undecodable;
            }
        }
        let read_ids = read_ids.unwrap_or_default();
        let verdict = match &undecodable {
            None => {
                acc.counts.records_to_transform += records;
                Verdict::Transform { reads: read_ids }
            }
            Some(sample) => {
                acc.diagnostic(
                    target_id.clone(),
                    format!(
                        "transform `{}` reads a member whose stored value does not decode under its current type (record {sample}); repair that data before activating",
                        transform.resource
                    ),
                );
                Verdict::RepairRequired {
                    reason: RepairReason::UndecodableTransformInput,
                }
            }
        };
        acc.push(target_id, verdict)?;
    }
    Ok(())
}

/// The checked saved places that own a transform's target member, found by the
/// resource the transform names.
fn transform_places<'a>(
    program: &CheckedProgram,
    places: &'a [CheckedSavedPlace],
    transform: &EvolveTransform,
) -> Vec<&'a CheckedSavedPlace> {
    let roots: HashSet<&str> = program
        .modules
        .iter()
        .flat_map(|module| {
            module.stores.iter().filter_map(|store| {
                let resource_path = crate::catalog::resource_path(&module.name, &store.resource);
                (resource_path == transform.resource).then_some(store.root.as_str())
            })
        })
        .collect();
    places
        .iter()
        .filter(|place| roots.contains(place.root.as_str()))
        .collect()
}

/// One transform scan: total record count, and the first record whose stored value for
/// some read member does not decode under its current leaf type. A record that simply
/// lacks a read member places no decodability obligation.
struct TransformScan {
    records: usize,
    undecodable: Option<String>,
}

/// Scan one place's records, counting total and capturing the first undecodable read in
/// scan order for the repair diagnostic.
fn scan_transform_records(
    store: &TreeStore,
    program: &CheckedProgram,
    place: &CheckedSavedPlace,
    reads: &[TransformReadMember],
    enum_members: &EnumMembers,
) -> Result<TransformScan, StoreError> {
    let store_id = required_catalog_id(&place.store_catalog_id)?;
    let mut records = 0usize;
    let mut undecodable = None;
    for_each_place_record(store, place, &mut |identity| {
        records += 1;
        if undecodable.is_none() {
            for read in reads {
                let path = [DataPathSegment::Member(read.catalog_id.clone())];
                if let Some(bytes) = store.read_data_value(&store_id, identity, &path)?
                    && !leaf_value_valid(program, &read.leaf, &bytes, enum_members)
                {
                    undecodable = Some(format_identity(identity));
                    break;
                }
            }
        }
        Ok(())
    })?;
    Ok(TransformScan {
        records,
        undecodable,
    })
}
