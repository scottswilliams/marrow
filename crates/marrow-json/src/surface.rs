use marrow_run::{
    SurfaceEnumValue, SurfaceReadField, SurfaceReadIdentity, SurfaceReadRecord, SurfaceValue,
};
use marrow_store::key::SavedKey;
use serde::{Deserialize, Serialize};

mod request;
pub use request::{
    DecodedSurfacePageRequest, DecodedSurfacePointRequest, DecodedSurfaceUniqueLookupRequest,
    SurfacePageRequestJson, SurfacePointRequestJson, SurfaceUniqueLookupRequestJson,
};

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
        SurfaceReadOperationKind, check_project,
    };
    use marrow_run::{
        SURFACE_CURSOR, SURFACE_REQUEST, SurfaceCollectionRead, SurfaceEnumValue, SurfaceNodeRead,
        SurfaceReadError, SurfaceReadField, SurfaceReadIdentity, SurfaceReadOperationRef,
        SurfaceReadRecord, SurfaceValue,
    };
    use marrow_store::Decimal;
    use marrow_store::cell::CatalogId;
    use marrow_store::key::{SavedKey, encode_identity_index_key, encode_identity_payload};
    use marrow_store::tree::{
        DataPathSegment, StoreUid, TreeEnumMember, TreeStore, encode_tree_enum_member,
    };
    use marrow_store::value::{SavedValue, encode_value};
    use serde_json::json;

    use crate::surface::{
        SurfaceArgumentJson, SurfaceCursorBoundaryJson, SurfaceCursorJson, SurfaceIdentityJson,
        SurfaceKeyJson, SurfacePageJson, SurfacePageRequestJson, SurfacePointRequestJson,
        SurfaceRecordJson, SurfaceValueJson,
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
        SurfacePageRequestJson {
            exact_keys: vec![
                SurfaceArgumentJson::Enum {
                    enum_catalog_id: enum_catalog_id(program, "Status").as_str().into(),
                    member_catalog_id: enum_member_catalog_id(program, "Status", "published")
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

    fn assert_surface_error<T: std::fmt::Debug>(result: Result<T, SurfaceReadError>, code: &str) {
        match result {
            Err(error) => assert_eq!(error.code(), code, "{error:?}"),
            Ok(value) => panic!("expected surface error {code}, got {value:?}"),
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
