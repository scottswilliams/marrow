use marrow_run::{
    SurfaceCollectionPage, SurfaceCollectionPageRequest, SurfaceCollectionRead, SurfaceCreate,
    SurfaceCreateField, SurfaceCreateInput, SurfaceCursorBoundaryInputShape, SurfaceDelete,
    SurfaceDeleteInput, SurfaceIdentityInputShape, SurfaceInputKeyShape, SurfaceNodeRead,
    SurfaceNodeReadShape, SurfacePageBoundary, SurfacePageCursor, SurfaceReadError,
    SurfaceReadIdentity, SurfaceUpdate, SurfaceUpdateField, SurfaceUpdateInput, SurfaceValue,
};
use marrow_store::Decimal;
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, decode_identity_index_key, encode_identity_index_key};
use marrow_store::tree::StoreUid;
use marrow_store::value::{ScalarType, scalar_key_matches_type, validate_scalar_key};
use serde::{Deserialize, Serialize};

use crate::lower_hex;

use super::{
    SurfaceArgumentJson, SurfaceCursorBoundaryJson, SurfaceCursorJson, SurfaceIdentityJson,
    SurfaceKeyJson, SurfacePageJson, SurfaceRecordJson,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfacePointRequestJson {
    pub identity: SurfaceIdentityJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfacePageRequestJson {
    #[serde(default)]
    pub exact_keys: Vec<SurfaceArgumentJson>,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<SurfaceCursorJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceUniqueLookupRequestJson {
    pub keys: Vec<SurfaceArgumentJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfacePointUpdateRequestJson {
    pub identity: SurfaceIdentityJson,
    pub fields: Vec<SurfaceUpdateFieldJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSingletonUpdateRequestJson {
    pub fields: Vec<SurfaceUpdateFieldJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfacePointCreateRequestJson {
    pub identity: SurfaceIdentityJson,
    pub fields: Vec<SurfaceCreateFieldJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSingletonCreateRequestJson {
    pub fields: Vec<SurfaceCreateFieldJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfacePointDeleteRequestJson {
    pub identity: SurfaceIdentityJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceUpdateFieldJson {
    pub catalog_id: String,
    pub value: SurfaceWriteValueJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceCreateFieldJson {
    pub catalog_id: String,
    pub value: SurfaceWriteValueJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceWriteValueJson {
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
    },
    Identity {
        store_catalog_id: String,
        keys: Vec<SurfaceKeyJson>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSurfacePointRequest {
    identity: Vec<SavedKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSurfacePageRequest {
    exact_keys: Vec<SavedKey>,
    limit: usize,
    cursor: Option<SurfacePageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSurfaceUniqueLookupRequest {
    keys: Vec<SavedKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSurfacePointUpdateRequest {
    identity: Vec<SavedKey>,
    fields: Vec<SurfaceUpdateField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSurfaceSingletonUpdateRequest {
    fields: Vec<SurfaceUpdateField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSurfacePointCreateRequest {
    identity: Vec<SavedKey>,
    fields: Vec<SurfaceCreateField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSurfaceSingletonCreateRequest {
    fields: Vec<SurfaceCreateField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSurfacePointDeleteRequest {
    identity: Vec<SavedKey>,
}

impl SurfacePointRequestJson {
    pub fn decode(
        &self,
        read: &SurfaceNodeRead<'_>,
    ) -> Result<DecodedSurfacePointRequest, SurfaceReadError> {
        let shape = read.point_identity_shape()?;
        self.decode_with_shape(&shape)
    }

    fn decode_with_shape(
        &self,
        shape: &SurfaceIdentityInputShape,
    ) -> Result<DecodedSurfacePointRequest, SurfaceReadError> {
        Ok(DecodedSurfacePointRequest {
            identity: decode_identity_json(
                &self.identity,
                shape,
                SurfaceJsonErrorContext::Request,
            )?,
        })
    }
}

impl DecodedSurfacePointRequest {
    pub fn identity(&self) -> &[SavedKey] {
        &self.identity
    }
}

impl SurfacePageRequestJson {
    pub fn decode(
        &self,
        read: &SurfaceCollectionRead<'_>,
    ) -> Result<DecodedSurfacePageRequest, SurfaceReadError> {
        let exact_key_shapes = read.page_exact_key_shapes()?;
        let cursor_boundary_shape = read.cursor_boundary_shape()?;
        self.decode_with_shapes(&exact_key_shapes, &cursor_boundary_shape)
    }

    fn decode_with_shapes(
        &self,
        exact_key_shapes: &[SurfaceInputKeyShape],
        cursor_boundary_shape: &SurfaceCursorBoundaryInputShape,
    ) -> Result<DecodedSurfacePageRequest, SurfaceReadError> {
        Ok(DecodedSurfacePageRequest {
            exact_keys: decode_argument_tuple(
                &self.exact_keys,
                exact_key_shapes,
                SurfaceJsonErrorContext::Request,
            )?,
            limit: self.limit,
            cursor: self
                .cursor
                .as_ref()
                .map(|cursor| cursor.decode_with_shape(cursor_boundary_shape))
                .transpose()?,
        })
    }

    pub(super) fn validate_cursor_operation_tag(
        &self,
        operation_tag: &str,
    ) -> Result<(), SurfaceReadError> {
        if let Some(cursor) = &self.cursor {
            cursor.validate_operation_tag(operation_tag)?;
        }
        Ok(())
    }
}

impl DecodedSurfacePageRequest {
    pub fn as_page_request(&self) -> SurfaceCollectionPageRequest<'_> {
        SurfaceCollectionPageRequest {
            exact_keys: &self.exact_keys,
            limit: self.limit,
            cursor: self.cursor.as_ref(),
        }
    }
}

impl SurfaceUniqueLookupRequestJson {
    pub fn decode(
        &self,
        read: &SurfaceCollectionRead<'_>,
    ) -> Result<DecodedSurfaceUniqueLookupRequest, SurfaceReadError> {
        let key_shapes = read.unique_lookup_key_shapes()?;
        self.decode_with_shapes(&key_shapes)
    }

    fn decode_with_shapes(
        &self,
        key_shapes: &[SurfaceInputKeyShape],
    ) -> Result<DecodedSurfaceUniqueLookupRequest, SurfaceReadError> {
        Ok(DecodedSurfaceUniqueLookupRequest {
            keys: decode_argument_tuple(&self.keys, key_shapes, SurfaceJsonErrorContext::Request)?,
        })
    }
}

impl DecodedSurfaceUniqueLookupRequest {
    pub fn keys(&self) -> &[SavedKey] {
        &self.keys
    }
}

impl SurfacePointUpdateRequestJson {
    pub fn decode(
        &self,
        update: &SurfaceUpdate<'_>,
    ) -> Result<DecodedSurfacePointUpdateRequest, SurfaceReadError> {
        let shape = update.point_identity_shape()?;
        Ok(DecodedSurfacePointUpdateRequest {
            identity: decode_identity_json(
                &self.identity,
                &shape,
                SurfaceJsonErrorContext::Request,
            )?,
            fields: decode_update_fields_json(&self.fields, SurfaceJsonErrorContext::Request)?,
        })
    }
}

impl SurfaceSingletonUpdateRequestJson {
    pub fn decode(
        &self,
        update: &SurfaceUpdate<'_>,
    ) -> Result<DecodedSurfaceSingletonUpdateRequest, SurfaceReadError> {
        if update.shape() != SurfaceNodeReadShape::Singleton {
            return Err(SurfaceJsonErrorContext::Request
                .error("surface singleton update request targets a keyed surface"));
        }
        Ok(DecodedSurfaceSingletonUpdateRequest {
            fields: decode_update_fields_json(&self.fields, SurfaceJsonErrorContext::Request)?,
        })
    }
}

impl SurfacePointCreateRequestJson {
    pub fn decode(
        &self,
        create: &SurfaceCreate<'_>,
    ) -> Result<DecodedSurfacePointCreateRequest, SurfaceReadError> {
        let shape = create.point_identity_shape()?;
        Ok(DecodedSurfacePointCreateRequest {
            identity: decode_identity_json(
                &self.identity,
                &shape,
                SurfaceJsonErrorContext::Request,
            )?,
            fields: decode_create_fields_json(&self.fields, SurfaceJsonErrorContext::Request)?,
        })
    }
}

impl SurfaceSingletonCreateRequestJson {
    pub fn decode(
        &self,
        create: &SurfaceCreate<'_>,
    ) -> Result<DecodedSurfaceSingletonCreateRequest, SurfaceReadError> {
        if create.shape() != SurfaceNodeReadShape::Singleton {
            return Err(SurfaceJsonErrorContext::Request
                .error("surface singleton create request targets a keyed surface"));
        }
        Ok(DecodedSurfaceSingletonCreateRequest {
            fields: decode_create_fields_json(&self.fields, SurfaceJsonErrorContext::Request)?,
        })
    }
}

impl SurfacePointDeleteRequestJson {
    pub fn decode(
        &self,
        delete: &SurfaceDelete<'_>,
    ) -> Result<DecodedSurfacePointDeleteRequest, SurfaceReadError> {
        let shape = delete.point_identity_shape()?;
        Ok(DecodedSurfacePointDeleteRequest {
            identity: decode_identity_json(
                &self.identity,
                &shape,
                SurfaceJsonErrorContext::Request,
            )?,
        })
    }
}

impl DecodedSurfacePointUpdateRequest {
    pub fn as_update_input(&self) -> SurfaceUpdateInput<'_> {
        SurfaceUpdateInput::Point {
            identity: &self.identity,
            fields: &self.fields,
        }
    }
}

impl DecodedSurfacePointCreateRequest {
    pub fn as_create_input(&self) -> SurfaceCreateInput<'_> {
        SurfaceCreateInput::Point {
            identity: &self.identity,
            fields: &self.fields,
        }
    }
}

impl DecodedSurfaceSingletonCreateRequest {
    pub fn as_create_input(&self) -> SurfaceCreateInput<'_> {
        SurfaceCreateInput::Singleton {
            fields: &self.fields,
        }
    }
}

impl DecodedSurfacePointDeleteRequest {
    pub fn as_delete_input(&self) -> SurfaceDeleteInput<'_> {
        SurfaceDeleteInput::Point {
            identity: &self.identity,
        }
    }
}

impl DecodedSurfaceSingletonUpdateRequest {
    pub fn as_update_input(&self) -> SurfaceUpdateInput<'_> {
        SurfaceUpdateInput::Singleton {
            fields: &self.fields,
        }
    }
}

impl SurfacePageJson {
    pub fn from_page(
        read: &SurfaceCollectionRead<'_>,
        page: &SurfaceCollectionPage,
    ) -> Result<Self, SurfaceReadError> {
        Ok(Self {
            rows: page.rows.iter().map(SurfaceRecordJson::from).collect(),
            next: page
                .next
                .as_ref()
                .map(|cursor| SurfaceCursorJson::from_cursor(read, cursor))
                .transpose()?,
        })
    }
}

impl SurfaceCursorJson {
    pub fn from_cursor(
        read: &SurfaceCollectionRead<'_>,
        cursor: &SurfacePageCursor,
    ) -> Result<Self, SurfaceReadError> {
        let shape = read.cursor_boundary_shape()?;
        Self::from_cursor_boundary_shape(&shape, cursor)
    }

    fn from_cursor_boundary_shape(
        shape: &SurfaceCursorBoundaryInputShape,
        cursor: &SurfacePageCursor,
    ) -> Result<Self, SurfaceReadError> {
        Ok(Self {
            operation_tag: cursor.operation_tag.clone(),
            store_uid: cursor.store_uid.as_str().to_string(),
            commit_id: Some(cursor.commit_id),
            catalog_digest: cursor.catalog_digest.clone(),
            source_digest: cursor.source_digest.clone(),
            engine_profile_digest: lower_hex(&cursor.engine_profile_digest),
            boundary: render_cursor_boundary_json(shape, &cursor.boundary)?,
        })
    }

    pub fn decode(
        &self,
        read: &SurfaceCollectionRead<'_>,
    ) -> Result<SurfacePageCursor, SurfaceReadError> {
        let shape = read.cursor_boundary_shape()?;
        self.decode_with_shape(&shape)
    }

    fn decode_with_shape(
        &self,
        shape: &SurfaceCursorBoundaryInputShape,
    ) -> Result<SurfacePageCursor, SurfaceReadError> {
        Ok(SurfacePageCursor {
            operation_tag: decode_sha256_digest(
                &self.operation_tag,
                "operation tag",
                SurfaceJsonErrorContext::Cursor,
            )?,
            store_uid: StoreUid::new(self.store_uid.clone()).map_err(|error| {
                SurfaceJsonErrorContext::Cursor
                    .error(format!("malformed surface cursor store uid: {error}"))
            })?,
            commit_id: self.commit_id.ok_or_else(|| {
                SurfaceJsonErrorContext::Cursor.error("surface cursor commit id is missing")
            })?,
            catalog_digest: decode_sha256_digest(
                &self.catalog_digest,
                "catalog digest",
                SurfaceJsonErrorContext::Cursor,
            )?,
            source_digest: decode_sha256_digest(
                &self.source_digest,
                "source digest",
                SurfaceJsonErrorContext::Cursor,
            )?,
            engine_profile_digest: decode_engine_profile_digest(
                &self.engine_profile_digest,
                SurfaceJsonErrorContext::Cursor,
            )?,
            boundary: decode_cursor_boundary_json(
                &self.boundary,
                shape,
                SurfaceJsonErrorContext::Cursor,
            )?,
        })
    }

    fn validate_operation_tag(&self, operation_tag: &str) -> Result<(), SurfaceReadError> {
        let cursor_operation_tag = decode_sha256_digest(
            &self.operation_tag,
            "operation tag",
            SurfaceJsonErrorContext::Cursor,
        )?;
        if cursor_operation_tag != operation_tag {
            return Err(SurfaceReadError::stale_cursor(
                "surface cursor targets a different operation",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum SurfaceJsonErrorContext {
    Request,
    Cursor,
}

impl SurfaceJsonErrorContext {
    fn error(self, message: impl Into<String>) -> SurfaceReadError {
        match self {
            Self::Request => SurfaceReadError::request(message),
            Self::Cursor => SurfaceReadError::cursor(message),
        }
    }
}

fn decode_argument_tuple(
    arguments: &[SurfaceArgumentJson],
    shapes: &[SurfaceInputKeyShape],
    context: SurfaceJsonErrorContext,
) -> Result<Vec<SavedKey>, SurfaceReadError> {
    if arguments.len() != shapes.len() {
        return Err(context.error(format!(
            "surface argument tuple expects {} key(s), got {}",
            shapes.len(),
            arguments.len()
        )));
    }
    arguments
        .iter()
        .zip(shapes)
        .map(|(argument, shape)| decode_argument_json(argument, shape, context))
        .collect()
}

fn decode_argument_json(
    argument: &SurfaceArgumentJson,
    shape: &SurfaceInputKeyShape,
    context: SurfaceJsonErrorContext,
) -> Result<SavedKey, SurfaceReadError> {
    match shape {
        SurfaceInputKeyShape::Scalar(scalar) => {
            decode_scalar_argument_json(argument, *scalar, context)
        }
        SurfaceInputKeyShape::Enum {
            enum_catalog_id,
            member_catalog_ids,
        } => decode_enum_argument_json(argument, enum_catalog_id, member_catalog_ids, context),
        SurfaceInputKeyShape::Identity(identity_shape) => {
            decode_identity_argument_json(argument, identity_shape, context)
        }
    }
}

fn decode_identity_json(
    identity: &SurfaceIdentityJson,
    shape: &SurfaceIdentityInputShape,
    context: SurfaceJsonErrorContext,
) -> Result<Vec<SavedKey>, SurfaceReadError> {
    let found_store = parse_catalog_id(&identity.store_catalog_id, "store", context)?;
    if found_store != shape.store_catalog_id {
        return Err(
            context.error("surface identity store catalog id does not match the request shape")
        );
    }
    if identity.keys.len() != shape.keys.len() {
        return Err(context.error(format!(
            "surface identity expects {} key(s), got {}",
            shape.keys.len(),
            identity.keys.len()
        )));
    }
    identity
        .keys
        .iter()
        .zip(&shape.keys)
        .map(|(key, key_shape)| {
            let SurfaceInputKeyShape::Scalar(scalar) = key_shape else {
                return Err(context.error("surface identity keys must be scalar"));
            };
            decode_scalar_key_json(key, *scalar, context)
        })
        .collect()
}

fn decode_identity_argument_json(
    argument: &SurfaceArgumentJson,
    shape: &SurfaceIdentityInputShape,
    context: SurfaceJsonErrorContext,
) -> Result<SavedKey, SurfaceReadError> {
    let SurfaceArgumentJson::Identity {
        store_catalog_id,
        keys,
    } = argument
    else {
        return Err(context.error("surface argument does not match the checked identity shape"));
    };
    let identity = SurfaceIdentityJson {
        store_catalog_id: store_catalog_id.clone(),
        keys: keys.clone(),
    };
    let keys = decode_identity_json(&identity, shape, context)?;
    Ok(SavedKey::Bytes(encode_identity_index_key(
        shape.store_catalog_id.as_str(),
        &keys,
    )))
}

fn decode_enum_argument_json(
    argument: &SurfaceArgumentJson,
    expected_enum_catalog_id: &CatalogId,
    member_catalog_ids: &[CatalogId],
    context: SurfaceJsonErrorContext,
) -> Result<SavedKey, SurfaceReadError> {
    let SurfaceArgumentJson::Enum {
        enum_catalog_id,
        member_catalog_id,
    } = argument
    else {
        return Err(context.error("surface argument does not match the checked enum shape"));
    };
    let found_enum = parse_catalog_id(enum_catalog_id, "enum", context)?;
    if found_enum != *expected_enum_catalog_id {
        return Err(context.error("surface enum catalog id does not match the request shape"));
    }
    let found_member = parse_catalog_id(member_catalog_id, "enum member", context)?;
    if !member_catalog_ids.contains(&found_member) {
        return Err(context.error("surface enum member is not allowed by the request shape"));
    }
    Ok(SavedKey::Str(found_member.as_str().to_string()))
}

fn decode_scalar_key_json(
    key: &SurfaceKeyJson,
    expected: ScalarType,
    context: SurfaceJsonErrorContext,
) -> Result<SavedKey, SurfaceReadError> {
    SurfaceScalarInput::from_key_json(key).decode(expected, context)
}

fn decode_scalar_argument_json(
    argument: &SurfaceArgumentJson,
    expected: ScalarType,
    context: SurfaceJsonErrorContext,
) -> Result<SavedKey, SurfaceReadError> {
    SurfaceScalarInput::from_argument_json(argument).decode(expected, context)
}

fn decode_update_fields_json(
    fields: &[SurfaceUpdateFieldJson],
    context: SurfaceJsonErrorContext,
) -> Result<Vec<SurfaceUpdateField>, SurfaceReadError> {
    fields
        .iter()
        .map(|field| {
            Ok(SurfaceUpdateField {
                catalog_id: parse_catalog_id(&field.catalog_id, "update field", context)?,
                value: decode_surface_write_value_json(&field.value, context)?,
            })
        })
        .collect()
}

fn decode_create_fields_json(
    fields: &[SurfaceCreateFieldJson],
    context: SurfaceJsonErrorContext,
) -> Result<Vec<SurfaceCreateField>, SurfaceReadError> {
    fields
        .iter()
        .map(|field| {
            Ok(SurfaceCreateField {
                catalog_id: parse_catalog_id(&field.catalog_id, "create field", context)?,
                value: decode_surface_write_value_json(&field.value, context)?,
            })
        })
        .collect()
}

fn decode_surface_write_value_json(
    value: &SurfaceWriteValueJson,
    context: SurfaceJsonErrorContext,
) -> Result<SurfaceValue, SurfaceReadError> {
    Ok(match value {
        SurfaceWriteValueJson::Int { value } => {
            SurfaceValue::Int(parse_i64_string(value, context)?)
        }
        SurfaceWriteValueJson::Bool { value } => SurfaceValue::Bool(*value),
        SurfaceWriteValueJson::String { value } => SurfaceValue::Str(value.clone()),
        SurfaceWriteValueJson::Date { days_since_epoch } => SurfaceValue::Date(*days_since_epoch),
        SurfaceWriteValueJson::Duration { nanos } => {
            SurfaceValue::Duration(parse_i128_string(nanos, context)?)
        }
        SurfaceWriteValueJson::Instant { nanos_since_epoch } => {
            SurfaceValue::Instant(parse_i128_string(nanos_since_epoch, context)?)
        }
        SurfaceWriteValueJson::Decimal { value } => {
            SurfaceValue::Decimal(parse_decimal_string(value, context)?)
        }
        SurfaceWriteValueJson::Bytes { value_b64 } => {
            SurfaceValue::Bytes(decode_base64(value_b64, context)?)
        }
        SurfaceWriteValueJson::Enum {
            enum_catalog_id,
            member_catalog_id,
        } => SurfaceValue::Enum(marrow_run::SurfaceEnumValue {
            enum_catalog_id: parse_catalog_id(enum_catalog_id, "enum", context)?,
            member_catalog_id: parse_catalog_id(member_catalog_id, "enum member", context)?,
            render_label: String::new(),
        }),
        SurfaceWriteValueJson::Identity {
            store_catalog_id,
            keys,
        } => SurfaceValue::Identity(SurfaceReadIdentity {
            store_catalog_id: parse_catalog_id(store_catalog_id, "store", context)?,
            keys: keys
                .iter()
                .map(|key| decode_surface_write_identity_key_json(key, context))
                .collect::<Result<Vec<_>, _>>()?,
        }),
    })
}

fn decode_surface_write_identity_key_json(
    key: &SurfaceKeyJson,
    context: SurfaceJsonErrorContext,
) -> Result<SavedKey, SurfaceReadError> {
    Ok(match key {
        SurfaceKeyJson::Int { value } => SavedKey::Int(parse_i64_string(value, context)?),
        SurfaceKeyJson::Bool { value } => SavedKey::Bool(*value),
        SurfaceKeyJson::String { value } => SavedKey::Str(value.clone()),
        SurfaceKeyJson::Date { days_since_epoch } => SavedKey::Date(*days_since_epoch),
        SurfaceKeyJson::Duration { nanos } => {
            SavedKey::Duration(parse_i128_string(nanos, context)?)
        }
        SurfaceKeyJson::Instant { nanos_since_epoch } => {
            SavedKey::Instant(parse_i128_string(nanos_since_epoch, context)?)
        }
        SurfaceKeyJson::Bytes { value_b64 } => SavedKey::Bytes(decode_base64(value_b64, context)?),
    })
}

enum SurfaceScalarInput<'a> {
    Int(&'a str),
    Bool(bool),
    String(&'a str),
    Date(i32),
    Duration(&'a str),
    Instant(&'a str),
    Bytes(&'a str),
    NonScalar,
}

impl<'a> SurfaceScalarInput<'a> {
    fn from_key_json(key: &'a SurfaceKeyJson) -> Self {
        match key {
            SurfaceKeyJson::Int { value } => Self::Int(value),
            SurfaceKeyJson::Bool { value } => Self::Bool(*value),
            SurfaceKeyJson::String { value } => Self::String(value),
            SurfaceKeyJson::Date { days_since_epoch } => Self::Date(*days_since_epoch),
            SurfaceKeyJson::Duration { nanos } => Self::Duration(nanos),
            SurfaceKeyJson::Instant { nanos_since_epoch } => Self::Instant(nanos_since_epoch),
            SurfaceKeyJson::Bytes { value_b64 } => Self::Bytes(value_b64),
        }
    }

    fn from_argument_json(argument: &'a SurfaceArgumentJson) -> Self {
        match argument {
            SurfaceArgumentJson::Int { value } => Self::Int(value),
            SurfaceArgumentJson::Bool { value } => Self::Bool(*value),
            SurfaceArgumentJson::String { value } => Self::String(value),
            SurfaceArgumentJson::Date { days_since_epoch } => Self::Date(*days_since_epoch),
            SurfaceArgumentJson::Duration { nanos } => Self::Duration(nanos),
            SurfaceArgumentJson::Instant { nanos_since_epoch } => Self::Instant(nanos_since_epoch),
            SurfaceArgumentJson::Bytes { value_b64 } => Self::Bytes(value_b64),
            SurfaceArgumentJson::Enum { .. } | SurfaceArgumentJson::Identity { .. } => {
                Self::NonScalar
            }
        }
    }

    fn decode(
        self,
        expected: ScalarType,
        context: SurfaceJsonErrorContext,
    ) -> Result<SavedKey, SurfaceReadError> {
        let key = match (expected, self) {
            (ScalarType::Int, Self::Int(value)) => SavedKey::Int(parse_i64_string(value, context)?),
            (ScalarType::Bool, Self::Bool(value)) => SavedKey::Bool(value),
            (ScalarType::Str, Self::String(value)) => SavedKey::Str(value.to_string()),
            (ScalarType::Date, Self::Date(days)) => SavedKey::Date(days),
            (ScalarType::Duration, Self::Duration(nanos)) => {
                SavedKey::Duration(parse_i128_string(nanos, context)?)
            }
            (ScalarType::Instant, Self::Instant(nanos)) => {
                SavedKey::Instant(parse_i128_string(nanos, context)?)
            }
            (ScalarType::Bytes, Self::Bytes(value_b64)) => {
                SavedKey::Bytes(decode_base64(value_b64, context)?)
            }
            (ScalarType::Decimal, _) => {
                return Err(context.error("surface decimal values are not supported as keys"));
            }
            _ => {
                return Err(context.error(format!(
                    "surface argument does not match the checked {} key shape",
                    expected.name()
                )));
            }
        };
        if scalar_key_matches_type(&key, expected) {
            Ok(key)
        } else {
            let message = validate_scalar_key(&key)
                .map(|()| "surface argument does not match the checked scalar type".to_string())
                .unwrap_or_else(|error| error.to_string());
            Err(context.error(message))
        }
    }
}

fn render_cursor_boundary_json(
    shape: &SurfaceCursorBoundaryInputShape,
    boundary: &SurfacePageBoundary,
) -> Result<SurfaceCursorBoundaryJson, SurfaceReadError> {
    match (shape, boundary) {
        (
            SurfaceCursorBoundaryInputShape::RootIdentity { identity: shape },
            SurfacePageBoundary::RootIdentity(identity),
        ) => Ok(SurfaceCursorBoundaryJson::RootIdentity {
            identity: render_identity_json(identity, shape, SurfaceJsonErrorContext::Cursor)?,
        }),
        (
            SurfaceCursorBoundaryInputShape::IndexIdentity {
                exact_keys: exact_key_shapes,
                identity: identity_shape,
            },
            SurfacePageBoundary::IndexIdentity {
                exact_keys,
                identity,
            },
        ) => Ok(SurfaceCursorBoundaryJson::IndexIdentity {
            exact_keys: render_argument_tuple(
                exact_keys,
                exact_key_shapes,
                SurfaceJsonErrorContext::Cursor,
            )?,
            identity: render_identity_json(
                identity,
                identity_shape,
                SurfaceJsonErrorContext::Cursor,
            )?,
        }),
        _ => Err(SurfaceJsonErrorContext::Cursor
            .error("surface cursor boundary does not match the collection shape")),
    }
}

fn decode_cursor_boundary_json(
    boundary: &SurfaceCursorBoundaryJson,
    shape: &SurfaceCursorBoundaryInputShape,
    context: SurfaceJsonErrorContext,
) -> Result<SurfacePageBoundary, SurfaceReadError> {
    match (shape, boundary) {
        (
            SurfaceCursorBoundaryInputShape::RootIdentity { identity: shape },
            SurfaceCursorBoundaryJson::RootIdentity { identity },
        ) => Ok(SurfacePageBoundary::RootIdentity(decode_identity_json(
            identity, shape, context,
        )?)),
        (
            SurfaceCursorBoundaryInputShape::IndexIdentity {
                exact_keys: exact_key_shapes,
                identity: identity_shape,
            },
            SurfaceCursorBoundaryJson::IndexIdentity {
                exact_keys,
                identity,
            },
        ) => Ok(SurfacePageBoundary::IndexIdentity {
            exact_keys: decode_argument_tuple(exact_keys, exact_key_shapes, context)?,
            identity: decode_identity_json(identity, identity_shape, context)?,
        }),
        _ => Err(context.error("surface cursor boundary does not match the collection shape")),
    }
}

fn render_argument_tuple(
    keys: &[SavedKey],
    shapes: &[SurfaceInputKeyShape],
    context: SurfaceJsonErrorContext,
) -> Result<Vec<SurfaceArgumentJson>, SurfaceReadError> {
    if keys.len() != shapes.len() {
        return Err(context.error(format!(
            "surface argument tuple expects {} key(s), got {}",
            shapes.len(),
            keys.len()
        )));
    }
    keys.iter()
        .zip(shapes)
        .map(|(key, shape)| render_argument_json(key, shape, context))
        .collect()
}

fn render_argument_json(
    key: &SavedKey,
    shape: &SurfaceInputKeyShape,
    context: SurfaceJsonErrorContext,
) -> Result<SurfaceArgumentJson, SurfaceReadError> {
    match shape {
        SurfaceInputKeyShape::Scalar(scalar) => render_scalar_argument_json(key, *scalar, context),
        SurfaceInputKeyShape::Enum {
            enum_catalog_id,
            member_catalog_ids,
        } => render_enum_argument_json(key, enum_catalog_id, member_catalog_ids, context),
        SurfaceInputKeyShape::Identity(shape) => render_identity_argument_json(key, shape, context),
    }
}

fn render_identity_json(
    keys: &[SavedKey],
    shape: &SurfaceIdentityInputShape,
    context: SurfaceJsonErrorContext,
) -> Result<SurfaceIdentityJson, SurfaceReadError> {
    if keys.len() != shape.keys.len() {
        return Err(context.error(format!(
            "surface identity expects {} key(s), got {}",
            shape.keys.len(),
            keys.len()
        )));
    }
    let keys = keys
        .iter()
        .zip(&shape.keys)
        .map(|(key, key_shape)| {
            let SurfaceInputKeyShape::Scalar(scalar) = key_shape else {
                return Err(context.error("surface identity keys must be scalar"));
            };
            render_scalar_key_json(key, *scalar, context)
        })
        .collect::<Result<Vec<_>, SurfaceReadError>>()?;
    Ok(SurfaceIdentityJson {
        store_catalog_id: shape.store_catalog_id.as_str().to_string(),
        keys,
    })
}

fn render_identity_argument_json(
    key: &SavedKey,
    shape: &SurfaceIdentityInputShape,
    context: SurfaceJsonErrorContext,
) -> Result<SurfaceArgumentJson, SurfaceReadError> {
    let SavedKey::Bytes(bytes) = key else {
        return Err(context.error("surface index key does not match the checked identity shape"));
    };
    let keys = decode_identity_index_key(bytes, shape.store_catalog_id.as_str(), shape.keys.len())
        .ok_or_else(|| context.error("surface identity index key did not decode"))?;
    Ok(SurfaceArgumentJson::Identity {
        store_catalog_id: shape.store_catalog_id.as_str().to_string(),
        keys: render_identity_json(&keys, shape, context)?.keys,
    })
}

fn render_enum_argument_json(
    key: &SavedKey,
    enum_catalog_id: &CatalogId,
    member_catalog_ids: &[CatalogId],
    context: SurfaceJsonErrorContext,
) -> Result<SurfaceArgumentJson, SurfaceReadError> {
    let SavedKey::Str(member_catalog_id) = key else {
        return Err(context.error("surface index key does not match the checked enum shape"));
    };
    let member_catalog_id = parse_catalog_id(member_catalog_id, "enum member", context)?;
    if !member_catalog_ids.contains(&member_catalog_id) {
        return Err(context.error("surface enum member is not allowed by the request shape"));
    }
    Ok(SurfaceArgumentJson::Enum {
        enum_catalog_id: enum_catalog_id.as_str().to_string(),
        member_catalog_id: member_catalog_id.as_str().to_string(),
    })
}

fn render_scalar_key_json(
    key: &SavedKey,
    expected: ScalarType,
    context: SurfaceJsonErrorContext,
) -> Result<SurfaceKeyJson, SurfaceReadError> {
    if !scalar_key_matches_type(key, expected) {
        return Err(context.error("surface key does not match the checked scalar shape"));
    }
    Ok(SurfaceKeyJson::from(key))
}

fn render_scalar_argument_json(
    key: &SavedKey,
    expected: ScalarType,
    context: SurfaceJsonErrorContext,
) -> Result<SurfaceArgumentJson, SurfaceReadError> {
    if !scalar_key_matches_type(key, expected) {
        return Err(context.error("surface argument does not match the checked scalar shape"));
    }
    Ok(match key {
        SavedKey::Int(value) => SurfaceArgumentJson::Int {
            value: value.to_string(),
        },
        SavedKey::Bool(value) => SurfaceArgumentJson::Bool { value: *value },
        SavedKey::Str(value) => SurfaceArgumentJson::String {
            value: value.clone(),
        },
        SavedKey::Date(value) => SurfaceArgumentJson::Date {
            days_since_epoch: *value,
        },
        SavedKey::Duration(value) => SurfaceArgumentJson::Duration {
            nanos: value.to_string(),
        },
        SavedKey::Instant(value) => SurfaceArgumentJson::Instant {
            nanos_since_epoch: value.to_string(),
        },
        SavedKey::Bytes(value) => SurfaceArgumentJson::Bytes {
            value_b64: marrow_run::base64::encode(value),
        },
    })
}

fn parse_catalog_id(
    raw: &str,
    what: &str,
    context: SurfaceJsonErrorContext,
) -> Result<CatalogId, SurfaceReadError> {
    CatalogId::new(raw.to_string())
        .map_err(|_| context.error(format!("malformed surface {what} catalog id")))
}

fn parse_i64_string(text: &str, context: SurfaceJsonErrorContext) -> Result<i64, SurfaceReadError> {
    let value = text
        .parse::<i64>()
        .ok()
        .filter(|value| value.to_string() == text)
        .ok_or_else(|| context.error("surface int argument must be a canonical decimal string"))?;
    Ok(value)
}

fn parse_i128_string(
    text: &str,
    context: SurfaceJsonErrorContext,
) -> Result<i128, SurfaceReadError> {
    let value = text
        .parse::<i128>()
        .ok()
        .filter(|value| value.to_string() == text)
        .ok_or_else(|| context.error("surface i128 argument must be a canonical decimal string"))?;
    Ok(value)
}

fn parse_decimal_string(
    text: &str,
    context: SurfaceJsonErrorContext,
) -> Result<Decimal, SurfaceReadError> {
    Decimal::parse_canonical(text)
        .map_err(|_| context.error("surface decimal value must be a canonical decimal string"))
}

fn decode_base64(
    text: &str,
    context: SurfaceJsonErrorContext,
) -> Result<Vec<u8>, SurfaceReadError> {
    marrow_run::base64::decode(text)
        .ok_or_else(|| context.error("surface bytes argument must be padded base64"))
}

fn decode_engine_profile_digest(
    text: &str,
    context: SurfaceJsonErrorContext,
) -> Result<[u8; 8], SurfaceReadError> {
    if !is_lower_hex(text, 16) {
        return Err(
            context.error("surface cursor engine profile digest must be 16 lowercase hex digits")
        );
    }
    let bytes = marrow_run::hex::decode(text)
        .ok_or_else(|| context.error("surface cursor engine profile digest must be hex encoded"))?;
    bytes
        .try_into()
        .map_err(|_| context.error("surface cursor engine profile digest must be exactly 8 bytes"))
}

fn decode_sha256_digest(
    text: &str,
    field: &str,
    context: SurfaceJsonErrorContext,
) -> Result<String, SurfaceReadError> {
    let Some(hex) = text.strip_prefix("sha256:") else {
        return Err(context.error(format!(
            "surface cursor {field} must be a canonical sha256 digest"
        )));
    };
    if !is_lower_hex(hex, 64) {
        return Err(context.error(format!(
            "surface cursor {field} must be a canonical sha256 digest"
        )));
    }
    Ok(text.to_string())
}

fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
