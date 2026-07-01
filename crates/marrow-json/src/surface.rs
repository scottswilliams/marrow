use std::collections::{BTreeMap, BTreeSet};

use marrow_run::{
    SurfaceEnumValue, SurfaceReadField, SurfaceReadIdentity, SurfaceReadRecord, SurfaceValue, Value,
};
use marrow_store::key::SavedKey;
use serde::{Deserialize, Serialize};

mod client_model;
mod client_ts;
mod cursor_token;
mod execute;
mod operation;
mod operation_catalog;
mod request;
mod route;
mod route_binding;
use client_model::{
    SurfaceClientModel, SurfaceClientRecord, SurfaceClientStore, SurfaceFieldType, SurfaceMethod,
    SurfaceMethodInput, SurfaceMethodParam, SurfaceMethodResult, SurfaceRecordField,
};
pub use client_ts::{
    SURFACE_ABI_DIGEST_PREFIX, SURFACE_CLIENT_DIGEST_PREFIX, SURFACE_CLIENT_DO_NOT_EDIT,
    SURFACE_CLIENT_PROFILE, SURFACE_CLIENT_PROFILE_PREFIX, SurfaceClientCursorProfile,
    SurfaceClientRenderError, SurfaceClientRenderErrorKind, render_typescript_client,
    render_typescript_client_with_cursor_profile, surface_abi_digest, surface_client_digest,
    surface_client_digest_with_cursor_profile, surface_client_header, surface_client_header_digest,
};
pub use cursor_token::{
    SURFACE_CURSOR_TOKEN_PROFILE_VERSION, SurfaceCursorTokenCodec, SurfaceCursorTokenError,
    SurfaceCursorTokenErrorKind, SurfaceCursorTokenKey, SurfaceCursorTokenKeyId,
};
pub use execute::{
    execute_project_surface_page_by_tag, execute_project_surface_point_create_by_tag,
    execute_project_surface_point_delete_by_tag, execute_project_surface_point_read_by_tag,
    execute_project_surface_point_update_by_tag, execute_project_surface_singleton_create_by_tag,
    execute_project_surface_singleton_delete_by_tag, execute_project_surface_singleton_read_by_tag,
    execute_project_surface_singleton_update_by_tag, execute_project_surface_unique_lookup_by_tag,
    execute_surface_page_by_tag, execute_surface_point_create_by_tag,
    execute_surface_point_delete_by_tag, execute_surface_point_read_by_tag,
    execute_surface_point_update_by_tag, execute_surface_singleton_create_by_tag,
    execute_surface_singleton_delete_by_tag, execute_surface_singleton_read_by_tag,
    execute_surface_singleton_update_by_tag, execute_surface_unique_lookup_by_tag,
};
pub use operation::{
    SURFACE_OPERATION_PROFILE_VERSION, SurfaceActionRequestJson, SurfaceActionResultJson,
    SurfaceComputedReadInvocationResultJson, SurfaceComputedReadRequestJson,
    SurfaceEmptyRequestJson, SurfaceOperationErrorJson, SurfaceOperationRequestBodyJson,
    SurfaceOperationRequestJson, SurfaceOperationResponseJson, SurfaceOperationResultJson,
    execute_project_surface_operation, execute_project_surface_operation_read_only,
    execute_project_surface_operation_with_host,
};
pub use operation_catalog::{
    SurfaceOperationBinding, SurfaceOperationCatalog, SurfaceOperationCatalogError,
    SurfaceOperationCatalogErrorKind, SurfaceOperationKind,
};
pub use request::{
    DecodedSurfacePageRequest, DecodedSurfacePointCreateRequest, DecodedSurfacePointDeleteRequest,
    DecodedSurfacePointRequest, DecodedSurfacePointUpdateRequest,
    DecodedSurfaceSingletonCreateRequest, DecodedSurfaceSingletonUpdateRequest,
    DecodedSurfaceUniqueLookupRequest, SurfaceCreateFieldJson, SurfacePageRequestJson,
    SurfacePointCreateRequestJson, SurfacePointDeleteRequestJson, SurfacePointRequestJson,
    SurfacePointUpdateRequestJson, SurfaceSingletonCreateRequestJson,
    SurfaceSingletonUpdateRequestJson, SurfaceUniqueLookupRequestJson, SurfaceUpdateFieldJson,
    SurfaceWriteValueJson,
};
pub use route::{
    SURFACE_ROUTE_PROFILE_VERSION, SurfaceRouteJson, SurfaceRouteManifestJson,
    SurfaceRouteMethodJson, SurfaceRouteRequestJson, SurfaceRouteSurfaceJson,
};
pub use route_binding::{
    SurfaceRouteBinding, SurfaceRouteBindingError, SurfaceRouteBindingErrorKind,
    SurfaceRouteBindings,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceAbiJson {
    pub surfaces: Vec<SurfaceDescriptorJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceDescriptorJson {
    pub module: String,
    pub name: String,
    pub catalog_status: SurfaceCatalogStatusJson,
    pub read: Vec<SurfaceReadOperationDescriptorJson>,
    pub computed_reads: Vec<SurfaceComputedReadOperationDescriptorJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<SurfaceUpdateOperationDescriptorJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create: Option<SurfaceCreateOperationDescriptorJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete: Option<SurfaceDeleteOperationDescriptorJson>,
    pub actions: Vec<SurfaceActionOperationDescriptorJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceCatalogStatusJson {
    Stable,
    SourceOnly { blockers: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceReadOperationDescriptorJson {
    pub profile_version: String,
    pub operation_tag: String,
    pub alias: String,
    pub kind: SurfaceReadOperationKindJson,
    pub store_catalog_id: String,
    pub resource_catalog_id: String,
    pub identity_keys: Vec<SurfaceOperationIdentityKeyJson>,
    pub projection: Vec<SurfaceReadProjectionFieldJson>,
    pub index_keys: Vec<SurfaceReadIndexKeyJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceReadOperationKindJson {
    SingletonRead,
    PointRead,
    PagedRootCollection,
    PagedIndexCollection {
        index_catalog_id: String,
        exact_key_count: usize,
        identity_key_count: usize,
    },
    UniqueIndexLookup {
        index_catalog_id: String,
        key_count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceOperationIdentityKeyJson {
    pub render_label: String,
    pub value: SurfaceOperationValueShapeJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceReadProjectionFieldJson {
    pub render_label: String,
    pub member_catalog_id: String,
    pub required: bool,
    pub value: SurfaceOperationValueShapeJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceReadIndexKeyJson {
    pub render_label: String,
    pub source: SurfaceReadIndexKeySourceJson,
    pub value: SurfaceOperationValueShapeJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceReadIndexKeySourceJson {
    IdentityKey,
    ResourceMember { member_catalog_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceOperationValueShapeJson {
    Scalar {
        scalar: String,
    },
    Enum {
        render_name: String,
        enum_catalog_id: String,
        members: Vec<SurfaceOperationEnumMemberJson>,
    },
    Identity {
        store_name: String,
        store_catalog_id: String,
        arity: usize,
        key_scalars: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceOperationEnumMemberJson {
    pub render_label: String,
    pub catalog_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceUpdateOperationDescriptorJson {
    pub profile_version: String,
    pub operation_tag: String,
    pub kind: SurfaceUpdateOperationKindJson,
    pub patch_semantics: SurfaceUpdatePatchSemanticsJson,
    pub store_catalog_id: String,
    pub resource_catalog_id: String,
    pub identity_keys: Vec<SurfaceOperationIdentityKeyJson>,
    pub fields: Vec<SurfaceUpdateFieldDescriptorJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceUpdateOperationKindJson {
    SingletonUpdate,
    PointUpdate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceUpdatePatchSemanticsJson {
    NonEmptyPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceUpdateFieldDescriptorJson {
    pub render_label: String,
    pub member_catalog_id: String,
    pub backing_required: bool,
    pub value: SurfaceOperationValueShapeJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceCreateOperationDescriptorJson {
    pub profile_version: String,
    pub operation_tag: String,
    pub kind: SurfaceCreateOperationKindJson,
    pub body_semantics: SurfaceCreateBodySemanticsJson,
    pub identity_policy: SurfaceCreateIdentityPolicyJson,
    pub existence_semantics: SurfaceCreateExistenceSemanticsJson,
    pub store_catalog_id: String,
    pub resource_catalog_id: String,
    pub identity_keys: Vec<SurfaceOperationIdentityKeyJson>,
    pub fields: Vec<SurfaceCreateFieldDescriptorJson>,
    pub projection: Vec<SurfaceReadProjectionFieldJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceCreateOperationKindJson {
    SingletonCreate,
    PointCreate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCreateBodySemanticsJson {
    ExactDeclaredBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCreateIdentityPolicyJson {
    SingletonNoIdentity,
    ClientSuppliedIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCreateExistenceSemanticsJson {
    RejectExistingNoReplace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceCreateFieldDescriptorJson {
    pub render_label: String,
    pub member_catalog_id: String,
    pub value: SurfaceOperationValueShapeJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceDeleteOperationDescriptorJson {
    pub profile_version: String,
    pub operation_tag: String,
    pub kind: SurfaceDeleteOperationKindJson,
    pub semantics: SurfaceDeleteSemanticsJson,
    pub store_catalog_id: String,
    pub resource_catalog_id: String,
    pub identity_keys: Vec<SurfaceOperationIdentityKeyJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceDeleteOperationKindJson {
    SingletonDelete,
    PointDelete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceDeleteSemanticsJson {
    RejectAbsentFullSubtree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceActionOperationDescriptorJson {
    pub profile_version: String,
    pub operation_tag: String,
    pub alias: String,
    pub identity: SurfaceCallableIdentityJson,
    pub parameters: Vec<SurfaceCallableParameterJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_value: Option<SurfaceCallableArgumentShapeJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceComputedReadOperationDescriptorJson {
    pub profile_version: String,
    pub operation_tag: String,
    pub alias: String,
    pub callable: SurfaceComputedReadCallableJson,
    pub cost_shape: SurfaceComputedReadCostShapeJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceComputedReadCallableJson {
    pub identity: SurfaceCallableIdentityJson,
    pub parameters: Vec<SurfaceCallableParameterJson>,
    pub result: SurfaceComputedReadResultJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceComputedReadResultJson {
    pub presence: SurfaceComputedReadPresenceJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<SurfaceComputedReadValueShapeJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceComputedReadPresenceJson {
    Always,
    MaybePresent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceComputedReadValueShapeJson {
    Scalar {
        scalar: String,
    },
    Enum {
        render_label: String,
        enum_catalog_id: String,
        members: Vec<SurfaceCallableEnumMemberJson>,
    },
    Identity {
        render_label: String,
        store_catalog_id: String,
        keys: Vec<SurfaceCallableIdentityKeyJson>,
    },
    Sequence {
        element: Box<SurfaceComputedReadValueShapeJson>,
    },
    Resource {
        render_label: String,
        resource_catalog_id: String,
        fields: Vec<SurfaceComputedReadResourceFieldJson>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceComputedReadResourceFieldJson {
    pub render_label: String,
    pub member_catalog_id: String,
    pub required: bool,
    pub value: SurfaceComputedReadValueShapeJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceComputedReadCostShapeJson {
    pub work_shape: SurfaceComputedReadWorkShapeJson,
    pub point_reads: usize,
    pub range_scans: usize,
    pub writes: usize,
    pub index_entry_touches: usize,
    pub commit_points: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceComputedReadWorkShapeJson {
    ComputeOnly,
    ReadOnly,
    WritesSavedData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceCallableIdentityJson {
    pub requested_name: String,
    pub canonical_name: String,
    pub entry_tag: String,
    pub accepted_catalog_epoch: Option<u64>,
    pub source_digest: String,
    pub read_only_context_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceCallableParameterJson {
    pub name: String,
    pub presence: SurfaceCallableParameterPresenceJson,
    pub shape: SurfaceCallableArgumentShapeJson,
}

/// Whether a callable parameter is required or optional (`T?`), read off the
/// `EntryParameterShape` carrier. It mirrors the result-side presence enum so a
/// generated client types an optional parameter as nullable and a required one as
/// required; presence rides the one carrier, never a parallel flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCallableParameterPresenceJson {
    Required,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceCallableArgumentShapeJson {
    Scalar {
        scalar: String,
    },
    Enum {
        render_label: String,
        enum_catalog_id: String,
        members: Vec<SurfaceCallableEnumMemberJson>,
    },
    Identity {
        render_label: String,
        store_catalog_id: String,
        keys: Vec<SurfaceCallableIdentityKeyJson>,
    },
    Sequence {
        element: Box<SurfaceCallableArgumentShapeJson>,
    },
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceCallableEnumMemberJson {
    pub render_label: String,
    pub catalog_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceCallableIdentityKeyJson {
    pub render_label: String,
    pub scalar: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceIdentityJson {
    pub store_catalog_id: String,
    pub keys: Vec<SurfaceKeyJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceFieldJson {
    pub catalog_id: String,
    pub render_label: String,
    pub value: Option<SurfaceValueJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceRecordJson {
    pub identity: Option<SurfaceIdentityJson>,
    pub fields: Vec<SurfaceFieldJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfacePageJson {
    pub rows: Vec<SurfaceRecordJson>,
    pub next: Option<SurfaceCursorJson>,
}

impl SurfaceAbiJson {
    pub fn from_program(program: &marrow_check::CheckedProgram) -> Self {
        let mut surfaces = program
            .facts
            .surfaces()
            .iter()
            .map(|surface| {
                let module = &program.facts.modules()[surface.module.0 as usize];
                SurfaceDescriptorJson::from_surface(program, &module.name, surface)
            })
            .collect::<Vec<_>>();
        omit_uncallable_operation_tags(&mut surfaces);
        surfaces.sort_by(|left, right| {
            left.module
                .cmp(&right.module)
                .then_with(|| left.name.cmp(&right.name))
        });
        Self { surfaces }
    }
}

impl SurfaceDescriptorJson {
    fn from_surface(
        program: &marrow_check::CheckedProgram,
        module: &str,
        surface: &marrow_check::SurfaceFact,
    ) -> Self {
        let stable = matches!(
            surface.catalog_status,
            marrow_check::SurfaceCatalogStatus::Stable
        );
        Self {
            module: module.to_string(),
            name: surface.name.clone(),
            catalog_status: SurfaceCatalogStatusJson::from(&surface.catalog_status),
            read: if stable {
                surface
                    .read_operations
                    .iter()
                    .filter_map(|operation| {
                        marrow_check::SurfaceReadOperationDescriptor::from_operation(
                            program, surface, operation,
                        )
                        .map(SurfaceReadOperationDescriptorJson::from)
                    })
                    .collect()
            } else {
                Vec::new()
            },
            computed_reads: if stable {
                surface
                    .computed_reads
                    .iter()
                    .filter_map(|computed_read| {
                        marrow_check::SurfaceComputedReadOperationDescriptor::from_computed_read(
                            program,
                            surface,
                            computed_read,
                        )
                        .map(SurfaceComputedReadOperationDescriptorJson::from)
                    })
                    .collect()
            } else {
                Vec::new()
            },
            update: if stable {
                marrow_check::SurfaceUpdateOperationDescriptor::from_surface(program, surface)
                    .map(SurfaceUpdateOperationDescriptorJson::from)
            } else {
                None
            },
            create: if stable {
                marrow_check::SurfaceCreateOperationDescriptor::from_surface(program, surface)
                    .map(SurfaceCreateOperationDescriptorJson::from)
            } else {
                None
            },
            delete: if stable {
                marrow_check::SurfaceDeleteOperationDescriptor::from_surface(program, surface)
                    .map(SurfaceDeleteOperationDescriptorJson::from)
            } else {
                None
            },
            actions: if stable {
                surface
                    .actions
                    .iter()
                    .filter_map(|action| {
                        marrow_check::SurfaceActionOperationDescriptor::from_action(
                            program, surface, action,
                        )
                        .map(SurfaceActionOperationDescriptorJson::from)
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }
    }
}

fn omit_uncallable_operation_tags(surfaces: &mut [SurfaceDescriptorJson]) {
    let duplicate_tags = duplicate_operation_tags(all_operation_tags(surfaces).into_iter());
    if duplicate_tags.is_empty() {
        return;
    }
    for surface in surfaces.iter_mut() {
        surface
            .read
            .retain(|read| !duplicate_tags.contains(&read.operation_tag));
        if surface
            .update
            .as_ref()
            .is_some_and(|update| duplicate_tags.contains(&update.operation_tag))
        {
            surface.update = None;
        }
        if surface
            .create
            .as_ref()
            .is_some_and(|create| duplicate_tags.contains(&create.operation_tag))
        {
            surface.create = None;
        }
        if surface
            .delete
            .as_ref()
            .is_some_and(|delete| duplicate_tags.contains(&delete.operation_tag))
        {
            surface.delete = None;
        }
        surface
            .actions
            .retain(|action| !duplicate_tags.contains(&action.operation_tag));
        surface
            .computed_reads
            .retain(|computed_read| !duplicate_tags.contains(&computed_read.operation_tag));
    }
}

fn all_operation_tags(surfaces: &[SurfaceDescriptorJson]) -> Vec<&str> {
    let mut tags = Vec::new();
    for surface in surfaces {
        tags.extend(surface.read.iter().map(|read| read.operation_tag.as_str()));
        if let Some(update) = &surface.update {
            tags.push(update.operation_tag.as_str());
        }
        if let Some(create) = &surface.create {
            tags.push(create.operation_tag.as_str());
        }
        if let Some(delete) = &surface.delete {
            tags.push(delete.operation_tag.as_str());
        }
        tags.extend(
            surface
                .actions
                .iter()
                .map(|action| action.operation_tag.as_str()),
        );
        tags.extend(
            surface
                .computed_reads
                .iter()
                .map(|computed_read| computed_read.operation_tag.as_str()),
        );
    }
    tags
}

fn duplicate_operation_tags<'a>(tags: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    let mut counts = BTreeMap::new();
    for tag in tags {
        *counts.entry(tag).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(tag, _)| tag.to_string())
        .collect()
}

impl From<&marrow_check::SurfaceCatalogStatus> for SurfaceCatalogStatusJson {
    fn from(status: &marrow_check::SurfaceCatalogStatus) -> Self {
        match status {
            marrow_check::SurfaceCatalogStatus::Stable => Self::Stable,
            marrow_check::SurfaceCatalogStatus::SourceOnly(blockers) => Self::SourceOnly {
                blockers: blockers
                    .iter()
                    .map(|blocker| match blocker {
                        marrow_check::SurfaceCatalogBlocker::PendingCatalogProposal => {
                            "pending_catalog_proposal"
                        }
                        marrow_check::SurfaceCatalogBlocker::MissingAcceptedCatalogIds => {
                            "missing_accepted_catalog_ids"
                        }
                    })
                    .map(str::to_string)
                    .collect(),
            },
        }
    }
}

impl From<marrow_check::SurfaceReadOperationDescriptor> for SurfaceReadOperationDescriptorJson {
    fn from(descriptor: marrow_check::SurfaceReadOperationDescriptor) -> Self {
        Self {
            profile_version: descriptor.profile_version.to_string(),
            operation_tag: descriptor.operation_tag,
            alias: descriptor.alias,
            kind: SurfaceReadOperationKindJson::from(descriptor.kind),
            store_catalog_id: descriptor.store_catalog_id.as_str().to_string(),
            resource_catalog_id: descriptor.resource_catalog_id.as_str().to_string(),
            identity_keys: descriptor
                .identity_keys
                .into_iter()
                .map(SurfaceOperationIdentityKeyJson::from)
                .collect(),
            projection: descriptor
                .projection
                .into_iter()
                .map(SurfaceReadProjectionFieldJson::from)
                .collect(),
            index_keys: descriptor
                .index_keys
                .into_iter()
                .map(SurfaceReadIndexKeyJson::from)
                .collect(),
        }
    }
}

impl From<marrow_check::SurfaceReadOperationDescriptorKind> for SurfaceReadOperationKindJson {
    fn from(kind: marrow_check::SurfaceReadOperationDescriptorKind) -> Self {
        match kind {
            marrow_check::SurfaceReadOperationDescriptorKind::SingletonRead => Self::SingletonRead,
            marrow_check::SurfaceReadOperationDescriptorKind::PointRead => Self::PointRead,
            marrow_check::SurfaceReadOperationDescriptorKind::PagedRootCollection => {
                Self::PagedRootCollection
            }
            marrow_check::SurfaceReadOperationDescriptorKind::PagedIndexCollection {
                index_catalog_id,
                exact_key_count,
                identity_key_count,
            } => Self::PagedIndexCollection {
                index_catalog_id: index_catalog_id.as_str().to_string(),
                exact_key_count,
                identity_key_count,
            },
            marrow_check::SurfaceReadOperationDescriptorKind::UniqueIndexLookup {
                index_catalog_id,
                key_count,
            } => Self::UniqueIndexLookup {
                index_catalog_id: index_catalog_id.as_str().to_string(),
                key_count,
            },
        }
    }
}

impl From<marrow_check::SurfaceOperationIdentityKey> for SurfaceOperationIdentityKeyJson {
    fn from(key: marrow_check::SurfaceOperationIdentityKey) -> Self {
        Self {
            render_label: key.render_label,
            value: SurfaceOperationValueShapeJson::from(key.value),
        }
    }
}

impl From<marrow_check::SurfaceReadOperationProjectionField> for SurfaceReadProjectionFieldJson {
    fn from(field: marrow_check::SurfaceReadOperationProjectionField) -> Self {
        Self {
            render_label: field.render_label,
            member_catalog_id: field.member_catalog_id.as_str().to_string(),
            required: field.required,
            value: SurfaceOperationValueShapeJson::from(field.value),
        }
    }
}

impl From<marrow_check::SurfaceReadOperationIndexKey> for SurfaceReadIndexKeyJson {
    fn from(key: marrow_check::SurfaceReadOperationIndexKey) -> Self {
        Self {
            render_label: key.render_label,
            source: SurfaceReadIndexKeySourceJson::from(key.source),
            value: SurfaceOperationValueShapeJson::from(key.value),
        }
    }
}

impl From<marrow_check::SurfaceReadOperationIndexKeySource> for SurfaceReadIndexKeySourceJson {
    fn from(source: marrow_check::SurfaceReadOperationIndexKeySource) -> Self {
        match source {
            marrow_check::SurfaceReadOperationIndexKeySource::IdentityKey => Self::IdentityKey,
            marrow_check::SurfaceReadOperationIndexKeySource::ResourceMember {
                member_catalog_id,
            } => Self::ResourceMember {
                member_catalog_id: member_catalog_id.as_str().to_string(),
            },
        }
    }
}

impl From<marrow_check::SurfaceOperationValueShape> for SurfaceOperationValueShapeJson {
    fn from(value: marrow_check::SurfaceOperationValueShape) -> Self {
        match value {
            marrow_check::SurfaceOperationValueShape::Scalar(scalar) => Self::Scalar {
                scalar: scalar.name().to_string(),
            },
            marrow_check::SurfaceOperationValueShape::Enum {
                render_name,
                enum_catalog_id,
                members,
            } => Self::Enum {
                render_name,
                enum_catalog_id: enum_catalog_id.as_str().to_string(),
                members: members
                    .into_iter()
                    .map(SurfaceOperationEnumMemberJson::from)
                    .collect(),
            },
            marrow_check::SurfaceOperationValueShape::Identity {
                store_name,
                store_catalog_id,
                arity,
                key_scalars,
            } => Self::Identity {
                store_name,
                store_catalog_id: store_catalog_id.as_str().to_string(),
                arity,
                key_scalars: key_scalars
                    .into_iter()
                    .map(|scalar| scalar.name().to_string())
                    .collect(),
            },
        }
    }
}

impl From<marrow_check::SurfaceOperationEnumMember> for SurfaceOperationEnumMemberJson {
    fn from(member: marrow_check::SurfaceOperationEnumMember) -> Self {
        Self {
            render_label: member.render_label,
            catalog_id: member.catalog_id.as_str().to_string(),
        }
    }
}

impl From<marrow_check::SurfaceUpdateOperationDescriptor> for SurfaceUpdateOperationDescriptorJson {
    fn from(descriptor: marrow_check::SurfaceUpdateOperationDescriptor) -> Self {
        Self {
            profile_version: descriptor.profile_version.to_string(),
            operation_tag: descriptor.operation_tag,
            kind: SurfaceUpdateOperationKindJson::from(descriptor.kind),
            patch_semantics: SurfaceUpdatePatchSemanticsJson::from(descriptor.patch_semantics),
            store_catalog_id: descriptor.store_catalog_id.as_str().to_string(),
            resource_catalog_id: descriptor.resource_catalog_id.as_str().to_string(),
            identity_keys: descriptor
                .identity_keys
                .into_iter()
                .map(SurfaceOperationIdentityKeyJson::from)
                .collect(),
            fields: descriptor
                .fields
                .into_iter()
                .map(SurfaceUpdateFieldDescriptorJson::from)
                .collect(),
        }
    }
}

impl From<marrow_check::SurfaceUpdateOperationDescriptorKind> for SurfaceUpdateOperationKindJson {
    fn from(kind: marrow_check::SurfaceUpdateOperationDescriptorKind) -> Self {
        match kind {
            marrow_check::SurfaceUpdateOperationDescriptorKind::SingletonUpdate => {
                Self::SingletonUpdate
            }
            marrow_check::SurfaceUpdateOperationDescriptorKind::PointUpdate => Self::PointUpdate,
        }
    }
}

impl From<marrow_check::SurfaceUpdatePatchSemantics> for SurfaceUpdatePatchSemanticsJson {
    fn from(semantics: marrow_check::SurfaceUpdatePatchSemantics) -> Self {
        match semantics {
            marrow_check::SurfaceUpdatePatchSemantics::NonEmptyPatch => Self::NonEmptyPatch,
        }
    }
}

impl From<marrow_check::SurfaceUpdateOperationField> for SurfaceUpdateFieldDescriptorJson {
    fn from(field: marrow_check::SurfaceUpdateOperationField) -> Self {
        Self {
            render_label: field.render_label,
            member_catalog_id: field.member_catalog_id.as_str().to_string(),
            backing_required: field.backing_required,
            value: SurfaceOperationValueShapeJson::from(field.value),
        }
    }
}

impl From<marrow_check::SurfaceCreateOperationDescriptor> for SurfaceCreateOperationDescriptorJson {
    fn from(descriptor: marrow_check::SurfaceCreateOperationDescriptor) -> Self {
        Self {
            profile_version: descriptor.profile_version.to_string(),
            operation_tag: descriptor.operation_tag,
            kind: SurfaceCreateOperationKindJson::from(descriptor.kind),
            body_semantics: SurfaceCreateBodySemanticsJson::from(descriptor.body_semantics),
            identity_policy: SurfaceCreateIdentityPolicyJson::from(descriptor.identity_policy),
            existence_semantics: SurfaceCreateExistenceSemanticsJson::from(
                descriptor.existence_semantics,
            ),
            store_catalog_id: descriptor.store_catalog_id.as_str().to_string(),
            resource_catalog_id: descriptor.resource_catalog_id.as_str().to_string(),
            identity_keys: descriptor
                .identity_keys
                .into_iter()
                .map(SurfaceOperationIdentityKeyJson::from)
                .collect(),
            fields: descriptor
                .fields
                .into_iter()
                .map(SurfaceCreateFieldDescriptorJson::from)
                .collect(),
            projection: descriptor
                .projection
                .into_iter()
                .map(SurfaceReadProjectionFieldJson::from)
                .collect(),
        }
    }
}

impl From<marrow_check::SurfaceCreateOperationDescriptorKind> for SurfaceCreateOperationKindJson {
    fn from(kind: marrow_check::SurfaceCreateOperationDescriptorKind) -> Self {
        match kind {
            marrow_check::SurfaceCreateOperationDescriptorKind::SingletonCreate => {
                Self::SingletonCreate
            }
            marrow_check::SurfaceCreateOperationDescriptorKind::PointCreate => Self::PointCreate,
        }
    }
}

impl From<marrow_check::SurfaceCreateBodySemantics> for SurfaceCreateBodySemanticsJson {
    fn from(semantics: marrow_check::SurfaceCreateBodySemantics) -> Self {
        match semantics {
            marrow_check::SurfaceCreateBodySemantics::ExactDeclaredBody => Self::ExactDeclaredBody,
        }
    }
}

impl From<marrow_check::SurfaceCreateIdentityPolicy> for SurfaceCreateIdentityPolicyJson {
    fn from(policy: marrow_check::SurfaceCreateIdentityPolicy) -> Self {
        match policy {
            marrow_check::SurfaceCreateIdentityPolicy::SingletonNoIdentity => {
                Self::SingletonNoIdentity
            }
            marrow_check::SurfaceCreateIdentityPolicy::ClientSuppliedIdentity => {
                Self::ClientSuppliedIdentity
            }
        }
    }
}

impl From<marrow_check::SurfaceCreateExistenceSemantics> for SurfaceCreateExistenceSemanticsJson {
    fn from(semantics: marrow_check::SurfaceCreateExistenceSemantics) -> Self {
        match semantics {
            marrow_check::SurfaceCreateExistenceSemantics::RejectExistingNoReplace => {
                Self::RejectExistingNoReplace
            }
        }
    }
}

impl From<marrow_check::SurfaceCreateOperationField> for SurfaceCreateFieldDescriptorJson {
    fn from(field: marrow_check::SurfaceCreateOperationField) -> Self {
        Self {
            render_label: field.render_label,
            member_catalog_id: field.member_catalog_id.as_str().to_string(),
            value: SurfaceOperationValueShapeJson::from(field.value),
        }
    }
}

impl From<marrow_check::SurfaceDeleteOperationDescriptor> for SurfaceDeleteOperationDescriptorJson {
    fn from(descriptor: marrow_check::SurfaceDeleteOperationDescriptor) -> Self {
        Self {
            profile_version: descriptor.profile_version.to_string(),
            operation_tag: descriptor.operation_tag,
            kind: SurfaceDeleteOperationKindJson::from(descriptor.kind),
            semantics: SurfaceDeleteSemanticsJson::from(descriptor.semantics),
            store_catalog_id: descriptor.store_catalog_id.as_str().to_string(),
            resource_catalog_id: descriptor.resource_catalog_id.as_str().to_string(),
            identity_keys: descriptor
                .identity_keys
                .into_iter()
                .map(SurfaceOperationIdentityKeyJson::from)
                .collect(),
        }
    }
}

impl From<marrow_check::SurfaceDeleteOperationDescriptorKind> for SurfaceDeleteOperationKindJson {
    fn from(kind: marrow_check::SurfaceDeleteOperationDescriptorKind) -> Self {
        match kind {
            marrow_check::SurfaceDeleteOperationDescriptorKind::SingletonDelete => {
                Self::SingletonDelete
            }
            marrow_check::SurfaceDeleteOperationDescriptorKind::PointDelete => Self::PointDelete,
        }
    }
}

impl From<marrow_check::SurfaceDeleteSemantics> for SurfaceDeleteSemanticsJson {
    fn from(semantics: marrow_check::SurfaceDeleteSemantics) -> Self {
        match semantics {
            marrow_check::SurfaceDeleteSemantics::RejectAbsentFullSubtree => {
                Self::RejectAbsentFullSubtree
            }
        }
    }
}

impl From<marrow_check::SurfaceActionOperationDescriptor> for SurfaceActionOperationDescriptorJson {
    fn from(descriptor: marrow_check::SurfaceActionOperationDescriptor) -> Self {
        Self {
            profile_version: descriptor.profile_version.to_string(),
            operation_tag: descriptor.operation_tag,
            alias: descriptor.alias,
            identity: SurfaceCallableIdentityJson::from(descriptor.identity),
            parameters: descriptor
                .parameters
                .into_iter()
                .map(SurfaceCallableParameterJson::from)
                .collect(),
            return_value: descriptor
                .return_value
                .map(SurfaceCallableArgumentShapeJson::from),
        }
    }
}

impl From<marrow_check::SurfaceComputedReadOperationDescriptor>
    for SurfaceComputedReadOperationDescriptorJson
{
    fn from(descriptor: marrow_check::SurfaceComputedReadOperationDescriptor) -> Self {
        Self {
            profile_version: descriptor.profile_version.to_string(),
            operation_tag: descriptor.operation_tag,
            alias: descriptor.alias,
            callable: SurfaceComputedReadCallableJson::from(descriptor.callable),
            cost_shape: SurfaceComputedReadCostShapeJson::from(descriptor.cost_shape),
        }
    }
}

impl From<marrow_check::EntryFunctionSurfaceDescriptor> for SurfaceComputedReadCallableJson {
    fn from(descriptor: marrow_check::EntryFunctionSurfaceDescriptor) -> Self {
        Self {
            identity: SurfaceCallableIdentityJson::from(descriptor.identity),
            parameters: descriptor
                .parameters
                .into_iter()
                .map(SurfaceCallableParameterJson::from)
                .collect(),
            result: SurfaceComputedReadResultJson::from(descriptor.result),
        }
    }
}

impl From<marrow_check::EntryResultShape> for SurfaceComputedReadResultJson {
    fn from(result: marrow_check::EntryResultShape) -> Self {
        let presence = if result.maybe_present() {
            SurfaceComputedReadPresenceJson::MaybePresent
        } else {
            SurfaceComputedReadPresenceJson::Always
        };
        let value = match result {
            marrow_check::EntryResultShape::Void => None,
            marrow_check::EntryResultShape::Present(shape)
            | marrow_check::EntryResultShape::Optional(shape) => {
                Some(SurfaceComputedReadValueShapeJson::from(shape))
            }
        };
        Self { presence, value }
    }
}

impl From<marrow_check::EntrySurfaceValueShape> for SurfaceComputedReadValueShapeJson {
    fn from(shape: marrow_check::EntrySurfaceValueShape) -> Self {
        match shape {
            marrow_check::EntrySurfaceValueShape::Scalar(scalar) => Self::Scalar {
                scalar: scalar.name().to_string(),
            },
            marrow_check::EntrySurfaceValueShape::Enum {
                render_label,
                catalog_id,
                members,
            } => Self::Enum {
                render_label,
                enum_catalog_id: catalog_id.as_str().to_string(),
                members: members
                    .into_iter()
                    .map(SurfaceCallableEnumMemberJson::from)
                    .collect(),
            },
            marrow_check::EntrySurfaceValueShape::Identity {
                render_label,
                store_catalog_id,
                keys,
            } => Self::Identity {
                render_label,
                store_catalog_id: store_catalog_id.as_str().to_string(),
                keys: keys
                    .into_iter()
                    .map(SurfaceCallableIdentityKeyJson::from)
                    .collect(),
            },
            marrow_check::EntrySurfaceValueShape::Sequence(element) => Self::Sequence {
                element: Box::new(SurfaceComputedReadValueShapeJson::from(*element)),
            },
            marrow_check::EntrySurfaceValueShape::Resource {
                render_label,
                resource_catalog_id,
                fields,
            } => Self::Resource {
                render_label,
                resource_catalog_id: resource_catalog_id.as_str().to_string(),
                fields: fields
                    .into_iter()
                    .map(SurfaceComputedReadResourceFieldJson::from)
                    .collect(),
            },
        }
    }
}

impl From<marrow_check::EntryResourceResultField> for SurfaceComputedReadResourceFieldJson {
    fn from(field: marrow_check::EntryResourceResultField) -> Self {
        Self {
            render_label: field.render_label,
            member_catalog_id: field.member_catalog_id.as_str().to_string(),
            required: field.required,
            value: SurfaceComputedReadValueShapeJson::from(field.shape),
        }
    }
}

impl From<marrow_check::SurfaceComputedReadCostShape> for SurfaceComputedReadCostShapeJson {
    fn from(shape: marrow_check::SurfaceComputedReadCostShape) -> Self {
        Self {
            work_shape: SurfaceComputedReadWorkShapeJson::from(shape.work_shape),
            point_reads: shape.point_reads,
            range_scans: shape.range_scans,
            writes: shape.writes,
            index_entry_touches: shape.index_entry_touches,
            commit_points: shape.commit_points,
        }
    }
}

impl From<marrow_check::WorkShapeClass> for SurfaceComputedReadWorkShapeJson {
    fn from(shape: marrow_check::WorkShapeClass) -> Self {
        match shape {
            marrow_check::WorkShapeClass::ComputeOnly => Self::ComputeOnly,
            marrow_check::WorkShapeClass::ReadOnly => Self::ReadOnly,
            marrow_check::WorkShapeClass::WritesSavedData => Self::WritesSavedData,
        }
    }
}

impl From<marrow_check::EntryIdentity> for SurfaceCallableIdentityJson {
    fn from(identity: marrow_check::EntryIdentity) -> Self {
        Self {
            requested_name: identity.requested_name,
            canonical_name: identity.canonical_name,
            entry_tag: identity.entry_tag,
            accepted_catalog_epoch: identity.accepted_catalog_epoch,
            source_digest: identity.source_digest,
            read_only_context_digest: identity.read_only_context_digest,
        }
    }
}

impl From<marrow_check::EntryParameter> for SurfaceCallableParameterJson {
    fn from(parameter: marrow_check::EntryParameter) -> Self {
        let presence = if parameter.shape.optional() {
            SurfaceCallableParameterPresenceJson::Optional
        } else {
            SurfaceCallableParameterPresenceJson::Required
        };
        Self {
            name: parameter.name,
            presence,
            shape: SurfaceCallableArgumentShapeJson::from(parameter.shape.into_shape()),
        }
    }
}

impl From<marrow_check::EntryArgumentShape> for SurfaceCallableArgumentShapeJson {
    fn from(shape: marrow_check::EntryArgumentShape) -> Self {
        match shape {
            marrow_check::EntryArgumentShape::Scalar(scalar) => Self::Scalar {
                scalar: scalar.name().to_string(),
            },
            marrow_check::EntryArgumentShape::Enum {
                render_label,
                catalog_id,
                members,
            } => Self::Enum {
                render_label,
                enum_catalog_id: catalog_id.as_str().to_string(),
                members: members
                    .into_iter()
                    .map(SurfaceCallableEnumMemberJson::from)
                    .collect(),
            },
            marrow_check::EntryArgumentShape::Identity {
                render_label,
                store_catalog_id,
                keys,
            } => Self::Identity {
                render_label,
                store_catalog_id: store_catalog_id.as_str().to_string(),
                keys: keys
                    .into_iter()
                    .map(SurfaceCallableIdentityKeyJson::from)
                    .collect(),
            },
            marrow_check::EntryArgumentShape::Sequence(element) => Self::Sequence {
                element: Box::new(SurfaceCallableArgumentShapeJson::from(*element)),
            },
            marrow_check::EntryArgumentShape::Unsupported => Self::Unsupported,
        }
    }
}

impl From<marrow_check::EntryEnumMember> for SurfaceCallableEnumMemberJson {
    fn from(member: marrow_check::EntryEnumMember) -> Self {
        Self {
            render_label: member.render_label,
            catalog_id: member.catalog_id.as_str().to_string(),
        }
    }
}

impl From<marrow_check::EntryIdentityKey> for SurfaceCallableIdentityKeyJson {
    fn from(key: marrow_check::EntryIdentityKey) -> Self {
        Self {
            render_label: key.render_label,
            scalar: key.scalar.name().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceCursorJson {
    pub operation_tag: String,
    pub store_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<u64>,
    pub catalog_digest: String,
    pub source_digest: String,
    pub engine_profile_digest: String,
    pub boundary: SurfaceCursorBoundaryJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceCursorBoundaryJson {
    RootIdentity {
        identity: SurfaceIdentityJson,
    },
    IndexIdentity {
        exact_keys: Vec<SurfaceArgumentJson>,
        identity: SurfaceIdentityJson,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceKeyJson {
    Int { value: String },
    Bool { value: bool },
    String { value: String },
    Date { days_since_epoch: i32 },
    Duration { nanos: String },
    Instant { nanos_since_epoch: String },
    Bytes { value_b64: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceArgumentJson {
    Int {
        value: String,
    },
    Bool {
        value: bool,
    },
    String {
        value: String,
    },
    Date {
        days_since_epoch: i32,
    },
    Duration {
        nanos: String,
    },
    Instant {
        nanos_since_epoch: String,
    },
    Bytes {
        value_b64: String,
    },
    Enum {
        enum_catalog_id: String,
        member_catalog_id: String,
    },
    Identity {
        store_catalog_id: String,
        keys: Vec<SurfaceKeyJson>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceValueJson {
    Int {
        value: String,
    },
    Bool {
        value: bool,
    },
    String {
        value: String,
    },
    Date {
        days_since_epoch: i32,
    },
    Duration {
        nanos: String,
    },
    Instant {
        nanos_since_epoch: String,
    },
    Decimal {
        value: String,
    },
    Bytes {
        value_b64: String,
    },
    Enum {
        enum_catalog_id: String,
        member_catalog_id: String,
        render_label: String,
    },
    Identity {
        store_catalog_id: String,
        keys: Vec<SurfaceKeyJson>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceActionValueJson {
    Int {
        value: String,
    },
    Bool {
        value: bool,
    },
    String {
        value: String,
    },
    Date {
        days_since_epoch: i32,
    },
    Duration {
        nanos: String,
    },
    Instant {
        nanos_since_epoch: String,
    },
    Decimal {
        value: String,
    },
    Bytes {
        value_b64: String,
    },
    Enum {
        enum_catalog_id: String,
        member_catalog_id: String,
        render_label: String,
    },
    Identity {
        store_catalog_id: String,
        keys: Vec<SurfaceKeyJson>,
    },
    Sequence {
        values: Vec<SurfaceActionValueJson>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceValueJsonError {
    UnsupportedValue,
}

pub(crate) fn surface_action_value_to_json(
    program: &marrow_check::CheckedProgram,
    value: &Value,
) -> Result<SurfaceActionValueJson, SurfaceValueJsonError> {
    Ok(match value {
        Value::Int(value) => SurfaceActionValueJson::Int {
            value: value.to_string(),
        },
        Value::Bool(value) => SurfaceActionValueJson::Bool { value: *value },
        Value::Str(value) => SurfaceActionValueJson::String {
            value: value.clone(),
        },
        Value::Date(value) => SurfaceActionValueJson::Date {
            days_since_epoch: *value,
        },
        Value::Duration(value) => SurfaceActionValueJson::Duration {
            nanos: value.to_string(),
        },
        Value::Instant(value) => SurfaceActionValueJson::Instant {
            nanos_since_epoch: value.to_string(),
        },
        Value::Decimal(value) => SurfaceActionValueJson::Decimal {
            value: value.to_text(),
        },
        Value::Bytes(value) => SurfaceActionValueJson::Bytes {
            value_b64: marrow_run::base64::encode(value),
        },
        Value::Enum(value) => SurfaceActionValueJson::Enum {
            enum_catalog_id: value.enum_catalog_id().to_string(),
            member_catalog_id: value.member_catalog_id().to_string(),
            render_label: value.render_label().to_string(),
        },
        Value::Identity(identity) => SurfaceActionValueJson::Identity {
            store_catalog_id: accepted_store_catalog_id(program, identity.root())?,
            keys: identity.keys().iter().map(SurfaceKeyJson::from).collect(),
        },
        Value::Sequence(items) => SurfaceActionValueJson::Sequence {
            values: items
                .values()
                .map(|item| surface_action_value_to_json(program, item))
                .collect::<Result<Vec<_>, _>>()?,
        },
        Value::Absent | Value::Resource(_) | Value::LocalTree(_) => {
            return Err(SurfaceValueJsonError::UnsupportedValue);
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceComputedReadValueJson {
    Int {
        value: String,
    },
    Bool {
        value: bool,
    },
    String {
        value: String,
    },
    Date {
        days_since_epoch: i32,
    },
    Duration {
        nanos: String,
    },
    Instant {
        nanos_since_epoch: String,
    },
    Decimal {
        value: String,
    },
    Bytes {
        value_b64: String,
    },
    Enum {
        enum_catalog_id: String,
        member_catalog_id: String,
        render_label: String,
    },
    Identity {
        store_catalog_id: String,
        keys: Vec<SurfaceKeyJson>,
    },
    Sequence {
        values: Vec<SurfaceComputedReadValueJson>,
    },
    Resource {
        resource_catalog_id: String,
        fields: Vec<SurfaceComputedReadFieldValueJson>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceComputedReadFieldValueJson {
    pub render_label: String,
    pub member_catalog_id: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<SurfaceComputedReadValueJson>,
}

pub(crate) fn surface_computed_read_value_to_json(
    program: &marrow_check::CheckedProgram,
    shape: Option<&marrow_check::EntrySurfaceValueShape>,
    value: &Value,
) -> Result<SurfaceComputedReadValueJson, SurfaceValueJsonError> {
    let Some(shape) = shape else {
        return Err(SurfaceValueJsonError::UnsupportedValue);
    };
    match (shape, value) {
        (marrow_check::EntrySurfaceValueShape::Scalar(_), value) => {
            computed_read_scalar_value_to_json(value)
        }
        (marrow_check::EntrySurfaceValueShape::Enum { .. }, Value::Enum(value)) => {
            Ok(SurfaceComputedReadValueJson::Enum {
                enum_catalog_id: value.enum_catalog_id().to_string(),
                member_catalog_id: value.member_catalog_id().to_string(),
                render_label: value.render_label().to_string(),
            })
        }
        (marrow_check::EntrySurfaceValueShape::Identity { .. }, Value::Identity(identity)) => {
            Ok(SurfaceComputedReadValueJson::Identity {
                store_catalog_id: accepted_store_catalog_id(program, identity.root())?,
                keys: identity.keys().iter().map(SurfaceKeyJson::from).collect(),
            })
        }
        (marrow_check::EntrySurfaceValueShape::Sequence(element), Value::Sequence(items)) => {
            Ok(SurfaceComputedReadValueJson::Sequence {
                values: items
                    .values()
                    .map(|item| surface_computed_read_value_to_json(program, Some(element), item))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
        (
            marrow_check::EntrySurfaceValueShape::Resource {
                resource_catalog_id,
                fields,
                ..
            },
            Value::Resource(values),
        ) => Ok(SurfaceComputedReadValueJson::Resource {
            resource_catalog_id: resource_catalog_id.as_str().to_string(),
            fields: fields
                .iter()
                .map(|field| computed_read_resource_field_to_json(program, field, values))
                .collect::<Result<Vec<_>, _>>()?,
        }),
        _ => Err(SurfaceValueJsonError::UnsupportedValue),
    }
}

fn computed_read_scalar_value_to_json(
    value: &Value,
) -> Result<SurfaceComputedReadValueJson, SurfaceValueJsonError> {
    Ok(match value {
        Value::Int(value) => SurfaceComputedReadValueJson::Int {
            value: value.to_string(),
        },
        Value::Bool(value) => SurfaceComputedReadValueJson::Bool { value: *value },
        Value::Str(value) => SurfaceComputedReadValueJson::String {
            value: value.clone(),
        },
        Value::Date(value) => SurfaceComputedReadValueJson::Date {
            days_since_epoch: *value,
        },
        Value::Duration(value) => SurfaceComputedReadValueJson::Duration {
            nanos: value.to_string(),
        },
        Value::Instant(value) => SurfaceComputedReadValueJson::Instant {
            nanos_since_epoch: value.to_string(),
        },
        Value::Decimal(value) => SurfaceComputedReadValueJson::Decimal {
            value: value.to_text(),
        },
        Value::Bytes(value) => SurfaceComputedReadValueJson::Bytes {
            value_b64: marrow_run::base64::encode(value),
        },
        Value::Absent
        | Value::Enum(_)
        | Value::Identity(_)
        | Value::Sequence(_)
        | Value::Resource(_)
        | Value::LocalTree(_) => {
            return Err(SurfaceValueJsonError::UnsupportedValue);
        }
    })
}

fn computed_read_resource_field_to_json(
    program: &marrow_check::CheckedProgram,
    field: &marrow_check::EntryResourceResultField,
    values: &[(String, Value)],
) -> Result<SurfaceComputedReadFieldValueJson, SurfaceValueJsonError> {
    let value = values
        .iter()
        .find(|(name, _)| name == &field.render_label)
        .map(|(_, value)| value);
    if field.required && value.is_none() {
        return Err(SurfaceValueJsonError::UnsupportedValue);
    }
    Ok(SurfaceComputedReadFieldValueJson {
        render_label: field.render_label.clone(),
        member_catalog_id: field.member_catalog_id.as_str().to_string(),
        required: field.required,
        value: value
            .map(|value| surface_computed_read_value_to_json(program, Some(&field.shape), value))
            .transpose()?,
    })
}

fn accepted_store_catalog_id(
    program: &marrow_check::CheckedProgram,
    root: &str,
) -> Result<String, SurfaceValueJsonError> {
    let Some(store) = program.facts.store_by_root(root) else {
        return Err(SurfaceValueJsonError::UnsupportedValue);
    };
    let Some(catalog_id) = store.catalog_id.as_deref() else {
        return Err(SurfaceValueJsonError::UnsupportedValue);
    };
    let accepted = program
        .catalog
        .accepted_entries
        .iter()
        .any(|entry| entry.stable_id == catalog_id);
    if accepted {
        Ok(catalog_id.to_string())
    } else {
        Err(SurfaceValueJsonError::UnsupportedValue)
    }
}

impl From<&SurfaceReadIdentity> for SurfaceIdentityJson {
    fn from(identity: &SurfaceReadIdentity) -> Self {
        Self {
            store_catalog_id: identity.store_catalog_id.as_str().to_string(),
            keys: identity.keys.iter().map(SurfaceKeyJson::from).collect(),
        }
    }
}

impl From<&SurfaceReadField> for SurfaceFieldJson {
    fn from(field: &SurfaceReadField) -> Self {
        Self {
            catalog_id: field.catalog_id.as_str().to_string(),
            render_label: field.render_label.clone(),
            value: field.value.as_ref().map(SurfaceValueJson::from),
        }
    }
}

impl From<&SurfaceReadRecord> for SurfaceRecordJson {
    fn from(record: &SurfaceReadRecord) -> Self {
        Self {
            identity: record.identity.as_ref().map(SurfaceIdentityJson::from),
            fields: record.fields.iter().map(SurfaceFieldJson::from).collect(),
        }
    }
}

impl From<&SavedKey> for SurfaceKeyJson {
    fn from(key: &SavedKey) -> Self {
        match key {
            SavedKey::Int(value) => Self::Int {
                value: value.to_string(),
            },
            SavedKey::Bool(value) => Self::Bool { value: *value },
            SavedKey::Str(value) => Self::String {
                value: value.clone(),
            },
            SavedKey::Date(value) => Self::Date {
                days_since_epoch: *value,
            },
            SavedKey::Duration(value) => Self::Duration {
                nanos: value.to_string(),
            },
            SavedKey::Instant(value) => Self::Instant {
                nanos_since_epoch: value.to_string(),
            },
            SavedKey::Bytes(value) => Self::Bytes {
                value_b64: marrow_run::base64::encode(value),
            },
        }
    }
}

impl From<&SurfaceValue> for SurfaceValueJson {
    fn from(value: &SurfaceValue) -> Self {
        match value {
            SurfaceValue::Int(value) => Self::Int {
                value: value.to_string(),
            },
            SurfaceValue::Bool(value) => Self::Bool { value: *value },
            SurfaceValue::Str(value) => Self::String {
                value: value.clone(),
            },
            SurfaceValue::Date(value) => Self::Date {
                days_since_epoch: *value,
            },
            SurfaceValue::Duration(value) => Self::Duration {
                nanos: value.to_string(),
            },
            SurfaceValue::Instant(value) => Self::Instant {
                nanos_since_epoch: value.to_string(),
            },
            SurfaceValue::Decimal(value) => Self::Decimal {
                value: value.to_text(),
            },
            SurfaceValue::Bytes(value) => Self::Bytes {
                value_b64: marrow_run::base64::encode(value),
            },
            SurfaceValue::Enum(value) => SurfaceValueJson::from(value),
            SurfaceValue::Identity(value) => Self::Identity {
                store_catalog_id: value.store_catalog_id.as_str().to_string(),
                keys: value.keys.iter().map(SurfaceKeyJson::from).collect(),
            },
        }
    }
}

impl From<&SurfaceEnumValue> for SurfaceValueJson {
    fn from(value: &SurfaceEnumValue) -> Self {
        Self::Enum {
            enum_catalog_id: value.enum_catalog_id.as_str().to_string(),
            member_catalog_id: value.member_catalog_id.as_str().to_string(),
            render_label: value.render_label.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use marrow_check::{
        CheckedProgram, CheckedRuntimeProgram, ENTRY_PROTOCOL_TAG_VERSION, EntryDescriptor,
        ProjectConfig, StoreBackend, StoreConfig, SurfaceActionOperationDescriptor,
        SurfaceComputedReadOperationDescriptor, SurfaceId, SurfaceReadOperationKind,
        SurfaceUpdateOperationDescriptor, check_project,
    };
    use marrow_run::{
        Host, ProjectOpen, ProjectSession, ProjectSurfaceReadSession, ProjectSurfaceSession,
        SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_ACTION, SURFACE_CONFLICT, SURFACE_CURSOR,
        SURFACE_INVALID_DATA, SURFACE_LIMIT, SURFACE_MAX_VALUE_BYTES, SURFACE_REQUEST,
        SURFACE_STALE_CURSOR, SessionEntry, SurfaceActionInvocation, SurfaceCollectionRead,
        SurfaceEnumValue, SurfaceNodeRead, SurfaceReadError, SurfaceReadField, SurfaceReadIdentity,
        SurfaceReadInput, SurfaceReadOperationRef, SurfaceReadRecord, SurfaceUpdate, SurfaceValue,
        entry_arguments_from_json,
    };
    use marrow_store::Decimal;
    use marrow_store::cell::CatalogId;
    use marrow_store::key::{SavedKey, encode_identity_index_key, encode_identity_payload};
    use marrow_store::tree::{
        DataPathSegment, StoreUid, TreeEnumMember, TreeStore, encode_tree_enum_member,
    };
    use marrow_store::value::{
        SUPPORTED_DATE_MIN_DAYS, SUPPORTED_INSTANT_MAX_NANOS, SavedValue, encode_value,
    };
    use serde_json::json;

    use crate::surface::{
        SURFACE_CLIENT_DIGEST_PREFIX, SURFACE_CLIENT_DO_NOT_EDIT, SURFACE_CLIENT_PROFILE_PREFIX,
        SURFACE_OPERATION_PROFILE_VERSION, SurfaceAbiJson, SurfaceActionRequestJson,
        SurfaceActionResultJson, SurfaceActionValueJson, SurfaceArgumentJson,
        SurfaceCallableArgumentShapeJson, SurfaceCallableParameterPresenceJson,
        SurfaceCatalogStatusJson, SurfaceClientCursorProfile, SurfaceClientRenderErrorKind,
        SurfaceComputedReadFieldValueJson, SurfaceComputedReadPresenceJson,
        SurfaceComputedReadRequestJson, SurfaceComputedReadValueJson,
        SurfaceComputedReadValueShapeJson, SurfaceCreateFieldJson, SurfaceCreateOperationKindJson,
        SurfaceCursorBoundaryJson, SurfaceCursorJson, SurfaceDeleteOperationKindJson,
        SurfaceEmptyRequestJson, SurfaceIdentityJson, SurfaceKeyJson, SurfaceOperationCatalog,
        SurfaceOperationCatalogErrorKind, SurfaceOperationErrorJson, SurfaceOperationKind,
        SurfaceOperationRequestBodyJson, SurfaceOperationRequestJson, SurfaceOperationResultJson,
        SurfacePageJson, SurfacePageRequestJson, SurfacePointCreateRequestJson,
        SurfacePointDeleteRequestJson, SurfacePointRequestJson, SurfacePointUpdateRequestJson,
        SurfaceReadOperationKindJson, SurfaceRecordJson, SurfaceRouteBindingErrorKind,
        SurfaceRouteBindings, SurfaceRouteManifestJson, SurfaceRouteMethodJson,
        SurfaceRouteRequestJson, SurfaceSingletonUpdateRequestJson, SurfaceUniqueLookupRequestJson,
        SurfaceUpdateFieldJson, SurfaceValueJson, SurfaceWriteValueJson, render_typescript_client,
        render_typescript_client_with_cursor_profile, surface_abi_digest, surface_client_digest,
        surface_client_digest_with_cursor_profile, surface_client_header,
        surface_client_header_digest,
    };

    static TEMP_PROJECT_COUNTER: AtomicU64 = AtomicU64::new(0);

    const SURFACE_WITH_ENUM_IDENTITY_INDEX: &str = "\
enum Status
    draft
    published

resource Author
    required name: string
store ^authors(id: int): Author

resource Book
    required title: string
    required status: Status
    required author: Id(^authors)
store ^books(id: int): Book
    index byStatusAuthor(status, author, id)

surface Books from ^books
    fields title
    collection ^books as list
    collection ^books.byStatusAuthor as byStatusAuthor
";

    const SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX: &str = "\
enum Status
    draft
    published

resource Author
    required name: string
store ^authors(id: int): Author

resource Book
    required title: string
    required privateCode: string
    required status: Status
    required author: Id(^authors)
store ^books(id: int): Book
    index byStatusAuthor(status, author, id)

surface Books from ^books
    fields title, status, author
    update status, author
    collection ^books.byStatusAuthor as byStatusAuthor
";

    const SINGLETON_UPDATE_SURFACE: &str = "\
resource Settings
    required theme: string
    mode: string
store ^settings: Settings

surface SettingsSurface from ^settings
    fields theme, mode
    update mode
";

    const SINGLETON_READ_DELETE_SURFACE: &str = "\
resource Settings
    required theme: string
    mode: string
store ^settings: Settings

surface SettingsSurface from ^settings
    fields theme, mode
    delete
";

    const TEMPORAL_UPDATE_SURFACE: &str = "\
resource Event
    required title: string
    required day: date
    required seenAt: instant
store ^events(id: int): Event

surface Events from ^events
    fields title, day, seenAt
    update day, seenAt
";

    const BYTES_INDEX_SURFACE: &str = "\
resource File
    required name: string
    required fingerprint: bytes
store ^files(id: int): File
    index byFingerprint(fingerprint, id)

surface Files from ^files
    fields name
    collection ^files.byFingerprint as byFingerprint
";

    const SURFACE_WITH_UNIQUE_INDEX: &str = "\
resource Book
    required title: string
    required isbn: string
store ^books(id: int): Book
    index byIsbn(isbn) unique

surface Books from ^books
    fields title, isbn
    collection ^books.byIsbn as byIsbn
";

    const DUPLICATE_READ_TAG_SURFACES: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title

surface Library from ^books
    fields title

resource Note
    required text: string
store ^notes(id: int): Note

surface Notes from ^notes
    fields text
";

    const DUPLICATE_UPDATE_TAG_SURFACES: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    update title

surface Library from ^books
    fields title
    update title

resource Note
    required text: string
store ^notes(id: int): Note

surface Notes from ^notes
    fields text
    update text
";

    const SURFACE_ACTIONS: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn addBook(title: string): string
    return title

surface Books from ^books
    fields title
    action addBook
";

    const SURFACE_ACTION_OPTIONAL_PARAM: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn addBook(title: string, note: string?): string
    return title

surface Books from ^books
    fields title
    action addBook
";

    const SURFACE_ACTION_UPDATE: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn addBook(title: string): string
    return title

surface Books from ^books
    fields title
    update title
    action addBook
";

    const SURFACE_PAGE_HELPERS: &str = "\
resource Book
    required title: string
    required author: string
store ^books(id: int): Book
    index byAuthor(author, id)

surface Books from ^books
    fields title, author
    collection ^books as all
    collection ^books.byAuthor as byAuthor
";

    const SURFACE_PAGE_HELPER_COLLISION: &str = "\
resource Book
    required title: string
    required author: string
store ^books(id: int): Book
    index byAuthor(author, id)

pub fn allPages(): string
    return \"ok\"

surface Books from ^books
    fields title, author
    collection ^books as all
    action allPages
";

    const SURFACE_PAGE_HELPER_RESERVED_PARAMETERS: &str = "\
resource Book
    required title: string
    required cursor: string
    required options: string
    required transport: string
store ^books(id: int): Book
    index byHelperScope(cursor, options, transport, id)

surface Books from ^books
    fields title, cursor, options, transport
    collection ^books.byHelperScope as byHelperScope
";

    const SURFACE_COMPUTED_READ: &str = "\
resource BookPage
    required title: string
resource Book
    required title: string
store ^books(id: int): Book

pub fn bookPage(id: Id(^books)): BookPage?
    return BookPage(title: ^books(id).title ?? \"\")

surface Books from ^books
    fields title
    read bookPage as page
";

    const SURFACE_CREATE_DELETE: &str = "\
resource Book
    required title: string
    required author: string
store ^books(id: int): Book

surface Books from ^books
    fields title, author
    create title, author
    delete
";

    const DUPLICATE_ACTION_TAG_SURFACES: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn addBook()
    return

surface Books from ^books
    fields title
    action addBook

surface Library from ^books
    fields title
    action addBook

resource Note
    required text: string
store ^notes(id: int): Note

pub fn addNote()
    return

surface Notes from ^notes
    fields text
    action addNote
";

    const SOURCE_ONLY_UPDATE_SURFACE: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    update title
";

    const PROJECT_READ_SURFACE: &str = "\
module shelf

resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    collection ^books as list

pub fn seed()
    var first: Book
    first.title = \"Dune\"
    var second: Book
    second.title = \"Dune Messiah\"
    transaction
        ^books(1) = first
        ^books(2) = second
";

    const PROJECT_UPDATE_SURFACE: &str = "\
module shelf

resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    update title
    collection ^books as list

pub fn seed()
    var first: Book
    first.title = \"Dune\"
    transaction
        ^books(1) = first
";

    const PROJECT_CREATE_DELETE_SURFACE: &str = "\
module shelf

resource Book
    required title: string
    required author: string
store ^books(id: int): Book

surface Books from ^books
    fields title, author
    create title, author
    delete

pub fn seed()
    return
";

    const PROJECT_COLLECTION_UPDATE_SURFACE: &str = "\
module shelf

resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    update title
    collection ^books as list

pub fn seed()
    var first: Book
    first.title = \"Dune\"
    var second: Book
    second.title = \"Dune Messiah\"
    transaction
        ^books(1) = first
        ^books(2) = second
";

    const PROJECT_ACTION_SURFACE: &str = "\
module shelf

resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    collection ^books as list
    action renameBook as rename
    action currentBook as current
    action failRename as fail

pub fn seed()
    var first: Book
    first.title = \"Dune\"
    transaction
        ^books(1) = first

pub fn renameBook(id: int, title: string): string
    transaction
        ^books(id).title = title
    return title

pub fn stealthRename(id: int, title: string): string
    transaction
        ^books(id).title = title
    return title

pub fn currentBook(): Id(^books)
    return Id(^books, 1)

pub fn failRename()
    throw Error(code: \"test.fail\", message: \"boom\")
";

    const PROJECT_COMPUTED_READ_SURFACE: &str = "\
module shelf

resource BookPage
    required title: string

resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    read bookPage as page

pub fn seed()
    var first: Book
    first.title = \"Dune\"
    transaction
        ^books(1) = first

pub fn bookPage(id: int): BookPage?
    return BookPage(title: ^books(id).title ?? \"\")
";

    const PROJECT_HOST_ACTION_SURFACE: &str = "\
module shelf

resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    action currentInstant as now

pub fn seed()
    var first: Book
    first.title = \"Dune\"
    transaction
        ^books(1) = first

pub fn currentInstant(): instant
    return std::clock::now()
";

    const PROJECT_STEALTH_ACTION_SURFACE: &str = "\
module shelf

resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    collection ^books as list
    action renameBook as rename
    action stealthRename as stealth
    action failRename as fail

pub fn seed()
    var first: Book
    first.title = \"Dune\"
    transaction
        ^books(1) = first

pub fn renameBook(id: int, title: string): string
    transaction
        ^books(id).title = title
    return title

pub fn stealthRename(id: int, title: string): string
    transaction
        ^books(id).title = title
    return title

pub fn failRename()
    throw Error(code: \"test.fail\", message: \"boom\")
";

    const PROJECT_PRIVATE_BACKING_SURFACE: &str = "\
module shelf

resource Book
    required title: string
    required privateCode: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    update title

pub fn seed()
    var first: Book
    first.title = \"Dune\"
    first.privateCode = \"internal\"
    transaction
        ^books(1) = first
";

    const PROJECT_UNIQUE_UPDATE_SURFACE: &str = "\
module shelf

resource Book
    required title: string
    required isbn: string
store ^books(id: int): Book
    index byIsbn(isbn) unique

surface Books from ^books
    fields title, isbn
    update isbn

pub fn seed()
    var first: Book
    first.title = \"Dune\"
    first.isbn = \"isbn-a1\"
    var second: Book
    second.title = \"Dune Messiah\"
    second.isbn = \"isbn-a2\"
    transaction
        ^books(1) = first
        ^books(2) = second
";

    struct TempProject {
        path: PathBuf,
    }

    impl TempProject {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let counter = TEMP_PROJECT_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("{prefix}-{}-{nonce}-{counter}", std::process::id()));
            fs::create_dir(&path).expect("create temp project");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_native_project(root: &TempProject, source: &str) {
        fs::write(
            root.path().join("marrow.json"),
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        )
        .expect("write marrow.json");
        let source_dir = root.path().join("src");
        fs::create_dir(&source_dir).expect("create source dir");
        fs::write(source_dir.join("shelf.mw"), source).expect("write source");
    }

    fn seed_project(root: &TempProject, entry: &str) {
        let session = ProjectSession::open(
            root.path(),
            ProjectOpen::run().with_entry_override(entry.to_string()),
        )
        .expect("open project session");
        let host = Host::new();
        let mut output = String::new();
        session
            .invoke(SessionEntry::new(entry, &host, &mut output))
            .expect("invoke seed entry");
        assert_eq!(output, "");
    }

    fn catalog_id(suffix: u8) -> CatalogId {
        CatalogId::new(format!("cat_{suffix:032x}")).expect("catalog id")
    }

    fn checked_surface_program(source: &str) -> (CheckedProgram, CheckedRuntimeProgram) {
        let root = TempProject::new("marrow-json-surface-test");
        let source_dir = root.path().join("src");
        fs::create_dir(&source_dir).expect("create source dir");
        fs::write(
            source_dir.join("test.mw"),
            format!("module test\n\n{source}"),
        )
        .expect("write source");
        let config = ProjectConfig {
            source_roots: vec!["src".into()],
            default_entry: None,
            store: StoreConfig {
                backend: StoreBackend::Native,
                data_dir: Some(".marrow/data".into()),
            },
            tests: Vec::new(),
            client: None,
        };
        let (report, program) = check_project(root.path(), &config).expect("check project");
        assert!(
            !report.has_errors(),
            "surface fixture must check cleanly: {:#?}",
            report.diagnostics
        );
        let program = commit_catalog(root.path(), &config, program);
        let runtime = program.runtime();
        (program, runtime)
    }

    fn checked_source_only_surface_program(source: &str) -> CheckedProgram {
        let root = TempProject::new("marrow-json-source-only-surface-test");
        let source_dir = root.path().join("src");
        fs::create_dir(&source_dir).expect("create source dir");
        fs::write(
            source_dir.join("test.mw"),
            format!("module test\n\n{source}"),
        )
        .expect("write source");
        let config = ProjectConfig {
            source_roots: vec!["src".into()],
            default_entry: None,
            store: StoreConfig {
                backend: StoreBackend::Native,
                data_dir: Some(".marrow/data".into()),
            },
            tests: Vec::new(),
            client: None,
        };
        let (report, program) = check_project(root.path(), &config).expect("check project");
        assert!(
            !report.has_errors(),
            "source-only surface fixture must check cleanly: {:#?}",
            report.diagnostics
        );
        program
    }

    fn commit_catalog(
        root: &Path,
        config: &ProjectConfig,
        program: CheckedProgram,
    ) -> CheckedProgram {
        let store = TreeStore::memory();
        if !marrow_run::evolution::commit_catalog_baseline(&store, &program)
            .expect("commit catalog baseline")
        {
            return program;
        }
        let accepted = store
            .read_catalog_snapshot()
            .expect("read catalog snapshot");
        let (report, program) =
            marrow_check::check_project_with_catalog(root, config, accepted.as_ref())
                .expect("re-check project with catalog");
        assert!(
            !report.has_errors(),
            "committed fixture must check cleanly: {:#?}",
            report.diagnostics
        );
        program
    }

    fn admitted_store(program: &CheckedProgram) -> TreeStore {
        let store = TreeStore::memory();
        marrow_run::evolution::commit_catalog_baseline(&store, program)
            .expect("commit surface test catalog baseline");
        store
            .write_store_uid(&StoreUid::from_entropy_bytes([7; 16]))
            .expect("write surface test store uid");
        store
    }

    fn surface_id(program: &CheckedProgram, name: &str) -> SurfaceId {
        program
            .facts
            .surfaces()
            .iter()
            .find(|surface| surface.name == name)
            .unwrap_or_else(|| panic!("surface `{name}` is present"))
            .id
    }

    fn operation_ref(
        program: &CheckedProgram,
        surface: SurfaceId,
        matches_kind: impl Fn(&SurfaceReadOperationKind) -> bool,
    ) -> SurfaceReadOperationRef {
        let surface = program.facts.surface(surface);
        let ordinal = surface
            .read_operations
            .iter()
            .position(|operation| matches_kind(&operation.kind))
            .expect("surface operation is present");
        SurfaceReadOperationRef {
            surface: surface.id,
            ordinal,
        }
    }

    fn index_collection_ref(
        program: &CheckedProgram,
        surface: SurfaceId,
        index_name: &str,
    ) -> SurfaceReadOperationRef {
        operation_ref(program, surface, |kind| match *kind {
            SurfaceReadOperationKind::PagedIndexCollection { index, .. }
            | SurfaceReadOperationKind::UniqueIndexLookup { index, .. } => {
                program.facts.store_index(index).name == index_name
            }
            _ => false,
        })
    }

    fn read_operation_tag(
        program: &CheckedProgram,
        operation_ref: SurfaceReadOperationRef,
    ) -> String {
        program.facts.surface(operation_ref.surface).read_operations[operation_ref.ordinal]
            .operation_tag
            .clone()
            .expect("stable read operation tag")
    }

    fn read_operation_tag_matching(
        program: &CheckedProgram,
        surface: SurfaceId,
        matches_kind: impl Fn(&SurfaceReadOperationKind) -> bool,
    ) -> String {
        read_operation_tag(program, operation_ref(program, surface, matches_kind))
    }

    fn update_operation_tag(program: &CheckedProgram, surface_name: &str) -> String {
        SurfaceAbiJson::from_program(program)
            .surfaces
            .into_iter()
            .find(|surface| surface.name == surface_name)
            .and_then(|surface| surface.update.map(|update| update.operation_tag))
            .unwrap_or_else(|| panic!("surface `{surface_name}` exposes an update tag"))
    }

    fn create_operation_tag(program: &CheckedProgram, surface_name: &str) -> String {
        SurfaceAbiJson::from_program(program)
            .surfaces
            .into_iter()
            .find(|surface| surface.name == surface_name)
            .and_then(|surface| surface.create.map(|create| create.operation_tag))
            .unwrap_or_else(|| panic!("surface `{surface_name}` exposes a create tag"))
    }

    fn delete_operation_tag(program: &CheckedProgram, surface_name: &str) -> String {
        SurfaceAbiJson::from_program(program)
            .surfaces
            .into_iter()
            .find(|surface| surface.name == surface_name)
            .and_then(|surface| surface.delete.map(|delete| delete.operation_tag))
            .unwrap_or_else(|| panic!("surface `{surface_name}` exposes a delete tag"))
    }

    fn checker_update_operation_tag(program: &CheckedProgram, surface: SurfaceId) -> String {
        SurfaceUpdateOperationDescriptor::from_surface(program, program.facts.surface(surface))
            .map(|descriptor| descriptor.operation_tag)
            .expect("stable update operation tag")
    }

    fn checker_action_operation_tag(
        program: &CheckedProgram,
        surface_name: &str,
        alias: &str,
    ) -> String {
        let surface = program
            .facts
            .surfaces()
            .iter()
            .find(|surface| surface.name == surface_name)
            .unwrap_or_else(|| panic!("surface `{surface_name}` is present"));
        let action = surface
            .actions
            .iter()
            .find(|action| action.alias == alias)
            .unwrap_or_else(|| panic!("surface `{surface_name}` has action `{alias}`"));
        SurfaceActionOperationDescriptor::from_action(program, surface, action)
            .map(|action| action.operation_tag)
            .unwrap_or_else(|| {
                panic!("surface `{surface_name}` exposes action `{alias}` operation tag")
            })
    }

    fn checker_computed_read_operation_tag(
        program: &CheckedProgram,
        surface_name: &str,
        alias: &str,
    ) -> String {
        let surface = program
            .facts
            .surfaces()
            .iter()
            .find(|surface| surface.name == surface_name)
            .unwrap_or_else(|| panic!("surface `{surface_name}` is present"));
        let computed_read = surface
            .computed_reads
            .iter()
            .find(|computed_read| computed_read.alias == alias)
            .unwrap_or_else(|| panic!("surface `{surface_name}` has computed read `{alias}`"));
        SurfaceComputedReadOperationDescriptor::from_computed_read(program, surface, computed_read)
            .map(|computed_read| computed_read.operation_tag)
            .unwrap_or_else(|| {
                panic!("surface `{surface_name}` exposes computed read `{alias}` operation tag")
            })
    }

    fn book_by_status_author_read<'a>(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
    ) -> SurfaceCollectionRead<'a> {
        let surface = surface_id(program, "Books");
        SurfaceCollectionRead::admit(
            program,
            store,
            index_collection_ref(program, surface, "byStatusAuthor"),
        )
        .expect("admit index collection")
    }

    fn store_catalog_id(program: &CheckedRuntimeProgram, root: &str) -> CatalogId {
        let store = program
            .facts()
            .stores()
            .iter()
            .find(|store| store.root == root)
            .unwrap_or_else(|| panic!("store `{root}` is present"));
        accepted_catalog_id(&store.catalog_id)
    }

    fn index_catalog_id(program: &CheckedRuntimeProgram, root: &str, name: &str) -> CatalogId {
        let store = program
            .facts()
            .stores()
            .iter()
            .find(|store| store.root == root)
            .unwrap_or_else(|| panic!("store `{root}` is present"));
        let index = program
            .facts()
            .store_indexes()
            .iter()
            .find(|index| index.store == store.id && index.name == name)
            .unwrap_or_else(|| panic!("index `{name}` is present"));
        accepted_catalog_id(&index.catalog_id)
    }

    fn enum_catalog_id(program: &CheckedRuntimeProgram, name: &str) -> CatalogId {
        let enum_fact = program
            .facts()
            .enums()
            .iter()
            .find(|enum_fact| enum_fact.name == name)
            .unwrap_or_else(|| panic!("enum `{name}` is present"));
        accepted_catalog_id(&enum_fact.catalog_id)
    }

    fn enum_member_catalog_id(
        program: &CheckedRuntimeProgram,
        enum_name: &str,
        member_name: &str,
    ) -> CatalogId {
        let enum_fact = program
            .facts()
            .enums()
            .iter()
            .find(|enum_fact| enum_fact.name == enum_name)
            .unwrap_or_else(|| panic!("enum `{enum_name}` is present"));
        let member = program
            .facts()
            .enum_members()
            .iter()
            .find(|member| member.enum_id == enum_fact.id && member.name == member_name)
            .unwrap_or_else(|| panic!("enum member `{enum_name}::{member_name}` is present"));
        accepted_catalog_id(&member.catalog_id)
    }

    fn data_path(
        program: &CheckedRuntimeProgram,
        root: &str,
        members: &[&str],
    ) -> Vec<DataPathSegment> {
        let store = program
            .facts()
            .stores()
            .iter()
            .find(|store| store.root == root)
            .unwrap_or_else(|| panic!("store `{root}` is present"));
        let mut parent = None;
        let mut path = Vec::new();
        for name in members {
            let member = program
                .facts()
                .resource_members()
                .iter()
                .find(|member| {
                    member.resource == store.resource
                        && member.parent == parent
                        && member.name == *name
                })
                .unwrap_or_else(|| panic!("member `{name}` is present"));
            path.push(DataPathSegment::Member(accepted_catalog_id(
                &member.catalog_id,
            )));
            parent = Some(member.id);
        }
        path
    }

    fn field_catalog_id(program: &CheckedRuntimeProgram, root: &str, member: &str) -> CatalogId {
        match data_path(program, root, &[member]).as_slice() {
            [DataPathSegment::Member(catalog_id)] => catalog_id.clone(),
            _ => panic!("member `{member}` is not a top-level data field"),
        }
    }

    fn accepted_catalog_id(raw: &Option<String>) -> CatalogId {
        CatalogId::new(raw.clone().expect("accepted catalog id")).expect("catalog id")
    }

    fn write_data_value(
        program: &CheckedRuntimeProgram,
        store: &TreeStore,
        root: &str,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        value: SavedValue,
    ) {
        write_data_bytes(
            program,
            store,
            root,
            identity,
            path,
            encode_value(&value).expect("value encodes"),
        );
    }

    fn write_data_bytes(
        program: &CheckedRuntimeProgram,
        store: &TreeStore,
        root: &str,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        bytes: Vec<u8>,
    ) {
        let store_id = store_catalog_id(program, root);
        store
            .write_record_presence(&store_id, identity)
            .expect("record presence write succeeds");
        store
            .write_data_value(&store_id, identity, path, bytes)
            .expect("data value write succeeds");
    }

    fn write_surface_book(
        program: &CheckedRuntimeProgram,
        store: &TreeStore,
        id: i64,
        title: &str,
        status_member: &str,
        author_id: i64,
    ) {
        let identity = [SavedKey::Int(id)];
        let author_identity = [SavedKey::Int(author_id)];
        write_data_value(
            program,
            store,
            "books",
            &identity,
            &data_path(program, "books", &["title"]),
            SavedValue::Str(title.into()),
        );
        let status = TreeEnumMember::new(
            enum_catalog_id(program, "Status"),
            enum_member_catalog_id(program, "Status", status_member),
        );
        write_data_bytes(
            program,
            store,
            "books",
            &identity,
            &data_path(program, "books", &["status"]),
            encode_tree_enum_member(&status).expect("enum value encodes"),
        );
        write_data_bytes(
            program,
            store,
            "books",
            &identity,
            &data_path(program, "books", &["author"]),
            encode_identity_payload(&author_identity),
        );
        store
            .write_index_entry(
                &index_catalog_id(program, "books", "byStatusAuthor"),
                &[
                    SavedKey::Str(
                        enum_member_catalog_id(program, "Status", status_member)
                            .as_str()
                            .into(),
                    ),
                    SavedKey::Bytes(encode_identity_index_key(
                        store_catalog_id(program, "authors").as_str(),
                        &author_identity,
                    )),
                    SavedKey::Int(id),
                ],
                &identity,
                Vec::new(),
            )
            .expect("index entry write succeeds");
    }

    fn write_surface_book_private_code(
        program: &CheckedRuntimeProgram,
        store: &TreeStore,
        id: i64,
        private_code: &str,
    ) {
        write_data_value(
            program,
            store,
            "books",
            &[SavedKey::Int(id)],
            &data_path(program, "books", &["privateCode"]),
            SavedValue::Str(private_code.into()),
        );
    }

    fn write_surface_book_with_isbn(
        program: &CheckedRuntimeProgram,
        store: &TreeStore,
        id: i64,
        title: &str,
        isbn: &str,
    ) {
        let identity = [SavedKey::Int(id)];
        write_data_value(
            program,
            store,
            "books",
            &identity,
            &data_path(program, "books", &["title"]),
            SavedValue::Str(title.into()),
        );
        write_data_value(
            program,
            store,
            "books",
            &identity,
            &data_path(program, "books", &["isbn"]),
            SavedValue::Str(isbn.into()),
        );
        store
            .write_index_entry(
                &index_catalog_id(program, "books", "byIsbn"),
                &[SavedKey::Str(isbn.into())],
                &identity,
                encode_identity_payload(&identity),
            )
            .expect("unique index entry write succeeds");
    }

    fn write_surface_event(
        program: &CheckedRuntimeProgram,
        store: &TreeStore,
        id: i64,
        title: &str,
        day: i32,
        seen_at: i128,
    ) {
        let identity = [SavedKey::Int(id)];
        write_data_value(
            program,
            store,
            "events",
            &identity,
            &data_path(program, "events", &["title"]),
            SavedValue::Str(title.into()),
        );
        write_data_value(
            program,
            store,
            "events",
            &identity,
            &data_path(program, "events", &["day"]),
            SavedValue::Date(day),
        );
        write_data_value(
            program,
            store,
            "events",
            &identity,
            &data_path(program, "events", &["seenAt"]),
            SavedValue::Instant(seen_at),
        );
    }

    fn index_cursor_json<'a>(
        program: &'a CheckedProgram,
        runtime: &CheckedRuntimeProgram,
        store: &'a TreeStore,
    ) -> (SurfaceCollectionRead<'a>, SurfaceCursorJson) {
        write_surface_book(runtime, store, 1, "Dune", "published", 7);
        write_surface_book(runtime, store, 2, "Dune Messiah", "published", 7);
        let read = book_by_status_author_read(program, store);
        let request = book_page_request(runtime, 7, 1);
        let decoded = request.decode(&read).expect("decode page request");
        let page = read.page(decoded.as_page_request()).expect("page read");
        let cursor = page.next.as_ref().expect("next cursor");
        let cursor_json = SurfaceCursorJson::from_cursor(&read, cursor).expect("cursor json");
        (read, cursor_json)
    }

    fn book_page_request(
        program: &CheckedRuntimeProgram,
        author_id: i64,
        limit: usize,
    ) -> SurfacePageRequestJson {
        book_status_author_page_request(program, "published", author_id, limit)
    }

    fn book_status_author_page_request(
        program: &CheckedRuntimeProgram,
        status_member: &str,
        author_id: i64,
        limit: usize,
    ) -> SurfacePageRequestJson {
        SurfacePageRequestJson {
            exact_keys: vec![
                SurfaceArgumentJson::Enum {
                    enum_catalog_id: enum_catalog_id(program, "Status").as_str().into(),
                    member_catalog_id: enum_member_catalog_id(program, "Status", status_member)
                        .as_str()
                        .into(),
                },
                SurfaceArgumentJson::Identity {
                    store_catalog_id: store_catalog_id(program, "authors").as_str().into(),
                    keys: vec![SurfaceKeyJson::Int {
                        value: author_id.to_string(),
                    }],
                },
            ],
            limit,
            cursor: None,
        }
    }

    fn update_field(catalog_id: CatalogId, value: SurfaceWriteValueJson) -> SurfaceUpdateFieldJson {
        SurfaceUpdateFieldJson {
            catalog_id: catalog_id.as_str().into(),
            value,
        }
    }

    fn create_field(catalog_id: CatalogId, value: SurfaceWriteValueJson) -> SurfaceCreateFieldJson {
        SurfaceCreateFieldJson {
            catalog_id: catalog_id.as_str().into(),
            value,
        }
    }

    fn point_update_request(
        runtime: &CheckedRuntimeProgram,
        id: i64,
        fields: Vec<SurfaceUpdateFieldJson>,
    ) -> SurfacePointUpdateRequestJson {
        SurfacePointUpdateRequestJson {
            identity: SurfaceIdentityJson {
                store_catalog_id: store_catalog_id(runtime, "books").as_str().into(),
                keys: vec![SurfaceKeyJson::Int {
                    value: id.to_string(),
                }],
            },
            fields,
        }
    }

    fn point_create_request(
        runtime: &CheckedRuntimeProgram,
        id: i64,
        fields: Vec<SurfaceCreateFieldJson>,
    ) -> SurfacePointCreateRequestJson {
        SurfacePointCreateRequestJson {
            identity: SurfaceIdentityJson {
                store_catalog_id: store_catalog_id(runtime, "books").as_str().into(),
                keys: vec![SurfaceKeyJson::Int {
                    value: id.to_string(),
                }],
            },
            fields,
        }
    }

    fn point_delete_request(
        runtime: &CheckedRuntimeProgram,
        id: i64,
    ) -> SurfacePointDeleteRequestJson {
        SurfacePointDeleteRequestJson {
            identity: SurfaceIdentityJson {
                store_catalog_id: store_catalog_id(runtime, "books").as_str().into(),
                keys: vec![SurfaceKeyJson::Int {
                    value: id.to_string(),
                }],
            },
        }
    }

    fn point_read_request(runtime: &CheckedRuntimeProgram, id: i64) -> SurfacePointRequestJson {
        SurfacePointRequestJson {
            identity: SurfaceIdentityJson {
                store_catalog_id: store_catalog_id(runtime, "books").as_str().into(),
                keys: vec![SurfaceKeyJson::Int {
                    value: id.to_string(),
                }],
            },
        }
    }

    fn field_value<'a>(
        record: &'a SurfaceRecordJson,
        catalog_id: &CatalogId,
    ) -> Option<&'a SurfaceValueJson> {
        record
            .fields
            .iter()
            .find(|field| field.catalog_id == catalog_id.as_str())
            .and_then(|field| field.value.as_ref())
    }

    fn assert_surface_error<T: std::fmt::Debug>(result: Result<T, SurfaceReadError>, code: &str) {
        match result {
            Err(error) => assert_eq!(error.code(), code, "{error:?}"),
            Ok(value) => panic!("expected surface error {code}, got {value:?}"),
        }
    }

    fn assert_operation_error<T: std::fmt::Debug>(
        result: Result<T, SurfaceOperationErrorJson>,
        code: &str,
    ) -> SurfaceOperationErrorJson {
        match result {
            Err(error) => {
                assert_eq!(error.code, code, "{error:?}");
                error
            }
            Ok(value) => panic!("expected surface operation error {code}, got {value:?}"),
        }
    }

    #[test]
    fn surface_abi_omits_duplicate_stable_read_operation_tags() {
        let (program, _runtime) = checked_surface_program(DUPLICATE_READ_TAG_SURFACES);
        let store = admitted_store(&program);
        let books = surface_id(&program, "Books");
        let library = surface_id(&program, "Library");
        let notes = surface_id(&program, "Notes");
        let duplicate_tag = read_operation_tag_matching(&program, books, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let distinct_tag = read_operation_tag_matching(&program, notes, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        assert_eq!(
            duplicate_tag,
            read_operation_tag_matching(&program, library, |kind| {
                matches!(kind, SurfaceReadOperationKind::PointRead { .. })
            })
        );

        let abi = SurfaceAbiJson::from_program(&program);
        assert!(
            abi.surfaces
                .iter()
                .flat_map(|surface| &surface.read)
                .all(|read| read.operation_tag != duplicate_tag),
            "duplicate read tag must not be exported: {abi:#?}"
        );
        let books_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books descriptor");
        let library_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Library")
            .expect("Library descriptor");
        let notes_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Notes")
            .expect("Notes descriptor");
        assert!(books_json.read.is_empty());
        assert!(library_json.read.is_empty());
        let [note_read] = notes_json.read.as_slice() else {
            panic!("distinct read descriptor remains exported: {abi:#?}");
        };
        assert_eq!(note_read.operation_tag, distinct_tag);
        SurfaceNodeRead::admit_by_operation_tag(&program, &store, &note_read.operation_tag)
            .expect("distinct exported read tag admits");
    }

    #[test]
    fn surface_abi_omits_duplicate_stable_update_operation_tags() {
        let (program, _runtime) = checked_surface_program(DUPLICATE_UPDATE_TAG_SURFACES);
        let store = admitted_store(&program);
        let books = surface_id(&program, "Books");
        let library = surface_id(&program, "Library");
        let notes = surface_id(&program, "Notes");
        let duplicate_tag = checker_update_operation_tag(&program, books);
        let distinct_tag = checker_update_operation_tag(&program, notes);
        assert_eq!(
            duplicate_tag,
            checker_update_operation_tag(&program, library)
        );

        let abi = SurfaceAbiJson::from_program(&program);
        assert!(
            abi.surfaces.iter().all(|surface| {
                surface
                    .update
                    .as_ref()
                    .is_none_or(|update| update.operation_tag != duplicate_tag)
            }),
            "duplicate update tag must not be exported: {abi:#?}"
        );
        let books_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books descriptor");
        let library_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Library")
            .expect("Library descriptor");
        let notes_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Notes")
            .expect("Notes descriptor");
        assert!(books_json.update.is_none());
        assert!(library_json.update.is_none());
        let note_update = notes_json
            .update
            .as_ref()
            .expect("distinct update descriptor remains exported");
        assert_eq!(note_update.operation_tag, distinct_tag);
        SurfaceUpdate::admit_by_operation_tag(&program, &store, &note_update.operation_tag)
            .expect("distinct exported update tag admits");
    }

    #[test]
    fn surface_abi_exports_action_descriptors() {
        let (program, _runtime) = checked_surface_program(SURFACE_ACTIONS);
        let abi = SurfaceAbiJson::from_program(&program);
        let books_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books descriptor");
        let [action] = books_json.actions.as_slice() else {
            panic!("expected one action descriptor: {abi:#?}");
        };

        assert_eq!(action.profile_version, ENTRY_PROTOCOL_TAG_VERSION);
        assert_eq!(action.alias, "addBook");
        assert_eq!(action.identity.canonical_name, "test::addBook");
        assert_eq!(action.operation_tag, action.identity.entry_tag);
        let [parameter] = action.parameters.as_slice() else {
            panic!("expected title parameter: {action:#?}");
        };
        assert_eq!(parameter.name, "title");
        assert_eq!(
            parameter.shape,
            SurfaceCallableArgumentShapeJson::Scalar {
                scalar: "string".into()
            }
        );
        assert_eq!(
            action.return_value,
            Some(SurfaceCallableArgumentShapeJson::Scalar {
                scalar: "string".into()
            })
        );
    }

    #[test]
    fn surface_abi_action_parameters_carry_presence() {
        // An optional (`T?`) parameter descriptor carries the optional presence
        // marker so a generated client can type it nullable; a required parameter
        // carries the required marker. Presence rides the parameter carrier.
        let (program, _runtime) = checked_surface_program(SURFACE_ACTION_OPTIONAL_PARAM);
        let abi = SurfaceAbiJson::from_program(&program);
        let books = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books descriptor");
        let [action] = books.actions.as_slice() else {
            panic!("expected one action descriptor: {abi:#?}");
        };
        let required = action
            .parameters
            .iter()
            .find(|parameter| parameter.name == "title")
            .expect("required parameter");
        let optional = action
            .parameters
            .iter()
            .find(|parameter| parameter.name == "note")
            .expect("optional parameter");
        assert_eq!(
            required.presence,
            SurfaceCallableParameterPresenceJson::Required
        );
        assert_eq!(
            optional.presence,
            SurfaceCallableParameterPresenceJson::Optional
        );
    }

    #[test]
    fn surface_abi_exports_read_operation_aliases() {
        let (program, _runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let abi = SurfaceAbiJson::from_program(&program);
        let books_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books descriptor");

        assert_eq!(
            books_json
                .read
                .iter()
                .map(|read| read.alias.as_str())
                .collect::<Vec<_>>(),
            vec!["get", "byStatusAuthor"]
        );
    }

    #[test]
    fn surface_abi_exports_create_delete_descriptors() {
        let (program, _runtime) = checked_surface_program(SURFACE_CREATE_DELETE);
        let abi = SurfaceAbiJson::from_program(&program);
        let books_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books descriptor");
        let create = books_json.create.as_ref().expect("create descriptor");
        let delete = books_json.delete.as_ref().expect("delete descriptor");

        assert_eq!(create.profile_version, "surface.create.v1");
        assert_eq!(delete.profile_version, "surface.delete.v1");
        assert_eq!(create.kind, SurfaceCreateOperationKindJson::PointCreate);
        assert_eq!(delete.kind, SurfaceDeleteOperationKindJson::PointDelete);
        assert_eq!(
            create
                .fields
                .iter()
                .map(|field| field.render_label.as_str())
                .collect::<Vec<_>>(),
            vec!["title", "author"]
        );
        assert_eq!(
            create
                .projection
                .iter()
                .map(|field| field.render_label.as_str())
                .collect::<Vec<_>>(),
            vec!["title", "author"]
        );
        assert_eq!(create.store_catalog_id, delete.store_catalog_id);
        assert_eq!(create.resource_catalog_id, delete.resource_catalog_id);
    }

    #[test]
    fn surface_route_manifest_derives_tag_routes_from_surface_abi() {
        let (program, _runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        assert_eq!(manifest.profile_version, "surface.route.v1");
        assert_eq!(manifest.operation_profile_version, "surface.operation.v1");
        let books_routes = manifest
            .routes
            .iter()
            .filter(|route| route.surface.name == "Books")
            .collect::<Vec<_>>();
        assert_eq!(
            books_routes
                .iter()
                .map(|route| route.alias.as_str())
                .collect::<Vec<_>>(),
            vec!["get", "byStatusAuthor", "update"]
        );
        assert_eq!(
            books_routes
                .iter()
                .map(|route| &route.request)
                .collect::<Vec<_>>(),
            vec![
                &SurfaceRouteRequestJson::PointRead,
                &SurfaceRouteRequestJson::Page,
                &SurfaceRouteRequestJson::PointUpdate,
            ]
        );
        for route in books_routes {
            assert_eq!(route.method, SurfaceRouteMethodJson::Post);
            assert_eq!(route.surface.module, "test");
            assert!(
                route.path.ends_with(&route.operation_tag),
                "route path carries the admitted operation tag: {route:#?}"
            );
        }
    }

    #[test]
    fn surface_route_manifest_and_catalog_include_create_delete_routes() {
        let (program, _runtime) = checked_surface_program(SURFACE_CREATE_DELETE);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);
        let catalog = SurfaceOperationCatalog::from_abi(&abi).expect("operation catalog");
        let create_tag = create_operation_tag(&program, "Books");
        let delete_tag = delete_operation_tag(&program, "Books");

        assert_eq!(
            catalog.kind(&create_tag),
            Some(SurfaceOperationKind::PointCreate)
        );
        assert_eq!(
            catalog.kind(&delete_tag),
            Some(SurfaceOperationKind::PointDelete)
        );
        let books_routes = manifest
            .routes
            .iter()
            .filter(|route| route.surface.name == "Books")
            .collect::<Vec<_>>();
        assert_eq!(
            books_routes
                .iter()
                .map(|route| (route.alias.as_str(), route.request))
                .collect::<Vec<_>>(),
            vec![
                ("get", SurfaceRouteRequestJson::PointRead),
                ("create", SurfaceRouteRequestJson::PointCreate),
                ("delete", SurfaceRouteRequestJson::PointDelete),
            ]
        );
        assert!(
            books_routes
                .iter()
                .any(|route| route.path.starts_with("/surface/v1/create/"))
        );
        assert!(
            books_routes
                .iter()
                .any(|route| route.path.starts_with("/surface/v1/delete/"))
        );
    }

    #[test]
    fn surface_route_manifest_includes_action_routes() {
        let (program, _runtime) = checked_surface_program(SURFACE_ACTIONS);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let action_route = manifest
            .routes
            .iter()
            .find(|route| route.alias == "addBook")
            .expect("action route");
        assert_eq!(action_route.request, SurfaceRouteRequestJson::Action);
        assert!(action_route.path.starts_with("/surface/v1/action/"));
        assert!(action_route.path.ends_with(&action_route.operation_tag));
    }

    #[test]
    fn surface_abi_json_includes_computed_read_descriptors() {
        let (program, _runtime) = checked_surface_program(SURFACE_COMPUTED_READ);
        let abi = SurfaceAbiJson::from_program(&program);
        let books = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books surface");
        let [computed] = books.computed_reads.as_slice() else {
            panic!(
                "expected one computed read, got {:#?}",
                books.computed_reads
            );
        };

        assert_eq!(computed.profile_version, "surface.computed_read.v1");
        assert_eq!(computed.alias, "page");
        assert!(computed.operation_tag.starts_with("sha256:"));
        assert_eq!(computed.callable.identity.canonical_name, "test::bookPage");
        assert_eq!(
            computed.callable.result.presence,
            SurfaceComputedReadPresenceJson::MaybePresent
        );
        let Some(SurfaceComputedReadValueShapeJson::Resource {
            resource_catalog_id,
            fields,
            ..
        }) = &computed.callable.result.value
        else {
            panic!("expected resource result: {computed:#?}");
        };
        assert!(resource_catalog_id.starts_with("cat_"));
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].render_label, "title");
        assert!(computed.cost_shape.point_reads > 0);
    }

    #[test]
    fn surface_route_manifest_and_catalog_include_computed_read_routes() {
        let (program, _runtime) = checked_surface_program(SURFACE_COMPUTED_READ);
        let abi = SurfaceAbiJson::from_program(&program);
        let catalog = SurfaceOperationCatalog::from_abi(&abi).expect("operation catalog");
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);
        let books = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books surface");
        let computed = books.computed_reads.first().expect("computed read");

        assert_eq!(
            catalog.kind(&computed.operation_tag),
            Some(SurfaceOperationKind::ComputedRead)
        );
        let route = manifest
            .routes
            .iter()
            .find(|route| route.operation_tag == computed.operation_tag)
            .expect("computed-read route");
        assert_eq!(route.alias, "page");
        assert_eq!(route.request, SurfaceRouteRequestJson::ComputedRead);
        assert!(route.path.starts_with("/surface/v1/read/"));
    }

    #[test]
    fn surface_route_manifest_uses_curated_stable_abi() {
        let (program, _runtime) = checked_surface_program(DUPLICATE_READ_TAG_SURFACES);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        assert_eq!(
            manifest
                .routes
                .iter()
                .map(|route| route.surface.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Notes"]
        );

        let source_only = checked_source_only_surface_program(SOURCE_ONLY_UPDATE_SURFACE);
        let source_only_abi = SurfaceAbiJson::from_program(&source_only);
        let source_only_manifest = SurfaceRouteManifestJson::from_abi(&source_only_abi);
        assert!(source_only_manifest.routes.is_empty());
    }

    #[test]
    fn surface_operation_catalog_derives_existing_operation_kinds() {
        let (program, _runtime) = checked_surface_program(SURFACE_ACTION_UPDATE);
        let abi = SurfaceAbiJson::from_program(&program);
        let catalog = SurfaceOperationCatalog::from_abi(&abi).expect("catalog from ABI");

        let books = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books surface");
        let read = books
            .read
            .iter()
            .find(|read| read.alias == "get")
            .expect("point read");
        let update = books.update.as_ref().expect("update descriptor");
        let action = books
            .actions
            .iter()
            .find(|action| action.alias == "addBook")
            .expect("action descriptor");

        assert_eq!(
            catalog.kind(&read.operation_tag),
            Some(SurfaceOperationKind::PointRead)
        );
        assert_eq!(
            catalog.kind(&update.operation_tag),
            Some(SurfaceOperationKind::PointUpdate)
        );
        assert_eq!(
            catalog.kind(&action.operation_tag),
            Some(SurfaceOperationKind::Action)
        );
    }

    #[test]
    fn surface_operation_catalog_rejects_malformed_duplicate_abi_rows() {
        let (program, _runtime) = checked_surface_program(SURFACE_ACTION_UPDATE);
        let mut abi = SurfaceAbiJson::from_program(&program);
        let duplicate = abi.surfaces[0].read[0].operation_tag.clone();
        abi.surfaces[0]
            .update
            .as_mut()
            .expect("update")
            .operation_tag = duplicate;

        let error = SurfaceOperationCatalog::from_abi(&abi).expect_err("duplicate ABI row rejects");
        assert_eq!(
            error.kind(),
            SurfaceOperationCatalogErrorKind::DuplicateOperationTag
        );
    }

    #[test]
    fn surface_route_bindings_validate_against_abi_catalog() {
        let (program, _runtime) = checked_surface_program(SURFACE_ACTION_UPDATE);
        let abi = SurfaceAbiJson::from_program(&program);
        let catalog = SurfaceOperationCatalog::from_abi(&abi).expect("catalog from ABI");
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);
        let bindings = SurfaceRouteBindings::from_manifest(&manifest, &catalog).expect("bindings");

        assert_eq!(bindings.len(), manifest.routes.len());
        for binding in bindings.iter() {
            let expected = catalog
                .binding(&binding.operation_tag)
                .expect("route operation has ABI binding");
            assert_eq!(binding.path, expected.path);
            assert_eq!(binding.kind, expected.kind);
            assert_eq!(binding.surface_module, expected.surface_module);
            assert_eq!(binding.surface_name, expected.surface_name);
            assert_eq!(binding.alias, expected.alias);
        }
    }

    #[test]
    fn client_ts_validates_routes_and_renders_typed_methods() {
        let (program, _runtime) = checked_surface_program(SURFACE_ACTION_UPDATE);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        assert!(client.contains("export function createClient"), "{client}");
        assert!(client.contains("Number.isSafeInteger"), "{client}");
        assert!(
            client.contains("export class MarrowSurfaceError"),
            "{client}"
        );
        assert!(client.contains("export type SurfaceErrorCode"), "{client}");
        assert!(client.contains("export function invokeRaw"), "{client}");
        assert!(
            !client.contains("import "),
            "the generated client must not depend on imports: {client}"
        );

        // Every operation tag appears in the per-tag request-kind table the transport dispatches on.
        for route in &manifest.routes {
            let kind = SurfaceOperationKind::from(&route.request);
            assert!(
                client.contains(&format!(
                    "{:?}: {:?}",
                    route.operation_tag,
                    kind.operation_request_kind()
                )),
                "missing request kind for tag {}: {client}",
                route.operation_tag
            );
        }
        // The typed surface exposes a branded point read and a typed `{ value, output }` action.
        assert!(client.contains("get: async (id: BooksId)"), "{client}");
        assert!(
            client.contains("Promise<{ value: string; output: string }>"),
            "{client}"
        );
    }

    fn fixture_abi_and_routes() -> (SurfaceAbiJson, SurfaceRouteManifestJson) {
        let program = checked_source_only_surface_program(SURFACE_ACTION_UPDATE);
        let abi = SurfaceAbiJson::from_program(&program);
        let routes = SurfaceRouteManifestJson::from_abi(&abi);
        (abi, routes)
    }

    #[test]
    fn digest_is_stable_and_sha256_prefixed() {
        let (abi, routes) = fixture_abi_and_routes();
        let a = surface_abi_digest(&abi, &routes);
        let b = surface_abi_digest(&abi, &routes);
        assert_eq!(a, b, "digest must be deterministic");
        assert!(a.starts_with("sha256:"), "{a}");
        assert_eq!(a.len(), "sha256:".len() + 64, "{a}");
    }

    #[test]
    fn header_round_trips_through_parser() {
        let header = surface_client_header("sha256:surface", "sha256:client");
        assert!(header.contains(SURFACE_CLIENT_DO_NOT_EDIT));
        assert!(header.contains(SURFACE_CLIENT_PROFILE_PREFIX));
        assert!(header.contains(SURFACE_CLIENT_DIGEST_PREFIX));
        assert_eq!(
            surface_client_header_digest(&header).as_deref(),
            Some("sha256:client")
        );
        assert_eq!(surface_client_header_digest("no header here"), None);
    }

    #[test]
    fn client_ts_header_carries_profile_surface_digest_and_client_digest() {
        let (abi, routes) = fixture_abi_and_routes();
        let surface_digest = surface_abi_digest(&abi, &routes);
        let client_digest = surface_client_digest(&abi, &routes);

        let client = render_typescript_client(&abi, &routes).expect("typescript client renders");

        assert!(
            client.starts_with(SURFACE_CLIENT_DO_NOT_EDIT),
            "generated client must start with the do-not-edit header: {client}"
        );
        assert!(
            client.contains("// marrow-client-profile: typescript.v2"),
            "generated client must name the client generator profile: {client}"
        );
        assert!(
            client.contains(&format!("// marrow-surface-digest: {surface_digest}")),
            "generated client must keep the surface ABI digest in the header: {client}"
        );
        assert!(
            client.contains(&format!("// marrow-client-digest: {client_digest}")),
            "generated client must carry the client freshness digest: {client}"
        );
        assert_eq!(
            surface_client_header_digest(&client).as_deref(),
            Some(client_digest.as_str())
        );
    }

    #[test]
    fn client_ts_cursor_token_profile_has_distinct_header_digest_and_string_cursor_brand() {
        let (program, _runtime) = checked_surface_program(SURFACE_PAGE_HELPERS);
        let abi = SurfaceAbiJson::from_program(&program);
        let routes = SurfaceRouteManifestJson::from_abi(&abi);

        let typed_client = render_typescript_client(&abi, &routes).expect("typed client renders");
        let token_client = render_typescript_client_with_cursor_profile(
            &abi,
            &routes,
            SurfaceClientCursorProfile::Token,
        )
        .expect("token client renders");

        let typed_digest = surface_client_digest(&abi, &routes);
        let token_digest = surface_client_digest_with_cursor_profile(
            &abi,
            &routes,
            SurfaceClientCursorProfile::Token,
        );
        assert_ne!(
            typed_digest, token_digest,
            "token cursor profile must not share typed-client freshness"
        );
        assert!(
            typed_client.contains("// marrow-client-profile: typescript.v2"),
            "{typed_client}"
        );
        assert!(
            token_client
                .contains("// marrow-client-profile: typescript.v2+surface.cursor_token.v1"),
            "{token_client}"
        );
        assert!(
            token_client.contains(&format!("// marrow-client-digest: {token_digest}")),
            "{token_client}"
        );
        assert_eq!(
            surface_client_header_digest(&token_client).as_deref(),
            Some(token_digest.as_str())
        );
        assert!(
            typed_client.contains(
                "export type BooksCursor = SurfaceCursorJson & { readonly __brand: \"BooksCursor\" };"
            ),
            "{typed_client}"
        );
        assert!(
            token_client.contains(
                "export type BooksCursor = string & { readonly __brand: \"BooksCursor\" };"
            ),
            "{token_client}"
        );
        assert!(
            token_client.contains(
                "all: async (limit: number, cursor?: BooksCursor | null): Promise<Page<BooksRecord, BooksCursor>>"
            ),
            "{token_client}"
        );
    }

    #[test]
    fn client_ts_renders_page_iteration_helpers() {
        let (program, _runtime) = checked_surface_program(SURFACE_PAGE_HELPERS);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        assert!(
            client.contains(
                "all: async (limit: number, cursor?: BooksCursor | null): Promise<Page<BooksRecord, BooksCursor>>"
            ),
            "existing root page method must stay unchanged: {client}"
        );
        assert!(
            client.contains(
                "byAuthor: async (exactKey0: string, limit: number, cursor?: BooksCursor | null): Promise<Page<BooksRecord, BooksCursor>>"
            ),
            "existing index page method must stay unchanged: {client}"
        );
        assert!(
            client.contains(
                "allPages: async function* (options: { limit: number; initialCursor?: BooksCursor | null }): AsyncIterable<Page<BooksRecord, BooksCursor>>"
            ),
            "root page iterator helper must take only the options object: {client}"
        );
        assert!(
            client.contains(
                "byAuthorPages: async function* (author: string, options: { limit: number; initialCursor?: BooksCursor | null }): AsyncIterable<Page<BooksRecord, BooksCursor>>"
            ),
            "index page iterator helper must take exact-key arguments plus options: {client}"
        );
        assert!(
            client.contains("let cursor = options.initialCursor ?? null;"),
            "page iterator must own cursor state: {client}"
        );
        assert!(
            client.contains("yield page;"),
            "helper must yield pages: {client}"
        );
        assert!(
            !client.contains("for (const row of page.rows)"),
            "helper must not yield rows: {client}"
        );
    }

    #[test]
    fn client_ts_disambiguates_page_helper_names() {
        let (program, _runtime) = checked_surface_program(SURFACE_PAGE_HELPER_COLLISION);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        let method_names = surface_method_names(&client, "    Books");
        let unique = method_names.iter().collect::<BTreeSet<_>>();
        assert_eq!(
            unique.len(),
            method_names.len(),
            "duplicate method key: {method_names:?}\n{client}"
        );
        assert!(
            method_names.iter().any(|name| name == "allPages"),
            "the page helper or action should keep the base name: {method_names:?}\n{client}"
        );
        assert!(
            method_names
                .iter()
                .any(|name| name.starts_with("allPages_")),
            "the colliding page helper/action should be disambiguated: {method_names:?}\n{client}"
        );
    }

    #[test]
    fn client_ts_page_iterator_parameters_avoid_generated_scope_collisions() {
        let (program, _runtime) = checked_surface_program(SURFACE_PAGE_HELPER_RESERVED_PARAMETERS);
        let mut abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        assert!(
            client.contains(
                "byHelperScopePages: async function* (cursorKey: string, optionsKey: string, transportKey: string, options: { limit: number; initialCursor?: BooksCursor | null }): AsyncIterable<Page<BooksRecord, BooksCursor>>"
            ),
            "helper exact-key parameters must not collide with generated bindings: {client}"
        );
        assert!(
            client.contains(
                "exact_keys: [stringKey(cursorKey), stringKey(optionsKey), stringKey(transportKey)]"
            ),
            "helper request body must use the allocated parameter names: {client}"
        );
        assert!(
            !client.contains("function* (cursor: string")
                && !client.contains("options: string, options:")
                && !client.contains("transport: string, options:"),
            "helper must not emit shadowing or duplicate parameter names: {client}"
        );

        let read = abi.surfaces[0]
            .read
            .iter_mut()
            .find(|read| read.alias == "byHelperScope")
            .expect("helper scope read");
        read.index_keys[0].render_label = "class".into();
        read.index_keys[1].render_label = "yield".into();
        read.index_keys[2].render_label = "let".into();
        let keyword_client =
            render_typescript_client(&abi, &manifest).expect("typescript client renders");
        assert!(
            keyword_client.contains(
                "byHelperScopePages: async function* (classKey: string, yieldKey: string, letKey: string, options: { limit: number; initialCursor?: BooksCursor | null }): AsyncIterable<Page<BooksRecord, BooksCursor>>"
            ),
            "helper exact-key parameters must avoid TypeScript keywords: {keyword_client}"
        );
    }

    #[test]
    fn client_ts_renders_typed_computed_read_method() {
        let (program, _runtime) = checked_surface_program(SURFACE_COMPUTED_READ);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        // The computed read decodes its result into the typed resource value rather than returning
        // the raw envelope, and an identity argument encodes through the entry argument shape the
        // entry decoder reads, not the request identity shape used by write fields and index keys.
        assert!(
            client.contains("computedReadValue(envelope, (value) =>"),
            "{client}"
        );
        assert!(client.contains("encodeIdentityArgument(id)"), "{client}");
        let computed_route = manifest
            .routes
            .iter()
            .find(|route| route.request == SurfaceRouteRequestJson::ComputedRead)
            .expect("computed-read route");
        assert!(
            client.contains(&format!(
                "{:?}: \"computed_read\"",
                computed_route.operation_tag
            )),
            "{client}"
        );
    }

    #[test]
    fn client_ts_reads_and_deletes_a_keyless_singleton_without_identity() {
        let (program, _runtime) = checked_surface_program(SINGLETON_READ_DELETE_SURFACE);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        // A keyless singleton record takes no identity, so the generated record type carries no
        // synthetic `id` field and its decoder never reads `record.identity.keys` (the server sends
        // `identity: null`, which would throw a raw TypeError on a null dereference).
        assert!(
            !client.contains("record.identity.keys"),
            "singleton record decoder must not dereference a null identity: {client}"
        );
        assert!(
            !client.contains("id: SettingsSurfaceId"),
            "a keyless singleton record has no synthetic id field: {client}"
        );

        // The singleton read returns the record with no argument, decoding through the surface read
        // envelope rather than the resource-value envelope used by computed reads.
        assert!(
            client.contains("get: async ()"),
            "singleton read takes no identity argument: {client}"
        );

        // The singleton delete takes no identity and sends the closed empty request body; it must
        // not thread a phantom `id` the server never supplies for a keyless store.
        assert!(
            client.contains("delete: async ()"),
            "singleton delete takes no identity argument: {client}"
        );
        assert!(
            !client.contains("delete: async (id:"),
            "singleton delete must not take an identity: {client}"
        );
    }

    #[test]
    fn client_ts_encodes_action_arguments_through_typed_entry_shapes() {
        let (program, _runtime) = checked_surface_program(SURFACE_ACTION_UPDATE);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        // The action method names its arguments and routes each scalar through the typed encoder,
        // never an `unknown[]` argument array.
        assert!(!client.contains("arguments: unknown[]"), "{client}");
        assert!(
            client.contains(
                "{ arguments: [{ name: \"title\", value: { kind: \"string\", value: title } }] }"
            ),
            "{client}"
        );
        assert!(
            client.contains("Promise<{ value: string; output: string }>"),
            "{client}"
        );
    }

    #[test]
    fn client_ts_update_body_is_sparse_with_optional_fields() {
        let (program, _runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        // The server applies a non-empty field patch and preserves omitted fields, so each updatable
        // field is an optional body key and a partial update type-checks without the other fields.
        assert!(
            client.contains("export type BooksUpdateBody = {"),
            "update body type must be generated: {client}"
        );
        assert!(
            client.contains("status?:"),
            "update body status field must be optional: {client}"
        );
        assert!(
            client.contains("author?:"),
            "update body author field must be optional: {client}"
        );
        // The serializer emits only the fields the caller provided, never an unconditional all-fields
        // body that would force a whole-record read-modify-write.
        assert!(
            client.contains(r#"body["status"] !== undefined"#),
            "update serializer must include a field only when present: {client}"
        );
        assert!(
            client.contains(r#"body["author"] !== undefined"#),
            "update serializer must include a field only when present: {client}"
        );
    }

    #[test]
    fn client_ts_singleton_update_body_is_sparse_with_optional_fields() {
        let (program, _runtime) = checked_surface_program(SINGLETON_UPDATE_SURFACE);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        assert!(
            client.contains("export type SettingsSurfaceUpdateBody = {\n  mode?:"),
            "singleton update body field must be optional: {client}"
        );
        assert!(
            client.contains(r#"body["mode"] !== undefined"#),
            "singleton update serializer must include a field only when present: {client}"
        );
    }

    #[test]
    fn client_ts_encodes_enum_and_identity_index_keys_in_the_request_shape() {
        let (program, _runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let abi = SurfaceAbiJson::from_program(&program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        // The paged index keys on an enum then an identity. Both exact-keys must serialize to the
        // request argument shapes the server validates (enum carries its catalog id; identity its
        // store), never the raw-string stand-in that prompted this fix or the entry `enum_member`
        // shape reserved for action arguments.
        assert!(
            client.contains("encodeEnum(exactKey0,"),
            "enum index key must use the request enum encoder: {client}"
        );
        assert!(
            client.contains("encodeIdentity(exactKey1)"),
            "identity index key must use the identity encoder: {client}"
        );
        assert!(
            !client.contains("encodeEnumMember(exactKey0"),
            "index keys must not use the entry enum_member shape: {client}"
        );
    }

    #[test]
    fn client_ts_sanitizes_reserved_words_and_label_collisions() {
        let (program, _runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let mut abi = SurfaceAbiJson::from_program(&program);
        let mut manifest = SurfaceRouteManifestJson::from_abi(&abi);
        let surface = abi
            .surfaces
            .iter_mut()
            .find(|surface| surface.name == "Books")
            .expect("Books surface");
        surface.module = "default".into();
        surface.name = "class".into();
        for read in &mut surface.read {
            read.alias = "delete".into();
        }
        let expected_read_count = surface.read.len();
        for route in manifest
            .routes
            .iter_mut()
            .filter(|route| route.surface.name == "Books")
        {
            route.surface.module = "default".into();
            route.surface.name = "class".into();
            if matches!(
                route.request,
                SurfaceRouteRequestJson::PointRead
                    | SurfaceRouteRequestJson::Page
                    | SurfaceRouteRequestJson::UniqueLookup
            ) {
                route.alias = "delete".into();
            }
        }

        let client = render_typescript_client(&abi, &manifest).expect("typescript client renders");

        // The colliding read aliases stay distinct method keys, none duplicated, so the emitted
        // surface object is valid TypeScript even when every read shares an alias.
        let method_names = surface_method_names(&client, "    class");
        let read_method_names = method_names
            .iter()
            .filter(|name| name.as_str() == "delete" || name.starts_with("delete_"))
            .collect::<Vec<_>>();
        assert_eq!(read_method_names.len(), expected_read_count, "{client}");
        assert!(
            read_method_names
                .iter()
                .all(|name| name.starts_with("delete")),
            "{read_method_names:?}"
        );
        let unique = method_names.iter().collect::<BTreeSet<_>>();
        assert_eq!(
            unique.len(),
            method_names.len(),
            "duplicate method key: {method_names:?}"
        );
        assert!(method_names.len() > 1, "{method_names:?}");
    }

    /// Pull the async-method keys from a generated surface block. Method headers are the only lines
    /// indented six spaces that contain an async property value; nested helper braces never start
    /// that way.
    fn surface_method_names(client: &str, surface_key: &str) -> Vec<String> {
        let needle = format!("{surface_key}: {{");
        let start = client.find(&needle).expect("surface block present") + needle.len();
        let mut names = Vec::new();
        for line in client[start..].lines() {
            if line == "    }," {
                break;
            }
            if let Some(key) = surface_method_key(line) {
                names.push(key);
            }
        }
        names
    }

    fn surface_method_key(line: &str) -> Option<String> {
        let rest = line.strip_prefix("      ")?;
        rest.split_once(": async (")
            .or_else(|| rest.split_once(": async function* ("))
            .map(|(key, _)| key.trim_matches('"').to_string())
    }

    #[test]
    fn client_ts_rejects_route_manifests_that_are_not_abi_bijections() {
        let (program, _runtime) = checked_surface_program(SURFACE_ACTION_UPDATE);
        let abi = SurfaceAbiJson::from_program(&program);
        let mut manifest = SurfaceRouteManifestJson::from_abi(&abi);
        manifest.routes.pop();

        let error = render_typescript_client(&abi, &manifest)
            .expect_err("client renderer rejects missing route rows");
        assert_eq!(error.kind(), SurfaceClientRenderErrorKind::RouteBinding);
    }

    #[test]
    fn surface_route_bindings_reject_forged_labels_duplicates_and_wrong_kind() {
        let (program, _runtime) = checked_surface_program(SURFACE_ACTION_UPDATE);
        let abi = SurfaceAbiJson::from_program(&program);
        let catalog = SurfaceOperationCatalog::from_abi(&abi).expect("catalog from ABI");
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);

        let mut wrong_alias = manifest.clone();
        wrong_alias.routes[0].alias = "forged".into();
        assert_eq!(
            SurfaceRouteBindings::from_manifest(&wrong_alias, &catalog)
                .expect_err("forged alias rejects")
                .kind(),
            SurfaceRouteBindingErrorKind::AliasMismatch
        );

        let mut duplicate_path = manifest.clone();
        duplicate_path.routes[1].path = duplicate_path.routes[0].path.clone();
        assert_eq!(
            SurfaceRouteBindings::from_manifest(&duplicate_path, &catalog)
                .expect_err("duplicate path rejects")
                .kind(),
            SurfaceRouteBindingErrorKind::DuplicatePath
        );

        let mut duplicate_tag = manifest.clone();
        duplicate_tag.routes[1].operation_tag = duplicate_tag.routes[0].operation_tag.clone();
        assert_eq!(
            SurfaceRouteBindings::from_manifest(&duplicate_tag, &catalog)
                .expect_err("duplicate route tag rejects")
                .kind(),
            SurfaceRouteBindingErrorKind::DuplicateOperationTag
        );

        let mut wrong_request = manifest.clone();
        wrong_request.routes[0].request = SurfaceRouteRequestJson::Action;
        assert_eq!(
            SurfaceRouteBindings::from_manifest(&wrong_request, &catalog)
                .expect_err("wrong request kind rejects")
                .kind(),
            SurfaceRouteBindingErrorKind::RequestKindMismatch
        );
    }

    #[test]
    fn singleton_read_and_delete_bodies_are_closed_objects() {
        for kind in ["singleton_read", "singleton_delete"] {
            serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
                "kind": kind,
                "request": {}
            }))
            .unwrap_or_else(|error| panic!("{kind} with empty request must parse: {error}"));

            assert!(
                serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
                    "kind": kind,
                    "request": { "unexpected": true }
                }))
                .is_err(),
                "{kind} must reject an unknown request field"
            );
            assert!(
                serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
                    "kind": kind,
                    "request": "garbage"
                }))
                .is_err(),
                "{kind} must reject a string request"
            );
            assert!(
                serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
                    "kind": kind,
                    "request": []
                }))
                .is_err(),
                "{kind} must reject an array request"
            );
            assert!(
                serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({ "kind": kind }))
                    .is_err(),
                "{kind} must reject an omitted request"
            );
            assert!(
                serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
                    "kind": kind,
                    "stray": 1,
                    "request": {}
                }))
                .is_err(),
                "{kind} must reject a sibling field on the envelope"
            );
        }
    }

    #[test]
    fn surface_operation_kind_matches_operation_body_kind() {
        let point_read = serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
            "kind": "point_read",
            "request": {
                "identity": {
                    "store_catalog_id": "cat_00000000000000000000000000000001",
                    "keys": [{ "kind": "int", "value": "1" }]
                }
            }
        }))
        .expect("point read body parses");
        let page = serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
            "kind": "page",
            "request": { "exact_keys": [], "limit": 1 }
        }))
        .expect("page body parses");
        let unique_lookup = serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
            "kind": "unique_lookup",
            "request": { "keys": [] }
        }))
        .expect("unique lookup body parses");
        let singleton_update = serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
            "kind": "singleton_update",
            "request": { "fields": [] }
        }))
        .expect("singleton update body parses");
        let point_update = serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
            "kind": "point_update",
            "request": {
                "identity": {
                    "store_catalog_id": "cat_00000000000000000000000000000001",
                    "keys": [{ "kind": "int", "value": "1" }]
                },
                "fields": []
            }
        }))
        .expect("point update body parses");
        let action = serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
            "kind": "action",
            "request": { "arguments": [] }
        }))
        .expect("action body parses");
        let computed_read = serde_json::from_value::<SurfaceOperationRequestBodyJson>(json!({
            "kind": "computed_read",
            "request": { "arguments": [] }
        }))
        .expect("computed read body parses");

        let cases = [
            (
                SurfaceOperationKind::SingletonRead,
                SurfaceOperationRequestBodyJson::SingletonRead {
                    request: SurfaceEmptyRequestJson,
                },
            ),
            (SurfaceOperationKind::PointRead, point_read),
            (SurfaceOperationKind::Page, page),
            (SurfaceOperationKind::UniqueLookup, unique_lookup),
            (SurfaceOperationKind::SingletonUpdate, singleton_update),
            (SurfaceOperationKind::PointUpdate, point_update),
            (SurfaceOperationKind::Action, action),
            (SurfaceOperationKind::ComputedRead, computed_read),
        ];
        for (kind, body) in cases {
            assert!(
                kind.matches_operation_body(&body),
                "{kind:?} should match {body:?}"
            );
        }
        assert!(SurfaceOperationKind::SingletonRead.is_read());
        assert!(SurfaceOperationKind::PointRead.is_read());
        assert!(SurfaceOperationKind::Page.is_read());
        assert!(SurfaceOperationKind::UniqueLookup.is_read());
        assert!(!SurfaceOperationKind::SingletonUpdate.is_read());
        assert!(!SurfaceOperationKind::PointUpdate.is_read());
        assert!(!SurfaceOperationKind::Action.is_read());
        assert!(!SurfaceOperationKind::Page.matches_operation_body(
            &SurfaceOperationRequestBodyJson::SingletonRead {
                request: SurfaceEmptyRequestJson,
            }
        ));
    }

    #[test]
    fn surface_abi_omits_duplicate_stable_action_operation_tags() {
        let (program, _runtime) = checked_surface_program(DUPLICATE_ACTION_TAG_SURFACES);
        let duplicate_tag = checker_action_operation_tag(&program, "Books", "addBook");
        let distinct_tag = checker_action_operation_tag(&program, "Notes", "addNote");
        assert_eq!(
            duplicate_tag,
            checker_action_operation_tag(&program, "Library", "addBook")
        );

        let abi = SurfaceAbiJson::from_program(&program);
        assert!(
            abi.surfaces
                .iter()
                .flat_map(|surface| &surface.actions)
                .all(|action| action.operation_tag != duplicate_tag),
            "duplicate action tag must not be exported: {abi:#?}"
        );
        let books_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Books")
            .expect("Books descriptor");
        let library_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Library")
            .expect("Library descriptor");
        let notes_json = abi
            .surfaces
            .iter()
            .find(|surface| surface.name == "Notes")
            .expect("Notes descriptor");
        assert!(books_json.actions.is_empty());
        assert!(library_json.actions.is_empty());
        let [note_action] = notes_json.actions.as_slice() else {
            panic!("distinct action descriptor remains exported: {abi:#?}");
        };
        assert_eq!(note_action.operation_tag, distinct_tag);
        assert_eq!(
            SurfaceActionInvocation::admit_by_operation_tag(&program, &duplicate_tag)
                .expect_err("duplicate action tag does not admit")
                .code(),
            SURFACE_ABI_MISMATCH
        );
        SurfaceActionInvocation::admit_by_operation_tag(&program, &note_action.operation_tag)
            .expect("distinct exported action tag admits");
    }

    #[test]
    fn surface_abi_exports_only_runtime_admitted_operation_tags() {
        let (program, _runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let abi = SurfaceAbiJson::from_program(&program);
        let mut read_count = 0;
        let mut update_count = 0;

        for surface in &abi.surfaces {
            for read in &surface.read {
                read_count += 1;
                match &read.kind {
                    SurfaceReadOperationKindJson::SingletonRead
                    | SurfaceReadOperationKindJson::PointRead => {
                        SurfaceNodeRead::admit_by_operation_tag(
                            &program,
                            &store,
                            &read.operation_tag,
                        )
                        .expect("exported node read tag admits");
                    }
                    SurfaceReadOperationKindJson::PagedRootCollection
                    | SurfaceReadOperationKindJson::PagedIndexCollection { .. }
                    | SurfaceReadOperationKindJson::UniqueIndexLookup { .. } => {
                        SurfaceCollectionRead::admit_by_operation_tag(
                            &program,
                            &store,
                            &read.operation_tag,
                        )
                        .expect("exported collection read tag admits");
                    }
                }
            }
            if let Some(update) = &surface.update {
                update_count += 1;
                SurfaceUpdate::admit_by_operation_tag(&program, &store, &update.operation_tag)
                    .expect("exported update tag admits");
            }
        }

        assert!(read_count > 0, "fixture exports read descriptors");
        assert!(update_count > 0, "fixture exports update descriptors");
    }

    #[test]
    fn source_only_surface_abi_serializes_blockers_and_no_descriptors() {
        let program = checked_source_only_surface_program(SOURCE_ONLY_UPDATE_SURFACE);
        let abi = SurfaceAbiJson::from_program(&program);
        let [surface] = abi.surfaces.as_slice() else {
            panic!("expected one surface, got {abi:#?}");
        };

        assert_eq!(
            surface.catalog_status,
            SurfaceCatalogStatusJson::SourceOnly {
                blockers: vec![
                    "pending_catalog_proposal".into(),
                    "missing_accepted_catalog_ids".into(),
                ],
            }
        );
        assert!(surface.read.is_empty(), "source-only read descriptors");
        assert!(surface.update.is_none(), "source-only update descriptor");
    }

    #[test]
    fn surface_execute_point_read_by_operation_tag_returns_record_json() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "published", 7);

        let surface = surface_id(&program, "Books");
        let operation_tag = read_operation_tag_matching(&program, surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let record = crate::surface::execute_surface_point_read_by_tag(
            &program,
            &store,
            &operation_tag,
            &point_read_request(&runtime, 1),
        )
        .expect("execute point read");

        assert_eq!(
            record.identity.as_ref().expect("record identity").keys,
            vec![SurfaceKeyJson::Int { value: "1".into() }]
        );
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune".into()
            })
        );
    }

    #[test]
    fn surface_execute_project_read_session_executes_read_json_without_exposing_the_store() {
        let root = TempProject::new("marrow-json-project-surface-read");
        write_native_project(&root, PROJECT_READ_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceReadSession::open(root.path()).expect("open project surface session");
        let runtime = session.program().runtime();
        let surface = surface_id(session.program(), "Books");
        let point_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let record = crate::surface::execute_project_surface_point_read_by_tag(
            &session,
            &point_tag,
            &point_read_request(&runtime, 1),
        )
        .expect("execute project point read");
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune".into()
            })
        );

        let root_page_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PagedRootCollection { .. })
        });
        let page = crate::surface::execute_project_surface_page_by_tag(
            &session,
            &root_page_tag,
            &SurfacePageRequestJson {
                exact_keys: Vec::new(),
                limit: 1,
                cursor: None,
            },
        )
        .expect("execute project page read");
        assert_eq!(page.rows.len(), 1);
        assert!(page.next.is_some(), "limited page returns a cursor");
    }

    #[test]
    fn surface_execute_project_session_executes_update_json_and_persists() {
        let root = TempProject::new("marrow-json-project-surface-update");
        write_native_project(&root, PROJECT_UPDATE_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let before = session
            .store_stamp()
            .expect("project surface session exposes the store stamp");
        let runtime = session.program().runtime();
        let surface = surface_id(session.program(), "Books");
        let update_tag = update_operation_tag(session.program(), "Books");
        let read_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });

        crate::surface::execute_project_surface_point_update_by_tag(
            &session,
            &update_tag,
            &point_update_request(
                &runtime,
                1,
                vec![update_field(
                    field_catalog_id(&runtime, "books", "title"),
                    SurfaceWriteValueJson::String {
                        value: "Dune Revised".into(),
                    },
                )],
            ),
        )
        .expect("execute project point update");

        let record = session
            .admit_read_by_operation_tag(&read_tag)
            .expect("admit point read through write session")
            .point_read()
            .expect("point read shape")
            .execute(SurfaceReadInput::Point {
                identity: &[SavedKey::Int(1)],
            })
            .expect("read updated record through write session");
        let record = SurfaceRecordJson::from(&record);
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune Revised".into()
            })
        );

        assert_surface_error(
            crate::surface::execute_project_surface_point_update_by_tag(
                &session,
                &read_tag,
                &point_update_request(&runtime, 1, Vec::new()),
            ),
            SURFACE_ABI_MISMATCH,
        );
        let after = session
            .store_stamp()
            .expect("project surface session exposes the updated store stamp");
        assert_eq!(after.store_uid, before.store_uid);
        assert_eq!(after.catalog_epoch, before.catalog_epoch);
        assert!(after.commit_id > before.commit_id);
        drop(session);

        let read_session = ProjectSurfaceReadSession::open(root.path())
            .expect("reopen project read session after update");
        let runtime = read_session.program().runtime();
        let record = crate::surface::execute_project_surface_point_read_by_tag(
            &read_session,
            &read_tag,
            &point_read_request(&runtime, 1),
        )
        .expect("read persisted update after reopening read-only");
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune Revised".into()
            })
        );
    }

    #[test]
    fn surface_operation_envelope_dispatches_project_read_and_update() {
        let root = TempProject::new("marrow-json-project-surface-operation");
        write_native_project(&root, PROJECT_UPDATE_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let runtime = session.program().runtime();
        let surface = surface_id(session.program(), "Books");
        let read_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let update_tag = update_operation_tag(session.program(), "Books");

        let read_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: read_tag.clone(),
                request: SurfaceOperationRequestBodyJson::PointRead {
                    request: point_read_request(&runtime, 1),
                },
            },
        )
        .expect("operation envelope executes point read");
        assert_eq!(
            read_response.profile_version,
            SURFACE_OPERATION_PROFILE_VERSION
        );
        assert_eq!(read_response.operation_tag, read_tag);
        let SurfaceOperationResultJson::Record { record } = read_response.result else {
            panic!("expected record result: {read_response:?}");
        };
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune".into()
            })
        );

        let update_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: update_tag.clone(),
                request: SurfaceOperationRequestBodyJson::PointUpdate {
                    request: point_update_request(
                        &runtime,
                        1,
                        vec![update_field(
                            field_catalog_id(&runtime, "books", "title"),
                            SurfaceWriteValueJson::String {
                                value: "Dune Revised".into(),
                            },
                        )],
                    ),
                },
            },
        )
        .expect("operation envelope executes point update");
        assert_eq!(
            update_response.profile_version,
            SURFACE_OPERATION_PROFILE_VERSION
        );
        assert_eq!(update_response.operation_tag, update_tag);
        assert_eq!(update_response.result, SurfaceOperationResultJson::Updated);

        let verify_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: read_tag,
                request: SurfaceOperationRequestBodyJson::PointRead {
                    request: point_read_request(&runtime, 1),
                },
            },
        )
        .expect("operation envelope reads updated record");
        let SurfaceOperationResultJson::Record { record } = verify_response.result else {
            panic!("expected record result: {verify_response:?}");
        };
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune Revised".into()
            })
        );
    }

    #[test]
    fn surface_operation_envelope_dispatches_project_create_and_delete() {
        let root = TempProject::new("marrow-json-project-surface-create-delete");
        write_native_project(&root, PROJECT_CREATE_DELETE_SURFACE);
        seed_project(&root, "shelf::seed");

        let read_session =
            ProjectSurfaceReadSession::open(root.path()).expect("open project read session");
        let runtime = read_session.program().runtime();
        let create_tag = create_operation_tag(read_session.program(), "Books");
        let read_only_create = SurfaceOperationRequestJson {
            profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
            operation_tag: create_tag.clone(),
            request: SurfaceOperationRequestBodyJson::PointCreate {
                request: point_create_request(
                    &runtime,
                    1,
                    vec![
                        create_field(
                            field_catalog_id(&runtime, "books", "title"),
                            SurfaceWriteValueJson::String {
                                value: "Dune".into(),
                            },
                        ),
                        create_field(
                            field_catalog_id(&runtime, "books", "author"),
                            SurfaceWriteValueJson::String {
                                value: "Frank".into(),
                            },
                        ),
                    ],
                ),
            },
        };
        assert_operation_error(
            crate::surface::execute_project_surface_operation_read_only(
                &read_session,
                &read_only_create,
            ),
            SURFACE_ABI_MISMATCH,
        );
        drop(read_session);

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let runtime = session.program().runtime();
        let surface = surface_id(session.program(), "Books");
        let read_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let create_tag = create_operation_tag(session.program(), "Books");
        let delete_tag = delete_operation_tag(session.program(), "Books");

        let create_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: create_tag.clone(),
                request: SurfaceOperationRequestBodyJson::PointCreate {
                    request: point_create_request(
                        &runtime,
                        1,
                        vec![
                            create_field(
                                field_catalog_id(&runtime, "books", "title"),
                                SurfaceWriteValueJson::String {
                                    value: "Dune".into(),
                                },
                            ),
                            create_field(
                                field_catalog_id(&runtime, "books", "author"),
                                SurfaceWriteValueJson::String {
                                    value: "Frank".into(),
                                },
                            ),
                        ],
                    ),
                },
            },
        )
        .expect("operation envelope executes point create");
        assert_eq!(create_response.operation_tag, create_tag);
        let SurfaceOperationResultJson::Created { record } = create_response.result else {
            panic!("expected created result: {create_response:?}");
        };
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune".into()
            })
        );

        let verify_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: read_tag.clone(),
                request: SurfaceOperationRequestBodyJson::PointRead {
                    request: point_read_request(&runtime, 1),
                },
            },
        )
        .expect("operation envelope reads created record");
        assert!(matches!(
            verify_response.result,
            SurfaceOperationResultJson::Record { .. }
        ));

        let delete_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: delete_tag.clone(),
                request: SurfaceOperationRequestBodyJson::PointDelete {
                    request: point_delete_request(&runtime, 1),
                },
            },
        )
        .expect("operation envelope executes point delete");
        assert_eq!(delete_response.operation_tag, delete_tag);
        assert_eq!(delete_response.result, SurfaceOperationResultJson::Deleted);
        assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: read_tag,
                    request: SurfaceOperationRequestBodyJson::PointRead {
                        request: point_read_request(&runtime, 1),
                    },
                },
            ),
            SURFACE_ABSENT,
        );
    }

    #[test]
    fn surface_operation_envelope_dispatches_project_action() {
        let root = TempProject::new("marrow-json-project-surface-action");
        write_native_project(&root, PROJECT_ACTION_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let before = session
            .store_stamp()
            .expect("project surface session exposes the store stamp");
        let runtime = session.program().runtime();
        let surface = surface_id(session.program(), "Books");
        let action_tag = checker_action_operation_tag(session.program(), "Books", "rename");
        let read_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });

        let response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: action_tag.clone(),
                request: SurfaceOperationRequestBodyJson::Action {
                    request: SurfaceActionRequestJson {
                        arguments: vec![
                            json!({
                                "name": "id",
                                "value": { "kind": "int", "value": "1" }
                            }),
                            json!({
                                "name": "title",
                                "value": { "kind": "string", "value": "Dune Action" }
                            }),
                        ],
                    },
                },
            },
        )
        .expect("operation envelope executes action");

        assert_eq!(response.operation_tag, action_tag);
        let SurfaceOperationResultJson::Action { result } = response.result else {
            panic!("expected action result: {response:?}");
        };
        assert_eq!(result.output, "");
        assert_eq!(
            result.value,
            Some(SurfaceActionValueJson::String {
                value: "Dune Action".into()
            })
        );

        let verify_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: read_tag,
                request: SurfaceOperationRequestBodyJson::PointRead {
                    request: point_read_request(&runtime, 1),
                },
            },
        )
        .expect("operation envelope reads action-updated record");
        let SurfaceOperationResultJson::Record { record } = verify_response.result else {
            panic!("expected record result: {verify_response:?}");
        };
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune Action".into()
            })
        );
        let after = session
            .store_stamp()
            .expect("project surface session exposes the updated store stamp");
        assert_eq!(after.store_uid, before.store_uid);
        assert_eq!(after.catalog_epoch, before.catalog_epoch);
        assert!(after.commit_id > before.commit_id);
    }

    #[test]
    fn surface_operation_envelope_renders_action_identity_result_with_catalog_ids() {
        let root = TempProject::new("marrow-json-project-surface-action-identity");
        write_native_project(&root, PROJECT_ACTION_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let runtime = session.program().runtime();
        let action_tag = checker_action_operation_tag(session.program(), "Books", "current");

        let response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: action_tag,
                request: SurfaceOperationRequestBodyJson::Action {
                    request: SurfaceActionRequestJson {
                        arguments: Vec::new(),
                    },
                },
            },
        )
        .expect("operation envelope executes identity action");

        let SurfaceOperationResultJson::Action { result } = response.result else {
            panic!("expected action result: {response:?}");
        };
        assert_eq!(result.output, "");
        assert_eq!(
            result.value,
            Some(SurfaceActionValueJson::Identity {
                store_catalog_id: store_catalog_id(&runtime, "books").as_str().into(),
                keys: vec![SurfaceKeyJson::Int { value: "1".into() }],
            })
        );
    }

    #[test]
    fn surface_operation_envelope_dispatches_computed_read_on_read_only_session() {
        let root = TempProject::new("marrow-json-project-surface-computed-read");
        write_native_project(&root, PROJECT_COMPUTED_READ_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceReadSession::open(root.path()).expect("open read-only surface session");
        let computed_tag = checker_computed_read_operation_tag(session.program(), "Books", "page");

        let response = crate::surface::execute_project_surface_operation_read_only(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: computed_tag.clone(),
                request: SurfaceOperationRequestBodyJson::ComputedRead {
                    request: SurfaceComputedReadRequestJson {
                        arguments: vec![json!({
                            "name": "id",
                            "value": { "kind": "int", "value": "1" }
                        })],
                    },
                },
            },
        )
        .expect("operation envelope executes computed read");

        assert_eq!(response.operation_tag, computed_tag);
        let SurfaceOperationResultJson::ComputedRead { result } = response.result else {
            panic!("expected computed-read result: {response:?}");
        };
        assert_eq!(result.output, "");
        let Some(SurfaceComputedReadValueJson::Resource { fields, .. }) = result.value else {
            panic!("expected resource value: {result:?}");
        };
        let title = fields
            .iter()
            .find(|field| field.render_label == "title")
            .expect("title field");
        assert_eq!(title.render_label, "title");
        assert!(title.member_catalog_id.starts_with("cat_"));
        assert_eq!(
            title.value,
            Some(SurfaceComputedReadValueJson::String {
                value: "Dune".into(),
            })
        );
    }

    #[test]
    fn surface_operation_envelope_can_execute_actions_with_explicit_host_capabilities() {
        let root = TempProject::new("marrow-json-project-surface-action-host");
        write_native_project(&root, PROJECT_HOST_ACTION_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let action_tag = checker_action_operation_tag(session.program(), "Books", "now");
        let request = SurfaceOperationRequestJson {
            profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
            operation_tag: action_tag,
            request: SurfaceOperationRequestBodyJson::Action {
                request: SurfaceActionRequestJson {
                    arguments: Vec::new(),
                },
            },
        };
        assert_operation_error(
            crate::surface::execute_project_surface_operation(&session, &request),
            SURFACE_ACTION,
        );

        let host = Host::new().with_clock(1_700_000_000_000_000_000);
        let response =
            crate::surface::execute_project_surface_operation_with_host(&session, &request, &host)
                .expect("operation envelope executes action with explicit host");

        let SurfaceOperationResultJson::Action { result } = response.result else {
            panic!("expected action result: {response:?}");
        };
        assert_eq!(
            result.value,
            Some(SurfaceActionValueJson::Instant {
                nanos_since_epoch: "1700000000000000000".into()
            })
        );
    }

    #[test]
    fn surface_operation_envelope_rejects_action_on_read_only_session() {
        let root = TempProject::new("marrow-json-project-surface-action-read-only");
        write_native_project(&root, PROJECT_ACTION_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceReadSession::open(root.path()).expect("open project surface session");
        let action_tag = checker_action_operation_tag(session.program(), "Books", "rename");

        assert_operation_error(
            crate::surface::execute_project_surface_operation_read_only(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: action_tag,
                    request: SurfaceOperationRequestBodyJson::Action {
                        request: SurfaceActionRequestJson {
                            arguments: Vec::new(),
                        },
                    },
                },
            ),
            SURFACE_ABI_MISMATCH,
        );
    }

    #[test]
    fn surface_operation_envelope_rejects_public_function_not_declared_as_action() {
        let root = TempProject::new("marrow-json-project-surface-unlisted-action");
        write_native_project(&root, PROJECT_ACTION_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let runtime_program = session.program().runtime();
        let stealth_tag = EntryDescriptor::resolve(&runtime_program, "shelf::stealthRename")
            .expect("unlisted public entry descriptor")
            .identity
            .entry_tag;
        assert_eq!(
            session
                .admit_action_by_operation_tag(&stealth_tag)
                .expect_err("unlisted public entry tag is not a surface action")
                .code(),
            SURFACE_ABI_MISMATCH
        );

        assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: stealth_tag,
                    request: SurfaceOperationRequestBodyJson::Action {
                        request: SurfaceActionRequestJson {
                            arguments: vec![
                                json!({
                                    "name": "id",
                                    "value": { "kind": "int", "value": "1" }
                                }),
                                json!({
                                    "name": "title",
                                    "value": { "kind": "string", "value": "Bypass" }
                                }),
                            ],
                        },
                    },
                },
            ),
            SURFACE_ABI_MISMATCH,
        );

        let runtime = session.program().runtime();
        let surface = surface_id(session.program(), "Books");
        let read_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let verify_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: read_tag,
                request: SurfaceOperationRequestBodyJson::PointRead {
                    request: point_read_request(&runtime, 1),
                },
            },
        )
        .expect("operation envelope reads unchanged record");
        let SurfaceOperationResultJson::Record { record } = verify_response.result else {
            panic!("expected record result: {verify_response:?}");
        };
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune".into()
            })
        );
    }

    #[test]
    fn project_surface_session_rechecks_stale_action_handles_against_current_surface() {
        let old_root = TempProject::new("marrow-json-project-surface-old-action-handle");
        write_native_project(&old_root, PROJECT_STEALTH_ACTION_SURFACE);
        seed_project(&old_root, "shelf::seed");
        let old_session =
            ProjectSurfaceSession::open(old_root.path()).expect("open old project surface session");
        let old_tag = checker_action_operation_tag(old_session.program(), "Books", "stealth");
        let old_action = old_session
            .admit_action_by_operation_tag(&old_tag)
            .expect("old surface action admits");

        let root = TempProject::new("marrow-json-project-surface-current-action-handle");
        write_native_project(&root, PROJECT_ACTION_SURFACE);
        seed_project(&root, "shelf::seed");
        let session =
            ProjectSurfaceSession::open(root.path()).expect("open current project surface session");
        let arguments = entry_arguments_from_json(&[
            json!({
                "name": "id",
                "value": { "kind": "int", "value": "1" }
            }),
            json!({
                "name": "title",
                "value": { "kind": "string", "value": "Stale Handle" }
            }),
        ])
        .expect("decode action arguments");
        let host = Host::new();
        let mut output = String::new();
        assert_eq!(
            session
                .invoke_action(&old_action, arguments, &host, &mut output)
                .expect_err("stale surface action handle is re-admitted")
                .code(),
            SURFACE_ABI_MISMATCH
        );
        assert_eq!(output, "");

        let runtime = session.program().runtime();
        let surface = surface_id(session.program(), "Books");
        let read_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let verify_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: read_tag,
                request: SurfaceOperationRequestBodyJson::PointRead {
                    request: point_read_request(&runtime, 1),
                },
            },
        )
        .expect("operation envelope reads unchanged record");
        let SurfaceOperationResultJson::Record { record } = verify_response.result else {
            panic!("expected record result: {verify_response:?}");
        };
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "title")),
            Some(&SurfaceValueJson::String {
                value: "Dune".into()
            })
        );
    }

    #[test]
    fn surface_operation_envelope_sanitizes_action_runtime_failure() {
        let root = TempProject::new("marrow-json-project-surface-action-failure");
        write_native_project(&root, PROJECT_ACTION_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let action_tag = checker_action_operation_tag(session.program(), "Books", "fail");

        assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: action_tag,
                    request: SurfaceOperationRequestBodyJson::Action {
                        request: SurfaceActionRequestJson {
                            arguments: Vec::new(),
                        },
                    },
                },
            ),
            SURFACE_ACTION,
        );
    }

    #[test]
    fn surface_operation_envelope_page_cursor_stales_after_update() {
        let root = TempProject::new("marrow-json-project-surface-operation-stale-cursor");
        write_native_project(&root, PROJECT_COLLECTION_UPDATE_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let before = session
            .store_stamp()
            .expect("project surface session exposes the store stamp");
        let runtime = session.program().runtime();
        let surface = surface_id(session.program(), "Books");
        let page_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PagedRootCollection { .. })
        });
        let update_tag = update_operation_tag(session.program(), "Books");

        let page_response = crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: page_tag.clone(),
                request: SurfaceOperationRequestBodyJson::Page {
                    request: SurfacePageRequestJson {
                        exact_keys: Vec::new(),
                        limit: 1,
                        cursor: None,
                    },
                },
            },
        )
        .expect("first operation-envelope page");
        let SurfaceOperationResultJson::Page { page } = page_response.result else {
            panic!("expected page result: {page_response:?}");
        };
        let cursor = page.next.expect("first page has cursor");
        assert_eq!(cursor.commit_id, Some(before.commit_id));

        crate::surface::execute_project_surface_operation(
            &session,
            &SurfaceOperationRequestJson {
                profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                operation_tag: update_tag,
                request: SurfaceOperationRequestBodyJson::PointUpdate {
                    request: point_update_request(
                        &runtime,
                        2,
                        vec![update_field(
                            field_catalog_id(&runtime, "books", "title"),
                            SurfaceWriteValueJson::String {
                                value: "Dune Messiah Revised".into(),
                            },
                        )],
                    ),
                },
            },
        )
        .expect("operation-envelope update");
        let after = session
            .store_stamp()
            .expect("project surface session exposes updated stamp");
        assert_eq!(after.commit_id, before.commit_id + 1);

        assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: page_tag,
                    request: SurfaceOperationRequestBodyJson::Page {
                        request: SurfacePageRequestJson {
                            exact_keys: Vec::new(),
                            limit: 10,
                            cursor: Some(cursor),
                        },
                    },
                },
            ),
            SURFACE_STALE_CURSOR,
        );
    }

    #[test]
    fn surface_operation_envelope_pins_json_wire_shape() {
        let wire = json!({
            "profile_version": SURFACE_OPERATION_PROFILE_VERSION,
            "operation_tag": "tag-1",
            "request": {
                "kind": "point_read",
                "request": {
                    "identity": {
                        "store_catalog_id": "cat_00000000000000000000000000000001",
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        });
        let request = serde_json::from_value::<SurfaceOperationRequestJson>(wire.clone())
            .expect("operation request wire shape decodes");
        assert_eq!(request.profile_version, SURFACE_OPERATION_PROFILE_VERSION);
        assert_eq!(request.operation_tag, "tag-1");
        assert_eq!(
            serde_json::to_value(&request).expect("request encodes"),
            wire
        );

        let action_wire = json!({
            "profile_version": SURFACE_OPERATION_PROFILE_VERSION,
            "operation_tag": "tag-action",
            "request": {
                "kind": "action",
                "request": {
                    "arguments": [
                        {
                            "name": "title",
                            "value": { "kind": "string", "value": "Dune" }
                        }
                    ]
                }
            }
        });
        let action_request =
            serde_json::from_value::<SurfaceOperationRequestJson>(action_wire.clone())
                .expect("action operation request wire shape decodes");
        assert_eq!(
            serde_json::to_value(&action_request).expect("action request encodes"),
            action_wire
        );

        let computed_read_wire = json!({
            "profile_version": SURFACE_OPERATION_PROFILE_VERSION,
            "operation_tag": "tag-computed",
            "request": {
                "kind": "computed_read",
                "request": {
                    "arguments": [
                        {
                            "name": "id",
                            "value": { "kind": "int", "value": "1" }
                        }
                    ]
                }
            }
        });
        let computed_read_request =
            serde_json::from_value::<SurfaceOperationRequestJson>(computed_read_wire.clone())
                .expect("computed-read operation request wire shape decodes");
        assert_eq!(
            serde_json::to_value(&computed_read_request).expect("computed-read request encodes"),
            computed_read_wire
        );

        let response = crate::surface::SurfaceOperationResponseJson {
            profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
            operation_tag: "tag-2".into(),
            result: SurfaceOperationResultJson::Updated,
        };
        assert_eq!(
            serde_json::to_value(response).expect("response encodes"),
            json!({
                "profile_version": SURFACE_OPERATION_PROFILE_VERSION,
                "operation_tag": "tag-2",
                "result": { "kind": "updated" }
            })
        );

        let action_response = crate::surface::SurfaceOperationResponseJson {
            profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
            operation_tag: "tag-action".into(),
            result: SurfaceOperationResultJson::Action {
                result: SurfaceActionResultJson {
                    output: String::new(),
                    value: Some(SurfaceActionValueJson::String {
                        value: "Dune".into(),
                    }),
                },
            },
        };
        assert_eq!(
            serde_json::to_value(action_response).expect("action response encodes"),
            json!({
                "profile_version": SURFACE_OPERATION_PROFILE_VERSION,
                "operation_tag": "tag-action",
                "result": {
                    "kind": "action",
                    "result": {
                        "output": "",
                        "value": { "kind": "string", "value": "Dune" }
                    }
                }
            })
        );

        let computed_response = crate::surface::SurfaceOperationResponseJson {
            profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
            operation_tag: "tag-computed".into(),
            result: SurfaceOperationResultJson::ComputedRead {
                result: crate::surface::SurfaceComputedReadInvocationResultJson {
                    output: String::new(),
                    value: Some(SurfaceComputedReadValueJson::Resource {
                        resource_catalog_id: "cat_00000000000000000000000000000002".into(),
                        fields: vec![SurfaceComputedReadFieldValueJson {
                            render_label: "title".into(),
                            member_catalog_id: "cat_00000000000000000000000000000003".into(),
                            required: true,
                            value: Some(SurfaceComputedReadValueJson::String {
                                value: "Dune".into(),
                            }),
                        }],
                    }),
                },
            },
        };
        assert_eq!(
            serde_json::to_value(computed_response).expect("computed-read response encodes"),
            json!({
                "profile_version": SURFACE_OPERATION_PROFILE_VERSION,
                "operation_tag": "tag-computed",
                "result": {
                    "kind": "computed_read",
                    "result": {
                        "output": "",
                        "value": {
                            "kind": "resource",
                            "resource_catalog_id": "cat_00000000000000000000000000000002",
                            "fields": [{
                                "render_label": "title",
                                "member_catalog_id": "cat_00000000000000000000000000000003",
                                "required": true,
                                "value": { "kind": "string", "value": "Dune" }
                            }]
                        }
                    }
                }
            })
        );

        let error = SurfaceOperationErrorJson {
            code: SURFACE_REQUEST.into(),
            message: "surface request is malformed".into(),
        };
        assert_eq!(
            serde_json::to_value(error).expect("error encodes"),
            json!({
                "code": SURFACE_REQUEST,
                "message": "surface request is malformed"
            })
        );
    }

    #[test]
    fn surface_operation_envelope_rejects_wrong_profile_version() {
        let root = TempProject::new("marrow-json-project-surface-operation-profile");
        write_native_project(&root, PROJECT_UPDATE_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let runtime = session.program().runtime();
        let surface = surface_id(session.program(), "Books");
        let read_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });

        let error = assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: "surface.operation.v0".into(),
                    operation_tag: read_tag,
                    request: SurfaceOperationRequestBodyJson::PointRead {
                        request: point_read_request(&runtime, 1),
                    },
                },
            ),
            SURFACE_ABI_MISMATCH,
        );
        assert_eq!(
            error.message,
            "surface operation profile version is not active"
        );
    }

    #[test]
    fn surface_operation_envelope_read_only_session_rejects_update_body() {
        let root = TempProject::new("marrow-json-project-surface-operation-read-only");
        write_native_project(&root, PROJECT_UPDATE_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceReadSession::open(root.path()).expect("open read-only surface session");
        let runtime = session.program().runtime();
        let update_tag = update_operation_tag(session.program(), "Books");

        let error = assert_operation_error(
            crate::surface::execute_project_surface_operation_read_only(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: update_tag,
                    request: SurfaceOperationRequestBodyJson::PointUpdate {
                        request: point_update_request(
                            &runtime,
                            1,
                            vec![update_field(
                                field_catalog_id(&runtime, "books", "title"),
                                SurfaceWriteValueJson::String {
                                    value: "Dune Revised".into(),
                                },
                            )],
                        ),
                    },
                },
            ),
            SURFACE_ABI_MISMATCH,
        );
        assert_eq!(
            error.message,
            "surface operation request requires a writable project surface session"
        );
    }

    #[test]
    fn surface_operation_envelope_wrong_request_body_kind_returns_surface_request() {
        let root = TempProject::new("marrow-json-project-surface-operation-body-kind");
        write_native_project(&root, PROJECT_UPDATE_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let surface = surface_id(session.program(), "Books");
        let point_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });

        assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: point_tag,
                    request: SurfaceOperationRequestBodyJson::Page {
                        request: SurfacePageRequestJson {
                            exact_keys: Vec::new(),
                            limit: 1,
                            cursor: None,
                        },
                    },
                },
            ),
            SURFACE_REQUEST,
        );
    }

    #[test]
    fn surface_operation_envelope_unknown_tag_returns_surface_abi_mismatch() {
        let root = TempProject::new("marrow-json-project-surface-operation-unknown-tag");
        write_native_project(&root, PROJECT_UPDATE_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let runtime = session.program().runtime();

        assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag:
                        "0000000000000000000000000000000000000000000000000000000000000000".into(),
                    request: SurfaceOperationRequestBodyJson::PointRead {
                        request: point_read_request(&runtime, 1),
                    },
                },
            ),
            SURFACE_ABI_MISMATCH,
        );
    }

    #[test]
    fn surface_operation_error_envelope_is_sanitized_code_and_message() {
        let root = TempProject::new("marrow-json-project-surface-operation-error-envelope");
        write_native_project(&root, PROJECT_UPDATE_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let surface = surface_id(session.program(), "Books");
        let point_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });

        let error = assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: point_tag,
                    request: SurfaceOperationRequestBodyJson::Page {
                        request: SurfacePageRequestJson {
                            exact_keys: Vec::new(),
                            limit: 1,
                            cursor: None,
                        },
                    },
                },
            ),
            SURFACE_REQUEST,
        );
        let value = serde_json::to_value(&error).expect("error envelope serializes");
        let object = value.as_object().expect("error envelope object");
        assert_eq!(object.len(), 2);
        assert_eq!(object.get("code"), Some(&json!(SURFACE_REQUEST)));
        assert!(object.contains_key("message"));
        let rendered = value.to_string();
        for forbidden in [
            "span",
            "SourceSpan",
            "shelf.mw",
            ".data",
            "marrow.redb",
            root.path().to_str().expect("temp path"),
        ] {
            assert!(
                !rendered.contains(forbidden),
                "surface operation error envelope leaked {forbidden}: {rendered}"
            );
        }
    }

    #[test]
    fn surface_operation_error_envelope_sanitizes_conflicts() {
        let root = TempProject::new("marrow-json-project-surface-operation-conflict");
        write_native_project(&root, PROJECT_UNIQUE_UPDATE_SURFACE);
        seed_project(&root, "shelf::seed");

        let session =
            ProjectSurfaceSession::open(root.path()).expect("open project surface session");
        let runtime = session.program().runtime();
        let update_tag = update_operation_tag(session.program(), "Books");

        let error = assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: update_tag,
                    request: SurfaceOperationRequestBodyJson::PointUpdate {
                        request: point_update_request(
                            &runtime,
                            2,
                            vec![update_field(
                                field_catalog_id(&runtime, "books", "isbn"),
                                SurfaceWriteValueJson::String {
                                    value: "isbn-a1".into(),
                                },
                            )],
                        ),
                    },
                },
            ),
            SURFACE_CONFLICT,
        );

        assert_eq!(
            error.message,
            "surface operation conflicts with existing saved data"
        );
        let rendered = serde_json::to_value(&error)
            .expect("error envelope serializes")
            .to_string();
        for forbidden in ["byIsbn", "isbn-a1", "isbn-a2", "key", "identity"] {
            assert!(
                !rendered.contains(forbidden),
                "surface operation error envelope leaked {forbidden}: {rendered}"
            );
        }
    }

    #[test]
    fn surface_operation_error_envelope_sanitizes_invalid_saved_data_faults() {
        let (program, runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "draft", 7);
        let read = SurfaceNodeRead::admit(&program, &store, surface_id(&program, "Books"))
            .expect("admit point read");

        let error = read
            .read_point(&[SavedKey::Int(1)])
            .expect_err("missing required private field faults");
        assert_eq!(error.code(), SURFACE_INVALID_DATA);
        let envelope = SurfaceOperationErrorJson::from(error);

        assert_eq!(envelope.code, SURFACE_INVALID_DATA);
        assert_eq!(
            envelope.message,
            "surface operation reached invalid saved data"
        );
        let rendered = serde_json::to_value(&envelope)
            .expect("error envelope serializes")
            .to_string();
        for forbidden in ["privateCode", "required", "backing", "field"] {
            assert!(
                !rendered.contains(forbidden),
                "surface operation error envelope leaked {forbidden}: {rendered}"
            );
        }
    }

    #[test]
    fn surface_operation_error_envelope_sanitizes_hidden_field_limit_faults() {
        let root = TempProject::new("marrow-json-project-surface-operation-limit");
        write_native_project(&root, PROJECT_PRIVATE_BACKING_SURFACE);
        seed_project(&root, "shelf::seed");

        let runtime = {
            let session =
                ProjectSurfaceSession::open(root.path()).expect("open project surface session");
            session.program().runtime()
        };
        let store_path = root.path().join(".data").join("marrow.redb");
        {
            let store = TreeStore::open(&store_path).expect("open native store for corruption");
            write_data_value(
                &runtime,
                &store,
                "books",
                &[SavedKey::Int(1)],
                &data_path(&runtime, "books", &["privateCode"]),
                SavedValue::Str("x".repeat(SURFACE_MAX_VALUE_BYTES + 1)),
            );
        }

        let session =
            ProjectSurfaceSession::open(root.path()).expect("reopen project surface session");
        let surface = surface_id(session.program(), "Books");
        let read_tag = read_operation_tag_matching(session.program(), surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let error = assert_operation_error(
            crate::surface::execute_project_surface_operation(
                &session,
                &SurfaceOperationRequestJson {
                    profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
                    operation_tag: read_tag,
                    request: SurfaceOperationRequestBodyJson::PointRead {
                        request: point_read_request(&runtime, 1),
                    },
                },
            ),
            SURFACE_LIMIT,
        );

        assert_eq!(error.message, "surface operation exceeded a public limit");
        let rendered = serde_json::to_value(&error)
            .expect("error envelope serializes")
            .to_string();
        for forbidden in ["privateCode", "stored value", "byte budget"] {
            assert!(
                !rendered.contains(forbidden),
                "surface operation error envelope leaked {forbidden}: {rendered}"
            );
        }
    }

    #[test]
    fn surface_execute_singleton_read_by_operation_tag_returns_record_json() {
        let (program, runtime) = checked_surface_program(SINGLETON_UPDATE_SURFACE);
        let store = admitted_store(&program);
        write_data_value(
            &runtime,
            &store,
            "settings",
            &[],
            &data_path(&runtime, "settings", &["theme"]),
            SavedValue::Str("dark".into()),
        );
        write_data_value(
            &runtime,
            &store,
            "settings",
            &[],
            &data_path(&runtime, "settings", &["mode"]),
            SavedValue::Str("compact".into()),
        );

        let surface = surface_id(&program, "SettingsSurface");
        let operation_tag = read_operation_tag_matching(&program, surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::SingletonRead { .. })
        });
        let record =
            crate::surface::execute_surface_singleton_read_by_tag(&program, &store, &operation_tag)
                .expect("execute singleton read");

        assert_eq!(record.identity, None);
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "settings", "mode")),
            Some(&SurfaceValueJson::String {
                value: "compact".into()
            })
        );
    }

    #[test]
    fn surface_execute_paged_collection_by_operation_tag_returns_page_json() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "published", 7);
        write_surface_book(&runtime, &store, 2, "Dune Messiah", "published", 7);

        let surface = surface_id(&program, "Books");
        let operation_tag = read_operation_tag(
            &program,
            index_collection_ref(&program, surface, "byStatusAuthor"),
        );
        let page = crate::surface::execute_surface_page_by_tag(
            &program,
            &store,
            &operation_tag,
            &book_page_request(&runtime, 7, 1),
        )
        .expect("execute page read");

        assert_eq!(page.rows.len(), 1);
        assert_eq!(
            page.rows[0].identity.as_ref().expect("row identity").keys,
            vec![SurfaceKeyJson::Int { value: "1".into() }]
        );
        assert!(page.next.is_some(), "first limited page returns a cursor");
    }

    #[test]
    fn surface_execute_unique_lookup_by_operation_tag_returns_optional_record_json() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_UNIQUE_INDEX);
        let store = admitted_store(&program);
        write_surface_book_with_isbn(&runtime, &store, 1, "Dune", "isbn-a1");

        let surface = surface_id(&program, "Books");
        let operation_tag =
            read_operation_tag(&program, index_collection_ref(&program, surface, "byIsbn"));
        let found = crate::surface::execute_surface_unique_lookup_by_tag(
            &program,
            &store,
            &operation_tag,
            &SurfaceUniqueLookupRequestJson {
                keys: vec![SurfaceArgumentJson::String {
                    value: "isbn-a1".into(),
                }],
            },
        )
        .expect("execute unique lookup")
        .expect("record found");

        assert_eq!(
            found.identity.expect("record identity").keys,
            vec![SurfaceKeyJson::Int { value: "1".into() }]
        );

        assert_eq!(
            crate::surface::execute_surface_unique_lookup_by_tag(
                &program,
                &store,
                &operation_tag,
                &SurfaceUniqueLookupRequestJson {
                    keys: vec![SurfaceArgumentJson::String {
                        value: "missing".into(),
                    }],
                },
            )
            .expect("execute absent unique lookup"),
            None
        );
    }

    #[test]
    fn surface_execute_updates_by_operation_tag_apply_existing_update_dtos() {
        let (program, runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "draft", 7);
        write_surface_book_private_code(&runtime, &store, 1, "internal");
        let operation_tag = update_operation_tag(&program, "Books");

        crate::surface::execute_surface_point_update_by_tag(
            &program,
            &store,
            &operation_tag,
            &point_update_request(
                &runtime,
                1,
                vec![update_field(
                    field_catalog_id(&runtime, "books", "status"),
                    SurfaceWriteValueJson::Enum {
                        enum_catalog_id: enum_catalog_id(&runtime, "Status").as_str().into(),
                        member_catalog_id: enum_member_catalog_id(&runtime, "Status", "published")
                            .as_str()
                            .into(),
                    },
                )],
            ),
        )
        .expect("execute point update");

        let read_tag =
            read_operation_tag_matching(&program, surface_id(&program, "Books"), |kind| {
                matches!(kind, SurfaceReadOperationKind::PointRead { .. })
            });
        let record = crate::surface::execute_surface_point_read_by_tag(
            &program,
            &store,
            &read_tag,
            &point_read_request(&runtime, 1),
        )
        .expect("read updated record");
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "books", "status")),
            Some(&SurfaceValueJson::Enum {
                enum_catalog_id: enum_catalog_id(&runtime, "Status").as_str().into(),
                member_catalog_id: enum_member_catalog_id(&runtime, "Status", "published")
                    .as_str()
                    .into(),
                render_label: "published".into(),
            })
        );

        let (program, runtime) = checked_surface_program(SINGLETON_UPDATE_SURFACE);
        let store = admitted_store(&program);
        write_data_value(
            &runtime,
            &store,
            "settings",
            &[],
            &data_path(&runtime, "settings", &["theme"]),
            SavedValue::Str("dark".into()),
        );
        let operation_tag = update_operation_tag(&program, "SettingsSurface");

        crate::surface::execute_surface_singleton_update_by_tag(
            &program,
            &store,
            &operation_tag,
            &SurfaceSingletonUpdateRequestJson {
                fields: vec![update_field(
                    field_catalog_id(&runtime, "settings", "mode"),
                    SurfaceWriteValueJson::String {
                        value: "compact".into(),
                    },
                )],
            },
        )
        .expect("execute singleton update");

        let read_tag = read_operation_tag_matching(
            &program,
            surface_id(&program, "SettingsSurface"),
            |kind| matches!(kind, SurfaceReadOperationKind::SingletonRead { .. }),
        );
        let record =
            crate::surface::execute_surface_singleton_read_by_tag(&program, &store, &read_tag)
                .expect("read updated singleton");
        assert_eq!(
            field_value(&record, &field_catalog_id(&runtime, "settings", "mode")),
            Some(&SurfaceValueJson::String {
                value: "compact".into()
            })
        );
    }

    #[test]
    fn surface_execute_rejects_wrong_profile_wrong_kind_and_unknown_tags() {
        let (program, runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "draft", 7);
        write_surface_book_private_code(&runtime, &store, 1, "internal");

        let surface = surface_id(&program, "Books");
        let point_tag = read_operation_tag_matching(&program, surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let collection_tag = read_operation_tag(
            &program,
            index_collection_ref(&program, surface, "byStatusAuthor"),
        );
        let update_tag = update_operation_tag(&program, "Books");

        assert_surface_error(
            crate::surface::execute_surface_point_update_by_tag(
                &program,
                &store,
                &point_tag,
                &point_update_request(&runtime, 1, Vec::new()),
            ),
            SURFACE_ABI_MISMATCH,
        );
        assert_surface_error(
            crate::surface::execute_surface_point_read_by_tag(
                &program,
                &store,
                &update_tag,
                &point_read_request(&runtime, 1),
            ),
            SURFACE_ABI_MISMATCH,
        );
        assert_surface_error(
            crate::surface::execute_surface_point_read_by_tag(
                &program,
                &store,
                &collection_tag,
                &point_read_request(&runtime, 1),
            ),
            SURFACE_REQUEST,
        );
        assert_surface_error(
            crate::surface::execute_surface_page_by_tag(
                &program,
                &store,
                &point_tag,
                &book_page_request(&runtime, 7, 1),
            ),
            SURFACE_REQUEST,
        );
        assert_surface_error(
            crate::surface::execute_surface_point_read_by_tag(
                &program,
                &store,
                "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                &point_read_request(&runtime, 1),
            ),
            SURFACE_ABI_MISMATCH,
        );
    }

    #[test]
    fn surface_execute_page_cursor_round_trips_and_mismatched_tag_stays_stale_cursor() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "published", 7);
        write_surface_book(&runtime, &store, 2, "Dune Messiah", "published", 7);

        let surface = surface_id(&program, "Books");
        let point_tag = read_operation_tag_matching(&program, surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PointRead { .. })
        });
        let root_tag = read_operation_tag_matching(&program, surface, |kind| {
            matches!(kind, SurfaceReadOperationKind::PagedRootCollection { .. })
        });
        let collection_tag = read_operation_tag(
            &program,
            index_collection_ref(&program, surface, "byStatusAuthor"),
        );

        let first_page = crate::surface::execute_surface_page_by_tag(
            &program,
            &store,
            &collection_tag,
            &book_page_request(&runtime, 7, 1),
        )
        .expect("first page");
        let cursor = first_page.next.expect("first page has cursor");
        let second_page = crate::surface::execute_surface_page_by_tag(
            &program,
            &store,
            &collection_tag,
            &SurfacePageRequestJson {
                cursor: Some(cursor.clone()),
                ..book_page_request(&runtime, 7, 10)
            },
        )
        .expect("second page");
        assert_eq!(
            second_page.rows[0]
                .identity
                .as_ref()
                .expect("row identity")
                .keys,
            vec![SurfaceKeyJson::Int { value: "2".into() }]
        );

        let wrong_cursor = SurfaceCursorJson {
            operation_tag: point_tag,
            ..cursor
        };
        assert_surface_error(
            crate::surface::execute_surface_page_by_tag(
                &program,
                &store,
                &collection_tag,
                &SurfacePageRequestJson {
                    cursor: Some(wrong_cursor),
                    ..book_page_request(&runtime, 7, 10)
                },
            ),
            SURFACE_STALE_CURSOR,
        );

        let root_page = crate::surface::execute_surface_page_by_tag(
            &program,
            &store,
            &root_tag,
            &SurfacePageRequestJson {
                exact_keys: Vec::new(),
                limit: 1,
                cursor: None,
            },
        )
        .expect("root page");
        let root_cursor = root_page.next.expect("root page has cursor");
        assert_surface_error(
            crate::surface::execute_surface_page_by_tag(
                &program,
                &store,
                &collection_tag,
                &SurfacePageRequestJson {
                    cursor: Some(root_cursor),
                    ..book_page_request(&runtime, 7, 10)
                },
            ),
            SURFACE_STALE_CURSOR,
        );
    }

    #[test]
    fn surface_execute_module_does_not_introduce_serving_or_lifecycle_concepts() {
        let source = include_str!("surface/execute.rs").to_lowercase();
        for forbidden in ["route", "server", "http", "client", "opaque"] {
            assert!(
                !source.contains(forbidden),
                "surface JSON execute boundary must stay transport-neutral: {forbidden}"
            );
        }
    }

    #[test]
    fn surface_operation_module_does_not_introduce_serving_or_lifecycle_concepts() {
        let source = include_str!("surface/operation.rs").to_lowercase();
        for forbidden in ["route", "server", "http", "client", "opaque"] {
            assert!(
                !source.contains(forbidden),
                "surface operation envelope must stay transport-neutral: {forbidden}"
            );
        }
    }

    #[test]
    fn surface_operation_envelope_does_not_build_json_abi_per_request() {
        let source = include_str!("surface/operation.rs");
        for forbidden in [
            "SurfaceAbiJson::from_program",
            "SurfaceOperationCatalog::from_abi",
        ] {
            assert!(
                !source.contains(forbidden),
                "surface operation envelope must not rebuild JSON ABI catalog on each request: {forbidden}"
            );
        }
    }

    #[test]
    fn surface_route_manifest_uses_operation_kind_projection() {
        let source = include_str!("surface/route.rs");
        for forbidden in [
            "SurfaceReadOperationKindJson",
            "SurfaceUpdateOperationKindJson",
            "fn read_request",
            "fn update_request",
        ] {
            assert!(
                !source.contains(forbidden),
                "surface route manifest must use the operation kind projection owner: {forbidden}"
            );
        }
    }

    #[test]
    fn surface_json_does_not_expose_shape_bypass_apis() {
        let source = include_str!("surface/request.rs");
        for forbidden in [
            "pub fn decode_with_shape",
            "pub fn decode_with_shapes",
            "pub fn from_cursor_boundary_shape",
        ] {
            assert!(
                !source.contains(forbidden),
                "shape bypass API must not be public: {forbidden}"
            );
        }
    }

    #[test]
    fn surface_record_json_preserves_catalog_identity_and_typed_values() {
        let books = catalog_id(1);
        let authors = catalog_id(2);
        let status = catalog_id(3);
        let active = catalog_id(4);
        let title = catalog_id(5);
        let state = catalog_id(6);
        let author = catalog_id(7);
        let cover = catalog_id(8);
        let rating = catalog_id(9);

        let record = SurfaceReadRecord {
            identity: Some(SurfaceReadIdentity {
                store_catalog_id: books.clone(),
                keys: vec![SavedKey::Int(i64::MAX), SavedKey::Str("paperback".into())],
            }),
            fields: vec![
                SurfaceReadField {
                    catalog_id: title.clone(),
                    render_label: "title".into(),
                    value: Some(SurfaceValue::Str("Dune".into())),
                },
                SurfaceReadField {
                    catalog_id: state.clone(),
                    render_label: "state".into(),
                    value: Some(SurfaceValue::Enum(SurfaceEnumValue {
                        enum_catalog_id: status.clone(),
                        member_catalog_id: active.clone(),
                        render_label: "active".into(),
                    })),
                },
                SurfaceReadField {
                    catalog_id: author.clone(),
                    render_label: "author".into(),
                    value: Some(SurfaceValue::Identity(SurfaceReadIdentity {
                        store_catalog_id: authors.clone(),
                        keys: vec![SavedKey::Bool(true), SavedKey::Date(-3)],
                    })),
                },
                SurfaceReadField {
                    catalog_id: cover.clone(),
                    render_label: "cover".into(),
                    value: Some(SurfaceValue::Bytes(vec![0, 255])),
                },
                SurfaceReadField {
                    catalog_id: rating.clone(),
                    render_label: "rating".into(),
                    value: Some(SurfaceValue::Decimal(
                        Decimal::parse("12.50").expect("decimal"),
                    )),
                },
                SurfaceReadField {
                    catalog_id: catalog_id(10),
                    render_label: "subtitle".into(),
                    value: None,
                },
            ],
        };

        assert_eq!(
            serde_json::to_value(SurfaceRecordJson::from(&record))
                .map_err(|error| error.to_string()),
            Ok(json!({
                "identity": {
                    "store_catalog_id": books.as_str(),
                    "keys": [
                        { "kind": "int", "value": "9223372036854775807" },
                        { "kind": "string", "value": "paperback" }
                    ]
                },
                "fields": [
                    {
                        "catalog_id": title.as_str(),
                        "render_label": "title",
                        "value": { "kind": "string", "value": "Dune" }
                    },
                    {
                        "catalog_id": state.as_str(),
                        "render_label": "state",
                        "value": {
                            "kind": "enum",
                            "enum_catalog_id": status.as_str(),
                            "member_catalog_id": active.as_str(),
                            "render_label": "active"
                        }
                    },
                    {
                        "catalog_id": author.as_str(),
                        "render_label": "author",
                        "value": {
                            "kind": "identity",
                            "store_catalog_id": authors.as_str(),
                            "keys": [
                                { "kind": "bool", "value": true },
                                { "kind": "date", "days_since_epoch": -3 }
                            ]
                        }
                    },
                    {
                        "catalog_id": cover.as_str(),
                        "render_label": "cover",
                        "value": { "kind": "bytes", "value_b64": "AP8=" }
                    },
                    {
                        "catalog_id": rating.as_str(),
                        "render_label": "rating",
                        "value": { "kind": "decimal", "value": "12.5" }
                    },
                    {
                        "catalog_id": "cat_0000000000000000000000000000000a",
                        "render_label": "subtitle",
                        "value": null
                    }
                ]
            }))
        );
    }

    #[test]
    fn surface_scalar_value_json_preserves_each_runtime_scalar_shape() {
        let cases = vec![
            (
                SurfaceValue::Int(i64::MIN),
                json!({ "kind": "int", "value": "-9223372036854775808" }),
            ),
            (
                SurfaceValue::Bool(false),
                json!({ "kind": "bool", "value": false }),
            ),
            (
                SurfaceValue::Date(-30),
                json!({ "kind": "date", "days_since_epoch": -30 }),
            ),
            (
                SurfaceValue::Duration(1_000_000_000_000_000_001),
                json!({ "kind": "duration", "nanos": "1000000000000000001" }),
            ),
            (
                SurfaceValue::Instant(-1_000_000_000_000_000_001),
                json!({ "kind": "instant", "nanos_since_epoch": "-1000000000000000001" }),
            ),
        ];

        for (value, expected) in cases {
            assert_eq!(
                serde_json::to_value(SurfaceValueJson::from(&value))
                    .map_err(|error| error.to_string()),
                Ok(expected)
            );
        }
    }

    #[test]
    fn point_update_request_decodes_and_executes_sparse_enum_identity_update() {
        let (program, runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "draft", 7);
        write_surface_book_private_code(&runtime, &store, 1, "internal");

        let surface = surface_id(&program, "Books");
        let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit update");
        let request = point_update_request(
            &runtime,
            1,
            vec![
                update_field(
                    field_catalog_id(&runtime, "books", "status"),
                    SurfaceWriteValueJson::Enum {
                        enum_catalog_id: enum_catalog_id(&runtime, "Status").as_str().into(),
                        member_catalog_id: enum_member_catalog_id(&runtime, "Status", "published")
                            .as_str()
                            .into(),
                    },
                ),
                update_field(
                    field_catalog_id(&runtime, "books", "author"),
                    SurfaceWriteValueJson::Identity {
                        store_catalog_id: store_catalog_id(&runtime, "authors").as_str().into(),
                        keys: vec![SurfaceKeyJson::Int { value: "8".into() }],
                    },
                ),
            ],
        );

        let decoded = request.decode(&update).expect("decode point update");
        update
            .execute(decoded.as_update_input())
            .expect("execute point update");

        let read = SurfaceNodeRead::admit(&program, &store, surface).expect("admit read");
        let record = read
            .read_point(&[SavedKey::Int(1)])
            .expect("read updated point");
        assert_eq!(
            record
                .fields
                .iter()
                .find(|field| field.catalog_id == field_catalog_id(&runtime, "books", "status"))
                .and_then(|field| field.value.clone()),
            Some(SurfaceValue::Enum(SurfaceEnumValue {
                enum_catalog_id: enum_catalog_id(&runtime, "Status"),
                member_catalog_id: enum_member_catalog_id(&runtime, "Status", "published"),
                render_label: "published".into(),
            }))
        );
        assert_eq!(
            record
                .fields
                .iter()
                .find(|field| field.catalog_id == field_catalog_id(&runtime, "books", "author"))
                .and_then(|field| field.value.clone()),
            Some(SurfaceValue::Identity(SurfaceReadIdentity {
                store_catalog_id: store_catalog_id(&runtime, "authors"),
                keys: vec![SavedKey::Int(8)],
            }))
        );

        let by_status_author = book_by_status_author_read(&program, &store);
        let old_page = book_status_author_page_request(&runtime, "draft", 7, 10)
            .decode(&by_status_author)
            .expect("decode old index lookup");
        assert_eq!(
            by_status_author
                .page(old_page.as_page_request())
                .expect("old index page")
                .rows,
            Vec::<SurfaceReadRecord>::new()
        );
        let new_page = book_status_author_page_request(&runtime, "published", 8, 10)
            .decode(&by_status_author)
            .expect("decode new index lookup");
        assert_eq!(
            by_status_author
                .page(new_page.as_page_request())
                .expect("new index page")
                .rows
                .into_iter()
                .map(|record| record.identity.expect("row identity").keys)
                .collect::<Vec<_>>(),
            vec![vec![SavedKey::Int(1)]]
        );
    }

    #[test]
    fn singleton_update_request_decodes_and_executes_against_keyless_surface() {
        let (program, runtime) = checked_surface_program(SINGLETON_UPDATE_SURFACE);
        let store = admitted_store(&program);
        write_data_value(
            &runtime,
            &store,
            "settings",
            &[],
            &data_path(&runtime, "settings", &["theme"]),
            SavedValue::Str("dark".into()),
        );

        let surface = surface_id(&program, "SettingsSurface");
        let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit update");
        let request = SurfaceSingletonUpdateRequestJson {
            fields: vec![update_field(
                field_catalog_id(&runtime, "settings", "mode"),
                SurfaceWriteValueJson::String {
                    value: "compact".into(),
                },
            )],
        };

        let decoded = request.decode(&update).expect("decode singleton update");
        update
            .execute(decoded.as_update_input())
            .expect("execute singleton update");

        let record = SurfaceNodeRead::admit(&program, &store, surface)
            .expect("admit singleton read")
            .read_singleton()
            .expect("read singleton");
        assert_eq!(
            record
                .fields
                .iter()
                .find(|field| field.catalog_id == field_catalog_id(&runtime, "settings", "mode"))
                .and_then(|field| field.value.clone()),
            Some(SurfaceValue::Str("compact".into()))
        );
    }

    #[test]
    fn point_update_request_decodes_and_executes_temporal_range_faults_as_surface_request() {
        for (member, value, expected) in [
            (
                "day",
                SurfaceWriteValueJson::Date {
                    days_since_epoch: SUPPORTED_DATE_MIN_DAYS - 1,
                },
                SurfaceValue::Date(0),
            ),
            (
                "seenAt",
                SurfaceWriteValueJson::Instant {
                    nanos_since_epoch: (SUPPORTED_INSTANT_MAX_NANOS + 1).to_string(),
                },
                SurfaceValue::Instant(0),
            ),
        ] {
            let (program, runtime) = checked_surface_program(TEMPORAL_UPDATE_SURFACE);
            let store = admitted_store(&program);
            write_surface_event(&runtime, &store, 1, "Launch", 0, 0);
            let baseline = store
                .read_commit_metadata()
                .expect("read baseline commit metadata")
                .expect("catalog baseline is stamped");

            let surface = surface_id(&program, "Events");
            let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit update");
            let request = SurfacePointUpdateRequestJson {
                identity: SurfaceIdentityJson {
                    store_catalog_id: store_catalog_id(&runtime, "events").as_str().into(),
                    keys: vec![SurfaceKeyJson::Int { value: "1".into() }],
                },
                fields: vec![update_field(
                    field_catalog_id(&runtime, "events", member),
                    value,
                )],
            };

            let decoded = request.decode(&update).expect("decode point update");
            assert_surface_error(update.execute(decoded.as_update_input()), SURFACE_REQUEST);
            assert_eq!(
                store
                    .read_commit_metadata()
                    .expect("read commit metadata after rejected update")
                    .expect("commit metadata remains"),
                baseline
            );

            let record = SurfaceNodeRead::admit(&program, &store, surface)
                .expect("admit read")
                .read_point(&[SavedKey::Int(1)])
                .expect("read unchanged point");
            assert_eq!(
                record
                    .fields
                    .iter()
                    .find(|field| field.catalog_id == field_catalog_id(&runtime, "events", member))
                    .and_then(|field| field.value.clone()),
                Some(expected)
            );
        }
    }

    #[test]
    fn point_update_request_against_singleton_surface_returns_surface_request() {
        let (program, runtime) = checked_surface_program(SINGLETON_UPDATE_SURFACE);
        let store = admitted_store(&program);
        let update =
            SurfaceUpdate::admit(&program, &store, surface_id(&program, "SettingsSurface"))
                .expect("admit singleton update");
        let request = SurfacePointUpdateRequestJson {
            identity: SurfaceIdentityJson {
                store_catalog_id: store_catalog_id(&runtime, "settings").as_str().into(),
                keys: Vec::new(),
            },
            fields: Vec::new(),
        };

        assert_surface_error(request.decode(&update), SURFACE_REQUEST);
    }

    #[test]
    fn singleton_update_request_against_keyed_surface_returns_surface_request() {
        let (program, _runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let update =
            SurfaceUpdate::admit(&program, &store, surface_id(&program, "Books")).expect("admit");
        let request = SurfaceSingletonUpdateRequestJson { fields: Vec::new() };

        assert_surface_error(request.decode(&update), SURFACE_REQUEST);
    }

    #[test]
    fn update_request_malformed_scalar_forms_return_surface_request() {
        let (program, runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let update =
            SurfaceUpdate::admit(&program, &store, surface_id(&program, "Books")).expect("admit");
        let status = field_catalog_id(&runtime, "books", "status");

        for value in [
            SurfaceWriteValueJson::Int { value: "01".into() },
            SurfaceWriteValueJson::Decimal {
                value: "1.50".into(),
            },
            SurfaceWriteValueJson::Bytes {
                value_b64: "!!!!".into(),
            },
        ] {
            let request =
                point_update_request(&runtime, 1, vec![update_field(status.clone(), value)]);
            assert_surface_error(request.decode(&update), SURFACE_REQUEST);
        }
    }

    #[test]
    fn update_request_malformed_field_catalog_id_returns_surface_request() {
        let (program, runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let update =
            SurfaceUpdate::admit(&program, &store, surface_id(&program, "Books")).expect("admit");
        let request = SurfacePointUpdateRequestJson {
            identity: SurfaceIdentityJson {
                store_catalog_id: store_catalog_id(&runtime, "books").as_str().into(),
                keys: vec![SurfaceKeyJson::Int { value: "1".into() }],
            },
            fields: vec![SurfaceUpdateFieldJson {
                catalog_id: "not-a-catalog-id".into(),
                value: SurfaceWriteValueJson::String {
                    value: "ignored".into(),
                },
            }],
        };

        assert_surface_error(request.decode(&update), SURFACE_REQUEST);
    }

    #[test]
    fn decoded_undeclared_update_field_is_rejected_by_runtime_without_writing() {
        let (program, runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "draft", 7);
        write_surface_book_private_code(&runtime, &store, 1, "internal");
        let baseline = store
            .read_commit_metadata()
            .expect("read baseline commit metadata")
            .expect("catalog baseline is stamped");
        let surface = surface_id(&program, "Books");
        let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit update");
        let request = point_update_request(
            &runtime,
            1,
            vec![update_field(
                field_catalog_id(&runtime, "books", "privateCode"),
                SurfaceWriteValueJson::String {
                    value: "leak".into(),
                },
            )],
        );

        let decoded = request.decode(&update).expect("decode syntactic update");
        assert_surface_error(update.execute(decoded.as_update_input()), SURFACE_REQUEST);
        assert_eq!(
            store
                .read_commit_metadata()
                .expect("read commit metadata after rejected update")
                .expect("commit metadata remains"),
            baseline
        );
        let record = SurfaceNodeRead::admit(&program, &store, surface)
            .expect("admit read")
            .read_point(&[SavedKey::Int(1)])
            .expect("read unchanged point");
        assert_eq!(
            record
                .fields
                .iter()
                .find(|field| field.catalog_id == field_catalog_id(&runtime, "books", "status"))
                .and_then(|field| field.value.clone()),
            Some(SurfaceValue::Enum(SurfaceEnumValue {
                enum_catalog_id: enum_catalog_id(&runtime, "Status"),
                member_catalog_id: enum_member_catalog_id(&runtime, "Status", "draft"),
                render_label: "draft".into(),
            }))
        );
    }

    #[test]
    fn empty_update_patch_decodes_and_runtime_returns_surface_request() {
        let (program, runtime) = checked_surface_program(SURFACE_UPDATE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "draft", 7);
        write_surface_book_private_code(&runtime, &store, 1, "internal");
        let update =
            SurfaceUpdate::admit(&program, &store, surface_id(&program, "Books")).expect("admit");
        let request = point_update_request(&runtime, 1, Vec::new());

        let decoded = request.decode(&update).expect("empty update decodes");
        assert_surface_error(update.execute(decoded.as_update_input()), SURFACE_REQUEST);
    }

    #[test]
    fn point_request_identity_decode_uses_admitted_node_read() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let read = SurfaceNodeRead::admit(&program, &store, surface_id(&program, "Books"))
            .expect("admit point read");
        let books = store_catalog_id(&runtime, "books");
        let request = SurfacePointRequestJson {
            identity: SurfaceIdentityJson {
                store_catalog_id: books.as_str().into(),
                keys: vec![SurfaceKeyJson::Int {
                    value: i64::MAX.to_string(),
                }],
            },
        };

        let decoded = request.decode(&read).expect("point request");
        assert_eq!(decoded.identity(), &[SavedKey::Int(i64::MAX)]);
    }

    #[test]
    fn point_request_wrong_store_brand_returns_surface_request() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let read = SurfaceNodeRead::admit(&program, &store, surface_id(&program, "Books"))
            .expect("admit point read");
        let wrong_store = SurfacePointRequestJson {
            identity: SurfaceIdentityJson {
                store_catalog_id: store_catalog_id(&runtime, "authors").as_str().into(),
                keys: vec![SurfaceKeyJson::Int { value: "1".into() }],
            },
        };
        assert_surface_error(wrong_store.decode(&read), SURFACE_REQUEST);
    }

    #[test]
    fn page_request_exact_args_decode_through_admitted_collection_read() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let read = book_by_status_author_read(&program, &store);
        let published = enum_member_catalog_id(&runtime, "Status", "published");
        let authors = store_catalog_id(&runtime, "authors");
        let request = book_page_request(&runtime, 7, 25);

        let decoded = request.decode(&read).expect("page request");
        let runtime = decoded.as_page_request();
        assert_eq!(runtime.limit, 25);
        assert_eq!(
            runtime.exact_keys,
            [
                SavedKey::Str(published.as_str().into()),
                SavedKey::Bytes(encode_identity_index_key(
                    authors.as_str(),
                    &[SavedKey::Int(7)]
                )),
            ]
        );
        assert_eq!(runtime.cursor, None);
    }

    #[test]
    fn page_request_defaults_omitted_exact_keys_to_empty() {
        let omitted_exact_keys =
            serde_json::from_value::<SurfacePageRequestJson>(json!({ "limit": 5 }))
                .expect("page request json");
        assert_eq!(omitted_exact_keys.exact_keys, Vec::new());
    }

    #[test]
    fn cursor_json_round_trips_context_aware_page_rendering() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        write_surface_book(&runtime, &store, 1, "Dune", "published", 7);
        write_surface_book(&runtime, &store, 2, "Dune Messiah", "published", 7);
        let read = book_by_status_author_read(&program, &store);
        let decoded = book_page_request(&runtime, 7, 1)
            .decode(&read)
            .expect("page request");
        let page = read.page(decoded.as_page_request()).expect("page read");
        let cursor = page.next.as_ref().expect("page cursor").clone();
        let rendered_page = SurfacePageJson::from_page(&read, &page).expect("page json");
        let rendered_cursor = rendered_page.next.as_ref().expect("rendered cursor");

        let SurfaceCursorBoundaryJson::IndexIdentity {
            exact_keys,
            identity,
        } = &rendered_cursor.boundary
        else {
            panic!("expected index cursor boundary: {rendered_cursor:?}");
        };
        assert_eq!(
            exact_keys,
            &vec![
                SurfaceArgumentJson::Enum {
                    enum_catalog_id: enum_catalog_id(&runtime, "Status").as_str().into(),
                    member_catalog_id: enum_member_catalog_id(&runtime, "Status", "published")
                        .as_str()
                        .into(),
                },
                SurfaceArgumentJson::Identity {
                    store_catalog_id: store_catalog_id(&runtime, "authors").as_str().into(),
                    keys: vec![SurfaceKeyJson::Int { value: "7".into() }],
                },
            ]
        );
        assert_eq!(
            identity,
            &SurfaceIdentityJson {
                store_catalog_id: store_catalog_id(&runtime, "books").as_str().into(),
                keys: vec![SurfaceKeyJson::Int { value: "1".into() }],
            }
        );
        assert_eq!(
            rendered_cursor.decode(&read).expect("decode cursor"),
            cursor
        );
        assert_eq!(
            serde_json::to_value(rendered_cursor).expect("cursor serializes")["commit_id"],
            json!(0)
        );

        let mut old_cursor_json = serde_json::to_value(rendered_cursor).expect("cursor serializes");
        old_cursor_json
            .as_object_mut()
            .expect("cursor json object")
            .remove("commit_id");
        let old_request = serde_json::from_value::<SurfacePageRequestJson>(json!({
            "exact_keys": book_page_request(&runtime, 7, 10).exact_keys,
            "limit": 10,
            "cursor": old_cursor_json
        }))
        .expect("old cursor request still decodes structurally");
        assert_surface_error(old_request.decode(&read), SURFACE_CURSOR);
    }

    #[test]
    fn cursor_decode_malformed_store_uid_returns_surface_cursor() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let (read, cursor) = index_cursor_json(&program, &runtime, &store);
        let bad = SurfaceCursorJson {
            store_uid: "not-a-store-uid".into(),
            ..cursor
        };
        assert_surface_error(bad.decode(&read), SURFACE_CURSOR);
    }

    #[test]
    fn cursor_decode_malformed_engine_profile_digest_returns_surface_cursor() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let (read, cursor) = index_cursor_json(&program, &runtime, &store);
        let bad = SurfaceCursorJson {
            engine_profile_digest: "abcd".into(),
            ..cursor
        };
        assert_surface_error(bad.decode(&read), SURFACE_CURSOR);
    }

    #[test]
    fn point_request_malformed_int_returns_surface_request() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let read = SurfaceNodeRead::admit(&program, &store, surface_id(&program, "Books"))
            .expect("admit point read");
        let malformed_int = SurfacePointRequestJson {
            identity: SurfaceIdentityJson {
                store_catalog_id: store_catalog_id(&runtime, "books").as_str().into(),
                keys: vec![SurfaceKeyJson::Int { value: "01".into() }],
            },
        };
        assert_surface_error(malformed_int.decode(&read), SURFACE_REQUEST);
    }

    #[test]
    fn page_request_malformed_base64_returns_surface_request() {
        let (program, _runtime) = checked_surface_program(BYTES_INDEX_SURFACE);
        let store = admitted_store(&program);
        let surface = surface_id(&program, "Files");
        let read = SurfaceCollectionRead::admit(
            &program,
            &store,
            index_collection_ref(&program, surface, "byFingerprint"),
        )
        .expect("admit bytes index collection");
        let bytes_request = SurfacePageRequestJson {
            exact_keys: vec![SurfaceArgumentJson::Bytes {
                value_b64: "!!!!".into(),
            }],
            limit: 1,
            cursor: None,
        };
        assert_surface_error(bytes_request.decode(&read), SURFACE_REQUEST);
    }

    #[test]
    fn page_request_wrong_identity_brand_returns_surface_request() {
        let (program, runtime) = checked_surface_program(SURFACE_WITH_ENUM_IDENTITY_INDEX);
        let store = admitted_store(&program);
        let read = book_by_status_author_read(&program, &store);
        let wrong_brand = SurfacePageRequestJson {
            exact_keys: vec![
                SurfaceArgumentJson::Enum {
                    enum_catalog_id: enum_catalog_id(&runtime, "Status").as_str().into(),
                    member_catalog_id: enum_member_catalog_id(&runtime, "Status", "published")
                        .as_str()
                        .into(),
                },
                SurfaceArgumentJson::Identity {
                    store_catalog_id: store_catalog_id(&runtime, "books").as_str().into(),
                    keys: vec![SurfaceKeyJson::Int { value: "1".into() }],
                },
            ],
            limit: 1,
            cursor: None,
        };
        assert_surface_error(wrong_brand.decode(&read), SURFACE_REQUEST);
    }
}
