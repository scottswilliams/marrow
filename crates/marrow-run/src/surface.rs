use std::collections::{HashMap, HashSet};

use marrow_check::{
    CheckedFacts, CheckedProgram, EnumId, EnumMemberId, ResourceId, ResourceMemberFact,
    ResourceMemberId, ResourceMemberKind, StoreFact, StoredValueMeaning, SurfaceCatalogStatus,
    SurfaceFact, SurfaceId, SurfaceReadFootprint, SurfaceReadOperationFact,
    SurfaceReadOperationKind,
};
use marrow_store::Decimal;
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_store::tree::{DataPathSegment, TreeStore, decode_tree_enum_member};
use marrow_store::value::{
    SavedValue, ScalarType, decode_value, scalar_key_matches_type, validate_scalar_key,
};
use marrow_syntax::{Diagnose, SourceSpan};

use crate::error::RuntimeError;
use crate::evolution::{FenceError, fence};

pub const SURFACE_REQUEST: &str = "surface.request";
pub const SURFACE_ABSENT: &str = "surface.absent";
pub const SURFACE_INVALID_DATA: &str = "surface.invalid_data";
pub const SURFACE_ABI_MISMATCH: &str = "surface.abi_mismatch";
pub const SURFACE_STORE: &str = "surface.store";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceReadError {
    code: &'static str,
    message: String,
    span: SourceSpan,
}

impl SurfaceReadError {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn span(&self) -> SourceSpan {
        self.span
    }

    pub fn into_runtime_error(self) -> RuntimeError {
        RuntimeError::fatal(self.code, self.message, self.span)
    }
}

impl Diagnose for SurfaceReadError {
    fn code(&self) -> &str {
        self.code
    }

    fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for SurfaceReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for SurfaceReadError {}

impl From<SurfaceReadError> for RuntimeError {
    fn from(error: SurfaceReadError) -> Self {
        error.into_runtime_error()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceNodeReadShape {
    Singleton,
    Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceReadInput<'a> {
    Singleton,
    Point { identity: &'a [SavedKey] },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceReadIdentity {
    /// The accepted catalog identity of the backing store.
    pub store_catalog_id: CatalogId,
    pub keys: Vec<SavedKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceEnumValue {
    pub enum_catalog_id: CatalogId,
    pub member_catalog_id: CatalogId,
    /// A display label from the source enum member; compatibility never depends on it.
    pub render_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceValue {
    Int(i64),
    Bool(bool),
    Str(String),
    Instant(i128),
    Date(i32),
    Duration(i128),
    Decimal(Decimal),
    Bytes(Vec<u8>),
    Enum(SurfaceEnumValue),
    Identity(SurfaceReadIdentity),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceReadField {
    /// The accepted catalog identity of the projected resource member.
    pub catalog_id: CatalogId,
    /// A display label from the source surface; compatibility never depends on it.
    pub render_label: String,
    pub value: Option<SurfaceValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceReadRecord {
    pub identity: Option<SurfaceReadIdentity>,
    pub fields: Vec<SurfaceReadField>,
}

pub struct SurfaceNodeRead<'a> {
    store: &'a TreeStore,
    plan: SurfaceNodeReadPlan<'a>,
}

impl<'a> SurfaceNodeRead<'a> {
    pub fn admit(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        surface: SurfaceId,
    ) -> Result<Self, SurfaceReadError> {
        admit_surface_store(program, store)?;
        Ok(Self {
            store,
            plan: SurfaceNodeReadPlan::prepare(program, surface)?,
        })
    }

    pub fn surface(&self) -> SurfaceId {
        self.plan.surface
    }

    pub fn shape(&self) -> SurfaceNodeReadShape {
        self.plan.shape
    }

    pub fn execute(
        &self,
        input: SurfaceReadInput<'_>,
    ) -> Result<SurfaceReadRecord, SurfaceReadError> {
        match (self.plan.shape, input) {
            (SurfaceNodeReadShape::Singleton, SurfaceReadInput::Singleton) => {
                self.read_identity(&[], None)
            }
            (SurfaceNodeReadShape::Point, SurfaceReadInput::Point { identity }) => {
                self.read_point(identity)
            }
            (SurfaceNodeReadShape::Singleton, SurfaceReadInput::Point { .. }) => Err(request_at(
                format!("surface `{}` is a singleton read", self.plan.surface_label),
                self.plan.span,
            )),
            (SurfaceNodeReadShape::Point, SurfaceReadInput::Singleton) => Err(request_at(
                format!("surface `{}` requires an identity", self.plan.surface_label),
                self.plan.span,
            )),
        }
    }

    pub fn read_point(&self, identity: &[SavedKey]) -> Result<SurfaceReadRecord, SurfaceReadError> {
        if self.plan.shape != SurfaceNodeReadShape::Point {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyed store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.plan.validate_identity(identity)?;
        self.read_identity(
            identity,
            Some(SurfaceReadIdentity {
                store_catalog_id: self.plan.store_catalog_id.clone(),
                keys: identity.to_vec(),
            }),
        )
    }

    pub fn read_singleton(&self) -> Result<SurfaceReadRecord, SurfaceReadError> {
        if self.plan.shape != SurfaceNodeReadShape::Singleton {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyless singleton store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.read_identity(&[], None)
    }

    fn read_identity(
        &self,
        identity: &[SavedKey],
        output_identity: Option<SurfaceReadIdentity>,
    ) -> Result<SurfaceReadRecord, SurfaceReadError> {
        let fields = self.plan.materialize(self.store, identity)?;
        Ok(SurfaceReadRecord {
            identity: output_identity,
            fields,
        })
    }
}

pub fn read_surface_point(
    program: &CheckedProgram,
    store: &TreeStore,
    surface: SurfaceId,
    identity: &[SavedKey],
) -> Result<SurfaceReadRecord, SurfaceReadError> {
    SurfaceNodeRead::admit(program, store, surface)?.read_point(identity)
}

pub fn read_surface_singleton(
    program: &CheckedProgram,
    store: &TreeStore,
    surface: SurfaceId,
) -> Result<SurfaceReadRecord, SurfaceReadError> {
    SurfaceNodeRead::admit(program, store, surface)?.read_singleton()
}

struct SurfaceNodeReadPlan<'a> {
    facts: &'a CheckedFacts,
    surface: SurfaceId,
    surface_label: String,
    store_catalog_id: CatalogId,
    identity_keys: Vec<StoredValueMeaning>,
    shape: SurfaceNodeReadShape,
    reads: Vec<SurfaceMemberRead>,
    projection: Vec<ResourceMemberId>,
    span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SurfaceMemberRead {
    member: ResourceMemberId,
    catalog_id: CatalogId,
    render_label: String,
    path: Vec<DataPathSegment>,
    value_meaning: StoredValueMeaning,
    required: bool,
    projected: bool,
}

impl<'a> SurfaceNodeReadPlan<'a> {
    fn prepare(program: &'a CheckedProgram, surface: SurfaceId) -> Result<Self, SurfaceReadError> {
        let surface = checked_surface(program, surface)?;
        require_stable_surface(surface)?;
        let store = program.facts.store(surface.store);
        let (operation, shape) = backing_node_operation(surface)?;
        let footprint_resource = checked_footprint_resource(operation, store, surface.span)?;
        let store_catalog_id = catalog_id(&store.catalog_id, "store", surface.span)?;
        let identity_keys = identity_key_meanings(store, surface.span)?;
        let reads =
            surface_member_reads(&program.facts, footprint_resource, operation, surface.span)?;
        Ok(Self {
            facts: &program.facts,
            surface: surface.id,
            surface_label: surface.name.clone(),
            store_catalog_id,
            identity_keys,
            shape,
            reads,
            projection: operation.projection.clone(),
            span: operation.span,
        })
    }

    fn validate_identity(&self, identity: &[SavedKey]) -> Result<(), SurfaceReadError> {
        if identity.len() != self.identity_keys.len() {
            return Err(request_at(
                format!(
                    "surface identity expects {} key(s), got {}",
                    self.identity_keys.len(),
                    identity.len()
                ),
                self.span,
            ));
        }
        for (key, meaning) in identity.iter().zip(&self.identity_keys) {
            validate_scalar_key(key).map_err(|error| request_at(error.to_string(), self.span))?;
            let StoredValueMeaning::Scalar(expected) = meaning else {
                return Err(abi_mismatch(
                    "checked store identity key is not scalar",
                    self.span,
                ));
            };
            if !scalar_key_matches_type(key, *expected) {
                return Err(request_at(
                    "surface identity key does not match the checked store key type",
                    self.span,
                ));
            }
        }
        Ok(())
    }

    fn materialize(
        &self,
        store: &TreeStore,
        identity: &[SavedKey],
    ) -> Result<Vec<SurfaceReadField>, SurfaceReadError> {
        if !store
            .data_subtree_exists(&self.store_catalog_id, identity, &[])
            .map_err(|error| surface_store_error(error, self.span))?
        {
            return Err(surface_error_at(
                SURFACE_ABSENT,
                "surface record is absent",
                self.span,
            ));
        }

        let mut values = HashMap::new();
        for read in &self.reads {
            let bytes = store
                .read_data_value(&self.store_catalog_id, identity, &read.path)
                .map_err(|error| surface_store_error(error, self.span))?;
            let Some(bytes) = bytes else {
                if read.required {
                    return Err(invalid_data(
                        format!("required stored field `{}` is absent", read.render_label),
                        self.span,
                    ));
                }
                if read.projected {
                    values.insert(
                        read.member,
                        SurfaceReadField {
                            catalog_id: read.catalog_id.clone(),
                            render_label: read.render_label.clone(),
                            value: None,
                        },
                    );
                }
                continue;
            };
            let value =
                decode_surface_value(self.facts, &bytes, &read.value_meaning).ok_or_else(|| {
                    invalid_data(
                        format!("stored value for `{}` did not decode", read.render_label),
                        self.span,
                    )
                })?;
            if read.projected {
                values.insert(
                    read.member,
                    SurfaceReadField {
                        catalog_id: read.catalog_id.clone(),
                        render_label: read.render_label.clone(),
                        value: Some(value),
                    },
                );
            }
        }

        let mut fields = Vec::with_capacity(self.projection.len());
        for member_id in &self.projection {
            let Some(field) = values.remove(member_id) else {
                return Err(abi_mismatch(
                    "checked surface projection member is missing from the read plan",
                    self.span,
                ));
            };
            fields.push(field);
        }
        Ok(fields)
    }
}

fn admit_surface_store(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(), SurfaceReadError> {
    let accepted_epoch = program.catalog.accepted_epoch.ok_or_else(|| {
        abi_mismatch(
            "surface serving requires a checked program bound to an accepted catalog",
            SourceSpan::default(),
        )
    })?;
    let accepted_digest = program.catalog.accepted_digest.as_deref().ok_or_else(|| {
        abi_mismatch(
            "surface serving requires an accepted catalog digest",
            SourceSpan::default(),
        )
    })?;
    let Some(_uid) = store
        .read_store_uid()
        .map_err(|error| surface_store_error(error, SourceSpan::default()))?
    else {
        return Err(abi_mismatch(
            "surface serving requires a stamped store uid",
            SourceSpan::default(),
        ));
    };
    let Some(_commit) = store
        .read_commit_metadata()
        .map_err(|error| surface_store_error(error, SourceSpan::default()))?
    else {
        return Err(abi_mismatch(
            "surface serving requires commit metadata",
            SourceSpan::default(),
        ));
    };
    fence(Some(accepted_epoch), &program.source_digest(), store).map_err(surface_fence_error)?;
    let found = store
        .catalog_snapshot_digest()
        .map_err(|error| surface_store_error(error, SourceSpan::default()))?;
    if found.as_deref() != Some(accepted_digest) {
        return Err(abi_mismatch(
            "store catalog digest does not match the checked surface catalog",
            SourceSpan::default(),
        ));
    }
    Ok(())
}

fn checked_surface(
    program: &CheckedProgram,
    surface: SurfaceId,
) -> Result<&SurfaceFact, SurfaceReadError> {
    program
        .facts
        .surfaces()
        .get(surface.0 as usize)
        .ok_or_else(|| request("checked surface id is not present in this program"))
}

fn require_stable_surface(surface: &SurfaceFact) -> Result<(), SurfaceReadError> {
    match surface.catalog_status {
        SurfaceCatalogStatus::Stable => Ok(()),
        SurfaceCatalogStatus::SourceOnly(_) => Err(abi_mismatch(
            format!(
                "surface `{}` is source-only; run against accepted catalog identities before serving it",
                surface.name
            ),
            surface.span,
        )),
    }
}

fn backing_node_operation(
    surface: &SurfaceFact,
) -> Result<(&SurfaceReadOperationFact, SurfaceNodeReadShape), SurfaceReadError> {
    surface
        .read_operations
        .iter()
        .find_map(|operation| match operation.kind {
            SurfaceReadOperationKind::SingletonRead { store } if store == surface.store => {
                Some((operation, SurfaceNodeReadShape::Singleton))
            }
            SurfaceReadOperationKind::PointRead { store } if store == surface.store => {
                Some((operation, SurfaceNodeReadShape::Point))
            }
            _ => None,
        })
        .ok_or_else(|| {
            abi_mismatch(
                format!(
                    "surface `{}` has no backing node read operation",
                    surface.name
                ),
                surface.span,
            )
        })
}

fn checked_footprint_resource(
    operation: &SurfaceReadOperationFact,
    store: &StoreFact,
    span: SourceSpan,
) -> Result<ResourceId, SurfaceReadError> {
    match operation.footprint {
        SurfaceReadFootprint::FullRecord { resource } if resource == store.resource => Ok(resource),
        SurfaceReadFootprint::FullRecord { .. } => Err(abi_mismatch(
            "checked surface read footprint does not match the backing store resource",
            span,
        )),
    }
}

fn identity_key_meanings(
    store: &StoreFact,
    span: SourceSpan,
) -> Result<Vec<StoredValueMeaning>, SurfaceReadError> {
    store
        .identity_keys
        .iter()
        .map(|key| {
            key.value_meaning
                .clone()
                .ok_or_else(|| abi_mismatch("checked store identity key has no meaning", span))
        })
        .collect()
}

fn surface_member_reads(
    facts: &CheckedFacts,
    resource: ResourceId,
    operation: &SurfaceReadOperationFact,
    span: SourceSpan,
) -> Result<Vec<SurfaceMemberRead>, SurfaceReadError> {
    let projected: HashSet<ResourceMemberId> = operation.projection.iter().copied().collect();
    let mut read_ids = HashSet::new();
    read_ids.extend(operation.projection.iter().copied());
    for member in facts.resource_members() {
        if member.resource == resource
            && member.plain_field_required == Some(true)
            && path_is_inside_unkeyed_record(facts, member.id)
        {
            read_ids.insert(member.id);
        }
    }

    let mut reads = Vec::with_capacity(read_ids.len());
    for member_id in read_ids {
        let member = resource_member(facts, member_id, span)?;
        if member.resource != resource {
            return Err(abi_mismatch(
                "checked surface projection member is outside the read footprint",
                span,
            ));
        }
        let required = member.plain_field_required.unwrap_or(false);
        let projected_member = projected.contains(&member_id);
        let path = member_data_path(facts, member_id, span)?;
        let catalog_id = catalog_id(&member.catalog_id, "resource member", span)?;
        let value_meaning = value_meaning_from_member(member, span)?;
        reads.push(SurfaceMemberRead {
            member: member_id,
            catalog_id,
            render_label: member.name.clone(),
            path,
            value_meaning,
            required,
            projected: projected_member,
        });
    }
    reads.sort_by_key(|read| (read.path.len(), read.member.0));
    Ok(reads)
}

fn path_is_inside_unkeyed_record(facts: &CheckedFacts, member_id: ResourceMemberId) -> bool {
    let mut current = Some(member_id);
    while let Some(id) = current {
        let Some(member) = facts.resource_members().get(id.0 as usize) else {
            return false;
        };
        if member.key_count != 0 {
            return false;
        }
        current = member.parent;
    }
    true
}

fn member_data_path(
    facts: &CheckedFacts,
    member_id: ResourceMemberId,
    span: SourceSpan,
) -> Result<Vec<DataPathSegment>, SurfaceReadError> {
    let mut members = Vec::new();
    let mut current = Some(member_id);
    while let Some(id) = current {
        let member = resource_member(facts, id, span)?;
        if member.key_count != 0 {
            return Err(abi_mismatch(
                "surface read plan cannot address a keyed member path",
                span,
            ));
        }
        members.push(member);
        current = member.parent;
    }
    members.reverse();
    members
        .iter()
        .map(|member| {
            catalog_id(&member.catalog_id, "resource member", span).map(DataPathSegment::Member)
        })
        .collect()
}

fn resource_member(
    facts: &CheckedFacts,
    member_id: ResourceMemberId,
    span: SourceSpan,
) -> Result<&ResourceMemberFact, SurfaceReadError> {
    facts
        .resource_members()
        .get(member_id.0 as usize)
        .ok_or_else(|| abi_mismatch("checked resource member id is missing", span))
}

fn value_meaning_from_member(
    member: &ResourceMemberFact,
    span: SourceSpan,
) -> Result<StoredValueMeaning, SurfaceReadError> {
    if member.kind != ResourceMemberKind::Field {
        return Err(abi_mismatch(
            "surface read plan expected a field member",
            span,
        ));
    }
    member
        .value_meaning
        .as_ref()
        .cloned()
        .ok_or_else(|| abi_mismatch("checked field member has no stored value meaning", span))
}

fn decode_surface_value(
    facts: &CheckedFacts,
    bytes: &[u8],
    meaning: &StoredValueMeaning,
) -> Option<SurfaceValue> {
    match meaning {
        StoredValueMeaning::Scalar(scalar) => {
            decode_value(bytes, *scalar).map(surface_scalar_value)
        }
        StoredValueMeaning::Enum { enum_id, members } => {
            decode_surface_enum(facts, bytes, *enum_id, members).map(SurfaceValue::Enum)
        }
        StoredValueMeaning::Identity {
            store_catalog_id,
            arity,
            key_scalars,
            ..
        } => decode_surface_identity(bytes, store_catalog_id, *arity, key_scalars)
            .map(SurfaceValue::Identity),
    }
}

fn surface_scalar_value(value: SavedValue) -> SurfaceValue {
    match value {
        SavedValue::Bool(value) => SurfaceValue::Bool(value),
        SavedValue::Int(value) => SurfaceValue::Int(value),
        SavedValue::Str(value) => SurfaceValue::Str(value),
        SavedValue::Bytes(value) => SurfaceValue::Bytes(value),
        SavedValue::Date(value) => SurfaceValue::Date(value),
        SavedValue::Duration(value) => SurfaceValue::Duration(value),
        SavedValue::Instant(value) => SurfaceValue::Instant(value),
        SavedValue::Decimal(value) => SurfaceValue::Decimal(value),
    }
}

fn decode_surface_enum(
    facts: &CheckedFacts,
    bytes: &[u8],
    enum_id: EnumId,
    members: &[EnumMemberId],
) -> Option<SurfaceEnumValue> {
    let stored = decode_tree_enum_member(bytes).ok()?;
    let enum_fact = facts.enum_(enum_id)?;
    if enum_fact.catalog_id.as_deref() != Some(stored.enum_id().as_str()) {
        return None;
    }
    let member = facts.enum_members().iter().find(|member| {
        member.enum_id == enum_id
            && members.contains(&member.id)
            && member.catalog_id.as_deref() == Some(stored.member_id().as_str())
    })?;
    if !facts.enum_member_is_selectable(member.id) {
        return None;
    }
    Some(SurfaceEnumValue {
        enum_catalog_id: stored.enum_id().clone(),
        member_catalog_id: stored.member_id().clone(),
        render_label: member.name.clone(),
    })
}

fn decode_surface_identity(
    bytes: &[u8],
    raw_store_catalog_id: &Option<String>,
    arity: usize,
    key_scalars: &[ScalarType],
) -> Option<SurfaceReadIdentity> {
    let store_catalog_id = CatalogId::new(raw_store_catalog_id.clone()?).ok()?;
    let keys = decode_identity_payload_arity(bytes, arity)?;
    identity_keys_match_scalars(&keys, key_scalars).then_some(SurfaceReadIdentity {
        store_catalog_id,
        keys,
    })
}

fn identity_keys_match_scalars(keys: &[SavedKey], key_scalars: &[ScalarType]) -> bool {
    keys.len() == key_scalars.len()
        && keys.iter().zip(key_scalars).all(|(key, scalar)| {
            validate_scalar_key(key).is_ok() && scalar_key_matches_type(key, *scalar)
        })
}

fn catalog_id(
    raw: &Option<String>,
    what: &'static str,
    span: SourceSpan,
) -> Result<CatalogId, SurfaceReadError> {
    let Some(raw) = raw.as_deref() else {
        return Err(abi_mismatch(
            format!("checked {what} catalog identity is missing"),
            span,
        ));
    };
    CatalogId::new(raw.to_string()).map_err(|_| {
        abi_mismatch(
            format!("checked {what} catalog identity is malformed"),
            span,
        )
    })
}

fn request(message: impl Into<String>) -> SurfaceReadError {
    request_at(message, SourceSpan::default())
}

fn request_at(message: impl Into<String>, span: SourceSpan) -> SurfaceReadError {
    surface_error_at(SURFACE_REQUEST, message, span)
}

fn invalid_data(message: impl Into<String>, span: SourceSpan) -> SurfaceReadError {
    surface_error_at(SURFACE_INVALID_DATA, message, span)
}

fn abi_mismatch(message: impl Into<String>, span: SourceSpan) -> SurfaceReadError {
    surface_error_at(SURFACE_ABI_MISMATCH, message, span)
}

fn store_error(message: impl Into<String>, span: SourceSpan) -> SurfaceReadError {
    surface_error_at(SURFACE_STORE, message, span)
}

fn surface_error_at(
    code: &'static str,
    message: impl Into<String>,
    span: SourceSpan,
) -> SurfaceReadError {
    SurfaceReadError {
        code,
        message: message.into(),
        span,
    }
}

fn surface_store_error(error: StoreError, span: SourceSpan) -> SurfaceReadError {
    store_error(format!("a surface read failed: {error}"), span)
}

fn surface_fence_error(error: FenceError) -> SurfaceReadError {
    match error {
        FenceError::Store(error) => surface_store_error(error, SourceSpan::default()),
        other => abi_mismatch(other.message(), SourceSpan::default()),
    }
}
