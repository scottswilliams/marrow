use marrow_check::ScalarType;
use marrow_check::tooling::{
    DataChildView, DataChildViewsPage, DataPathError, DataPresence, DataPreviewReadResult,
    IntegrityProblem, IntegrityProblemSample, MemberFlavor, SavedDataPathSegment, StampedData,
};
use marrow_run::{
    DataViewBoundary, DataViewWatchTarget, DataViewWatchTargetKind, ProjectSurfaceReadSession,
};
use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::run::EntryRunAnalysisJson;
use crate::{DataGenerationJson, SavedKeyJson};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DataPathSegmentJson {
    Root { store_catalog_id: String },
    Field { member_catalog_id: String },
    Layer { member_catalog_id: String },
    Key { value: DataKeyJson },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DataKeyJson {
    Int(i64),
    Bool(bool),
    String(String),
    Date(i32),
    Instant(#[serde(with = "i128_string")] i128),
    Duration(#[serde(with = "i128_string")] i128),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataChildrenRequestJson {
    pub segments: Vec<DataPathSegmentJson>,
    pub limit: usize,
    pub cursor: Option<DataKeyJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataReadRequestJson {
    pub segments: Vec<DataPathSegmentJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataChildViewJson {
    pub segment: DataPathSegmentJson,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataChildViewsPageJson {
    pub children: Vec<DataChildViewJson>,
    pub truncated: bool,
    pub cursor: Option<DataKeyJson>,
    pub store_snapshot: Option<DataGenerationJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_view_boundary: Option<DataViewBoundaryJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataReadResultJson {
    pub presence: DataPresenceJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub value_truncated: bool,
    pub store_snapshot: Option<DataGenerationJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_view_boundary: Option<DataViewBoundaryJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataIntegrityResultJson {
    pub available: bool,
    pub findings: Vec<DataIntegrityFindingJson>,
    pub scanned: usize,
    pub truncated: bool,
    pub store_snapshot: Option<DataGenerationJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_view_boundary: Option<DataViewBoundaryJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataIntegrityFindingJson {
    pub code: String,
    pub kind: String,
    pub message: String,
    pub source_span: DataIntegritySourceSpanJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_catalog_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_identity: Option<Vec<SavedKeyJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_path: Option<Vec<DataIntegrityPathSegmentJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing_member_catalog_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub containing_identity: Option<Vec<SavedKeyJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_catalog_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referenced_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referenced_identity: Option<Vec<SavedKeyJson>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataIntegritySourceSpanJson {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum DataIntegrityPathSegmentJson {
    Member { member_catalog_id: String },
    Key { key: SavedKeyJson },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataViewBoundaryJson {
    #[serde(rename = "sourceAnalysisGeneration")]
    pub source_analysis_generation: EntryRunAnalysisJson,
    #[serde(rename = "storeSnapshot")]
    pub store_snapshot: DataGenerationJson,
    pub compatibility: DataViewAdmissionJson,
    #[serde(rename = "watchTargets")]
    pub watch_targets: Vec<DataViewWatchTargetJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataViewAdmissionJson {
    pub verdict: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataViewWatchTargetJson {
    pub kind: &'static str,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataPresenceJson {
    Absent,
    ValueOnly,
    ChildrenOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataStoreErrorJson {
    pub code: DataStoreErrorCodeJson,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataStoreErrorCodeJson {
    Io,
    PermissionDenied,
    Locked,
    FormatVersion,
    Corruption,
    RecoveryRequired,
    LimitExceeded,
    InvalidCursor,
    InvalidTransaction,
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum DataPathErrorJson {
    MissingRoot,
    UnknownRoot {
        root: String,
    },
    UnknownRootCatalogId {
        store_catalog_id: String,
    },
    TooManyIdentityKeys {
        root: String,
    },
    IdentityKeyType {
        root: String,
        expected: ScalarTypeJson,
        found: ScalarTypeJson,
    },
    MissingIdentityKeys {
        root: String,
        expected: usize,
    },
    UnexpectedKey,
    UnknownMember {
        flavor: MemberFlavorJson,
        name: String,
    },
    UnknownMemberCatalogId {
        flavor: MemberFlavorJson,
        member_catalog_id: String,
    },
    TooManyMemberKeys {
        member: String,
    },
    MemberKeyType {
        member: String,
        expected: ScalarTypeJson,
        found: ScalarTypeJson,
    },
    IncompleteMemberKeys {
        member: String,
    },
    ZeroLimit,
    CursorOutsidePath,
    CursorNotAPosition,
    CursorNotAnEntry,
    MembersTakeNoCursor,
    NoChildScan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarTypeJson {
    Bool,
    Int,
    String,
    Bytes,
    Date,
    Duration,
    Instant,
    Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberFlavorJson {
    Field,
    Layer,
    Member,
}

impl DataChildrenRequestJson {
    pub fn into_path_parts(self) -> (Vec<SavedDataPathSegment>, usize, Option<SavedKey>) {
        let segments = self
            .segments
            .into_iter()
            .map(SavedDataPathSegment::from)
            .collect();
        let cursor = self.cursor.map(SavedKey::from);
        (segments, self.limit, cursor)
    }
}

impl DataReadRequestJson {
    pub fn preview_limit_or_default(&self) -> usize {
        self.preview_limit
            .unwrap_or(marrow_check::tooling::DEFAULT_VALUE_PREVIEW_LIMIT)
            .min(marrow_check::tooling::MAX_VALUE_PREVIEW_LIMIT)
    }

    pub fn into_path_segments(self) -> Vec<SavedDataPathSegment> {
        self.segments
            .into_iter()
            .map(SavedDataPathSegment::from)
            .collect()
    }
}

impl From<DataPathSegmentJson> for SavedDataPathSegment {
    fn from(segment: DataPathSegmentJson) -> Self {
        match segment {
            DataPathSegmentJson::Root { store_catalog_id } => Self::Root { store_catalog_id },
            DataPathSegmentJson::Field { member_catalog_id } => Self::Field { member_catalog_id },
            DataPathSegmentJson::Layer { member_catalog_id } => Self::Layer { member_catalog_id },
            DataPathSegmentJson::Key { value } => Self::Key(SavedKey::from(value)),
        }
    }
}

impl From<SavedDataPathSegment> for DataPathSegmentJson {
    fn from(segment: SavedDataPathSegment) -> Self {
        match segment {
            SavedDataPathSegment::Root { store_catalog_id } => Self::Root { store_catalog_id },
            SavedDataPathSegment::Field { member_catalog_id } => Self::Field { member_catalog_id },
            SavedDataPathSegment::Layer { member_catalog_id } => Self::Layer { member_catalog_id },
            SavedDataPathSegment::Key(key) => Self::Key {
                value: DataKeyJson::from(key),
            },
        }
    }
}

impl From<DataKeyJson> for SavedKey {
    fn from(key: DataKeyJson) -> Self {
        match key {
            DataKeyJson::Int(value) => Self::Int(value),
            DataKeyJson::Bool(value) => Self::Bool(value),
            DataKeyJson::String(value) => Self::Str(value),
            DataKeyJson::Date(value) => Self::Date(value),
            DataKeyJson::Instant(value) => Self::Instant(value),
            DataKeyJson::Duration(value) => Self::Duration(value),
            DataKeyJson::Bytes(value) => Self::Bytes(value),
        }
    }
}

impl From<SavedKey> for DataKeyJson {
    fn from(key: SavedKey) -> Self {
        match key {
            SavedKey::Int(value) => Self::Int(value),
            SavedKey::Bool(value) => Self::Bool(value),
            SavedKey::Str(value) => Self::String(value),
            SavedKey::Date(value) => Self::Date(value),
            SavedKey::Instant(value) => Self::Instant(value),
            SavedKey::Duration(value) => Self::Duration(value),
            SavedKey::Bytes(value) => Self::Bytes(value),
        }
    }
}

impl From<DataChildView> for DataChildViewJson {
    fn from(child: DataChildView) -> Self {
        Self {
            segment: DataPathSegmentJson::from(child.segment),
            label: child.label,
        }
    }
}

pub fn data_view_boundary_to_json(session: &ProjectSurfaceReadSession) -> DataViewBoundaryJson {
    DataViewBoundaryJson::from(session.data_view_boundary())
}

impl From<StampedData<DataChildViewsPage>> for DataChildViewsPageJson {
    fn from(stamped: StampedData<DataChildViewsPage>) -> Self {
        let page = stamped.data;
        Self {
            children: page
                .children
                .into_iter()
                .map(DataChildViewJson::from)
                .collect(),
            truncated: page.truncated,
            cursor: page.cursor.map(DataKeyJson::from),
            store_snapshot: Some(DataGenerationJson::from(&stamped.stamp)),
            data_view_boundary: None,
        }
    }
}

impl DataChildViewsPageJson {
    pub fn with_data_view_boundary(mut self, boundary: &DataViewBoundary) -> Self {
        self.data_view_boundary = Some(DataViewBoundaryJson::from(boundary));
        self
    }
}

impl From<StampedData<DataPreviewReadResult>> for DataReadResultJson {
    fn from(stamped: StampedData<DataPreviewReadResult>) -> Self {
        let data = stamped.data;
        let (value, value_truncated) = match data.preview {
            Some(preview) => (Some(preview.text), preview.truncated),
            None => (None, false),
        };
        Self {
            presence: DataPresenceJson::from(data.presence),
            value,
            value_truncated,
            store_snapshot: Some(DataGenerationJson::from(&stamped.stamp)),
            data_view_boundary: None,
        }
    }
}

impl DataReadResultJson {
    pub fn with_data_view_boundary(mut self, boundary: &DataViewBoundary) -> Self {
        self.data_view_boundary = Some(DataViewBoundaryJson::from(boundary));
        self
    }
}

impl DataIntegrityResultJson {
    pub fn unavailable() -> Self {
        Self {
            available: false,
            findings: Vec::new(),
            scanned: 0,
            truncated: false,
            store_snapshot: None,
            data_view_boundary: None,
        }
    }

    pub fn with_data_view_boundary(mut self, boundary: &DataViewBoundary) -> Self {
        self.data_view_boundary = Some(DataViewBoundaryJson::from(boundary));
        self
    }
}

impl From<StampedData<IntegrityProblemSample>> for DataIntegrityResultJson {
    fn from(stamped: StampedData<IntegrityProblemSample>) -> Self {
        let data = stamped.data;
        Self {
            available: true,
            findings: data
                .problems
                .into_iter()
                .map(DataIntegrityFindingJson::from)
                .collect(),
            scanned: data.items_checked,
            truncated: data.truncated,
            store_snapshot: Some(DataGenerationJson::from(&stamped.stamp)),
            data_view_boundary: None,
        }
    }
}

impl From<IntegrityProblem> for DataIntegrityFindingJson {
    fn from(problem: IntegrityProblem) -> Self {
        let mut finding = Self {
            code: problem.code.to_string(),
            kind: marrow_check::kind_for_code(problem.code).to_string(),
            message: problem.message,
            source_span: DataIntegritySourceSpanJson { path: problem.path },
            help: problem.help.map(str::to_string),
            store_catalog_id: None,
            record_identity: None,
            parent_path: None,
            missing_member_catalog_id: None,
            containing_identity: None,
            field_catalog_id: None,
            referenced_root: None,
            referenced_identity: None,
        };
        if let Some(incomplete) = problem.incomplete {
            finding.store_catalog_id = Some(incomplete.store_catalog_id.as_str().to_string());
            finding.record_identity = Some(data_integrity_saved_keys(incomplete.record_identity));
            finding.parent_path = Some(
                incomplete
                    .parent_path
                    .into_iter()
                    .map(DataIntegrityPathSegmentJson::from)
                    .collect(),
            );
            finding.missing_member_catalog_id =
                Some(incomplete.missing_member_catalog_id.as_str().to_string());
        }
        if let Some(dangling_ref) = problem.dangling_ref {
            finding.containing_identity =
                Some(data_integrity_saved_keys(dangling_ref.containing_identity));
            finding.field_catalog_id = Some(dangling_ref.field_catalog_id.as_str().to_string());
            finding.referenced_root = Some(dangling_ref.referenced_root);
            finding.referenced_identity =
                Some(data_integrity_saved_keys(dangling_ref.referenced_identity));
        }
        finding
    }
}

impl From<&IntegrityProblem> for DataIntegrityFindingJson {
    fn from(problem: &IntegrityProblem) -> Self {
        Self::from(problem.clone())
    }
}

pub fn integrity_problem_record_to_json(problem: &IntegrityProblem) -> Json {
    let mut record =
        serde_json::to_value(DataIntegrityFindingJson::from(problem)).expect("integrity finding");
    if let Some(record) = record.as_object_mut() {
        record
            .entry("help".to_string())
            .or_insert(serde_json::Value::Null);
    }
    record
}

impl From<&DataViewBoundary> for DataViewBoundaryJson {
    fn from(boundary: &DataViewBoundary) -> Self {
        Self {
            source_analysis_generation: EntryRunAnalysisJson::from(
                boundary.source_analysis_generation.clone(),
            ),
            store_snapshot: DataGenerationJson::from(&boundary.store_snapshot),
            compatibility: DataViewAdmissionJson::admitted(),
            watch_targets: boundary
                .watch_targets
                .iter()
                .map(DataViewWatchTargetJson::from)
                .collect(),
        }
    }
}

impl DataViewAdmissionJson {
    fn admitted() -> Self {
        Self {
            verdict: "admitted",
        }
    }
}

impl From<&DataViewWatchTarget> for DataViewWatchTargetJson {
    fn from(target: &DataViewWatchTarget) -> Self {
        Self {
            kind: match target.kind {
                DataViewWatchTargetKind::StoreFile => "store_file",
                DataViewWatchTargetKind::CatalogLock => "catalog_lock",
            },
            path: target.path.to_string_lossy().into_owned(),
        }
    }
}

fn data_integrity_saved_keys(keys: Vec<SavedKey>) -> Vec<SavedKeyJson> {
    keys.into_iter().map(SavedKeyJson::from).collect()
}

impl From<DataPathSegment> for DataIntegrityPathSegmentJson {
    fn from(segment: DataPathSegment) -> Self {
        match segment {
            DataPathSegment::Member(catalog_id) => Self::Member {
                member_catalog_id: catalog_id.as_str().to_string(),
            },
            DataPathSegment::Key(key) => Self::Key {
                key: SavedKeyJson::from(key),
            },
        }
    }
}

impl From<DataPresence> for DataPresenceJson {
    fn from(presence: DataPresence) -> Self {
        match presence {
            DataPresence::Absent => Self::Absent,
            DataPresence::ValueOnly => Self::ValueOnly,
            DataPresence::ChildrenOnly => Self::ChildrenOnly,
        }
    }
}

impl From<StoreError> for DataStoreErrorJson {
    fn from(error: StoreError) -> Self {
        let code = match &error {
            StoreError::Io { .. } => DataStoreErrorCodeJson::Io,
            StoreError::PermissionDenied { .. } => DataStoreErrorCodeJson::PermissionDenied,
            StoreError::Locked { .. } => DataStoreErrorCodeJson::Locked,
            StoreError::FormatVersion { .. } => DataStoreErrorCodeJson::FormatVersion,
            StoreError::Corruption { .. } => DataStoreErrorCodeJson::Corruption,
            StoreError::RecoveryRequired => DataStoreErrorCodeJson::RecoveryRequired,
            StoreError::LimitExceeded { .. } => DataStoreErrorCodeJson::LimitExceeded,
            StoreError::InvalidCursor { .. } => DataStoreErrorCodeJson::InvalidCursor,
            StoreError::InvalidTransaction { .. } => DataStoreErrorCodeJson::InvalidTransaction,
            StoreError::ReadOnly { .. } => DataStoreErrorCodeJson::ReadOnly,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

impl From<DataPathError> for DataPathErrorJson {
    fn from(error: DataPathError) -> Self {
        match error {
            DataPathError::MissingRoot => Self::MissingRoot,
            DataPathError::UnknownRoot { root } => Self::UnknownRoot { root },
            DataPathError::UnknownRootCatalogId { store_catalog_id } => {
                Self::UnknownRootCatalogId { store_catalog_id }
            }
            DataPathError::TooManyIdentityKeys { root } => Self::TooManyIdentityKeys { root },
            DataPathError::IdentityKeyType {
                root,
                expected,
                found,
            } => Self::IdentityKeyType {
                root,
                expected: ScalarTypeJson::from(expected),
                found: ScalarTypeJson::from(found),
            },
            DataPathError::MissingIdentityKeys { root, expected } => {
                Self::MissingIdentityKeys { root, expected }
            }
            DataPathError::UnexpectedKey => Self::UnexpectedKey,
            DataPathError::UnknownMember { flavor, name } => Self::UnknownMember {
                flavor: MemberFlavorJson::from(flavor),
                name,
            },
            DataPathError::UnknownMemberCatalogId {
                flavor,
                member_catalog_id,
            } => Self::UnknownMemberCatalogId {
                flavor: MemberFlavorJson::from(flavor),
                member_catalog_id,
            },
            DataPathError::TooManyMemberKeys { member } => Self::TooManyMemberKeys { member },
            DataPathError::MemberKeyType {
                member,
                expected,
                found,
            } => Self::MemberKeyType {
                member,
                expected: ScalarTypeJson::from(expected),
                found: ScalarTypeJson::from(found),
            },
            DataPathError::IncompleteMemberKeys { member } => Self::IncompleteMemberKeys { member },
            DataPathError::ZeroLimit => Self::ZeroLimit,
            DataPathError::CursorOutsidePath => Self::CursorOutsidePath,
            DataPathError::CursorNotAPosition => Self::CursorNotAPosition,
            DataPathError::CursorNotAnEntry => Self::CursorNotAnEntry,
            DataPathError::MembersTakeNoCursor => Self::MembersTakeNoCursor,
            DataPathError::NoChildScan => Self::NoChildScan,
        }
    }
}

impl From<ScalarType> for ScalarTypeJson {
    fn from(ty: ScalarType) -> Self {
        match ty {
            ScalarType::Bool => Self::Bool,
            ScalarType::Int => Self::Int,
            ScalarType::Str => Self::String,
            ScalarType::Bytes => Self::Bytes,
            ScalarType::Date => Self::Date,
            ScalarType::Duration => Self::Duration,
            ScalarType::Instant => Self::Instant,
            ScalarType::Decimal => Self::Decimal,
        }
    }
}

impl From<MemberFlavor> for MemberFlavorJson {
    fn from(flavor: MemberFlavor) -> Self {
        match flavor {
            MemberFlavor::Field => Self::Field,
            MemberFlavor::Layer => Self::Layer,
            MemberFlavor::Member => Self::Member,
        }
    }
}

mod i128_string {
    pub fn serialize<S>(value: &i128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<i128, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use marrow_check::tooling::integrity::{
        DanglingRefIntegrityProblem, IncompleteIntegrityProblem,
    };
    use marrow_check::tooling::{
        DataChildView, DataChildViewsPage, DataPresence, DataPreviewReadResult, DataSnapshotStamp,
        DataValuePreview, IntegrityProblem, IntegrityProblemSample, MAX_VALUE_PREVIEW_LIMIT,
        SavedDataPathSegment, StampedData,
    };
    use marrow_store::cell::CatalogId;
    use marrow_store::key::SavedKey;
    use marrow_store::tree::DataPathSegment;
    use serde_json::json;

    use crate::DATA_GENERATION_PROFILE_VERSION;

    use super::*;

    #[test]
    fn saved_data_dtos_serialize_catalog_bound_wire_shape() {
        let request_json = json!({
            "segments": [
                {
                    "kind": "root",
                    "store_catalog_id": "cat_00000000000000000000000000000001",
                },
                { "kind": "key", "value": { "kind": "int", "value": 1 } },
                {
                    "kind": "field",
                    "member_catalog_id": "cat_00000000000000000000000000000002",
                },
            ],
            "limit": 200,
            "cursor": { "kind": "duration", "value": i128::MIN.to_string() },
        });
        let request: DataChildrenRequestJson =
            serde_json::from_value(request_json.clone()).expect("catalog-bound request DTO");
        assert_eq!(serde_json::to_value(&request).unwrap(), request_json);
        assert_eq!(
            SavedDataPathSegment::from(DataPathSegmentJson::Key {
                value: DataKeyJson::String("alpha".into())
            }),
            SavedDataPathSegment::Key(SavedKey::Str("alpha".into()))
        );

        assert!(
            serde_json::from_value::<DataChildrenRequestJson>(json!({
                "segments": [
                    { "kind": "root", "value": "counter" },
                    { "kind": "field", "value": "value" },
                ],
                "limit": 200,
                "cursor": null,
            }))
            .is_err(),
            "production saved-data DTOs must not accept source-spelling path authority"
        );

        let omitted_limit = DataReadRequestJson {
            segments: vec![DataPathSegmentJson::Root {
                store_catalog_id: "cat_00000000000000000000000000000001".into(),
            }],
            preview_limit: None,
        };
        assert_eq!(
            serde_json::to_value(&omitted_limit).unwrap(),
            json!({
                "segments": [
                    {
                        "kind": "root",
                        "store_catalog_id": "cat_00000000000000000000000000000001",
                    },
                ],
            })
        );
        assert_eq!(
            serde_json::from_value::<DataReadRequestJson>(json!({
                "segments": [
                    {
                        "kind": "root",
                        "store_catalog_id": "cat_00000000000000000000000000000001",
                    },
                ],
            }))
            .unwrap()
            .preview_limit_or_default(),
            marrow_check::tooling::DEFAULT_VALUE_PREVIEW_LIMIT
        );

        let present_limit = DataReadRequestJson {
            segments: vec![DataPathSegmentJson::Root {
                store_catalog_id: "cat_00000000000000000000000000000001".into(),
            }],
            preview_limit: Some(32),
        };
        assert_eq!(
            serde_json::to_value(&present_limit).unwrap(),
            json!({
                "segments": [
                    {
                        "kind": "root",
                        "store_catalog_id": "cat_00000000000000000000000000000001",
                    },
                ],
                "preview_limit": 32,
            })
        );
        assert_eq!(present_limit.preview_limit_or_default(), 32);

        let oversized_limit = DataReadRequestJson {
            segments: vec![DataPathSegmentJson::Root {
                store_catalog_id: "cat_00000000000000000000000000000001".into(),
            }],
            preview_limit: Some(usize::MAX),
        };
        assert_eq!(
            oversized_limit.preview_limit_or_default(),
            MAX_VALUE_PREVIEW_LIMIT
        );

        let stamp = DataSnapshotStamp {
            store_uid: None,
            store_catalog_digest: None,
            store_commit: None,
            open_transaction: None,
            checked_source_digest: "sha256:checked".into(),
        };
        let children = DataChildViewsPageJson::from(StampedData {
            data: DataChildViewsPage {
                children: vec![DataChildView {
                    segment: SavedDataPathSegment::Key(SavedKey::Int(1)),
                    label: "(1)".into(),
                }],
                truncated: true,
                cursor: Some(SavedKey::Int(1)),
            },
            stamp: stamp.clone(),
        });
        assert_eq!(
            serde_json::to_value(&children).unwrap(),
            json!({
                "children": [
                    {
                        "segment": {
                            "kind": "key",
                            "value": { "kind": "int", "value": 1 },
                        },
                        "label": "(1)",
                    },
                ],
                "truncated": true,
                "cursor": { "kind": "int", "value": 1 },
                "store_snapshot": {
                    "profile_version": DATA_GENERATION_PROFILE_VERSION,
                    "store_uid": null,
                    "catalog_digest": null,
                    "commit": null,
                    "open_transaction": null,
                    "checked_source_digest": "sha256:checked",
                },
            })
        );
        assert_eq!(children.data_view_boundary, None);

        let read = DataReadResultJson::from(StampedData {
            data: DataPreviewReadResult {
                preview: Some(DataValuePreview {
                    text: "\"aaaaaaaa\"".into(),
                    truncated: true,
                }),
                presence: DataPresence::ValueOnly,
            },
            stamp,
        });
        assert_eq!(
            serde_json::to_value(&read).unwrap(),
            json!({
                "presence": "value_only",
                "value": "\"aaaaaaaa\"",
                "value_truncated": true,
                "store_snapshot": {
                    "profile_version": DATA_GENERATION_PROFILE_VERSION,
                    "store_uid": null,
                    "catalog_digest": null,
                    "commit": null,
                    "open_transaction": null,
                    "checked_source_digest": "sha256:checked",
                },
            })
        );
        assert_eq!(read.data_view_boundary, None);
    }

    #[test]
    fn data_integrity_result_serializes_typed_problem_fields() {
        let store_catalog_id = CatalogId::new("cat_00000000000000000000000000000001").unwrap();
        let parent_member_id = CatalogId::new("cat_00000000000000000000000000000002").unwrap();
        let missing_member_id = CatalogId::new("cat_00000000000000000000000000000003").unwrap();
        let field_catalog_id = CatalogId::new("cat_00000000000000000000000000000004").unwrap();
        let stamp = DataSnapshotStamp {
            store_uid: None,
            store_catalog_digest: Some("sha256:catalog".into()),
            store_commit: None,
            open_transaction: None,
            checked_source_digest: "sha256:checked".into(),
        };
        let result = DataIntegrityResultJson::from(StampedData {
            data: IntegrityProblemSample {
                items_checked: 7,
                problems: vec![
                    IntegrityProblem {
                        code: "data.incomplete",
                        path: "^books(1).chapters(7).title".into(),
                        message: "stored record is missing required member".into(),
                        help: Some("run marrow data integrity for the full report"),
                        incomplete: Some(IncompleteIntegrityProblem {
                            store_catalog_id: store_catalog_id.clone(),
                            record_identity: vec![SavedKey::Int(1)],
                            parent_path: vec![
                                DataPathSegment::Member(parent_member_id.clone()),
                                DataPathSegment::Key(SavedKey::Int(7)),
                            ],
                            missing_member_catalog_id: missing_member_id.clone(),
                        }),
                        dangling_ref: None,
                    },
                    IntegrityProblem {
                        code: "data.dangling_ref",
                        path: "^books(1).authorId".into(),
                        message: "stored identity reference points at a missing record".into(),
                        help: None,
                        incomplete: None,
                        dangling_ref: Some(DanglingRefIntegrityProblem {
                            containing_identity: vec![SavedKey::Int(1)],
                            field_catalog_id: field_catalog_id.clone(),
                            referenced_root: "authors".into(),
                            referenced_identity: vec![SavedKey::Int(7)],
                        }),
                    },
                ],
                truncated: true,
            },
            stamp,
        });

        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({
                "available": true,
                "findings": [
                    {
                        "code": "data.incomplete",
                        "kind": "tooling",
                        "message": "stored record is missing required member",
                        "source_span": { "path": "^books(1).chapters(7).title" },
                        "help": "run marrow data integrity for the full report",
                        "store_catalog_id": store_catalog_id.as_str(),
                        "record_identity": [{ "type": "int", "value": 1 }],
                        "parent_path": [
                            { "member_catalog_id": parent_member_id.as_str() },
                            { "key": { "type": "int", "value": 7 } }
                        ],
                        "missing_member_catalog_id": missing_member_id.as_str(),
                    },
                    {
                        "code": "data.dangling_ref",
                        "kind": "tooling",
                        "message": "stored identity reference points at a missing record",
                        "source_span": { "path": "^books(1).authorId" },
                        "containing_identity": [{ "type": "int", "value": 1 }],
                        "field_catalog_id": field_catalog_id.as_str(),
                        "referenced_root": "authors",
                        "referenced_identity": [{ "type": "int", "value": 7 }],
                    },
                ],
                "scanned": 7,
                "truncated": true,
                "store_snapshot": {
                    "profile_version": DATA_GENERATION_PROFILE_VERSION,
                    "store_uid": null,
                    "catalog_digest": "sha256:catalog",
                    "commit": null,
                    "open_transaction": null,
                    "checked_source_digest": "sha256:checked",
                },
            })
        );

        assert_eq!(
            serde_json::to_value(DataIntegrityResultJson::unavailable()).unwrap(),
            json!({
                "available": false,
                "findings": [],
                "scanned": 0,
                "truncated": false,
                "store_snapshot": null,
            })
        );
    }
}
