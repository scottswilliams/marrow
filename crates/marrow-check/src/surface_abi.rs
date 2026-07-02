use marrow_store::cell::CatalogId;

use crate::entry_abi::{
    ENTRY_PROTOCOL_TAG_VERSION, EntryActionResultShape, EntryFunctionSurfaceDescriptor,
    EntryIdentity, EntryParameter, EntryResourceResultField, EntryResultShape, EntrySurfaceProfile,
    EntrySurfaceValueShape,
};
use crate::facts::{
    EntryCostShapeFact, ResourceMemberFact, ResourceMemberId, ResourceMemberKind, StoreFact,
    StoreIndexFact, StoreIndexId, StoreIndexKeySource, StoredValueMeaning, SurfaceActionFact,
    SurfaceCatalogStatus, SurfaceComputedReadFact, SurfaceFact, SurfaceFieldFact,
    SurfaceReadFootprint, SurfaceReadOperationFact, SurfaceReadOperationKind, WorkShapeClass,
};
use crate::program::CheckedProgram;

pub const SURFACE_READ_OPERATION_TAG_VERSION: &str = "surface.read.v1";
pub const SURFACE_UPDATE_OPERATION_TAG_VERSION: &str = "surface.update.v1";
pub const SURFACE_CREATE_OPERATION_TAG_VERSION: &str = "surface.create.v1";
pub const SURFACE_DELETE_OPERATION_TAG_VERSION: &str = "surface.delete.v1";
pub const SURFACE_COMPUTED_READ_OPERATION_TAG_VERSION: &str = "surface.computed_read.v1";

#[derive(Debug, Clone)]
pub struct SurfaceActionOperationDescriptor {
    pub profile_version: &'static str,
    pub operation_tag: String,
    pub alias: String,
    pub identity: EntryIdentity,
    pub parameters: Vec<EntryParameter>,
    pub return_value: EntryActionResultShape,
}

#[derive(Debug, Clone)]
pub struct SurfaceComputedReadOperationDescriptor {
    pub profile_version: &'static str,
    pub operation_tag: String,
    pub alias: String,
    pub callable: EntryFunctionSurfaceDescriptor,
    pub cost_shape: SurfaceComputedReadCostShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceComputedReadCostShape {
    pub work_shape: WorkShapeClass,
    pub point_reads: usize,
    pub range_scans: usize,
    pub writes: usize,
    pub index_entry_touches: usize,
    pub commit_points: usize,
}

#[derive(Debug, Clone)]
pub struct SurfaceReadOperationDescriptor {
    pub profile_version: &'static str,
    pub operation_tag: String,
    pub alias: String,
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
    PagedIndexRangeCollection {
        index_catalog_id: CatalogId,
        exact_key_count: usize,
        range_key_index: usize,
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
pub struct SurfaceOperationEnumMember {
    pub render_label: String,
    pub catalog_id: CatalogId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceOperationValueShape {
    Scalar(marrow_schema::ScalarType),
    Enum {
        render_name: String,
        enum_catalog_id: CatalogId,
        members: Vec<SurfaceOperationEnumMember>,
    },
    Identity {
        /// The referenced store's source name (`projects` for `Id(^projects)`). The TypeScript
        /// client brands a reference after this name when the target store has no surface of its
        /// own, so no catalog-id hash reaches a user-facing symbol.
        store_name: String,
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
            alias: operation.alias.clone(),
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

#[derive(Debug, Clone)]
pub struct SurfaceCreateOperationDescriptor {
    pub profile_version: &'static str,
    pub operation_tag: String,
    pub kind: SurfaceCreateOperationDescriptorKind,
    pub body_semantics: SurfaceCreateBodySemantics,
    pub identity_policy: SurfaceCreateIdentityPolicy,
    pub existence_semantics: SurfaceCreateExistenceSemantics,
    pub store_catalog_id: CatalogId,
    pub resource_catalog_id: CatalogId,
    pub identity_keys: Vec<SurfaceOperationIdentityKey>,
    pub fields: Vec<SurfaceCreateOperationField>,
    pub projection: Vec<SurfaceReadOperationProjectionField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceCreateOperationDescriptorKind {
    SingletonCreate,
    PointCreate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCreateBodySemantics {
    ExactDeclaredBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCreateIdentityPolicy {
    SingletonNoIdentity,
    ClientSuppliedIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCreateExistenceSemantics {
    RejectExistingNoReplace,
}

#[derive(Debug, Clone)]
pub struct SurfaceCreateOperationField {
    pub render_label: String,
    pub member_catalog_id: CatalogId,
    pub value: SurfaceOperationValueShape,
}

#[derive(Debug, Clone)]
pub struct SurfaceDeleteOperationDescriptor {
    pub profile_version: &'static str,
    pub operation_tag: String,
    pub kind: SurfaceDeleteOperationDescriptorKind,
    pub semantics: SurfaceDeleteSemantics,
    pub store_catalog_id: CatalogId,
    pub resource_catalog_id: CatalogId,
    pub identity_keys: Vec<SurfaceOperationIdentityKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceDeleteOperationDescriptorKind {
    SingletonDelete,
    PointDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDeleteSemantics {
    RejectAbsentFullSubtree,
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

impl SurfaceCreateOperationDescriptor {
    pub fn from_surface(program: &CheckedProgram, surface: &SurfaceFact) -> Option<Self> {
        require_stable_surface(surface)?;
        if surface.create.is_empty() {
            return None;
        }
        let store = program.facts.store(surface.store);
        let projection = surface
            .fields
            .iter()
            .map(|field| field.member)
            .collect::<Vec<_>>();
        let resource_catalog_id = accepted_catalog_id(
            program,
            program.facts.resource(store.resource).catalog_id.as_deref(),
        )?;
        Some(Self {
            profile_version: SURFACE_CREATE_OPERATION_TAG_VERSION,
            operation_tag: surface_create_operation_tag(
                program,
                store,
                &surface.create,
                &projection,
            )?,
            kind: create_descriptor_kind(store),
            body_semantics: SurfaceCreateBodySemantics::ExactDeclaredBody,
            identity_policy: create_identity_policy(store),
            existence_semantics: SurfaceCreateExistenceSemantics::RejectExistingNoReplace,
            store_catalog_id: accepted_catalog_id(program, store.catalog_id.as_deref())?,
            resource_catalog_id,
            identity_keys: identity_key_descriptors(program, store)?,
            fields: create_field_descriptors(program, &surface.create)?,
            projection: projection_descriptors(program, &projection)?,
        })
    }
}

impl SurfaceDeleteOperationDescriptor {
    pub fn from_surface(program: &CheckedProgram, surface: &SurfaceFact) -> Option<Self> {
        require_stable_surface(surface)?;
        surface.delete.as_ref()?;
        let store = program.facts.store(surface.store);
        let resource_catalog_id = accepted_catalog_id(
            program,
            program.facts.resource(store.resource).catalog_id.as_deref(),
        )?;
        Some(Self {
            profile_version: SURFACE_DELETE_OPERATION_TAG_VERSION,
            operation_tag: surface_delete_operation_tag(program, store)?,
            kind: delete_descriptor_kind(store),
            semantics: SurfaceDeleteSemantics::RejectAbsentFullSubtree,
            store_catalog_id: accepted_catalog_id(program, store.catalog_id.as_deref())?,
            resource_catalog_id,
            identity_keys: identity_key_descriptors(program, store)?,
        })
    }
}

impl SurfaceActionOperationDescriptor {
    pub fn from_action(
        program: &CheckedProgram,
        surface: &SurfaceFact,
        action: &SurfaceActionFact,
    ) -> Option<Self> {
        require_stable_surface(surface)?;
        let requested_name = canonical_action_name(program, action)?;
        let descriptor = EntryFunctionSurfaceDescriptor::from_function_ref(
            program,
            &requested_name,
            action.function,
            EntrySurfaceProfile::Action,
        )?;
        Some(Self {
            profile_version: ENTRY_PROTOCOL_TAG_VERSION,
            operation_tag: descriptor.identity.entry_tag.clone(),
            alias: action.alias.clone(),
            identity: descriptor.identity,
            parameters: descriptor.parameters,
            return_value: EntryActionResultShape::from_result(descriptor.result),
        })
    }
}

impl SurfaceComputedReadOperationDescriptor {
    pub fn from_computed_read(
        program: &CheckedProgram,
        surface: &SurfaceFact,
        computed_read: &SurfaceComputedReadFact,
    ) -> Option<Self> {
        require_stable_surface(surface)?;
        let requested_name = canonical_function_name(program, computed_read.function)?;
        let callable = EntryFunctionSurfaceDescriptor::from_function_ref(
            program,
            &requested_name,
            computed_read.function,
            EntrySurfaceProfile::ComputedRead,
        )?;
        let cost_shape = computed_read_cost_shape(program, computed_read.function)?;
        let operation_tag = surface_computed_read_operation_tag(&callable, &cost_shape);
        Some(Self {
            profile_version: SURFACE_COMPUTED_READ_OPERATION_TAG_VERSION,
            operation_tag,
            alias: computed_read.alias.clone(),
            callable,
            cost_shape,
        })
    }
}

impl From<EntryCostShapeFact> for SurfaceComputedReadCostShape {
    fn from(shape: EntryCostShapeFact) -> Self {
        Self {
            work_shape: shape.work_shape,
            point_reads: shape.point_reads,
            range_scans: shape.range_scans,
            writes: shape.writes,
            index_entry_touches: shape.index_entry_touches,
            commit_points: shape.commit_points,
        }
    }
}

fn canonical_action_name(program: &CheckedProgram, action: &SurfaceActionFact) -> Option<String> {
    canonical_function_name(program, action.function)
}

fn canonical_function_name(
    program: &CheckedProgram,
    function_ref: crate::CheckedFunctionRef,
) -> Option<String> {
    let function = program.facts.function_for_ref(function_ref)?;
    let module = program.facts.modules().get(function.module.0 as usize)?;
    if module.name.is_empty() {
        Some(function.name.clone())
    } else {
        Some(format!("{}::{}", module.name, function.name))
    }
}

fn computed_read_cost_shape(
    program: &CheckedProgram,
    function_ref: crate::CheckedFunctionRef,
) -> Option<SurfaceComputedReadCostShape> {
    let function = program.facts.function_id_for_ref(function_ref)?;
    program
        .entry_cost_shapes()
        .into_iter()
        .find(|shape| shape.function == function)
        .map(SurfaceComputedReadCostShape::from)
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

pub(crate) fn surface_create_operation_tag(
    program: &CheckedProgram,
    store: &StoreFact,
    create: &[SurfaceFieldFact],
    projection: &[ResourceMemberId],
) -> Option<String> {
    let mut payload = String::new();
    push_create_operation_payload(program, &mut payload, store, create, projection)?;
    Some(marrow_project::sha256_digest(payload.as_bytes()))
}

pub(crate) fn surface_delete_operation_tag(
    program: &CheckedProgram,
    store: &StoreFact,
) -> Option<String> {
    let mut payload = String::new();
    push_delete_operation_payload(program, &mut payload, store)?;
    Some(marrow_project::sha256_digest(payload.as_bytes()))
}

fn surface_computed_read_operation_tag(
    callable: &EntryFunctionSurfaceDescriptor,
    cost_shape: &SurfaceComputedReadCostShape,
) -> String {
    let mut payload = String::new();
    push_computed_read_operation_payload(&mut payload, callable, cost_shape);
    marrow_project::sha256_digest(payload.as_bytes())
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
        SurfaceReadOperationKind::PagedIndexRangeCollection {
            index,
            exact_key_count,
            range_key_index,
            identity_key_count,
        } => Some(
            SurfaceReadOperationDescriptorKind::PagedIndexRangeCollection {
                index_catalog_id: index_catalog_id(program, index)?,
                exact_key_count,
                range_key_index,
                identity_key_count,
            },
        ),
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

fn create_descriptor_kind(store: &StoreFact) -> SurfaceCreateOperationDescriptorKind {
    if store.identity_keys.is_empty() {
        SurfaceCreateOperationDescriptorKind::SingletonCreate
    } else {
        SurfaceCreateOperationDescriptorKind::PointCreate
    }
}

fn delete_descriptor_kind(store: &StoreFact) -> SurfaceDeleteOperationDescriptorKind {
    if store.identity_keys.is_empty() {
        SurfaceDeleteOperationDescriptorKind::SingletonDelete
    } else {
        SurfaceDeleteOperationDescriptorKind::PointDelete
    }
}

fn create_identity_policy(store: &StoreFact) -> SurfaceCreateIdentityPolicy {
    if store.identity_keys.is_empty() {
        SurfaceCreateIdentityPolicy::SingletonNoIdentity
    } else {
        SurfaceCreateIdentityPolicy::ClientSuppliedIdentity
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

fn create_field_descriptors(
    program: &CheckedProgram,
    create: &[SurfaceFieldFact],
) -> Option<Vec<SurfaceCreateOperationField>> {
    create
        .iter()
        .map(|field| {
            let member = resource_member(program, field.member)?;
            Some(SurfaceCreateOperationField {
                render_label: field.name.clone(),
                member_catalog_id: accepted_catalog_id(program, member.catalog_id.as_deref())?,
                value: value_shape(program, member.value_meaning.as_ref()?)?,
            })
        })
        .collect()
}

fn index_key_descriptors_for_operation(
    program: &CheckedProgram,
    kind: SurfaceReadOperationKind,
) -> Option<Vec<SurfaceReadOperationIndexKey>> {
    let index = match kind {
        SurfaceReadOperationKind::PagedIndexCollection { index, .. }
        | SurfaceReadOperationKind::PagedIndexRangeCollection { index, .. }
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
                render_name: enum_fact.name.clone(),
                enum_catalog_id: accepted_catalog_id(program, enum_fact.catalog_id.as_deref())?,
                members: members
                    .iter()
                    .map(|member_id| {
                        let member = program
                            .facts
                            .enum_members()
                            .iter()
                            .find(|member| member.id == *member_id)?;
                        Some(SurfaceOperationEnumMember {
                            render_label: member.name.clone(),
                            catalog_id: accepted_catalog_id(program, member.catalog_id.as_deref())?,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            })
        }
        StoredValueMeaning::Identity {
            root,
            store_catalog_id,
            arity,
            key_scalars,
            ..
        } => Some(SurfaceOperationValueShape::Identity {
            store_name: root.clone(),
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
        SurfaceReadOperationKind::PagedIndexRangeCollection {
            index,
            exact_key_count,
            range_key_index,
            identity_key_count,
        } => {
            push_tag_part(payload, "kind", "paged-index-range");
            push_index_tag_parts(program, payload, program.facts.store_index(index))?;
            push_tag_part(payload, "exact", &exact_key_count.to_string());
            push_tag_part(payload, "range", &range_key_index.to_string());
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

fn push_create_operation_payload(
    program: &CheckedProgram,
    payload: &mut String,
    store: &StoreFact,
    create: &[SurfaceFieldFact],
    projection: &[ResourceMemberId],
) -> Option<()> {
    push_tag_part(payload, "version", SURFACE_CREATE_OPERATION_TAG_VERSION);
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
    push_tag_part(payload, "body", "exact_declared");
    push_tag_part(
        payload,
        "identity_policy",
        match create_identity_policy(store) {
            SurfaceCreateIdentityPolicy::SingletonNoIdentity => "singleton_no_identity",
            SurfaceCreateIdentityPolicy::ClientSuppliedIdentity => "client_supplied_identity",
        },
    );
    push_tag_part(payload, "existence", "reject_existing_no_replace");
    push_tag_part(
        payload,
        "kind",
        match create_descriptor_kind(store) {
            SurfaceCreateOperationDescriptorKind::SingletonCreate => "singleton",
            SurfaceCreateOperationDescriptorKind::PointCreate => "point",
        },
    );
    let mut fields = create
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
    Some(())
}

fn push_delete_operation_payload(
    program: &CheckedProgram,
    payload: &mut String,
    store: &StoreFact,
) -> Option<()> {
    push_tag_part(payload, "version", SURFACE_DELETE_OPERATION_TAG_VERSION);
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
    push_tag_part(payload, "semantics", "reject_absent_full_subtree");
    push_tag_part(
        payload,
        "kind",
        match delete_descriptor_kind(store) {
            SurfaceDeleteOperationDescriptorKind::SingletonDelete => "singleton",
            SurfaceDeleteOperationDescriptorKind::PointDelete => "point",
        },
    );
    Some(())
}

fn push_computed_read_operation_payload(
    payload: &mut String,
    callable: &EntryFunctionSurfaceDescriptor,
    cost_shape: &SurfaceComputedReadCostShape,
) {
    push_tag_part(
        payload,
        "version",
        SURFACE_COMPUTED_READ_OPERATION_TAG_VERSION,
    );
    push_tag_part(payload, "callable.entry_tag", &callable.identity.entry_tag);
    push_tag_part(
        payload,
        "callable.canonical_name",
        &callable.identity.canonical_name,
    );
    push_tag_part(
        payload,
        "callable.source_digest",
        &callable.identity.source_digest,
    );
    push_tag_part(
        payload,
        "callable.read_only_context_digest",
        &callable.identity.read_only_context_digest,
    );
    push_result_tag_parts(payload, &callable.result);
    push_cost_shape_tag_parts(payload, cost_shape);
}

fn push_result_tag_parts(payload: &mut String, result: &EntryResultShape) {
    push_tag_part(
        payload,
        "result.presence",
        if result.maybe_present() {
            "maybe_present"
        } else {
            "always"
        },
    );
    match result.value() {
        Some(shape) => {
            push_tag_part(payload, "result.value", "some");
            push_entry_surface_value_tag_parts(payload, "result.value.shape", shape);
        }
        None => push_tag_part(payload, "result.value", "none"),
    }
}

fn push_entry_surface_value_tag_parts(
    payload: &mut String,
    prefix: &str,
    shape: &EntrySurfaceValueShape,
) {
    match shape {
        EntrySurfaceValueShape::Scalar(scalar) => {
            push_prefixed_tag_part(payload, prefix, "kind", "scalar");
            push_prefixed_tag_part(payload, prefix, "scalar", scalar.name());
        }
        EntrySurfaceValueShape::Enum {
            catalog_id,
            members,
            ..
        } => {
            push_prefixed_tag_part(payload, prefix, "kind", "enum");
            push_prefixed_tag_part(payload, prefix, "catalog_id", catalog_id.as_str());
            push_prefixed_tag_part(payload, prefix, "members.len", &members.len().to_string());
            for member in members {
                push_prefixed_tag_part(payload, prefix, "member", member.catalog_id.as_str());
            }
        }
        EntrySurfaceValueShape::Identity {
            store_catalog_id,
            keys,
            ..
        } => {
            push_prefixed_tag_part(payload, prefix, "kind", "identity");
            push_prefixed_tag_part(payload, prefix, "store", store_catalog_id.as_str());
            push_prefixed_tag_part(payload, prefix, "keys.len", &keys.len().to_string());
            for key in keys {
                push_prefixed_tag_part(payload, prefix, "key.scalar", key.scalar.name());
            }
        }
        EntrySurfaceValueShape::Sequence(element) => {
            push_prefixed_tag_part(payload, prefix, "kind", "sequence");
            push_entry_surface_value_tag_parts(payload, &format!("{prefix}.element"), element);
        }
        EntrySurfaceValueShape::Resource {
            resource_catalog_id,
            fields,
            ..
        } => {
            push_prefixed_tag_part(payload, prefix, "kind", "resource");
            push_prefixed_tag_part(payload, prefix, "resource", resource_catalog_id.as_str());
            push_prefixed_tag_part(payload, prefix, "fields.len", &fields.len().to_string());
            for field in fields {
                push_resource_result_field_tag_parts(payload, prefix, field);
            }
        }
    }
}

fn push_resource_result_field_tag_parts(
    payload: &mut String,
    prefix: &str,
    field: &EntryResourceResultField,
) {
    push_prefixed_tag_part(payload, prefix, "field", field.member_catalog_id.as_str());
    push_prefixed_tag_part(
        payload,
        prefix,
        "field.required",
        if field.required { "true" } else { "false" },
    );
    push_entry_surface_value_tag_parts(payload, &format!("{prefix}.field.shape"), &field.shape);
}

fn push_cost_shape_tag_parts(payload: &mut String, cost_shape: &SurfaceComputedReadCostShape) {
    push_tag_part(
        payload,
        "cost.work_shape",
        match cost_shape.work_shape {
            WorkShapeClass::ComputeOnly => "compute_only",
            WorkShapeClass::ReadOnly => "read_only",
            WorkShapeClass::WritesSavedData => "writes_saved_data",
        },
    );
    push_tag_part(
        payload,
        "cost.point_reads",
        &cost_shape.point_reads.to_string(),
    );
    push_tag_part(
        payload,
        "cost.range_scans",
        &cost_shape.range_scans.to_string(),
    );
    push_tag_part(payload, "cost.writes", &cost_shape.writes.to_string());
    push_tag_part(
        payload,
        "cost.index_entry_touches",
        &cost_shape.index_entry_touches.to_string(),
    );
    push_tag_part(
        payload,
        "cost.commit_points",
        &cost_shape.commit_points.to_string(),
    );
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
