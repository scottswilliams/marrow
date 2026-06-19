use marrow_store::cell::CatalogId;

use crate::facts::{
    ResourceMemberFact, ResourceMemberId, ResourceMemberKind, StoreFact, StoreIndexFact,
    StoreIndexId, StoreIndexKeySource, StoredValueMeaning, SurfaceCatalogStatus, SurfaceFact,
    SurfaceFieldFact, SurfaceReadFootprint, SurfaceReadOperationFact, SurfaceReadOperationKind,
};
use crate::program::CheckedProgram;

pub const SURFACE_READ_OPERATION_TAG_VERSION: &str = "surface.read.v1";
pub const SURFACE_UPDATE_OPERATION_TAG_VERSION: &str = "surface.update.v1";

#[derive(Debug, Clone)]
pub struct SurfaceReadOperationDescriptor {
    pub profile_version: &'static str,
    pub operation_tag: String,
    pub kind: SurfaceReadOperationDescriptorKind,
    pub store_catalog_id: CatalogId,
    pub resource_catalog_id: CatalogId,
    pub identity_keys: Vec<SurfaceOperationIdentityKey>,
    pub projection: Vec<SurfaceReadOperationProjectionField>,
    pub index_keys: Vec<SurfaceReadOperationIndexKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceReadOperationDescriptorKind {
    SingletonRead,
    PointRead,
    PagedRootCollection,
    PagedIndexCollection {
        index_catalog_id: CatalogId,
        exact_key_count: usize,
        identity_key_count: usize,
    },
    UniqueIndexLookup {
        index_catalog_id: CatalogId,
        key_count: usize,
    },
}

#[derive(Debug, Clone)]
pub struct SurfaceOperationIdentityKey {
    pub render_label: String,
    pub value: SurfaceOperationValueShape,
}

#[derive(Debug, Clone)]
pub struct SurfaceReadOperationProjectionField {
    pub render_label: String,
    pub member_catalog_id: CatalogId,
    pub required: bool,
    pub value: SurfaceOperationValueShape,
}

#[derive(Debug, Clone)]
pub struct SurfaceReadOperationIndexKey {
    pub render_label: String,
    pub source: SurfaceReadOperationIndexKeySource,
    pub value: SurfaceOperationValueShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceReadOperationIndexKeySource {
    IdentityKey,
    ResourceMember { member_catalog_id: CatalogId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceOperationValueShape {
    Scalar(marrow_schema::ScalarType),
    Enum {
        enum_catalog_id: CatalogId,
        member_catalog_ids: Vec<CatalogId>,
    },
    Identity {
        store_catalog_id: CatalogId,
        arity: usize,
        key_scalars: Vec<marrow_schema::ScalarType>,
    },
}

impl SurfaceReadOperationDescriptor {
    pub fn from_operation(
        program: &CheckedProgram,
        surface: &SurfaceFact,
        operation: &SurfaceReadOperationFact,
    ) -> Option<Self> {
        require_stable_surface(surface)?;
        let store = program.facts.store(surface.store);
        let resource_catalog_id = match operation.footprint {
            SurfaceReadFootprint::FullRecord { resource } => accepted_catalog_id(
                program,
                program.facts.resource(resource).catalog_id.as_deref(),
            )?,
        };
        let operation_tag = surface_read_operation_tag(
            program,
            store,
            operation.kind,
            operation.footprint,
            &operation.projection,
        )?;
        Some(Self {
            profile_version: SURFACE_READ_OPERATION_TAG_VERSION,
            operation_tag,
            kind: descriptor_kind(program, operation.kind)?,
            store_catalog_id: accepted_catalog_id(program, store.catalog_id.as_deref())?,
            resource_catalog_id,
            identity_keys: identity_key_descriptors(program, store)?,
            projection: projection_descriptors(program, &operation.projection)?,
            index_keys: index_key_descriptors_for_operation(program, operation.kind)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SurfaceUpdateOperationDescriptor {
    pub profile_version: &'static str,
    pub operation_tag: String,
    pub kind: SurfaceUpdateOperationDescriptorKind,
    pub patch_semantics: SurfaceUpdatePatchSemantics,
    pub store_catalog_id: CatalogId,
    pub resource_catalog_id: CatalogId,
    pub identity_keys: Vec<SurfaceOperationIdentityKey>,
    pub fields: Vec<SurfaceUpdateOperationField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceUpdateOperationDescriptorKind {
    SingletonUpdate,
    PointUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceUpdatePatchSemantics {
    NonEmptyPatch,
}

#[derive(Debug, Clone)]
pub struct SurfaceUpdateOperationField {
    pub render_label: String,
    pub member_catalog_id: CatalogId,
    pub backing_required: bool,
    pub value: SurfaceOperationValueShape,
}

impl SurfaceUpdateOperationDescriptor {
    pub fn from_surface(program: &CheckedProgram, surface: &SurfaceFact) -> Option<Self> {
        require_stable_surface(surface)?;
        if surface.update.is_empty() {
            return None;
        }
        let store = program.facts.store(surface.store);
        let resource_catalog_id = accepted_catalog_id(
            program,
            program.facts.resource(store.resource).catalog_id.as_deref(),
        )?;
        Some(Self {
            profile_version: SURFACE_UPDATE_OPERATION_TAG_VERSION,
            operation_tag: surface_update_operation_tag(program, store, &surface.update)?,
            kind: update_descriptor_kind(store),
            patch_semantics: SurfaceUpdatePatchSemantics::NonEmptyPatch,
            store_catalog_id: accepted_catalog_id(program, store.catalog_id.as_deref())?,
            resource_catalog_id,
            identity_keys: identity_key_descriptors(program, store)?,
            fields: update_field_descriptors(program, &surface.update)?,
        })
    }
}

pub(crate) fn surface_read_operation_tag(
    program: &CheckedProgram,
    store: &StoreFact,
    kind: SurfaceReadOperationKind,
    footprint: SurfaceReadFootprint,
    projection: &[ResourceMemberId],
) -> Option<String> {
    let mut payload = String::new();
    push_read_operation_payload(program, &mut payload, store, kind, footprint, projection)?;
    Some(marrow_project::sha256_digest(payload.as_bytes()))
}

pub(crate) fn surface_update_operation_tag(
    program: &CheckedProgram,
    store: &StoreFact,
    update: &[SurfaceFieldFact],
) -> Option<String> {
    let mut payload = String::new();
    push_update_operation_payload(program, &mut payload, store, update)?;
    Some(marrow_project::sha256_digest(payload.as_bytes()))
}

fn descriptor_kind(
    program: &CheckedProgram,
    kind: SurfaceReadOperationKind,
) -> Option<SurfaceReadOperationDescriptorKind> {
    match kind {
        SurfaceReadOperationKind::SingletonRead { .. } => {
            Some(SurfaceReadOperationDescriptorKind::SingletonRead)
        }
        SurfaceReadOperationKind::PointRead { .. } => {
            Some(SurfaceReadOperationDescriptorKind::PointRead)
        }
        SurfaceReadOperationKind::PagedRootCollection { .. } => {
            Some(SurfaceReadOperationDescriptorKind::PagedRootCollection)
        }
        SurfaceReadOperationKind::PagedIndexCollection {
            index,
            exact_key_count,
            identity_key_count,
        } => Some(SurfaceReadOperationDescriptorKind::PagedIndexCollection {
            index_catalog_id: index_catalog_id(program, index)?,
            exact_key_count,
            identity_key_count,
        }),
        SurfaceReadOperationKind::UniqueIndexLookup { index, key_count } => {
            Some(SurfaceReadOperationDescriptorKind::UniqueIndexLookup {
                index_catalog_id: index_catalog_id(program, index)?,
                key_count,
            })
        }
    }
}

fn update_descriptor_kind(store: &StoreFact) -> SurfaceUpdateOperationDescriptorKind {
    if store.identity_keys.is_empty() {
        SurfaceUpdateOperationDescriptorKind::SingletonUpdate
    } else {
        SurfaceUpdateOperationDescriptorKind::PointUpdate
    }
}

fn identity_key_descriptors(
    program: &CheckedProgram,
    store: &StoreFact,
) -> Option<Vec<SurfaceOperationIdentityKey>> {
    store
        .identity_keys
        .iter()
        .map(|key| {
            Some(SurfaceOperationIdentityKey {
                render_label: key.name.clone(),
                value: value_shape(program, key.value_meaning.as_ref()?)?,
            })
        })
        .collect()
}

fn projection_descriptors(
    program: &CheckedProgram,
    projection: &[ResourceMemberId],
) -> Option<Vec<SurfaceReadOperationProjectionField>> {
    projection
        .iter()
        .map(|member_id| {
            let member = resource_member(program, *member_id)?;
            Some(SurfaceReadOperationProjectionField {
                render_label: member.name.clone(),
                member_catalog_id: accepted_catalog_id(program, member.catalog_id.as_deref())?,
                required: member.plain_field_required?,
                value: value_shape(program, member.value_meaning.as_ref()?)?,
            })
        })
        .collect()
}

fn update_field_descriptors(
    program: &CheckedProgram,
    update: &[SurfaceFieldFact],
) -> Option<Vec<SurfaceUpdateOperationField>> {
    let mut fields = update
        .iter()
        .map(|field| {
            let member = resource_member(program, field.member)?;
            Some(SurfaceUpdateOperationField {
                render_label: field.name.clone(),
                member_catalog_id: accepted_catalog_id(program, member.catalog_id.as_deref())?,
                backing_required: member.plain_field_required?,
                value: value_shape(program, member.value_meaning.as_ref()?)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    fields.sort_by(|left, right| left.member_catalog_id.cmp(&right.member_catalog_id));
    Some(fields)
}

fn index_key_descriptors_for_operation(
    program: &CheckedProgram,
    kind: SurfaceReadOperationKind,
) -> Option<Vec<SurfaceReadOperationIndexKey>> {
    let index = match kind {
        SurfaceReadOperationKind::PagedIndexCollection { index, .. }
        | SurfaceReadOperationKind::UniqueIndexLookup { index, .. } => Some(index),
        SurfaceReadOperationKind::SingletonRead { .. }
        | SurfaceReadOperationKind::PointRead { .. }
        | SurfaceReadOperationKind::PagedRootCollection { .. } => None,
    };
    let Some(index) = index else {
        return Some(Vec::new());
    };
    program
        .facts
        .store_index(index)
        .keys
        .iter()
        .map(|key| {
            Some(SurfaceReadOperationIndexKey {
                render_label: key.name.clone(),
                source: match key.source {
                    StoreIndexKeySource::IdentityKey => {
                        SurfaceReadOperationIndexKeySource::IdentityKey
                    }
                    StoreIndexKeySource::ResourceMember(member) => {
                        SurfaceReadOperationIndexKeySource::ResourceMember {
                            member_catalog_id: accepted_catalog_id(
                                program,
                                resource_member(program, member)?.catalog_id.as_deref(),
                            )?,
                        }
                    }
                },
                value: value_shape(program, &key.value_meaning)?,
            })
        })
        .collect()
}

fn index_catalog_id(program: &CheckedProgram, index: StoreIndexId) -> Option<CatalogId> {
    accepted_catalog_id(
        program,
        program.facts.store_index(index).catalog_id.as_deref(),
    )
}

fn resource_member(
    program: &CheckedProgram,
    member_id: ResourceMemberId,
) -> Option<&ResourceMemberFact> {
    program
        .facts
        .resource_members()
        .iter()
        .find(|member| member.id == member_id)
}

fn require_stable_surface(surface: &SurfaceFact) -> Option<()> {
    match surface.catalog_status {
        SurfaceCatalogStatus::Stable => Some(()),
        SurfaceCatalogStatus::SourceOnly(_) => None,
    }
}

fn value_shape(
    program: &CheckedProgram,
    meaning: &StoredValueMeaning,
) -> Option<SurfaceOperationValueShape> {
    match meaning {
        StoredValueMeaning::Scalar(scalar) => Some(SurfaceOperationValueShape::Scalar(*scalar)),
        StoredValueMeaning::Enum { enum_id, members } => {
            let enum_fact = program.facts.enum_(*enum_id)?;
            Some(SurfaceOperationValueShape::Enum {
                enum_catalog_id: accepted_catalog_id(program, enum_fact.catalog_id.as_deref())?,
                member_catalog_ids: members
                    .iter()
                    .map(|member_id| {
                        let member = program
                            .facts
                            .enum_members()
                            .iter()
                            .find(|member| member.id == *member_id)?;
                        accepted_catalog_id(program, member.catalog_id.as_deref())
                    })
                    .collect::<Option<Vec<_>>>()?,
            })
        }
        StoredValueMeaning::Identity {
            store_catalog_id,
            arity,
            key_scalars,
            ..
        } => Some(SurfaceOperationValueShape::Identity {
            store_catalog_id: accepted_catalog_id(program, store_catalog_id.as_deref())?,
            arity: *arity,
            key_scalars: key_scalars.clone(),
        }),
    }
}

fn push_read_operation_payload(
    program: &CheckedProgram,
    payload: &mut String,
    store: &StoreFact,
    kind: SurfaceReadOperationKind,
    footprint: SurfaceReadFootprint,
    projection: &[ResourceMemberId],
) -> Option<()> {
    push_tag_part(payload, "version", SURFACE_READ_OPERATION_TAG_VERSION);
    push_tag_part(
        payload,
        "store",
        accepted_catalog_id_text(program, store.catalog_id.as_deref())?,
    );
    push_identity_key_tag_parts(program, payload, store)?;
    match footprint {
        SurfaceReadFootprint::FullRecord { resource } => {
            let resource = program.facts.resource(resource);
            push_tag_part(
                payload,
                "footprint.resource",
                accepted_catalog_id_text(program, resource.catalog_id.as_deref())?,
            );
        }
    }
    push_tag_part(payload, "projection.len", &projection.len().to_string());
    for member_id in projection {
        let member = resource_member(program, *member_id)?;
        push_tag_part(
            payload,
            "projection.member",
            accepted_catalog_id_text(program, member.catalog_id.as_deref())?,
        );
        push_member_tag_parts(program, payload, "projection.member", member)?;
    }
    match kind {
        SurfaceReadOperationKind::SingletonRead { .. } => {
            push_tag_part(payload, "kind", "singleton");
        }
        SurfaceReadOperationKind::PointRead { .. } => {
            push_tag_part(payload, "kind", "point");
        }
        SurfaceReadOperationKind::PagedRootCollection { .. } => {
            push_tag_part(payload, "kind", "paged-root");
        }
        SurfaceReadOperationKind::PagedIndexCollection {
            index,
            exact_key_count,
            identity_key_count,
        } => {
            push_tag_part(payload, "kind", "paged-index");
            push_index_tag_parts(program, payload, program.facts.store_index(index))?;
            push_tag_part(payload, "exact", &exact_key_count.to_string());
            push_tag_part(payload, "identity", &identity_key_count.to_string());
        }
        SurfaceReadOperationKind::UniqueIndexLookup { index, key_count } => {
            push_tag_part(payload, "kind", "unique-index");
            push_index_tag_parts(program, payload, program.facts.store_index(index))?;
            push_tag_part(payload, "keys", &key_count.to_string());
        }
    }
    Some(())
}

fn push_update_operation_payload(
    program: &CheckedProgram,
    payload: &mut String,
    store: &StoreFact,
    update: &[SurfaceFieldFact],
) -> Option<()> {
    push_tag_part(payload, "version", SURFACE_UPDATE_OPERATION_TAG_VERSION);
    push_tag_part(
        payload,
        "store",
        accepted_catalog_id_text(program, store.catalog_id.as_deref())?,
    );
    push_tag_part(
        payload,
        "footprint.resource",
        accepted_catalog_id_text(
            program,
            program.facts.resource(store.resource).catalog_id.as_deref(),
        )?,
    );
    push_identity_key_tag_parts(program, payload, store)?;
    push_tag_part(payload, "patch", "non_empty_patch");
    push_tag_part(
        payload,
        "kind",
        match update_descriptor_kind(store) {
            SurfaceUpdateOperationDescriptorKind::SingletonUpdate => "singleton",
            SurfaceUpdateOperationDescriptorKind::PointUpdate => "point",
        },
    );
    let mut fields = update
        .iter()
        .map(|field| {
            let member = resource_member(program, field.member)?;
            let catalog_id = accepted_catalog_id_text(program, member.catalog_id.as_deref())?;
            Some((catalog_id, member))
        })
        .collect::<Option<Vec<_>>>()?;
    fields.sort_by_key(|(catalog_id, _)| *catalog_id);
    push_tag_part(payload, "fields.len", &fields.len().to_string());
    for (catalog_id, member) in fields {
        push_tag_part(payload, "field.member", catalog_id);
        push_member_tag_parts(program, payload, "field.member", member)?;
    }
    Some(())
}

fn push_index_tag_parts(
    program: &CheckedProgram,
    payload: &mut String,
    index: &StoreIndexFact,
) -> Option<()> {
    push_tag_part(
        payload,
        "index",
        accepted_catalog_id_text(program, index.catalog_id.as_deref())?,
    );
    push_tag_part(
        payload,
        "index.unique",
        if index.unique { "true" } else { "false" },
    );
    push_tag_part(payload, "index.keys.len", &index.keys.len().to_string());
    for key in &index.keys {
        push_meaning_tag_parts(program, payload, "index.key", &key.value_meaning)?;
    }
    Some(())
}

fn push_identity_key_tag_parts(
    program: &CheckedProgram,
    payload: &mut String,
    store: &StoreFact,
) -> Option<()> {
    push_tag_part(
        payload,
        "identity.keys.len",
        &store.identity_keys.len().to_string(),
    );
    for key in &store.identity_keys {
        push_meaning_tag_parts(
            program,
            payload,
            "identity.key",
            key.value_meaning.as_ref()?,
        )?;
    }
    Some(())
}

fn push_member_tag_parts(
    program: &CheckedProgram,
    payload: &mut String,
    prefix: &str,
    member: &ResourceMemberFact,
) -> Option<()> {
    push_prefixed_tag_part(
        payload,
        prefix,
        "kind",
        match member.kind {
            ResourceMemberKind::Field => "field",
            ResourceMemberKind::Group => "group",
        },
    );
    push_prefixed_tag_part(payload, prefix, "key_count", &member.key_count.to_string());
    push_prefixed_tag_part(
        payload,
        prefix,
        "required",
        match member.plain_field_required? {
            true => "true",
            false => "false",
        },
    );
    push_meaning_tag_parts(
        program,
        payload,
        &format!("{prefix}.value"),
        member.value_meaning.as_ref()?,
    )
}

fn push_meaning_tag_parts(
    program: &CheckedProgram,
    payload: &mut String,
    prefix: &str,
    meaning: &StoredValueMeaning,
) -> Option<()> {
    match meaning {
        StoredValueMeaning::Scalar(scalar) => {
            push_prefixed_tag_part(payload, prefix, "scalar", scalar.name());
        }
        StoredValueMeaning::Enum { enum_id, members } => {
            let enum_fact = program.facts.enum_(*enum_id)?;
            push_prefixed_tag_part(
                payload,
                prefix,
                "enum",
                accepted_catalog_id_text(program, enum_fact.catalog_id.as_deref())?,
            );
            push_prefixed_tag_part(
                payload,
                prefix,
                "enum.members.len",
                &members.len().to_string(),
            );
            for member_id in members {
                let member = program
                    .facts
                    .enum_members()
                    .iter()
                    .find(|member| member.id == *member_id)?;
                push_prefixed_tag_part(
                    payload,
                    prefix,
                    "enum.member",
                    accepted_catalog_id_text(program, member.catalog_id.as_deref())?,
                );
            }
        }
        StoredValueMeaning::Identity {
            store_catalog_id,
            arity,
            key_scalars,
            ..
        } => {
            push_prefixed_tag_part(
                payload,
                prefix,
                "identity.store",
                accepted_catalog_id_text(program, store_catalog_id.as_deref())?,
            );
            push_prefixed_tag_part(payload, prefix, "identity.arity", &arity.to_string());
            for scalar in key_scalars {
                push_prefixed_tag_part(payload, prefix, "identity.scalar", scalar.name());
            }
        }
    }
    Some(())
}

fn accepted_catalog_id(program: &CheckedProgram, catalog_id: Option<&str>) -> Option<CatalogId> {
    let catalog_id = catalog_id?;
    let id = CatalogId::new(catalog_id.to_string()).ok()?;
    program
        .catalog
        .accepted_entries
        .iter()
        .any(|entry| entry.stable_id == catalog_id)
        .then_some(id)
}

fn accepted_catalog_id_text<'a>(
    program: &CheckedProgram,
    catalog_id: Option<&'a str>,
) -> Option<&'a str> {
    let catalog_id = catalog_id?;
    CatalogId::new(catalog_id.to_string()).ok()?;
    program
        .catalog
        .accepted_entries
        .iter()
        .any(|entry| entry.stable_id == catalog_id)
        .then_some(catalog_id)
}

fn push_tag_part(payload: &mut String, label: &str, value: &str) {
    payload.push_str(label);
    payload.push('\0');
    payload.push_str(&value.len().to_string());
    payload.push('\0');
    payload.push_str(value);
    payload.push('\0');
}

fn push_prefixed_tag_part(payload: &mut String, prefix: &str, label: &str, value: &str) {
    push_tag_part(payload, &format!("{prefix}.{label}"), value);
}
