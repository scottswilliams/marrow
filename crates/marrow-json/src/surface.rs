use std::collections::{BTreeMap, BTreeSet};

use marrow_run::{
    SurfaceEnumValue, SurfaceReadField, SurfaceReadIdentity, SurfaceReadRecord, SurfaceValue,
};
use marrow_store::key::SavedKey;
use serde::{Deserialize, Serialize};

mod execute;
mod request;
pub use execute::{
    execute_surface_page_by_tag, execute_surface_point_read_by_tag,
    execute_surface_point_update_by_tag, execute_surface_singleton_read_by_tag,
    execute_surface_singleton_update_by_tag, execute_surface_unique_lookup_by_tag,
};
pub use request::{
    DecodedSurfacePageRequest, DecodedSurfacePointRequest, DecodedSurfacePointUpdateRequest,
    DecodedSurfaceSingletonUpdateRequest, DecodedSurfaceUniqueLookupRequest,
    SurfacePageRequestJson, SurfacePointRequestJson, SurfacePointUpdateRequestJson,
    SurfaceSingletonUpdateRequestJson, SurfaceUniqueLookupRequestJson, SurfaceUpdateFieldJson,
    SurfaceUpdateValueJson,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<SurfaceUpdateOperationDescriptorJson>,
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
        enum_catalog_id: String,
        member_catalog_ids: Vec<String>,
    },
    Identity {
        store_catalog_id: String,
        arity: usize,
        key_scalars: Vec<String>,
    },
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            update: if stable {
                marrow_check::SurfaceUpdateOperationDescriptor::from_surface(program, surface)
                    .map(SurfaceUpdateOperationDescriptorJson::from)
            } else {
                None
            },
        }
    }
}

fn omit_uncallable_operation_tags(surfaces: &mut [SurfaceDescriptorJson]) {
    let duplicate_read_tags = duplicate_operation_tags(
        surfaces
            .iter()
            .flat_map(|surface| surface.read.iter().map(|read| read.operation_tag.as_str())),
    );
    if !duplicate_read_tags.is_empty() {
        for surface in surfaces.iter_mut() {
            surface
                .read
                .retain(|read| !duplicate_read_tags.contains(&read.operation_tag));
        }
    }

    let duplicate_update_tags = duplicate_operation_tags(surfaces.iter().filter_map(|surface| {
        surface
            .update
            .as_ref()
            .map(|update| update.operation_tag.as_str())
    }));
    if !duplicate_update_tags.is_empty() {
        for surface in surfaces.iter_mut() {
            if surface
                .update
                .as_ref()
                .is_some_and(|update| duplicate_update_tags.contains(&update.operation_tag))
            {
                surface.update = None;
            }
        }
    }
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
                enum_catalog_id,
                member_catalog_ids,
            } => Self::Enum {
                enum_catalog_id: enum_catalog_id.as_str().to_string(),
                member_catalog_ids: member_catalog_ids
                    .into_iter()
                    .map(|id| id.as_str().to_string())
                    .collect(),
            },
            marrow_check::SurfaceOperationValueShape::Identity {
                store_catalog_id,
                arity,
                key_scalars,
            } => Self::Identity {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceCursorJson {
    pub operation_tag: String,
    pub store_uid: String,
    pub catalog_digest: String,
    pub source_digest: String,
    pub engine_profile_digest: String,
    pub boundary: SurfaceCursorBoundaryJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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
#[serde(tag = "kind", rename_all = "snake_case")]
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
#[serde(tag = "kind", rename_all = "snake_case")]
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
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use marrow_check::{
        CheckedProgram, CheckedRuntimeProgram, ProjectConfig, StoreBackend, StoreConfig, SurfaceId,
        SurfaceReadOperationKind, SurfaceUpdateOperationDescriptor, check_project,
    };
    use marrow_run::{
        SURFACE_ABI_MISMATCH, SURFACE_CURSOR, SURFACE_REQUEST, SURFACE_STALE_CURSOR,
        SurfaceCollectionRead, SurfaceEnumValue, SurfaceNodeRead, SurfaceReadError,
        SurfaceReadField, SurfaceReadIdentity, SurfaceReadOperationRef, SurfaceReadRecord,
        SurfaceUpdate, SurfaceValue,
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
        SurfaceAbiJson, SurfaceArgumentJson, SurfaceCatalogStatusJson, SurfaceCursorBoundaryJson,
        SurfaceCursorJson, SurfaceIdentityJson, SurfaceKeyJson, SurfacePageJson,
        SurfacePageRequestJson, SurfacePointRequestJson, SurfacePointUpdateRequestJson,
        SurfaceReadOperationKindJson, SurfaceRecordJson, SurfaceSingletonUpdateRequestJson,
        SurfaceUniqueLookupRequestJson, SurfaceUpdateFieldJson, SurfaceUpdateValueJson,
        SurfaceValueJson,
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

    const SOURCE_ONLY_UPDATE_SURFACE: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    update title
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
                backend: StoreBackend::Memory,
                data_dir: None,
            },
            tests: Vec::new(),
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
                backend: StoreBackend::Memory,
                data_dir: None,
            },
            tests: Vec::new(),
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

    fn checker_update_operation_tag(program: &CheckedProgram, surface: SurfaceId) -> String {
        SurfaceUpdateOperationDescriptor::from_surface(program, program.facts.surface(surface))
            .map(|descriptor| descriptor.operation_tag)
            .expect("stable update operation tag")
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

    fn update_field(
        catalog_id: CatalogId,
        value: SurfaceUpdateValueJson,
    ) -> SurfaceUpdateFieldJson {
        SurfaceUpdateFieldJson {
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
                    SurfaceUpdateValueJson::Enum {
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
                    SurfaceUpdateValueJson::String {
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
        for forbidden in [
            "route", "server", "http", "client", "create", "delete", "opaque",
        ] {
            assert!(
                !source.contains(forbidden),
                "surface JSON execute boundary must stay transport-neutral: {forbidden}"
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
                    SurfaceUpdateValueJson::Enum {
                        enum_catalog_id: enum_catalog_id(&runtime, "Status").as_str().into(),
                        member_catalog_id: enum_member_catalog_id(&runtime, "Status", "published")
                            .as_str()
                            .into(),
                    },
                ),
                update_field(
                    field_catalog_id(&runtime, "books", "author"),
                    SurfaceUpdateValueJson::Identity {
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
                SurfaceUpdateValueJson::String {
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
                SurfaceUpdateValueJson::Date {
                    days_since_epoch: SUPPORTED_DATE_MIN_DAYS - 1,
                },
                SurfaceValue::Date(0),
            ),
            (
                "seenAt",
                SurfaceUpdateValueJson::Instant {
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
            SurfaceUpdateValueJson::Int { value: "01".into() },
            SurfaceUpdateValueJson::Decimal {
                value: "1.50".into(),
            },
            SurfaceUpdateValueJson::Bytes {
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
                value: SurfaceUpdateValueJson::String {
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
                SurfaceUpdateValueJson::String {
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
