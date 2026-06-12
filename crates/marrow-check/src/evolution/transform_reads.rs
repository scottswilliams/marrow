//! The read members an `evolve transform` body binds as `old`.
//!
//! A transform reads only top-level plain fields of its resource: read resolution and
//! the per-record write address handle a single named cell directly under the record
//! node, never a nested or keyed member. This is the one place that rule lives. Both
//! discharge (which proves every read member's stored bytes decode under its current
//! type) and apply (which decodes those bytes to bind `old`) resolve their read members
//! through here, so the check and run sides never drift on which member a read names.

use marrow_store::cell::CatalogId;

use crate::executable::CheckedSavedPlace;
use crate::facts::{ResourceId, ResourceMemberId};
use crate::{CheckedProgram, catalog};

/// One resolved transform read member: the stable catalog id its data cells use, the
/// member name the body reads it under (`old.<name>`), and its leaf kind for decoding.
#[derive(Debug, Clone)]
pub struct TransformReadMember {
    pub catalog_id: CatalogId,
    pub name: String,
    pub leaf: crate::StoreLeafKind,
}

pub(crate) struct TransformOldMember {
    pub(crate) resource: ResourceId,
    pub(crate) member: ResourceMemberId,
    pub(crate) required: bool,
}

/// Resolve each read-member stable id against a place to a top-level plain field. A read
/// whose stable id resolves no such field or has no leaf is dropped: there is no cell to
/// read, so the body simply sees that member as absent from `old`. The result preserves the
/// order of `reads`. Reads are already the canonical [`CatalogId`] the witness carries, so a
/// root member matches by typed-id equality with no re-validation.
pub fn transform_read_members(
    place: &CheckedSavedPlace,
    reads: &[CatalogId],
) -> Vec<TransformReadMember> {
    reads
        .iter()
        .filter_map(|read_id| {
            let member = place.root_members.iter().find(|member| {
                member.is_plain_field() && member.catalog_id.as_deref() == Some(read_id.as_str())
            })?;
            Some(TransformReadMember {
                catalog_id: read_id.clone(),
                name: member.name.clone(),
                leaf: member.leaf.clone()?,
            })
        })
        .collect()
}

pub(crate) fn transform_old_member(
    program: &CheckedProgram,
    resource_path: &str,
    member_name: &str,
) -> Option<TransformOldMember> {
    let resource = transform_resource(program, resource_path)?;
    let member = program.facts.resource_members().iter().find(|member| {
        member.resource == resource
            && member.parent.is_none()
            && member.name == member_name
            && member.plain_field_required.is_some()
    })?;
    Some(TransformOldMember {
        resource,
        member: member.id,
        required: member.plain_field_required?,
    })
}

fn transform_resource(program: &CheckedProgram, expected_path: &str) -> Option<ResourceId> {
    program.facts.resources().iter().find_map(|resource| {
        let module = program.facts.modules().get(resource.module.0 as usize)?;
        (catalog::resource_path(&module.name, &resource.name) == expected_path)
            .then_some(resource.id)
    })
}
