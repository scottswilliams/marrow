use marrow_run::{
    SurfaceCollectionPage, SurfaceEnumValue, SurfacePageBoundary, SurfacePageCursor,
    SurfaceReadField, SurfaceReadIdentity, SurfaceReadRecord, SurfaceValue,
};
use marrow_store::key::SavedKey;
use serde::Serialize;

use crate::lower_hex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceCursorJson {
    pub operation_tag: String,
    pub store_uid: String,
    pub catalog_digest: String,
    pub source_digest: String,
    pub engine_profile_digest: String,
    pub boundary: SurfaceCursorBoundaryJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceCursorBoundaryJson {
    RootIdentity {
        identity: Vec<SurfaceKeyJson>,
    },
    IndexIdentity {
        exact_keys: Vec<SurfaceKeyJson>,
        identity: Vec<SurfaceKeyJson>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

impl From<&SurfaceCollectionPage> for SurfacePageJson {
    fn from(page: &SurfaceCollectionPage) -> Self {
        Self {
            rows: page.rows.iter().map(SurfaceRecordJson::from).collect(),
            next: page.next.as_ref().map(SurfaceCursorJson::from),
        }
    }
}

impl From<&SurfacePageCursor> for SurfaceCursorJson {
    fn from(cursor: &SurfacePageCursor) -> Self {
        Self {
            operation_tag: cursor.operation_tag.clone(),
            store_uid: cursor.store_uid.as_str().to_string(),
            catalog_digest: cursor.catalog_digest.clone(),
            source_digest: cursor.source_digest.clone(),
            engine_profile_digest: lower_hex(&cursor.engine_profile_digest),
            boundary: SurfaceCursorBoundaryJson::from(&cursor.boundary),
        }
    }
}

impl From<&SurfacePageBoundary> for SurfaceCursorBoundaryJson {
    fn from(boundary: &SurfacePageBoundary) -> Self {
        match boundary {
            SurfacePageBoundary::RootIdentity(identity) => Self::RootIdentity {
                identity: identity.iter().map(SurfaceKeyJson::from).collect(),
            },
            SurfacePageBoundary::IndexIdentity {
                exact_keys,
                identity,
            } => Self::IndexIdentity {
                exact_keys: exact_keys.iter().map(SurfaceKeyJson::from).collect(),
                identity: identity.iter().map(SurfaceKeyJson::from).collect(),
            },
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
    use marrow_run::{
        SurfaceCollectionPage, SurfaceEnumValue, SurfacePageBoundary, SurfacePageCursor,
        SurfaceReadField, SurfaceReadIdentity, SurfaceReadRecord, SurfaceValue,
    };
    use marrow_store::Decimal;
    use marrow_store::cell::CatalogId;
    use marrow_store::key::SavedKey;
    use marrow_store::tree::StoreUid;
    use serde_json::json;

    use crate::surface::{SurfaceCursorJson, SurfacePageJson, SurfaceRecordJson, SurfaceValueJson};

    fn catalog_id(suffix: u8) -> CatalogId {
        CatalogId::new(format!("cat_{suffix:032x}")).expect("catalog id")
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
    fn surface_page_json_preserves_typed_cursor_lineage() {
        let record = SurfaceReadRecord {
            identity: None,
            fields: Vec::new(),
        };
        let cursor = SurfacePageCursor {
            operation_tag: "sha256:op".into(),
            store_uid: StoreUid::from_entropy_bytes([1; 16]),
            catalog_digest: "sha256:catalog".into(),
            source_digest: "sha256:source".into(),
            engine_profile_digest: [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
            boundary: SurfacePageBoundary::IndexIdentity {
                exact_keys: vec![SavedKey::Duration(100), SavedKey::Bytes(vec![0xaa])],
                identity: vec![SavedKey::Instant(-200)],
            },
        };
        let page = SurfaceCollectionPage {
            rows: vec![record],
            next: Some(cursor.clone()),
        };

        let page_json =
            serde_json::to_value(SurfacePageJson::from(&page)).map_err(|error| error.to_string());
        let cursor_json = serde_json::to_value(SurfaceCursorJson::from(&cursor))
            .map_err(|error| error.to_string());
        assert_eq!(
            page_json,
            Ok(json!({
                "rows": [
                    {
                        "identity": null,
                        "fields": []
                    }
                ],
                "next": {
                    "operation_tag": "sha256:op",
                    "store_uid": "store_01010101010101010101010101010101",
                    "catalog_digest": "sha256:catalog",
                    "source_digest": "sha256:source",
                    "engine_profile_digest": "0123456789abcdef",
                    "boundary": {
                        "kind": "index_identity",
                        "exact_keys": [
                            { "kind": "duration", "nanos": "100" },
                            { "kind": "bytes", "value_b64": "qg==" }
                        ],
                        "identity": [
                            { "kind": "instant", "nanos_since_epoch": "-200" }
                        ]
                    }
                }
            }))
        );
        assert_eq!(
            cursor_json.as_ref().ok(),
            page_json.as_ref().ok().and_then(|value| value.get("next"))
        );
    }

    #[test]
    fn surface_cursor_json_preserves_root_boundaries() {
        let cursor = SurfacePageCursor {
            operation_tag: "sha256:root".into(),
            store_uid: StoreUid::from_entropy_bytes([2; 16]),
            catalog_digest: "sha256:catalog".into(),
            source_digest: "sha256:source".into(),
            engine_profile_digest: [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11],
            boundary: SurfacePageBoundary::RootIdentity(vec![
                SavedKey::Int(i64::MIN),
                SavedKey::Str("edition".into()),
            ]),
        };

        assert_eq!(
            serde_json::to_value(SurfaceCursorJson::from(&cursor))
                .map_err(|error| error.to_string()),
            Ok(json!({
                "operation_tag": "sha256:root",
                "store_uid": "store_02020202020202020202020202020202",
                "catalog_digest": "sha256:catalog",
                "source_digest": "sha256:source",
                "engine_profile_digest": "aabbccddeeff0011",
                "boundary": {
                    "kind": "root_identity",
                    "identity": [
                        { "kind": "int", "value": "-9223372036854775808" },
                        { "kind": "string", "value": "edition" }
                    ]
                }
            }))
        );
    }
}
