use marrow_check::ScalarType;
use marrow_check::tooling::{
    DataChild, DataChildrenPage, DataPathError, DataPathSegment, DataPresence,
    DataPreviewReadResult, MemberFlavor, StampedData, render_data_path_segments,
};
use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use serde::{Deserialize, Serialize};

use crate::DataSnapshotJson;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DataPathSegmentJson {
    Root(String),
    Field(String),
    Layer(String),
    Key(DataKeyJson),
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
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DataChildJson {
    Root(String),
    Key(DataKeyJson),
    Field(String),
    Layer(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataChildViewJson {
    pub segment: DataPathSegmentJson,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataChildrenPageJson {
    pub children: Vec<DataChildJson>,
    pub truncated: bool,
    pub cursor: Option<DataKeyJson>,
    pub store_snapshot: Option<DataSnapshotJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataChildViewsPageJson {
    pub children: Vec<DataChildViewJson>,
    pub truncated: bool,
    pub cursor: Option<DataKeyJson>,
    pub store_snapshot: Option<DataSnapshotJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataReadResultJson {
    pub presence: DataPresenceJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub value_truncated: bool,
    pub store_snapshot: Option<DataSnapshotJson>,
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
    pub fn into_path_parts(self) -> (Vec<DataPathSegment>, usize, Option<SavedKey>) {
        let segments = self
            .segments
            .into_iter()
            .map(DataPathSegment::from)
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

    pub fn into_path_segments(self) -> Vec<DataPathSegment> {
        self.segments
            .into_iter()
            .map(DataPathSegment::from)
            .collect()
    }
}

impl From<DataPathSegmentJson> for DataPathSegment {
    fn from(segment: DataPathSegmentJson) -> Self {
        match segment {
            DataPathSegmentJson::Root(root) => Self::Root(root),
            DataPathSegmentJson::Field(field) => Self::Field(field),
            DataPathSegmentJson::Layer(layer) => Self::Layer(layer),
            DataPathSegmentJson::Key(key) => Self::Key(SavedKey::from(key)),
        }
    }
}

impl From<DataPathSegment> for DataPathSegmentJson {
    fn from(segment: DataPathSegment) -> Self {
        match segment {
            DataPathSegment::Root(root) => Self::Root(root),
            DataPathSegment::Field(field) => Self::Field(field),
            DataPathSegment::Layer(layer) => Self::Layer(layer),
            DataPathSegment::Key(key) => Self::Key(DataKeyJson::from(key)),
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

impl From<DataChild> for DataChildJson {
    fn from(child: DataChild) -> Self {
        match child {
            DataChild::Root(root) => Self::Root(root),
            DataChild::Key(key) => Self::Key(DataKeyJson::from(key)),
            DataChild::Field(field) => Self::Field(field),
            DataChild::Layer(layer) => Self::Layer(layer),
        }
    }
}

impl From<DataChild> for DataChildViewJson {
    fn from(child: DataChild) -> Self {
        match child {
            DataChild::Root(root) => Self {
                segment: DataPathSegmentJson::Root(root.clone()),
                label: root,
            },
            DataChild::Key(key) => {
                let label = render_data_path_segments(&[DataPathSegment::Key(key.clone())]);
                Self {
                    segment: DataPathSegmentJson::Key(DataKeyJson::from(key)),
                    label,
                }
            }
            DataChild::Field(field) => Self {
                segment: DataPathSegmentJson::Field(field.clone()),
                label: field,
            },
            DataChild::Layer(layer) => Self {
                segment: DataPathSegmentJson::Layer(layer.clone()),
                label: layer,
            },
        }
    }
}

impl From<StampedData<DataChildrenPage>> for DataChildrenPageJson {
    fn from(stamped: StampedData<DataChildrenPage>) -> Self {
        let page = stamped.data;
        Self {
            children: page.children.into_iter().map(DataChildJson::from).collect(),
            truncated: page.truncated,
            cursor: page.cursor.map(DataKeyJson::from),
            store_snapshot: Some(DataSnapshotJson::from(&stamped.stamp)),
        }
    }
}

impl From<StampedData<DataChildrenPage>> for DataChildViewsPageJson {
    fn from(stamped: StampedData<DataChildrenPage>) -> Self {
        let page = stamped.data;
        Self {
            children: page
                .children
                .into_iter()
                .map(DataChildViewJson::from)
                .collect(),
            truncated: page.truncated,
            cursor: page.cursor.map(DataKeyJson::from),
            store_snapshot: Some(DataSnapshotJson::from(&stamped.stamp)),
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
            store_snapshot: Some(DataSnapshotJson::from(&stamped.stamp)),
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
        DataChild, DataChildrenPage, DataPathSegment, DataPresence, DataPreviewReadResult,
        DataSnapshotStamp, DataValuePreview, MAX_VALUE_PREVIEW_LIMIT, StampedData,
    };
    use marrow_store::key::SavedKey;
    use serde_json::json;

    use super::*;

    #[test]
    fn saved_data_dtos_serialize_current_wire_shape() {
        let request = DataChildrenRequestJson {
            segments: vec![
                DataPathSegmentJson::Root("counter".into()),
                DataPathSegmentJson::Key(DataKeyJson::Int(1)),
                DataPathSegmentJson::Field("value".into()),
            ],
            limit: 200,
            cursor: Some(DataKeyJson::Duration(i128::MIN)),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "segments": [
                    { "kind": "root", "value": "counter" },
                    { "kind": "key", "value": { "kind": "int", "value": 1 } },
                    { "kind": "field", "value": "value" },
                ],
                "limit": 200,
                "cursor": { "kind": "duration", "value": i128::MIN.to_string() },
            })
        );
        assert_eq!(
            DataPathSegment::from(DataPathSegmentJson::Key(DataKeyJson::String(
                "alpha".into()
            ))),
            DataPathSegment::Key(SavedKey::Str("alpha".into()))
        );

        let omitted_limit = DataReadRequestJson {
            segments: vec![DataPathSegmentJson::Root("counter".into())],
            preview_limit: None,
        };
        assert_eq!(
            serde_json::to_value(&omitted_limit).unwrap(),
            json!({
                "segments": [
                    { "kind": "root", "value": "counter" },
                ],
            })
        );
        assert_eq!(
            serde_json::from_value::<DataReadRequestJson>(json!({
                "segments": [
                    { "kind": "root", "value": "counter" },
                ],
            }))
            .unwrap()
            .preview_limit_or_default(),
            marrow_check::tooling::DEFAULT_VALUE_PREVIEW_LIMIT
        );

        let present_limit = DataReadRequestJson {
            segments: vec![DataPathSegmentJson::Root("counter".into())],
            preview_limit: Some(32),
        };
        assert_eq!(
            serde_json::to_value(&present_limit).unwrap(),
            json!({
                "segments": [
                    { "kind": "root", "value": "counter" },
                ],
                "preview_limit": 32,
            })
        );
        assert_eq!(present_limit.preview_limit_or_default(), 32);

        let oversized_limit = DataReadRequestJson {
            segments: vec![DataPathSegmentJson::Root("counter".into())],
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
            checked_source_digest: "sha256:checked".into(),
        };
        let children = DataChildViewsPageJson::from(StampedData {
            data: DataChildrenPage {
                children: vec![DataChild::Key(SavedKey::Int(1))],
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
                    "store_uid": null,
                    "catalog_digest": null,
                    "commit": null,
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
                    "store_uid": null,
                    "catalog_digest": null,
                    "commit": null,
                    "checked_source_digest": "sha256:checked",
                },
            })
        );
    }
}
