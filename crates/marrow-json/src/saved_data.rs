use marrow_check::ScalarType;
use marrow_check::tooling::{
    DataChildView, DataChildViewsPage, DataPathError, DataPresence, DataPreviewReadResult,
    MemberFlavor, SavedDataPathSegment, StampedData,
};
use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use serde::{Deserialize, Serialize};

use crate::DataGenerationJson;

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataReadResultJson {
    pub presence: DataPresenceJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub value_truncated: bool,
    pub store_snapshot: Option<DataGenerationJson>,
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
        }
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
    use marrow_check::tooling::{
        DataChildView, DataChildViewsPage, DataPresence, DataPreviewReadResult, DataSnapshotStamp,
        DataValuePreview, MAX_VALUE_PREVIEW_LIMIT, SavedDataPathSegment, StampedData,
    };
    use marrow_store::key::SavedKey;
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
    }
}
