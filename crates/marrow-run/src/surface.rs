use marrow_codes::Code;
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

use marrow_check::{
    CheckedFacts, CheckedProgram, EntryIdentity, EnumId, EnumMemberId, ResourceId,
    ResourceMemberFact, ResourceMemberId, ResourceMemberKind, StoreFact, StoreIndexFact,
    StoredValueMeaning, SurfaceActionOperationDescriptor, SurfaceCatalogStatus,
    SurfaceComputedReadOperationDescriptor, SurfaceCreateOperationDescriptor,
    SurfaceDeleteOperationDescriptor, SurfaceFact, SurfaceId, SurfaceReadFootprint,
    SurfaceReadOperationFact, SurfaceReadOperationKind, SurfaceUpdateOperationDescriptor,
    checked_saved_root_place,
};
use marrow_store::Decimal;
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, decode_identity_index_key, decode_identity_payload_arity};
use marrow_store::tree::{
    DataPathSegment, EngineProfileDigest, IndexRangeBounds, StoreUid, TreeEnumMember, TreeStore,
    decode_tree_enum_member, encode_tree_enum_member,
};
use marrow_store::value::{
    SavedValue, ScalarType, ValueError, decode_value, scalar_key_matches_type, supported_date_days,
    supported_instant_nanos, validate_scalar_key,
};
use marrow_syntax::{Diagnose, SourceSpan};

use crate::entry::{EntryArgument, EntryInvocation};
use crate::error::RuntimeError;
use crate::evolution::{FenceError, fence};
use crate::saved_iter::{KeyedChildrenWalk, walk_keyed_children_after};
use crate::value::LeafValue;
use crate::write::{
    FieldPatchValue, WRITE_IDENTITY_MISMATCH, WRITE_INVALID_DATA, WRITE_STORE, WRITE_TYPE_MISMATCH,
    WRITE_UNIQUE_CONFLICT, WRITE_UNKNOWN_FIELD, WriteError, plan_field_patch_write,
};
use crate::write_plan::WritePlan;

mod create_delete;
pub use create_delete::{
    SurfaceCreate, SurfaceCreateField, SurfaceCreateInput, SurfaceDelete, SurfaceDeleteInput,
};

pub const SURFACE_REQUEST: &str = Code::SurfaceRequest.as_str();
pub const SURFACE_ABSENT: &str = Code::SurfaceAbsent.as_str();
pub const SURFACE_CONFLICT: &str = Code::SurfaceConflict.as_str();
pub const SURFACE_WRITE: &str = Code::SurfaceWrite.as_str();
pub const SURFACE_INVALID_DATA: &str = Code::SurfaceInvalidData.as_str();
pub const SURFACE_CURSOR: &str = Code::SurfaceCursor.as_str();
pub const SURFACE_STALE_CURSOR: &str = Code::SurfaceStaleCursor.as_str();
pub const SURFACE_LIMIT: &str = Code::SurfaceLimit.as_str();
pub const SURFACE_ABI_MISMATCH: &str = Code::SurfaceAbiMismatch.as_str();
pub const SURFACE_AUTH: &str = Code::SurfaceAuth.as_str();
pub const SURFACE_ACTION: &str = Code::SurfaceAction.as_str();
pub const SURFACE_COMPUTED: &str = Code::SurfaceComputed.as_str();
pub const SURFACE_STORE: &str = Code::SurfaceStore.as_str();
pub const SURFACE_MAX_PAGE_LIMIT: usize = 128;
pub const SURFACE_MAX_VALUE_BYTES: usize = 1024 * 1024;
pub const SURFACE_MAX_MATERIALIZED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceError {
    code: &'static str,
    message: String,
    span: SourceSpan,
}

pub type SurfaceReadError = SurfaceError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceActionInvocation {
    identity: EntryIdentity,
}

#[derive(Debug, Clone)]
pub struct SurfaceComputedReadInvocation {
    operation_tag: String,
    descriptor: SurfaceComputedReadOperationDescriptor,
}

impl SurfaceActionInvocation {
    pub fn admit_by_operation_tag(
        program: &CheckedProgram,
        operation_tag: &str,
    ) -> Result<Self, SurfaceReadError> {
        require_unique_surface_operation_tag(program, operation_tag)?;
        let mut matches = program.facts.surfaces().iter().flat_map(|surface| {
            surface.actions.iter().filter_map(move |action| {
                let descriptor =
                    SurfaceActionOperationDescriptor::from_action(program, surface, action)?;
                (descriptor.operation_tag == operation_tag)
                    .then_some((descriptor.identity, action.span))
            })
        });
        let Some((identity, span)) = matches.next() else {
            return Err(abi_mismatch(
                "surface action operation tag is not active",
                SourceSpan::default(),
            ));
        };
        if matches.next().is_some() {
            return Err(abi_mismatch(
                "surface action operation tag is ambiguous",
                span,
            ));
        }
        Ok(Self { identity })
    }

    pub(crate) fn invocation(&self, arguments: Vec<EntryArgument>) -> EntryInvocation {
        EntryInvocation {
            identity: self.identity.clone(),
            arguments,
        }
    }

    pub(crate) fn operation_tag(&self) -> &str {
        &self.identity.entry_tag
    }
}

impl SurfaceComputedReadInvocation {
    pub fn admit_by_operation_tag(
        program: &CheckedProgram,
        operation_tag: &str,
    ) -> Result<Self, SurfaceReadError> {
        require_unique_surface_operation_tag(program, operation_tag)?;
        let mut matches = program.facts.surfaces().iter().flat_map(|surface| {
            surface
                .computed_reads
                .iter()
                .filter_map(move |computed_read| {
                    let descriptor = SurfaceComputedReadOperationDescriptor::from_computed_read(
                        program,
                        surface,
                        computed_read,
                    )?;
                    (descriptor.operation_tag == operation_tag)
                        .then_some((descriptor, computed_read.span))
                })
        });
        let Some((descriptor, span)) = matches.next() else {
            return Err(abi_mismatch(
                "surface computed read operation tag is not active",
                SourceSpan::default(),
            ));
        };
        if matches.next().is_some() {
            return Err(abi_mismatch(
                "surface computed read operation tag is ambiguous",
                span,
            ));
        }
        Ok(Self {
            operation_tag: operation_tag.to_string(),
            descriptor,
        })
    }

    pub(crate) fn invocation(&self, arguments: Vec<EntryArgument>) -> EntryInvocation {
        EntryInvocation {
            identity: self.descriptor.callable.identity.clone(),
            arguments,
        }
    }

    pub fn descriptor(&self) -> &SurfaceComputedReadOperationDescriptor {
        &self.descriptor
    }

    pub(crate) fn operation_tag(&self) -> &str {
        &self.operation_tag
    }
}

impl SurfaceError {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn span(&self) -> SourceSpan {
        self.span
    }

    pub fn into_runtime_error(self) -> RuntimeError {
        RuntimeError::fatal(self.code, self.message, self.span)
    }

    pub fn request(message: impl Into<String>) -> Self {
        request(message)
    }

    pub fn cursor(message: impl Into<String>) -> Self {
        cursor_error(message, SourceSpan::default())
    }

    pub fn stale_cursor(message: impl Into<String>) -> Self {
        stale_cursor(message, SourceSpan::default())
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

impl From<SurfaceError> for RuntimeError {
    fn from(error: SurfaceError) -> Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceUpdateInput<'a> {
    Singleton {
        fields: &'a [SurfaceUpdateField],
    },
    Point {
        identity: &'a [SavedKey],
        fields: &'a [SurfaceUpdateField],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceReadOperationRef {
    pub surface: SurfaceId,
    pub ordinal: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCollectionReadShape {
    RootPage,
    IndexPage,
    IndexRangePage,
    UniqueLookup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceCollectionPageRequest<'a> {
    pub exact_keys: &'a [SavedKey],
    pub range: Option<&'a SurfaceIndexRangeRequest>,
    pub limit: usize,
    pub cursor: Option<&'a SurfacePageCursor>,
}

#[derive(Debug, Clone)]
pub struct SurfaceIndexRangeRequest {
    pub lower: Option<SavedKey>,
    pub lower_inclusive: bool,
    pub upper: Option<SavedKey>,
    pub upper_inclusive: bool,
}

impl PartialEq for SurfaceIndexRangeRequest {
    fn eq(&self, other: &Self) -> bool {
        self.lower == other.lower
            && self.upper == other.upper
            && (self.lower.is_none() || self.lower_inclusive == other.lower_inclusive)
            && (self.upper.is_none() || self.upper_inclusive == other.upper_inclusive)
    }
}

impl Eq for SurfaceIndexRangeRequest {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceCollectionPage {
    pub rows: Vec<SurfaceReadRecord>,
    pub next: Option<SurfacePageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfacePageCursor {
    pub operation_tag: String,
    pub store_uid: StoreUid,
    pub commit_id: u64,
    pub catalog_digest: String,
    pub source_digest: String,
    pub engine_profile_digest: EngineProfileDigest,
    pub boundary: SurfacePageBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfacePageBoundary {
    RootIdentity(Vec<SavedKey>),
    IndexIdentity {
        exact_keys: Vec<SavedKey>,
        identity: Vec<SavedKey>,
    },
    IndexRange {
        exact_keys: Vec<SavedKey>,
        range: SurfaceIndexRangeRequest,
        index_keys: Vec<SavedKey>,
        identity: Vec<SavedKey>,
    },
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
    /// A display label: the member's path below the enum owner (`internal::admin`), bare
    /// for a flat enum, so duplicate leaf names stay distinguishable in rendered envelopes.
    /// Compatibility never depends on it.
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
pub struct SurfaceIdentityInputShape {
    pub store_catalog_id: CatalogId,
    pub keys: Vec<SurfaceInputKeyShape>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceInputKeyShape {
    Scalar(ScalarType),
    Enum {
        enum_catalog_id: CatalogId,
        member_catalog_ids: Vec<CatalogId>,
    },
    Identity(SurfaceIdentityInputShape),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceCursorBoundaryInputShape {
    RootIdentity {
        identity: SurfaceIdentityInputShape,
    },
    IndexIdentity {
        exact_keys: Vec<SurfaceInputKeyShape>,
        identity: SurfaceIdentityInputShape,
    },
    IndexRange {
        exact_keys: Vec<SurfaceInputKeyShape>,
        range_key: SurfaceInputKeyShape,
        index_keys: Vec<SurfaceInputKeyShape>,
        identity: SurfaceIdentityInputShape,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceUpdateField {
    pub catalog_id: CatalogId,
    pub value: SurfaceValue,
}

pub struct SurfaceNodeRead<'a> {
    store: &'a TreeStore,
    plan: SurfaceNodeReadPlan<'a>,
}

pub struct SurfaceCollectionRead<'a> {
    store: &'a TreeStore,
    lineage: SurfaceStoreLineage,
    plan: SurfaceCollectionReadPlan<'a>,
}

pub struct SurfaceUpdate<'a> {
    program: &'a CheckedProgram,
    store: &'a TreeStore,
    plan: SurfaceUpdatePlan<'a>,
}

pub struct SurfaceReadOperation<'a> {
    kind: AdmittedSurfaceReadOperationKind<'a>,
}

enum AdmittedSurfaceReadOperationKind<'a> {
    Singleton(SurfaceNodeRead<'a>),
    Point(SurfaceNodeRead<'a>),
    Page(SurfaceCollectionRead<'a>),
    Unique(SurfaceCollectionRead<'a>),
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

    pub fn admit_by_operation_tag(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        operation_tag: &str,
    ) -> Result<Self, SurfaceReadError> {
        let matched = checked_read_operation_by_tag(program, operation_tag)?;
        admit_surface_store(program, store)?;
        Ok(Self {
            store,
            plan: SurfaceNodeReadPlan::prepare_operation(
                program,
                matched.surface,
                matched.operation,
            )?,
        })
    }

    pub fn surface(&self) -> SurfaceId {
        self.plan.surface
    }

    pub fn shape(&self) -> SurfaceNodeReadShape {
        self.plan.shape
    }

    pub fn point_identity_shape(&self) -> Result<SurfaceIdentityInputShape, SurfaceReadError> {
        if self.plan.shape != SurfaceNodeReadShape::Point {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyed store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.plan.record.identity_input_shape()
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
        self.plan.record.validate_identity(identity)?;
        self.read_identity(
            identity,
            Some(SurfaceReadIdentity {
                store_catalog_id: self.plan.record.store_catalog_id.clone(),
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
        let _snapshot = self
            .store
            .read_snapshot()
            .map_err(|error| surface_store_error(error, self.plan.span))?;
        let mut budget = SurfaceMaterializationBudget::new();
        self.plan.record.materialize(
            self.store,
            identity,
            output_identity,
            MissingRecord::Absent,
            &mut budget,
        )
    }
}

impl<'a> SurfaceCollectionRead<'a> {
    pub fn admit(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        operation_ref: SurfaceReadOperationRef,
    ) -> Result<Self, SurfaceReadError> {
        let lineage = admit_surface_store(program, store)?;
        Ok(Self {
            store,
            lineage,
            plan: SurfaceCollectionReadPlan::prepare(program, operation_ref)?,
        })
    }

    pub fn admit_by_operation_tag(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        operation_tag: &str,
    ) -> Result<Self, SurfaceReadError> {
        let matched = checked_read_operation_by_tag(program, operation_tag)?;
        let lineage = admit_surface_store(program, store)?;
        Ok(Self {
            store,
            lineage,
            plan: SurfaceCollectionReadPlan::prepare_operation(
                program,
                matched.operation_ref,
                matched.surface,
                matched.operation,
            )?,
        })
    }

    pub fn operation_ref(&self) -> SurfaceReadOperationRef {
        self.plan.operation_ref
    }

    pub fn shape(&self) -> SurfaceCollectionReadShape {
        self.plan.shape()
    }

    pub fn page_exact_key_shapes(&self) -> Result<Vec<SurfaceInputKeyShape>, SurfaceReadError> {
        self.plan.page_exact_key_shapes()
    }

    pub fn page_range_key_shape(&self) -> Result<Option<SurfaceInputKeyShape>, SurfaceReadError> {
        self.plan.page_range_key_shape()
    }

    pub fn unique_lookup_key_shapes(&self) -> Result<Vec<SurfaceInputKeyShape>, SurfaceReadError> {
        self.plan.unique_lookup_key_shapes()
    }

    pub fn cursor_boundary_shape(
        &self,
    ) -> Result<SurfaceCursorBoundaryInputShape, SurfaceReadError> {
        self.plan.cursor_boundary_shape()
    }

    pub fn page(
        &self,
        request: SurfaceCollectionPageRequest<'_>,
    ) -> Result<SurfaceCollectionPage, SurfaceReadError> {
        self.plan.validate_page_request(request)?;
        // Holding the snapshot guard pins all cursor and materialization reads
        // below to one coherent store view.
        let _snapshot = self
            .store
            .read_snapshot()
            .map_err(|error| surface_store_error(error, self.plan.span))?;
        let lineage = read_current_cursor_lineage(self.store, &self.lineage, self.plan.span)?;
        self.validate_cursor_lineage(request.cursor, &lineage)?;
        let anchors = match &self.plan.kind {
            SurfaceCollectionPlanKind::Root => {
                let cursor = self.plan.root_cursor_boundary(request.cursor)?;
                self.plan
                    .root_identities_after(
                        self.store,
                        cursor.as_deref(),
                        request.limit.saturating_add(1),
                    )?
                    .into_iter()
                    .map(SurfacePageRowAnchor::identity)
                    .collect::<Vec<_>>()
            }
            SurfaceCollectionPlanKind::Index(index) => {
                let cursor = self.plan.index_cursor_boundary(request.cursor)?;
                index
                    .identities_after(
                        self.store,
                        &self.plan.record,
                        request.exact_keys,
                        cursor.as_ref(),
                        request.limit.saturating_add(1),
                    )?
                    .into_iter()
                    .map(SurfacePageRowAnchor::identity)
                    .collect::<Vec<_>>()
            }
            SurfaceCollectionPlanKind::Range(range) => {
                let cursor = self.plan.range_cursor_boundary(request.cursor)?;
                range.anchors_after(
                    self.store,
                    &self.plan.record,
                    request.exact_keys,
                    request.range.expect("range page request validated"),
                    cursor.as_ref(),
                    request.limit.saturating_add(1),
                )?
            }
            SurfaceCollectionPlanKind::Unique(_) => {
                return Err(request_at(
                    "unique surface collection operations are lookups, not pages",
                    self.plan.span,
                ));
            }
        };

        let has_more = anchors.len() > request.limit;
        let page_anchors = anchors
            .iter()
            .take(request.limit)
            .cloned()
            .collect::<Vec<_>>();
        let mut rows = Vec::with_capacity(page_anchors.len());
        let mut budget = SurfaceMaterializationBudget::new();
        for anchor in &page_anchors {
            rows.push(
                self.plan
                    .materialize_row(self.store, &anchor.identity, &mut budget)?,
            );
        }
        let next = if has_more {
            page_anchors
                .last()
                .map(|anchor| {
                    self.plan
                        .page_cursor(&lineage, request.exact_keys, request.range, anchor)
                })
                .transpose()?
        } else {
            None
        };
        Ok(SurfaceCollectionPage { rows, next })
    }

    pub fn lookup_unique(
        &self,
        keys: &[SavedKey],
    ) -> Result<Option<SurfaceReadRecord>, SurfaceReadError> {
        let SurfaceCollectionPlanKind::Unique(unique) = &self.plan.kind else {
            return Err(request_at(
                "surface collection operation is not a unique lookup",
                self.plan.span,
            ));
        };
        unique.validate_request_keys(&self.plan.record, keys, self.plan.span)?;
        // Holding the snapshot guard pins the index lookup and row materialization
        // below to one coherent store view.
        let _snapshot = self
            .store
            .read_snapshot()
            .map_err(|error| surface_store_error(error, self.plan.span))?;
        let Some(identity) = unique.lookup_identity(
            self.store,
            keys,
            self.plan.record.identity_keys.len(),
            self.plan.span,
        )?
        else {
            return Ok(None);
        };
        let mut budget = SurfaceMaterializationBudget::new();
        self.plan
            .materialize_row(self.store, &identity, &mut budget)
            .map(Some)
    }

    fn validate_cursor_lineage(
        &self,
        cursor: Option<&SurfacePageCursor>,
        lineage: &SurfaceStoreLineage,
    ) -> Result<(), SurfaceReadError> {
        let Some(cursor) = cursor else {
            return Ok(());
        };
        if cursor.store_uid != lineage.store_uid
            || cursor.commit_id != lineage.commit_id
            || cursor.catalog_digest != lineage.catalog_digest
            || cursor.source_digest != lineage.source_digest
            || cursor.engine_profile_digest != lineage.engine_profile_digest
        {
            return Err(stale_cursor(
                "surface cursor store lineage no longer matches",
                self.plan.span,
            ));
        }
        Ok(())
    }
}

impl<'a> SurfaceReadOperation<'a> {
    pub fn admit_by_operation_tag(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        operation_tag: &str,
    ) -> Result<Self, SurfaceReadError> {
        let matched = checked_read_operation_by_tag(program, operation_tag)?;
        if let Some(shape) = node_read_shape(matched.surface, matched.operation) {
            admit_surface_store(program, store)?;
            let read = SurfaceNodeRead {
                store,
                plan: SurfaceNodeReadPlan::prepare_operation(
                    program,
                    matched.surface,
                    matched.operation,
                )?,
            };
            let kind = match shape {
                SurfaceNodeReadShape::Singleton => {
                    AdmittedSurfaceReadOperationKind::Singleton(read)
                }
                SurfaceNodeReadShape::Point => AdmittedSurfaceReadOperationKind::Point(read),
            };
            return Ok(Self { kind });
        }

        let lineage = admit_surface_store(program, store)?;
        let plan = SurfaceCollectionReadPlan::prepare_operation(
            program,
            matched.operation_ref,
            matched.surface,
            matched.operation,
        )?;
        let shape = plan.shape();
        let read = SurfaceCollectionRead {
            store,
            lineage,
            plan,
        };
        let kind = match shape {
            SurfaceCollectionReadShape::RootPage
            | SurfaceCollectionReadShape::IndexPage
            | SurfaceCollectionReadShape::IndexRangePage => {
                AdmittedSurfaceReadOperationKind::Page(read)
            }
            SurfaceCollectionReadShape::UniqueLookup => {
                AdmittedSurfaceReadOperationKind::Unique(read)
            }
        };
        Ok(Self { kind })
    }

    pub fn singleton_read(&self) -> Result<&SurfaceNodeRead<'a>, SurfaceReadError> {
        match &self.kind {
            AdmittedSurfaceReadOperationKind::Singleton(read) => Ok(read),
            AdmittedSurfaceReadOperationKind::Point(_)
            | AdmittedSurfaceReadOperationKind::Page(_)
            | AdmittedSurfaceReadOperationKind::Unique(_) => Err(SurfaceError::request(
                "surface operation tag does not admit singleton read requests",
            )),
        }
    }

    pub fn point_read(&self) -> Result<&SurfaceNodeRead<'a>, SurfaceReadError> {
        match &self.kind {
            AdmittedSurfaceReadOperationKind::Point(read) => Ok(read),
            AdmittedSurfaceReadOperationKind::Singleton(_)
            | AdmittedSurfaceReadOperationKind::Page(_)
            | AdmittedSurfaceReadOperationKind::Unique(_) => Err(SurfaceError::request(
                "surface operation tag does not admit point read requests",
            )),
        }
    }

    pub fn page_read(&self) -> Result<&SurfaceCollectionRead<'a>, SurfaceReadError> {
        match &self.kind {
            AdmittedSurfaceReadOperationKind::Page(read) => Ok(read),
            AdmittedSurfaceReadOperationKind::Singleton(_)
            | AdmittedSurfaceReadOperationKind::Point(_)
            | AdmittedSurfaceReadOperationKind::Unique(_) => Err(SurfaceError::request(
                "surface operation tag does not admit page requests",
            )),
        }
    }

    pub fn unique_lookup(&self) -> Result<&SurfaceCollectionRead<'a>, SurfaceReadError> {
        match &self.kind {
            AdmittedSurfaceReadOperationKind::Unique(read) => Ok(read),
            AdmittedSurfaceReadOperationKind::Singleton(_)
            | AdmittedSurfaceReadOperationKind::Point(_)
            | AdmittedSurfaceReadOperationKind::Page(_) => Err(SurfaceError::request(
                "surface operation tag does not admit unique lookup requests",
            )),
        }
    }
}

impl<'a> SurfaceUpdate<'a> {
    pub fn admit(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        surface: SurfaceId,
    ) -> Result<Self, SurfaceError> {
        admit_surface_store(program, store)?;
        Ok(Self {
            program,
            store,
            plan: SurfaceUpdatePlan::prepare(program, surface)?,
        })
    }

    pub fn admit_by_operation_tag(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        operation_tag: &str,
    ) -> Result<Self, SurfaceError> {
        let surface = checked_update_surface_by_tag(program, operation_tag)?;
        admit_surface_store(program, store)?;
        Ok(Self {
            program,
            store,
            plan: SurfaceUpdatePlan::prepare(program, surface.id)?,
        })
    }

    pub fn surface(&self) -> SurfaceId {
        self.plan.surface
    }

    pub fn shape(&self) -> SurfaceNodeReadShape {
        self.plan.shape
    }

    pub fn point_identity_shape(&self) -> Result<SurfaceIdentityInputShape, SurfaceError> {
        if self.plan.shape != SurfaceNodeReadShape::Point {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyed store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.plan.record.identity_input_shape()
    }

    pub fn execute(&self, input: SurfaceUpdateInput<'_>) -> Result<(), SurfaceError> {
        match (self.plan.shape, input) {
            (SurfaceNodeReadShape::Singleton, SurfaceUpdateInput::Singleton { fields }) => {
                self.update_identity(&[], fields)
            }
            (SurfaceNodeReadShape::Point, SurfaceUpdateInput::Point { identity, fields }) => {
                self.update_point(identity, fields)
            }
            (SurfaceNodeReadShape::Singleton, SurfaceUpdateInput::Point { .. }) => Err(request_at(
                format!(
                    "surface `{}` is a singleton update",
                    self.plan.surface_label
                ),
                self.plan.span,
            )),
            (SurfaceNodeReadShape::Point, SurfaceUpdateInput::Singleton { .. }) => Err(request_at(
                format!("surface `{}` requires an identity", self.plan.surface_label),
                self.plan.span,
            )),
        }
    }

    pub fn update_point(
        &self,
        identity: &[SavedKey],
        fields: &[SurfaceUpdateField],
    ) -> Result<(), SurfaceError> {
        if self.plan.shape != SurfaceNodeReadShape::Point {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyed store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.plan.record.validate_identity(identity)?;
        self.update_identity(identity, fields)
    }

    pub fn update_singleton(&self, fields: &[SurfaceUpdateField]) -> Result<(), SurfaceError> {
        if self.plan.shape != SurfaceNodeReadShape::Singleton {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyless singleton store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.update_identity(&[], fields)
    }

    fn update_identity(
        &self,
        identity: &[SavedKey],
        fields: &[SurfaceUpdateField],
    ) -> Result<(), SurfaceError> {
        self.plan
            .commit_update(self.program, self.store, identity, fields)
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
    surface: SurfaceId,
    surface_label: String,
    shape: SurfaceNodeReadShape,
    record: SurfaceRecordReadPlan<'a>,
    span: SourceSpan,
}

struct SurfaceCollectionReadPlan<'a> {
    operation_ref: SurfaceReadOperationRef,
    operation_tag: String,
    kind: SurfaceCollectionPlanKind,
    record: SurfaceRecordReadPlan<'a>,
    span: SourceSpan,
}

struct SurfaceUpdatePlan<'a> {
    surface: SurfaceId,
    surface_label: String,
    shape: SurfaceNodeReadShape,
    accepted_epoch: Option<u64>,
    source_digest: String,
    place: marrow_check::CheckedSavedPlace,
    record: SurfaceRecordReadPlan<'a>,
    fields: HashMap<CatalogId, SurfaceWriteMember>,
    span: SourceSpan,
}

struct SurfaceRecordReadPlan<'a> {
    facts: &'a CheckedFacts,
    store_catalog_id: CatalogId,
    identity_keys: Vec<StoredValueMeaning>,
    reads: Vec<SurfaceMemberRead>,
    projection: Vec<ResourceMemberId>,
    span: SourceSpan,
}

enum SurfaceCollectionPlanKind {
    Root,
    Index(SurfaceIndexPagePlan),
    Range(SurfaceIndexRangePagePlan),
    Unique(SurfaceUniqueLookupPlan),
}

struct SurfaceIndexPagePlan {
    index_catalog_id: CatalogId,
    key_meanings: Vec<StoredValueMeaning>,
    exact_key_count: usize,
    identity_key_count: usize,
}

struct SurfaceIndexRangePagePlan {
    index_catalog_id: CatalogId,
    key_meanings: Vec<StoredValueMeaning>,
    exact_key_count: usize,
    range_key_index: usize,
    identity_key_count: usize,
}

struct SurfaceUniqueLookupPlan {
    index_catalog_id: CatalogId,
    key_meanings: Vec<StoredValueMeaning>,
    key_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SurfaceWriteMember {
    member: ResourceMemberId,
    value_meaning: StoredValueMeaning,
}

enum PlannedSurfaceWriteValue {
    Leaf(LeafValue),
    Identity {
        keys: Vec<SavedKey>,
        referenced_arity: usize,
    },
}

#[derive(Clone, PartialEq, Eq)]
struct SurfaceStoreLineage {
    store_uid: StoreUid,
    commit_id: u64,
    catalog_digest: String,
    source_digest: String,
    engine_profile_digest: EngineProfileDigest,
}

struct SurfaceReadOperationMatch<'a> {
    surface: &'a SurfaceFact,
    operation: &'a SurfaceReadOperationFact,
    operation_ref: SurfaceReadOperationRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SurfacePageRowAnchor {
    identity: Vec<SavedKey>,
    index_keys: Option<Vec<SavedKey>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SurfaceIndexRangeAnchor {
    index_keys: Vec<SavedKey>,
    identity: Vec<SavedKey>,
}

struct SurfaceIndexRangeEntry<'a> {
    exact_keys: &'a [SavedKey],
    range: &'a SurfaceIndexRangeRequest,
    index_keys: &'a [SavedKey],
    identity: &'a [SavedKey],
}

impl SurfacePageRowAnchor {
    fn identity(identity: Vec<SavedKey>) -> Self {
        Self {
            identity,
            index_keys: None,
        }
    }

    fn index_range(index_keys: Vec<SavedKey>, identity: Vec<SavedKey>) -> Self {
        Self {
            identity,
            index_keys: Some(index_keys),
        }
    }
}

#[derive(Clone, Copy)]
enum MissingRecord {
    Absent,
    InvalidData,
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

#[derive(Debug, Clone, Copy)]
struct SurfaceMaterializationBudget {
    remaining_bytes: usize,
}

impl SurfaceMaterializationBudget {
    fn new() -> Self {
        Self {
            remaining_bytes: SURFACE_MAX_MATERIALIZED_BYTES,
        }
    }

    fn take(&mut self, bytes: usize, span: SourceSpan) -> Result<(), SurfaceReadError> {
        if bytes > self.remaining_bytes {
            return Err(limit_error(
                "surface read materialization byte budget exceeded",
                span,
            ));
        }
        self.remaining_bytes -= bytes;
        Ok(())
    }
}

pub(super) struct SurfaceWriteCommit<'a> {
    program: &'a CheckedProgram,
    store: &'a TreeStore,
    span: SourceSpan,
    accepted_epoch: Option<u64>,
    source_digest: &'a str,
}

impl<'a> SurfaceWriteCommit<'a> {
    pub(super) fn new(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        span: SourceSpan,
        accepted_epoch: Option<u64>,
        source_digest: &'a str,
    ) -> Self {
        Self {
            program,
            store,
            span,
            accepted_epoch,
            source_digest,
        }
    }

    pub(super) fn run<R>(
        self,
        before_plan: impl FnOnce(&TreeStore) -> Result<(), SurfaceError>,
        build_plan: impl FnOnce(&TreeStore) -> Result<WritePlan, SurfaceError>,
        after_plan: impl FnOnce(&TreeStore) -> Result<R, SurfaceError>,
    ) -> Result<R, SurfaceError> {
        self.store
            .begin()
            .map_err(|error| surface_store_error(error, self.span))?;
        if let Err(error) = admit_surface_store(self.program, self.store) {
            let _ = self.store.rollback();
            return Err(error);
        }
        if let Err(error) = before_plan(self.store) {
            let _ = self.store.rollback();
            return Err(error);
        }
        let mut plan = match build_plan(self.store) {
            Ok(plan) => plan,
            Err(error) => {
                let _ = self.store.rollback();
                return Err(error);
            }
        };
        if let Err(error) =
            crate::env::stamp_managed_write(&mut plan, self.accepted_epoch, self.source_digest)
        {
            let _ = self.store.rollback();
            return Err(surface_store_error(error, self.span));
        }
        if let Err(error) = plan
            .commit(self.store, true)
            .map_err(|error| surface_store_error(error, self.span))
        {
            let _ = self.store.rollback();
            return Err(error);
        }
        let result = match after_plan(self.store) {
            Ok(result) => result,
            Err(error) => {
                let _ = self.store.rollback();
                return Err(error);
            }
        };
        if let Err(error) = self
            .store
            .commit()
            .map_err(|error| surface_store_error(error, self.span))
        {
            let _ = self.store.rollback();
            return Err(error);
        }
        Ok(result)
    }
}

impl<'a> SurfaceNodeReadPlan<'a> {
    fn prepare(program: &'a CheckedProgram, surface: SurfaceId) -> Result<Self, SurfaceReadError> {
        let surface = checked_surface(program, surface)?;
        let (operation, shape) = backing_node_operation(surface)?;
        Self::from_operation(program, surface, operation, shape)
    }

    fn prepare_operation(
        program: &'a CheckedProgram,
        surface: &'a SurfaceFact,
        operation: &'a SurfaceReadOperationFact,
    ) -> Result<Self, SurfaceReadError> {
        let shape = node_read_shape(surface, operation).ok_or_else(|| {
            abi_mismatch(
                "surface operation tag is not a node read operation",
                operation.span,
            )
        })?;
        Self::from_operation(program, surface, operation, shape)
    }

    fn from_operation(
        program: &'a CheckedProgram,
        surface: &'a SurfaceFact,
        operation: &'a SurfaceReadOperationFact,
        shape: SurfaceNodeReadShape,
    ) -> Result<Self, SurfaceReadError> {
        require_stable_surface(surface)?;
        let store = program.facts.store(surface.store);
        let record =
            SurfaceRecordReadPlan::prepare(&program.facts, store, operation, surface.span)?;
        Ok(Self {
            surface: surface.id,
            surface_label: surface.name.clone(),
            shape,
            record,
            span: operation.span,
        })
    }
}

impl<'a> SurfaceCollectionReadPlan<'a> {
    fn prepare(
        program: &'a CheckedProgram,
        operation_ref: SurfaceReadOperationRef,
    ) -> Result<Self, SurfaceReadError> {
        let (surface, operation) = checked_surface_operation(program, operation_ref)?;
        Self::prepare_operation(program, operation_ref, surface, operation)
    }

    fn prepare_operation(
        program: &'a CheckedProgram,
        operation_ref: SurfaceReadOperationRef,
        surface: &'a SurfaceFact,
        operation: &'a SurfaceReadOperationFact,
    ) -> Result<Self, SurfaceReadError> {
        require_stable_surface(surface)?;
        let store = program.facts.store(surface.store);
        let record =
            SurfaceRecordReadPlan::prepare(&program.facts, store, operation, surface.span)?;
        let kind = collection_plan_kind(&program.facts, surface, operation, surface.span)?;
        let operation_tag = operation.operation_tag.clone().ok_or_else(|| {
            abi_mismatch(
                "checked surface read operation has no stable operation tag",
                surface.span,
            )
        })?;
        Ok(Self {
            operation_ref,
            operation_tag,
            kind,
            record,
            span: operation.span,
        })
    }

    fn shape(&self) -> SurfaceCollectionReadShape {
        match self.kind {
            SurfaceCollectionPlanKind::Root => SurfaceCollectionReadShape::RootPage,
            SurfaceCollectionPlanKind::Index(_) => SurfaceCollectionReadShape::IndexPage,
            SurfaceCollectionPlanKind::Range(_) => SurfaceCollectionReadShape::IndexRangePage,
            SurfaceCollectionPlanKind::Unique(_) => SurfaceCollectionReadShape::UniqueLookup,
        }
    }

    fn validate_page_request(
        &self,
        request: SurfaceCollectionPageRequest<'_>,
    ) -> Result<(), SurfaceReadError> {
        validate_page_limit(request.limit, self.span)?;
        self.validate_cursor(request.cursor)?;
        match &self.kind {
            SurfaceCollectionPlanKind::Root => {
                if request.range.is_some() {
                    return Err(request_at(
                        "root collection pages do not take an index range",
                        self.span,
                    ));
                }
                if !request.exact_keys.is_empty() {
                    return Err(request_at(
                        "root collection pages do not take exact index keys",
                        self.span,
                    ));
                }
                Ok(())
            }
            SurfaceCollectionPlanKind::Index(index) => {
                if request.range.is_some() {
                    return Err(request_at(
                        "exact index collection pages do not take an index range",
                        self.span,
                    ));
                }
                index.validate_request_exact_keys(&self.record, request.exact_keys, self.span)?;
                if let Some(cursor) = request.cursor {
                    let SurfacePageBoundary::IndexIdentity { exact_keys, .. } = &cursor.boundary
                    else {
                        return Err(cursor_error(
                            "surface cursor boundary does not match the index collection",
                            self.span,
                        ));
                    };
                    if exact_keys.as_slice() != request.exact_keys {
                        return Err(cursor_error(
                            "surface cursor exact arguments do not match the request",
                            self.span,
                        ));
                    }
                }
                Ok(())
            }
            SurfaceCollectionPlanKind::Range(range) => {
                let Some(request_range) = request.range else {
                    return Err(request_at(
                        "range index collection pages require an index range",
                        self.span,
                    ));
                };
                range.validate_request(
                    &self.record,
                    request.exact_keys,
                    request_range,
                    self.span,
                )?;
                if let Some(cursor) = request.cursor {
                    let SurfacePageBoundary::IndexRange {
                        exact_keys, range, ..
                    } = &cursor.boundary
                    else {
                        return Err(cursor_error(
                            "surface cursor boundary does not match the range index collection",
                            self.span,
                        ));
                    };
                    if exact_keys.as_slice() != request.exact_keys || range != request_range {
                        return Err(cursor_error(
                            "surface cursor range arguments do not match the request",
                            self.span,
                        ));
                    }
                }
                Ok(())
            }
            SurfaceCollectionPlanKind::Unique(_) => Err(request_at(
                "unique surface collection operations are lookups, not pages",
                self.span,
            )),
        }
    }

    fn page_exact_key_shapes(&self) -> Result<Vec<SurfaceInputKeyShape>, SurfaceReadError> {
        match &self.kind {
            SurfaceCollectionPlanKind::Root => Ok(Vec::new()),
            SurfaceCollectionPlanKind::Index(index) => input_key_shapes(
                self.record.facts,
                &index.key_meanings[..index.exact_key_count],
                self.span,
            ),
            SurfaceCollectionPlanKind::Range(range) => input_key_shapes(
                self.record.facts,
                &range.key_meanings[..range.exact_key_count],
                self.span,
            ),
            SurfaceCollectionPlanKind::Unique(_) => Err(request_at(
                "unique surface collection operations are lookups, not pages",
                self.span,
            )),
        }
    }

    fn page_range_key_shape(&self) -> Result<Option<SurfaceInputKeyShape>, SurfaceReadError> {
        match &self.kind {
            SurfaceCollectionPlanKind::Root | SurfaceCollectionPlanKind::Index(_) => Ok(None),
            SurfaceCollectionPlanKind::Range(range) => Ok(Some(input_key_shape(
                self.record.facts,
                &range.key_meanings[range.range_key_index],
                self.span,
            )?)),
            SurfaceCollectionPlanKind::Unique(_) => Err(request_at(
                "unique surface collection operations are lookups, not pages",
                self.span,
            )),
        }
    }

    fn unique_lookup_key_shapes(&self) -> Result<Vec<SurfaceInputKeyShape>, SurfaceReadError> {
        match &self.kind {
            SurfaceCollectionPlanKind::Unique(unique) => {
                input_key_shapes(self.record.facts, &unique.key_meanings, self.span)
            }
            _ => Err(request_at(
                "surface collection operation is not a unique lookup",
                self.span,
            )),
        }
    }

    fn cursor_boundary_shape(&self) -> Result<SurfaceCursorBoundaryInputShape, SurfaceReadError> {
        match &self.kind {
            SurfaceCollectionPlanKind::Root => Ok(SurfaceCursorBoundaryInputShape::RootIdentity {
                identity: self.record.identity_input_shape()?,
            }),
            SurfaceCollectionPlanKind::Index(index) => {
                Ok(SurfaceCursorBoundaryInputShape::IndexIdentity {
                    exact_keys: input_key_shapes(
                        self.record.facts,
                        &index.key_meanings[..index.exact_key_count],
                        self.span,
                    )?,
                    identity: self.record.identity_input_shape()?,
                })
            }
            SurfaceCollectionPlanKind::Range(range) => {
                Ok(SurfaceCursorBoundaryInputShape::IndexRange {
                    exact_keys: input_key_shapes(
                        self.record.facts,
                        &range.key_meanings[..range.exact_key_count],
                        self.span,
                    )?,
                    range_key: input_key_shape(
                        self.record.facts,
                        &range.key_meanings[range.range_key_index],
                        self.span,
                    )?,
                    index_keys: input_key_shapes(
                        self.record.facts,
                        &range.key_meanings,
                        self.span,
                    )?,
                    identity: self.record.identity_input_shape()?,
                })
            }
            SurfaceCollectionPlanKind::Unique(_) => Err(request_at(
                "unique lookup operations do not produce page cursors",
                self.span,
            )),
        }
    }

    fn validate_cursor(&self, cursor: Option<&SurfacePageCursor>) -> Result<(), SurfaceReadError> {
        let Some(cursor) = cursor else {
            return Ok(());
        };
        if cursor.operation_tag != self.operation_tag {
            return Err(stale_cursor(
                "surface cursor targets a different operation",
                self.span,
            ));
        }
        match (&self.kind, &cursor.boundary) {
            (SurfaceCollectionPlanKind::Root, SurfacePageBoundary::RootIdentity(identity)) => {
                self.record.validate_identity_cursor(identity)
            }
            (
                SurfaceCollectionPlanKind::Index(index),
                SurfacePageBoundary::IndexIdentity {
                    exact_keys,
                    identity,
                },
            ) => index.validate_cursor_boundary(&self.record, exact_keys, identity, self.span),
            (
                SurfaceCollectionPlanKind::Range(range),
                SurfacePageBoundary::IndexRange {
                    exact_keys,
                    range: request_range,
                    index_keys,
                    identity,
                },
            ) => range.validate_cursor_boundary(
                &self.record,
                exact_keys,
                request_range,
                index_keys,
                identity,
                self.span,
            ),
            _ => Err(cursor_error(
                "surface cursor boundary does not match the collection shape",
                self.span,
            )),
        }
    }

    fn root_cursor_boundary(
        &self,
        cursor: Option<&SurfacePageCursor>,
    ) -> Result<Option<Vec<SavedKey>>, SurfaceReadError> {
        let Some(cursor) = cursor else {
            return Ok(None);
        };
        let SurfacePageBoundary::RootIdentity(identity) = &cursor.boundary else {
            return Err(cursor_error(
                "surface cursor boundary does not match the root collection",
                self.span,
            ));
        };
        Ok(Some(identity.clone()))
    }

    fn index_cursor_boundary(
        &self,
        cursor: Option<&SurfacePageCursor>,
    ) -> Result<Option<Vec<SavedKey>>, SurfaceReadError> {
        let Some(cursor) = cursor else {
            return Ok(None);
        };
        let SurfacePageBoundary::IndexIdentity { identity, .. } = &cursor.boundary else {
            return Err(cursor_error(
                "surface cursor boundary does not match the index collection",
                self.span,
            ));
        };
        Ok(Some(identity.clone()))
    }

    fn range_cursor_boundary(
        &self,
        cursor: Option<&SurfacePageCursor>,
    ) -> Result<Option<SurfaceIndexRangeAnchor>, SurfaceReadError> {
        let Some(cursor) = cursor else {
            return Ok(None);
        };
        let SurfacePageBoundary::IndexRange {
            index_keys,
            identity,
            ..
        } = &cursor.boundary
        else {
            return Err(cursor_error(
                "surface cursor boundary does not match the range index collection",
                self.span,
            ));
        };
        Ok(Some(SurfaceIndexRangeAnchor {
            index_keys: index_keys.clone(),
            identity: identity.clone(),
        }))
    }

    fn root_identities_after(
        &self,
        store: &TreeStore,
        after: Option<&[SavedKey]>,
        limit: usize,
    ) -> Result<Vec<Vec<SavedKey>>, SurfaceReadError> {
        let mut identities = Vec::new();
        let arity = self.record.identity_keys.len();
        let mut context = store;
        let mut first = |store: &mut &TreeStore, prefix: &[SavedKey]| {
            store
                .record_first_child_at_arity(&self.record.store_catalog_id, prefix, arity)
                .map_err(|error| surface_scan_error(error, self.span))
        };
        let mut next = |store: &mut &TreeStore, prefix: &[SavedKey], anchor: &SavedKey| {
            store
                .record_next_child_at_arity(&self.record.store_catalog_id, prefix, arity, anchor)
                .map_err(|error| surface_scan_error(error, self.span))
        };
        let mut visit = |identity: Vec<SavedKey>,
                         _store: &mut &TreeStore|
         -> Result<ControlFlow<()>, SurfaceReadError> {
            self.record.validate_identity_data(&identity)?;
            identities.push(identity);
            if identities.len() >= limit {
                Ok(ControlFlow::Break(()))
            } else {
                Ok(ControlFlow::Continue(()))
            }
        };
        let _ = walk_keyed_children_after(
            &mut context,
            KeyedChildrenWalk {
                depth: arity,
                query_prefix: &[],
                identity_prefix: &[],
                after_identity: after,
            },
            &mut first,
            &mut next,
            &mut visit,
        )?;
        Ok(identities)
    }

    fn materialize_row(
        &self,
        store: &TreeStore,
        identity: &[SavedKey],
        budget: &mut SurfaceMaterializationBudget,
    ) -> Result<SurfaceReadRecord, SurfaceReadError> {
        self.record.validate_identity_data(identity)?;
        self.record.materialize(
            store,
            identity,
            Some(self.record.surface_identity(identity)),
            MissingRecord::InvalidData,
            budget,
        )
    }

    fn page_cursor(
        &self,
        lineage: &SurfaceStoreLineage,
        exact_keys: &[SavedKey],
        range: Option<&SurfaceIndexRangeRequest>,
        anchor: &SurfacePageRowAnchor,
    ) -> Result<SurfacePageCursor, SurfaceReadError> {
        let boundary = match &self.kind {
            SurfaceCollectionPlanKind::Root => {
                SurfacePageBoundary::RootIdentity(anchor.identity.clone())
            }
            SurfaceCollectionPlanKind::Index(_) => SurfacePageBoundary::IndexIdentity {
                exact_keys: exact_keys.to_vec(),
                identity: anchor.identity.clone(),
            },
            SurfaceCollectionPlanKind::Range(_) => {
                let range = range.ok_or_else(|| {
                    abi_mismatch("range page cursor requires range arguments", self.span)
                })?;
                let index_keys = anchor.index_keys.clone().ok_or_else(|| {
                    abi_mismatch(
                        "range page cursor requires an index tuple anchor",
                        self.span,
                    )
                })?;
                SurfacePageBoundary::IndexRange {
                    exact_keys: exact_keys.to_vec(),
                    range: range.clone(),
                    index_keys,
                    identity: anchor.identity.clone(),
                }
            }
            SurfaceCollectionPlanKind::Unique(_) => {
                return Err(abi_mismatch(
                    "unique lookup operations do not produce page cursors",
                    self.span,
                ));
            }
        };
        Ok(SurfacePageCursor {
            operation_tag: self.operation_tag.clone(),
            store_uid: lineage.store_uid.clone(),
            commit_id: lineage.commit_id,
            catalog_digest: lineage.catalog_digest.clone(),
            source_digest: lineage.source_digest.clone(),
            engine_profile_digest: lineage.engine_profile_digest,
            boundary,
        })
    }
}

impl<'a> SurfaceUpdatePlan<'a> {
    fn prepare(program: &'a CheckedProgram, surface: SurfaceId) -> Result<Self, SurfaceReadError> {
        let surface = checked_surface(program, surface)?;
        require_stable_surface(surface)?;
        let store = program.facts.store(surface.store);
        if surface.update.is_empty() {
            return Err(request_at(
                format!("surface `{}` declares no update fields", surface.name),
                surface.span,
            ));
        }
        let (operation, shape) = backing_node_operation(surface)?;
        let record =
            SurfaceRecordReadPlan::prepare(&program.facts, store, operation, surface.span)?;
        let place =
            checked_saved_root_place(program, &store.root, surface.span).ok_or_else(|| {
                abi_mismatch(
                    format!(
                        "surface `{}` backing store cannot be used for managed writes",
                        surface.name
                    ),
                    surface.span,
                )
            })?;
        let fields = surface_update_members(&program.facts, surface, store)?;
        Ok(Self {
            surface: surface.id,
            surface_label: surface.name.clone(),
            shape,
            accepted_epoch: program.catalog.accepted_epoch,
            source_digest: program.source_digest(),
            place,
            record,
            fields,
            span: surface.span,
        })
    }

    fn update_plan(
        &self,
        identity: &[SavedKey],
        fields: &[SurfaceUpdateField],
        store: &TreeStore,
    ) -> Result<WritePlan, SurfaceReadError> {
        if fields.is_empty() {
            return Err(request_at("surface update patch is empty", self.span));
        }
        let mut seen = HashSet::new();
        let mut patch = Vec::with_capacity(fields.len());
        for field in fields {
            if !seen.insert(field.catalog_id.clone()) {
                return Err(request_at("surface update field is repeated", self.span));
            }
            let member = self.fields.get(&field.catalog_id).ok_or_else(|| {
                request_at(
                    "surface update field is not declared in the update set",
                    self.span,
                )
            })?;
            let value = lower_surface_write_value(
                &field.value,
                &member.value_meaning,
                self.record.facts,
                self.span,
            )?;
            patch.push(match value {
                PlannedSurfaceWriteValue::Leaf(value) => FieldPatchValue::Leaf {
                    member: member.member,
                    value,
                },
                PlannedSurfaceWriteValue::Identity {
                    keys,
                    referenced_arity,
                } => FieldPatchValue::Identity {
                    member: member.member,
                    keys,
                    referenced_arity,
                },
            });
        }
        plan_field_patch_write(
            &self.place,
            identity,
            &patch,
            store,
            self.record.facts,
            self.span,
        )
        .map_err(map_surface_write_plan_error)
    }

    fn commit_update(
        &self,
        program: &CheckedProgram,
        store: &TreeStore,
        identity: &[SavedKey],
        fields: &[SurfaceUpdateField],
    ) -> Result<(), SurfaceReadError> {
        SurfaceWriteCommit::new(
            program,
            store,
            self.span,
            self.accepted_epoch,
            &self.source_digest,
        )
        .run(
            |store| self.require_present(store, identity),
            |store| self.update_plan(identity, fields, store),
            |store| self.validate_after_patch(store, identity),
        )
    }

    fn require_present(
        &self,
        store: &TreeStore,
        identity: &[SavedKey],
    ) -> Result<(), SurfaceError> {
        if store
            .data_subtree_exists(&self.record.store_catalog_id, identity, &[])
            .map_err(|error| surface_store_error(error, self.span))?
        {
            Ok(())
        } else {
            Err(surface_error_at(
                SURFACE_ABSENT,
                "surface record is absent",
                self.span,
            ))
        }
    }

    fn validate_after_patch(
        &self,
        store: &TreeStore,
        identity: &[SavedKey],
    ) -> Result<(), SurfaceError> {
        let mut budget = SurfaceMaterializationBudget::new();
        self.record
            .materialize(
                store,
                identity,
                Some(self.record.surface_identity(identity)),
                MissingRecord::InvalidData,
                &mut budget,
            )
            .map(|_| ())
    }
}

impl SurfaceIndexPagePlan {
    fn validate_request_exact_keys(
        &self,
        record: &SurfaceRecordReadPlan<'_>,
        exact_keys: &[SavedKey],
        span: SourceSpan,
    ) -> Result<(), SurfaceReadError> {
        if exact_keys.len() != self.exact_key_count {
            return Err(request_at(
                format!(
                    "surface index page expects {} exact key(s), got {}",
                    self.exact_key_count,
                    exact_keys.len()
                ),
                span,
            ));
        }
        validate_index_keys(
            record.facts,
            exact_keys,
            &self.key_meanings[..self.exact_key_count],
            span,
            key_request_error,
        )
    }

    fn validate_cursor_boundary(
        &self,
        record: &SurfaceRecordReadPlan<'_>,
        exact_keys: &[SavedKey],
        identity: &[SavedKey],
        span: SourceSpan,
    ) -> Result<(), SurfaceReadError> {
        self.validate_cursor_exact_keys(record, exact_keys, span)?;
        record.validate_identity_cursor(identity)?;
        let mut index_keys = exact_keys.to_vec();
        index_keys.extend_from_slice(identity);
        validate_index_keys(
            record.facts,
            &index_keys,
            &self.key_meanings,
            span,
            key_cursor_error,
        )
    }

    fn validate_cursor_exact_keys(
        &self,
        record: &SurfaceRecordReadPlan<'_>,
        exact_keys: &[SavedKey],
        span: SourceSpan,
    ) -> Result<(), SurfaceReadError> {
        if exact_keys.len() != self.exact_key_count {
            return Err(cursor_error(
                "surface cursor exact key boundary has the wrong arity",
                span,
            ));
        }
        validate_index_keys(
            record.facts,
            exact_keys,
            &self.key_meanings[..self.exact_key_count],
            span,
            key_cursor_error,
        )
    }

    fn identities_after(
        &self,
        store: &TreeStore,
        record: &SurfaceRecordReadPlan<'_>,
        exact_keys: &[SavedKey],
        after: Option<&Vec<SavedKey>>,
        limit: usize,
    ) -> Result<Vec<Vec<SavedKey>>, SurfaceReadError> {
        let mut identities = Vec::new();
        let span = record.span;
        let mut context = store;
        let mut first = |store: &mut &TreeStore, prefix: &[SavedKey]| {
            store
                .index_first_child(&self.index_catalog_id, prefix)
                .map_err(|error| surface_scan_error(error, span))
        };
        let mut next = |store: &mut &TreeStore, prefix: &[SavedKey], anchor: &SavedKey| {
            store
                .index_next_child(&self.index_catalog_id, prefix, anchor)
                .map_err(|error| surface_scan_error(error, span))
        };
        let mut visit = |identity: Vec<SavedKey>,
                         _store: &mut &TreeStore|
         -> Result<ControlFlow<()>, SurfaceReadError> {
            let mut index_keys = exact_keys.to_vec();
            index_keys.extend_from_slice(&identity);
            validate_index_keys(
                record.facts,
                &index_keys,
                &self.key_meanings,
                span,
                key_data_error,
            )?;
            record.validate_identity_data(&identity)?;
            identities.push(identity);
            if identities.len() >= limit {
                Ok(ControlFlow::Break(()))
            } else {
                Ok(ControlFlow::Continue(()))
            }
        };
        let _ = walk_keyed_children_after(
            &mut context,
            KeyedChildrenWalk {
                depth: self.identity_key_count,
                query_prefix: exact_keys,
                identity_prefix: &[],
                after_identity: after.map(Vec::as_slice),
            },
            &mut first,
            &mut next,
            &mut visit,
        )?;
        Ok(identities)
    }
}

impl SurfaceIndexRangePagePlan {
    fn validate_request(
        &self,
        record: &SurfaceRecordReadPlan<'_>,
        exact_keys: &[SavedKey],
        range: &SurfaceIndexRangeRequest,
        span: SourceSpan,
    ) -> Result<(), SurfaceReadError> {
        self.validate_exact_keys(record, exact_keys, span, key_request_error)?;
        self.validate_range_bounds(record, range, span, key_request_error)
    }

    fn validate_cursor_boundary(
        &self,
        record: &SurfaceRecordReadPlan<'_>,
        exact_keys: &[SavedKey],
        range: &SurfaceIndexRangeRequest,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
        span: SourceSpan,
    ) -> Result<(), SurfaceReadError> {
        self.validate_exact_keys(record, exact_keys, span, key_cursor_error)?;
        self.validate_range_bounds(record, range, span, key_cursor_error)?;
        self.validate_entry(
            record,
            SurfaceIndexRangeEntry {
                exact_keys,
                range,
                index_keys,
                identity,
            },
            span,
            key_cursor_error,
        )
    }

    fn anchors_after(
        &self,
        store: &TreeStore,
        record: &SurfaceRecordReadPlan<'_>,
        exact_keys: &[SavedKey],
        range: &SurfaceIndexRangeRequest,
        after: Option<&SurfaceIndexRangeAnchor>,
        limit: usize,
    ) -> Result<Vec<SurfacePageRowAnchor>, SurfaceReadError> {
        let bounds = range_bounds(range);
        let page = match after {
            Some(anchor) => store.scan_index_range_after_entry(
                &self.index_catalog_id,
                exact_keys,
                &bounds,
                &anchor.index_keys,
                &anchor.identity,
                limit,
            ),
            None => store.scan_index_range(&self.index_catalog_id, exact_keys, &bounds, limit),
        }
        .map_err(|error| surface_scan_error(error, record.span))?;
        let mut anchors = Vec::with_capacity(page.entries.len());
        for entry in page.entries {
            self.validate_entry(
                record,
                SurfaceIndexRangeEntry {
                    exact_keys,
                    range,
                    index_keys: &entry.index_keys,
                    identity: &entry.identity,
                },
                record.span,
                key_data_error,
            )?;
            record.validate_identity_data(&entry.identity)?;
            anchors.push(SurfacePageRowAnchor::index_range(
                entry.index_keys,
                entry.identity,
            ));
        }
        Ok(anchors)
    }

    fn validate_exact_keys(
        &self,
        record: &SurfaceRecordReadPlan<'_>,
        exact_keys: &[SavedKey],
        span: SourceSpan,
        error: fn(String, SourceSpan) -> SurfaceReadError,
    ) -> Result<(), SurfaceReadError> {
        if exact_keys.len() != self.exact_key_count {
            return Err(error(
                format!(
                    "surface range index page expects {} exact key(s), got {}",
                    self.exact_key_count,
                    exact_keys.len()
                ),
                span,
            ));
        }
        validate_index_keys(
            record.facts,
            exact_keys,
            &self.key_meanings[..self.exact_key_count],
            span,
            error,
        )
    }

    fn validate_range_bounds(
        &self,
        record: &SurfaceRecordReadPlan<'_>,
        range: &SurfaceIndexRangeRequest,
        span: SourceSpan,
        error: fn(String, SourceSpan) -> SurfaceReadError,
    ) -> Result<(), SurfaceReadError> {
        if range.lower.is_none() && range.upper.is_none() {
            return Err(error(
                "surface range page requires at least one bound".into(),
                span,
            ));
        }
        let meaning = &self.key_meanings[self.range_key_index];
        for bound in range.lower.iter().chain(range.upper.iter()) {
            validate_index_key(record.facts, bound, meaning, span, error)?;
        }
        Ok(())
    }

    fn validate_entry(
        &self,
        record: &SurfaceRecordReadPlan<'_>,
        entry: SurfaceIndexRangeEntry<'_>,
        span: SourceSpan,
        error: fn(String, SourceSpan) -> SurfaceReadError,
    ) -> Result<(), SurfaceReadError> {
        validate_index_keys(
            record.facts,
            entry.index_keys,
            &self.key_meanings,
            span,
            error,
        )?;
        if !entry.index_keys.starts_with(entry.exact_keys) {
            return Err(error(
                "surface range index entry does not match exact key prefix".into(),
                span,
            ));
        }
        let range_key = entry.index_keys.get(self.range_key_index).ok_or_else(|| {
            error(
                "surface range index entry is missing the range key".into(),
                span,
            )
        })?;
        if !range_contains_key(entry.range, range_key) {
            return Err(error(
                "surface range index entry is outside the requested bounds".into(),
                span,
            ));
        }
        let identity_suffix_start = entry
            .index_keys
            .len()
            .checked_sub(self.identity_key_count)
            .ok_or_else(|| {
                error(
                    "surface range index entry is missing the identity suffix".into(),
                    span,
                )
            })?;
        if entry.index_keys[identity_suffix_start..] != *entry.identity {
            return Err(error(
                "surface range index entry identity suffix does not match the row identity".into(),
                span,
            ));
        }
        record.validate_identity_with(entry.identity, error)
    }
}

fn range_bounds(range: &SurfaceIndexRangeRequest) -> IndexRangeBounds {
    IndexRangeBounds {
        lower: range.lower.clone(),
        lower_inclusive: range.lower_inclusive,
        upper: range.upper.clone(),
        upper_inclusive: range.upper_inclusive,
    }
}

fn range_contains_key(range: &SurfaceIndexRangeRequest, key: &SavedKey) -> bool {
    if let Some(lower) = &range.lower {
        let above_lower = if range.lower_inclusive {
            key >= lower
        } else {
            key > lower
        };
        if !above_lower {
            return false;
        }
    }
    if let Some(upper) = &range.upper {
        let below_upper = if range.upper_inclusive {
            key <= upper
        } else {
            key < upper
        };
        if !below_upper {
            return false;
        }
    }
    true
}

fn input_key_shapes(
    facts: &CheckedFacts,
    meanings: &[StoredValueMeaning],
    span: SourceSpan,
) -> Result<Vec<SurfaceInputKeyShape>, SurfaceReadError> {
    meanings
        .iter()
        .map(|meaning| input_key_shape(facts, meaning, span))
        .collect()
}

fn input_key_shape(
    facts: &CheckedFacts,
    meaning: &StoredValueMeaning,
    span: SourceSpan,
) -> Result<SurfaceInputKeyShape, SurfaceReadError> {
    match meaning {
        StoredValueMeaning::Scalar(scalar) => Ok(SurfaceInputKeyShape::Scalar(*scalar)),
        StoredValueMeaning::Enum { enum_id, members } => {
            let enum_fact = facts
                .enum_(*enum_id)
                .ok_or_else(|| abi_mismatch("checked enum id is missing", span))?;
            let enum_catalog_id = catalog_id(&enum_fact.catalog_id, "enum", span)?;
            let mut member_catalog_ids = Vec::new();
            for member_id in members {
                let member = facts
                    .enum_members()
                    .get(member_id.0 as usize)
                    .ok_or_else(|| abi_mismatch("checked enum member id is missing", span))?;
                if member.enum_id != *enum_id {
                    return Err(abi_mismatch(
                        "checked enum member is outside the enum shape",
                        span,
                    ));
                }
                if facts.enum_member_is_selectable(member.id) {
                    member_catalog_ids.push(catalog_id(&member.catalog_id, "enum member", span)?);
                }
            }
            Ok(SurfaceInputKeyShape::Enum {
                enum_catalog_id,
                member_catalog_ids,
            })
        }
        StoredValueMeaning::Identity {
            store_catalog_id,
            arity,
            key_scalars,
            ..
        } => {
            if *arity != key_scalars.len() {
                return Err(abi_mismatch(
                    "checked identity key shape arity does not match its scalar keys",
                    span,
                ));
            }
            Ok(SurfaceInputKeyShape::Identity(SurfaceIdentityInputShape {
                store_catalog_id: catalog_id(store_catalog_id, "identity store", span)?,
                keys: key_scalars
                    .iter()
                    .copied()
                    .map(SurfaceInputKeyShape::Scalar)
                    .collect(),
            }))
        }
    }
}

impl SurfaceUniqueLookupPlan {
    fn validate_request_keys(
        &self,
        record: &SurfaceRecordReadPlan<'_>,
        keys: &[SavedKey],
        span: SourceSpan,
    ) -> Result<(), SurfaceReadError> {
        if keys.len() != self.key_count {
            return Err(request_at(
                format!(
                    "surface unique lookup expects {} key(s), got {}",
                    self.key_count,
                    keys.len()
                ),
                span,
            ));
        }
        validate_index_keys(
            record.facts,
            keys,
            &self.key_meanings,
            span,
            key_request_error,
        )
    }

    fn lookup_identity(
        &self,
        store: &TreeStore,
        keys: &[SavedKey],
        identity_arity: usize,
        span: SourceSpan,
    ) -> Result<Option<Vec<SavedKey>>, SurfaceReadError> {
        let page = store
            .scan_index_tuple(&self.index_catalog_id, keys, 2)
            .map_err(|error| surface_scan_error(error, span))?;
        if page.truncated || page.entries.len() > 1 {
            return Err(invalid_data(
                "stored unique index has multiple entries for one tuple",
                span,
            ));
        }
        let Some(entry) = page.entries.first() else {
            return Ok(None);
        };
        if entry.index_keys != keys {
            return Err(invalid_data(
                "stored unique index entry does not match the requested tuple",
                span,
            ));
        }
        let identity = decode_identity_payload_arity(&entry.value, identity_arity)
            .ok_or_else(|| invalid_data("stored unique index identity did not decode", span))?;
        if entry.identity != identity {
            return Err(invalid_data(
                "stored unique index identity does not match the entry payload",
                span,
            ));
        }
        Ok(Some(identity))
    }
}

fn validate_page_limit(limit: usize, span: SourceSpan) -> Result<(), SurfaceReadError> {
    if limit == 0 {
        return Err(request_at(
            "surface page limit must be greater than zero",
            span,
        ));
    }
    if limit > SURFACE_MAX_PAGE_LIMIT {
        return Err(limit_error(
            format!("surface page limit {limit} exceeds the maximum {SURFACE_MAX_PAGE_LIMIT}"),
            span,
        ));
    }
    Ok(())
}

fn validate_index_keys(
    facts: &CheckedFacts,
    keys: &[SavedKey],
    meanings: &[StoredValueMeaning],
    span: SourceSpan,
    error: fn(String, SourceSpan) -> SurfaceReadError,
) -> Result<(), SurfaceReadError> {
    if keys.len() != meanings.len() {
        return Err(error(
            format!(
                "surface index key tuple expects {} key(s), got {}",
                meanings.len(),
                keys.len()
            ),
            span,
        ));
    }
    for (key, meaning) in keys.iter().zip(meanings) {
        validate_index_key(facts, key, meaning, span, error)?;
    }
    Ok(())
}

fn validate_index_key(
    facts: &CheckedFacts,
    key: &SavedKey,
    meaning: &StoredValueMeaning,
    span: SourceSpan,
    error: fn(String, SourceSpan) -> SurfaceReadError,
) -> Result<(), SurfaceReadError> {
    validate_scalar_key(key).map_err(|err| error(err.to_string(), span))?;
    match meaning {
        StoredValueMeaning::Scalar(expected) => {
            if scalar_key_matches_type(key, *expected) {
                Ok(())
            } else {
                Err(error(
                    "surface index key does not match the checked scalar type".to_string(),
                    span,
                ))
            }
        }
        StoredValueMeaning::Enum { enum_id, members } => {
            let SavedKey::Str(member_catalog_id) = key else {
                return Err(error(
                    "surface index key does not match the checked enum type".to_string(),
                    span,
                ));
            };
            let member = facts.enum_members().iter().find(|member| {
                member.enum_id == *enum_id
                    && members.contains(&member.id)
                    && member.catalog_id.as_deref() == Some(member_catalog_id.as_str())
            });
            if member.is_some_and(|member| facts.enum_member_is_selectable(member.id)) {
                Ok(())
            } else {
                Err(error(
                    "surface index key names no selectable enum member".to_string(),
                    span,
                ))
            }
        }
        StoredValueMeaning::Identity {
            store_catalog_id,
            arity,
            key_scalars,
            ..
        } => {
            let SavedKey::Bytes(bytes) = key else {
                return Err(error(
                    "surface index key does not match the checked identity type".to_string(),
                    span,
                ));
            };
            let Some(store_catalog_id) = store_catalog_id.as_deref() else {
                return Err(abi_mismatch(
                    "checked identity index key has no accepted store catalog id",
                    span,
                ));
            };
            let keys =
                decode_identity_index_key(bytes, store_catalog_id, *arity).ok_or_else(|| {
                    error(
                        "surface identity index key did not decode".to_string(),
                        span,
                    )
                })?;
            if identity_keys_match_scalars(&keys, key_scalars) {
                Ok(())
            } else {
                Err(error(
                    "surface identity index key does not match the checked identity type"
                        .to_string(),
                    span,
                ))
            }
        }
    }
}

fn key_request_error(message: String, span: SourceSpan) -> SurfaceReadError {
    request_at(message, span)
}

fn key_cursor_error(message: String, span: SourceSpan) -> SurfaceReadError {
    cursor_error(message, span)
}

fn key_data_error(message: String, span: SourceSpan) -> SurfaceReadError {
    invalid_data(message, span)
}

impl<'a> SurfaceRecordReadPlan<'a> {
    fn prepare(
        facts: &'a CheckedFacts,
        store: &StoreFact,
        operation: &SurfaceReadOperationFact,
        surface_span: SourceSpan,
    ) -> Result<Self, SurfaceReadError> {
        let footprint_resource = checked_footprint_resource(operation, store, surface_span)?;
        let store_catalog_id = catalog_id(&store.catalog_id, "store", surface_span)?;
        let identity_keys = identity_key_meanings(store, surface_span)?;
        let reads = surface_member_reads(facts, footprint_resource, operation, surface_span)?;
        Ok(Self {
            facts,
            store_catalog_id,
            identity_keys,
            reads,
            projection: operation.projection.clone(),
            span: operation.span,
        })
    }

    fn validate_identity(&self, identity: &[SavedKey]) -> Result<(), SurfaceReadError> {
        self.validate_identity_with(identity, key_request_error)
    }

    fn validate_identity_cursor(&self, identity: &[SavedKey]) -> Result<(), SurfaceReadError> {
        self.validate_identity_with(identity, cursor_error)
    }

    fn validate_identity_data(&self, identity: &[SavedKey]) -> Result<(), SurfaceReadError> {
        self.validate_identity_with(identity, invalid_data)
    }

    fn validate_identity_with(
        &self,
        identity: &[SavedKey],
        error: fn(String, SourceSpan) -> SurfaceReadError,
    ) -> Result<(), SurfaceReadError> {
        if identity.len() != self.identity_keys.len() {
            return Err(error(
                format!(
                    "surface identity expects {} key(s), got {}",
                    self.identity_keys.len(),
                    identity.len()
                ),
                self.span,
            ));
        }
        for (key, meaning) in identity.iter().zip(&self.identity_keys) {
            validate_scalar_key(key).map_err(|err| error(err.to_string(), self.span))?;
            let StoredValueMeaning::Scalar(expected) = meaning else {
                return Err(abi_mismatch(
                    "checked store identity key is not scalar",
                    self.span,
                ));
            };
            if !scalar_key_matches_type(key, *expected) {
                return Err(error(
                    "surface identity key does not match the checked store key type".to_string(),
                    self.span,
                ));
            }
        }
        Ok(())
    }

    fn identity_input_shape(&self) -> Result<SurfaceIdentityInputShape, SurfaceReadError> {
        Ok(SurfaceIdentityInputShape {
            store_catalog_id: self.store_catalog_id.clone(),
            keys: self
                .identity_keys
                .iter()
                .map(|meaning| {
                    let StoredValueMeaning::Scalar(scalar) = meaning else {
                        return Err(abi_mismatch(
                            "checked store identity key is not scalar",
                            self.span,
                        ));
                    };
                    Ok(SurfaceInputKeyShape::Scalar(*scalar))
                })
                .collect::<Result<Vec<_>, SurfaceReadError>>()?,
        })
    }

    fn surface_identity(&self, identity: &[SavedKey]) -> SurfaceReadIdentity {
        SurfaceReadIdentity {
            store_catalog_id: self.store_catalog_id.clone(),
            keys: identity.to_vec(),
        }
    }

    fn materialize(
        &self,
        store: &TreeStore,
        identity: &[SavedKey],
        output_identity: Option<SurfaceReadIdentity>,
        missing_record: MissingRecord,
        budget: &mut SurfaceMaterializationBudget,
    ) -> Result<SurfaceReadRecord, SurfaceReadError> {
        if !store
            .data_subtree_exists(&self.store_catalog_id, identity, &[])
            .map_err(|error| surface_store_error(error, self.span))?
        {
            return Err(match missing_record {
                MissingRecord::Absent => {
                    surface_error_at(SURFACE_ABSENT, "surface record is absent", self.span)
                }
                MissingRecord::InvalidData => {
                    invalid_data("surface index row points at an absent record", self.span)
                }
            });
        }

        let mut values = HashMap::new();
        for read in &self.reads {
            let prefix = store
                .read_data_value_prefix(
                    &self.store_catalog_id,
                    identity,
                    &read.path,
                    SURFACE_MAX_VALUE_BYTES,
                )
                .map_err(|error| surface_store_error(error, self.span))?;
            let Some(prefix) = prefix else {
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
            if prefix.truncated {
                return Err(limit_error(
                    format!(
                        "stored value for `{}` exceeds the surface value byte budget",
                        read.render_label
                    ),
                    self.span,
                ));
            }
            budget.take(prefix.bytes.len(), self.span)?;
            let value = decode_surface_value(self.facts, &prefix.bytes, &read.value_meaning)
                .ok_or_else(|| {
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
        Ok(SurfaceReadRecord {
            identity: output_identity,
            fields,
        })
    }
}

fn admit_surface_store(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<SurfaceStoreLineage, SurfaceReadError> {
    let accepted_epoch = program.catalog.accepted_epoch.ok_or_else(|| {
        abi_mismatch(
            "surface operation requires a checked program bound to an accepted catalog",
            SourceSpan::default(),
        )
    })?;
    let accepted_digest = program.catalog.accepted_digest.as_deref().ok_or_else(|| {
        abi_mismatch(
            "surface operation requires an accepted catalog digest",
            SourceSpan::default(),
        )
    })?;
    let Some(store_uid) = store
        .read_store_uid()
        .map_err(|error| surface_store_error(error, SourceSpan::default()))?
    else {
        return Err(abi_mismatch(
            "surface operation requires a stamped store uid",
            SourceSpan::default(),
        ));
    };
    let Some(commit) = store
        .read_commit_metadata()
        .map_err(|error| surface_store_error(error, SourceSpan::default()))?
    else {
        return Err(abi_mismatch(
            "surface operation requires commit metadata",
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
    Ok(SurfaceStoreLineage {
        store_uid,
        commit_id: commit.commit_id,
        catalog_digest: accepted_digest.to_string(),
        source_digest: commit.source_digest,
        engine_profile_digest: commit.engine_profile_digest,
    })
}

fn read_current_cursor_lineage(
    store: &TreeStore,
    admitted: &SurfaceStoreLineage,
    span: SourceSpan,
) -> Result<SurfaceStoreLineage, SurfaceReadError> {
    let Some(store_uid) = store
        .read_store_uid()
        .map_err(|error| surface_store_error(error, span))?
    else {
        return Err(abi_mismatch(
            "surface operation requires a stamped store uid",
            span,
        ));
    };
    let Some(commit) = store
        .read_commit_metadata()
        .map_err(|error| surface_store_error(error, span))?
    else {
        return Err(abi_mismatch(
            "surface operation requires commit metadata",
            span,
        ));
    };
    Ok(SurfaceStoreLineage {
        store_uid,
        commit_id: commit.commit_id,
        catalog_digest: admitted.catalog_digest.clone(),
        source_digest: commit.source_digest,
        engine_profile_digest: commit.engine_profile_digest,
    })
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

fn checked_surface_operation(
    program: &CheckedProgram,
    operation_ref: SurfaceReadOperationRef,
) -> Result<(&SurfaceFact, &SurfaceReadOperationFact), SurfaceReadError> {
    let surface = checked_surface(program, operation_ref.surface)?;
    let operation = surface
        .read_operations
        .get(operation_ref.ordinal)
        .ok_or_else(|| request("checked surface operation ordinal is not present"))?;
    Ok((surface, operation))
}

fn checked_read_operation_by_tag<'a>(
    program: &'a CheckedProgram,
    operation_tag: &str,
) -> Result<SurfaceReadOperationMatch<'a>, SurfaceReadError> {
    require_unique_surface_operation_tag(program, operation_tag)?;
    let mut matched = None;
    for surface in program.facts.surfaces() {
        for (ordinal, operation) in surface.read_operations.iter().enumerate() {
            if operation.operation_tag.as_deref() != Some(operation_tag) {
                continue;
            }
            if matched.is_some() {
                return Err(abi_mismatch(
                    "surface operation tag matches multiple checked read operations",
                    operation.span,
                ));
            }
            matched = Some(SurfaceReadOperationMatch {
                surface,
                operation,
                operation_ref: SurfaceReadOperationRef {
                    surface: surface.id,
                    ordinal,
                },
            });
        }
    }
    matched.ok_or_else(|| {
        abi_mismatch(
            "surface operation tag is not exported by this checked program",
            SourceSpan::default(),
        )
    })
}

fn checked_update_surface_by_tag<'a>(
    program: &'a CheckedProgram,
    operation_tag: &str,
) -> Result<&'a SurfaceFact, SurfaceError> {
    require_unique_surface_operation_tag(program, operation_tag)?;
    let mut matched = None;
    for surface in program.facts.surfaces() {
        let Some(descriptor) = SurfaceUpdateOperationDescriptor::from_surface(program, surface)
        else {
            continue;
        };
        if descriptor.operation_tag != operation_tag {
            continue;
        }
        if matched.is_some() {
            return Err(abi_mismatch(
                "surface operation tag matches multiple checked update operations",
                surface.span,
            ));
        }
        matched = Some(surface);
    }
    matched.ok_or_else(|| {
        abi_mismatch(
            "surface operation tag is not exported by this checked program",
            SourceSpan::default(),
        )
    })
}

fn require_unique_surface_operation_tag(
    program: &CheckedProgram,
    operation_tag: &str,
) -> Result<(), SurfaceReadError> {
    let count = surface_operation_tag_count(program, operation_tag);
    if count > 1 {
        return Err(abi_mismatch(
            "surface operation tag is ambiguous",
            SourceSpan::default(),
        ));
    }
    Ok(())
}

fn surface_operation_tag_count(program: &CheckedProgram, operation_tag: &str) -> usize {
    let mut count = 0;
    for surface in program.facts.surfaces() {
        count += surface
            .read_operations
            .iter()
            .filter(|read| read.operation_tag.as_deref() == Some(operation_tag))
            .count();
        if SurfaceUpdateOperationDescriptor::from_surface(program, surface)
            .is_some_and(|descriptor| descriptor.operation_tag == operation_tag)
        {
            count += 1;
        }
        if SurfaceCreateOperationDescriptor::from_surface(program, surface)
            .is_some_and(|descriptor| descriptor.operation_tag == operation_tag)
        {
            count += 1;
        }
        if SurfaceDeleteOperationDescriptor::from_surface(program, surface)
            .is_some_and(|descriptor| descriptor.operation_tag == operation_tag)
        {
            count += 1;
        }
        count += surface
            .actions
            .iter()
            .filter(|action| {
                SurfaceActionOperationDescriptor::from_action(program, surface, action)
                    .is_some_and(|descriptor| descriptor.operation_tag == operation_tag)
            })
            .count();
        count += surface
            .computed_reads
            .iter()
            .filter(|computed_read| {
                SurfaceComputedReadOperationDescriptor::from_computed_read(
                    program,
                    surface,
                    computed_read,
                )
                .is_some_and(|descriptor| descriptor.operation_tag == operation_tag)
            })
            .count();
    }
    count
}

fn require_stable_surface(surface: &SurfaceFact) -> Result<(), SurfaceReadError> {
    match surface.catalog_status {
        SurfaceCatalogStatus::Stable => Ok(()),
        SurfaceCatalogStatus::SourceOnly(_) => Err(abi_mismatch(
            format!(
                "surface `{}` is source-only; run against accepted catalog identities before exporting it",
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
        .find_map(|operation| node_read_shape(surface, operation).map(|shape| (operation, shape)))
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

fn node_read_shape(
    surface: &SurfaceFact,
    operation: &SurfaceReadOperationFact,
) -> Option<SurfaceNodeReadShape> {
    match operation.kind {
        SurfaceReadOperationKind::SingletonRead { store } if store == surface.store => {
            Some(SurfaceNodeReadShape::Singleton)
        }
        SurfaceReadOperationKind::PointRead { store } if store == surface.store => {
            Some(SurfaceNodeReadShape::Point)
        }
        _ => None,
    }
}

fn collection_plan_kind(
    facts: &CheckedFacts,
    surface: &SurfaceFact,
    operation: &SurfaceReadOperationFact,
    span: SourceSpan,
) -> Result<SurfaceCollectionPlanKind, SurfaceReadError> {
    match operation.kind {
        SurfaceReadOperationKind::PagedRootCollection { store } if store == surface.store => {
            Ok(SurfaceCollectionPlanKind::Root)
        }
        SurfaceReadOperationKind::PagedIndexCollection {
            index,
            exact_key_count,
            identity_key_count,
        } => {
            let fact = surface_index_fact(facts, surface, index, span)?;
            let index_catalog_id = catalog_id(&fact.catalog_id, "store index", span)?;
            if fact.unique {
                return Err(abi_mismatch(
                    "checked paged collection operation names a unique index",
                    span,
                ));
            }
            let store = facts.store(surface.store);
            if exact_key_count + identity_key_count != fact.keys.len()
                || identity_key_count != store.identity_keys.len()
            {
                return Err(abi_mismatch(
                    "checked paged collection operation does not match the index key shape",
                    span,
                ));
            }
            Ok(SurfaceCollectionPlanKind::Index(SurfaceIndexPagePlan {
                index_catalog_id,
                key_meanings: fact
                    .keys
                    .iter()
                    .map(|key| key.value_meaning.clone())
                    .collect(),
                exact_key_count,
                identity_key_count,
            }))
        }
        SurfaceReadOperationKind::PagedIndexRangeCollection {
            index,
            exact_key_count,
            range_key_index,
            identity_key_count,
        } => {
            let fact = surface_index_fact(facts, surface, index, span)?;
            let index_catalog_id = catalog_id(&fact.catalog_id, "store index", span)?;
            if fact.unique {
                return Err(abi_mismatch(
                    "checked range collection operation names a unique index",
                    span,
                ));
            }
            let store = facts.store(surface.store);
            if exact_key_count + 1 + identity_key_count != fact.keys.len()
                || range_key_index != exact_key_count
                || identity_key_count != store.identity_keys.len()
            {
                return Err(abi_mismatch(
                    "checked range collection operation does not match the index key shape",
                    span,
                ));
            }
            if !matches!(
                fact.keys[range_key_index].value_meaning,
                StoredValueMeaning::Scalar(_)
            ) {
                return Err(abi_mismatch(
                    "checked range collection operation has a non-scalar range key",
                    span,
                ));
            }
            Ok(SurfaceCollectionPlanKind::Range(
                SurfaceIndexRangePagePlan {
                    index_catalog_id,
                    key_meanings: fact
                        .keys
                        .iter()
                        .map(|key| key.value_meaning.clone())
                        .collect(),
                    exact_key_count,
                    range_key_index,
                    identity_key_count,
                },
            ))
        }
        SurfaceReadOperationKind::UniqueIndexLookup { index, key_count } => {
            let fact = surface_index_fact(facts, surface, index, span)?;
            let index_catalog_id = catalog_id(&fact.catalog_id, "store index", span)?;
            if !fact.unique {
                return Err(abi_mismatch(
                    "checked unique lookup operation names a non-unique index",
                    span,
                ));
            }
            if key_count != fact.keys.len() {
                return Err(abi_mismatch(
                    "checked unique lookup operation does not match the index key shape",
                    span,
                ));
            }
            Ok(SurfaceCollectionPlanKind::Unique(SurfaceUniqueLookupPlan {
                index_catalog_id,
                key_meanings: fact
                    .keys
                    .iter()
                    .map(|key| key.value_meaning.clone())
                    .collect(),
                key_count,
            }))
        }
        _ => Err(abi_mismatch(
            "checked surface operation is not a collection read",
            operation.span,
        )),
    }
}

fn surface_index_fact<'a>(
    facts: &'a CheckedFacts,
    surface: &SurfaceFact,
    index: marrow_check::StoreIndexId,
    span: SourceSpan,
) -> Result<&'a StoreIndexFact, SurfaceReadError> {
    let fact = facts.store_index(index);
    if fact.store != surface.store {
        return Err(abi_mismatch(
            "checked surface collection index is outside the backing store",
            span,
        ));
    }
    Ok(fact)
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
            && member.plain_field_required.is_some()
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

fn surface_update_members(
    facts: &CheckedFacts,
    surface: &SurfaceFact,
    store: &StoreFact,
) -> Result<HashMap<CatalogId, SurfaceWriteMember>, SurfaceReadError> {
    let mut fields = HashMap::new();
    for field in &surface.update {
        let member = resource_member(facts, field.member, field.span)?;
        if member.resource != store.resource || member.parent.is_some() {
            return Err(abi_mismatch(
                "checked surface update member is outside the backing root fields",
                field.span,
            ));
        }
        let catalog_id = catalog_id(&member.catalog_id, "resource member", field.span)?;
        let value_meaning = value_meaning_from_member(member, field.span)?;
        if fields
            .insert(
                catalog_id,
                SurfaceWriteMember {
                    member: member.id,
                    value_meaning,
                },
            )
            .is_some()
        {
            return Err(abi_mismatch(
                "checked surface update set repeats a member catalog id",
                field.span,
            ));
        }
    }
    Ok(fields)
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

fn lower_surface_write_value(
    value: &SurfaceValue,
    meaning: &StoredValueMeaning,
    facts: &CheckedFacts,
    span: SourceSpan,
) -> Result<PlannedSurfaceWriteValue, SurfaceReadError> {
    match meaning {
        StoredValueMeaning::Scalar(expected) => lower_surface_scalar_write(value, *expected, span),
        StoredValueMeaning::Enum { enum_id, members } => match value {
            SurfaceValue::Enum(value) => {
                lower_surface_enum_write(value, facts, *enum_id, members, span)
            }
            _ => Err(request_at(
                "surface write value does not match the checked field shape",
                span,
            )),
        },
        StoredValueMeaning::Identity {
            store_catalog_id,
            arity,
            key_scalars,
            ..
        } => match value {
            SurfaceValue::Identity(value) => {
                lower_surface_identity_write(value, store_catalog_id, *arity, key_scalars)
            }
            _ => Err(request_at(
                "surface write value does not match the checked field shape",
                span,
            )),
        },
    }
}

fn lower_surface_scalar_write(
    value: &SurfaceValue,
    expected: ScalarType,
    span: SourceSpan,
) -> Result<PlannedSurfaceWriteValue, SurfaceReadError> {
    let saved = match value {
        SurfaceValue::Int(value) if expected == ScalarType::Int => SavedValue::Int(*value),
        SurfaceValue::Bool(value) if expected == ScalarType::Bool => SavedValue::Bool(*value),
        SurfaceValue::Str(value) if expected == ScalarType::Str => SavedValue::Str(value.clone()),
        SurfaceValue::Instant(value) if expected == ScalarType::Instant => {
            SavedValue::Instant(*value)
        }
        SurfaceValue::Date(value) if expected == ScalarType::Date => SavedValue::Date(*value),
        SurfaceValue::Duration(value) if expected == ScalarType::Duration => {
            SavedValue::Duration(*value)
        }
        SurfaceValue::Decimal(value) if expected == ScalarType::Decimal => {
            SavedValue::Decimal(*value)
        }
        SurfaceValue::Bytes(value) if expected == ScalarType::Bytes => {
            SavedValue::Bytes(value.clone())
        }
        _ => {
            return Err(request_at(
                "surface write scalar value does not match the checked field type",
                span,
            ));
        }
    };
    validate_surface_scalar_write_range(&saved, span)?;
    Ok(leaf_scalar(saved))
}

fn validate_surface_scalar_write_range(
    value: &SavedValue,
    span: SourceSpan,
) -> Result<(), SurfaceReadError> {
    match value {
        SavedValue::Date(days) if !supported_date_days(*days) => Err(request_at(
            ValueError::DateOutOfRange { days: *days }.to_string(),
            span,
        )),
        SavedValue::Instant(nanos) if !supported_instant_nanos(*nanos) => Err(request_at(
            ValueError::InstantOutOfRange { nanos: *nanos }.to_string(),
            span,
        )),
        _ => Ok(()),
    }
}

fn leaf_scalar(value: SavedValue) -> PlannedSurfaceWriteValue {
    PlannedSurfaceWriteValue::Leaf(LeafValue::Scalar(value))
}

fn lower_surface_enum_write(
    value: &SurfaceEnumValue,
    facts: &CheckedFacts,
    enum_id: EnumId,
    members: &[EnumMemberId],
    span: SourceSpan,
) -> Result<PlannedSurfaceWriteValue, SurfaceReadError> {
    let enum_fact = facts
        .enum_(enum_id)
        .ok_or_else(|| abi_mismatch("checked enum id is missing", span))?;
    if enum_fact.catalog_id.as_deref() != Some(value.enum_catalog_id.as_str()) {
        return Err(request_at(
            "surface write enum value names a different enum",
            span,
        ));
    }
    let member = facts.enum_members().iter().find(|member| {
        member.enum_id == enum_id
            && members.contains(&member.id)
            && member.catalog_id.as_deref() == Some(value.member_catalog_id.as_str())
    });
    let Some(member) = member else {
        return Err(request_at(
            "surface write enum value names no accepted enum member",
            span,
        ));
    };
    if !facts.enum_member_is_selectable(member.id) {
        return Err(request_at(
            "surface write enum value names an unselectable enum member",
            span,
        ));
    }
    let bytes = encode_tree_enum_member(&TreeEnumMember::new(
        value.enum_catalog_id.clone(),
        value.member_catalog_id.clone(),
    ))
    .map_err(|_| request_at("surface write enum value could not be encoded", span))?;
    let display_name = facts
        .enum_member_catalog_path(member.id)
        .ok_or_else(|| abi_mismatch("enum member has no catalog path", span))?;
    Ok(PlannedSurfaceWriteValue::Leaf(LeafValue::Enum {
        bytes,
        index_key: SavedKey::Str(value.member_catalog_id.as_str().to_string()),
        display_name,
    }))
}

fn lower_surface_identity_write(
    value: &SurfaceReadIdentity,
    raw_store_catalog_id: &Option<String>,
    arity: usize,
    key_scalars: &[ScalarType],
) -> Result<PlannedSurfaceWriteValue, SurfaceReadError> {
    if raw_store_catalog_id.as_deref() != Some(value.store_catalog_id.as_str()) {
        return Err(request(
            "surface write identity value names a different store",
        ));
    }
    if value.keys.len() != arity || !identity_keys_match_scalars(&value.keys, key_scalars) {
        return Err(request(
            "surface write identity value does not match the checked store key shape",
        ));
    }
    Ok(PlannedSurfaceWriteValue::Identity {
        keys: value.keys.clone(),
        referenced_arity: arity,
    })
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
        render_label: facts.enum_member_render_path(member.id)?,
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

fn cursor_error(message: impl Into<String>, span: SourceSpan) -> SurfaceReadError {
    surface_error_at(SURFACE_CURSOR, message, span)
}

fn stale_cursor(message: impl Into<String>, span: SourceSpan) -> SurfaceReadError {
    surface_error_at(SURFACE_STALE_CURSOR, message, span)
}

fn limit_error(message: impl Into<String>, span: SourceSpan) -> SurfaceReadError {
    surface_error_at(SURFACE_LIMIT, message, span)
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

fn conflict_error(message: impl Into<String>, span: SourceSpan) -> SurfaceReadError {
    surface_error_at(SURFACE_CONFLICT, message, span)
}

fn write_error(message: impl Into<String>, span: SourceSpan) -> SurfaceReadError {
    surface_error_at(SURFACE_WRITE, message, span)
}

fn map_surface_write_plan_error(error: WriteError) -> SurfaceReadError {
    match error.code {
        WRITE_UNIQUE_CONFLICT => conflict_error(error.message, SourceSpan::default()),
        WRITE_INVALID_DATA => invalid_data(error.message, SourceSpan::default()),
        WRITE_STORE => store_error(error.message, SourceSpan::default()),
        WRITE_TYPE_MISMATCH | WRITE_IDENTITY_MISMATCH | WRITE_UNKNOWN_FIELD => {
            request(error.message)
        }
        _ => write_error(error.message, SourceSpan::default()),
    }
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
    store_error(format!("a surface operation failed: {error}"), span)
}

fn surface_scan_error(error: StoreError, span: SourceSpan) -> SurfaceReadError {
    match error {
        StoreError::Corruption { .. } => {
            invalid_data("surface collection reached corrupt stored key data", span)
        }
        StoreError::InvalidCursor { .. } => cursor_error("surface cursor is invalid", span),
        StoreError::LimitExceeded { .. } => {
            limit_error("surface collection exceeded a store limit", span)
        }
        other => surface_store_error(other, span),
    }
}

fn surface_fence_error(error: FenceError) -> SurfaceReadError {
    match error {
        FenceError::Store(error) => surface_store_error(error, SourceSpan::default()),
        other => abi_mismatch(other.message(), SourceSpan::default()),
    }
}
