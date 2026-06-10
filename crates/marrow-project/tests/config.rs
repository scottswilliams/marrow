use marrow_project::{
    ConfigErrorKind, ConfigPathField, ConfigPathViolation, StoreBackend, parse_config,
};

#[test]
fn parses_the_documented_example_config() {
    let json = r#"{
        "sourceRoots": ["src"],
        "run": { "defaultEntry": "shelf::sample::main" },
        "store": { "backend": "native", "dataDir": ".marrow/data" },
        "tests": ["tests/**/*.mw"]
    }"#;
    let config = parse_config(json).expect("valid config");
    assert_eq!(config.source_roots, ["src"]);
    assert_eq!(config.default_entry.as_deref(), Some("shelf::sample::main"));
    let store = config.store.expect("store");
    assert_eq!(store.backend, StoreBackend::Native);
    assert_eq!(store.data_dir.as_deref(), Some(".marrow/data"));
    assert_eq!(config.tests, ["tests/**/*.mw"]);
}

#[test]
fn fills_optional_fields_with_defaults() {
    let config = parse_config(r#"{ "sourceRoots": ["src", "lib"] }"#).expect("valid config");
    assert_eq!(config.source_roots, ["src", "lib"]);
    assert_eq!(config.default_entry, None);
    assert_eq!(config.store, None);
    assert!(config.tests.is_empty());
}

#[test]
fn accepts_the_memory_backend() {
    let config = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#)
        .expect("valid config");
    let store = config.store.expect("store");
    assert_eq!(store.backend, StoreBackend::Memory);
    assert_eq!(store.data_dir, None);
}

#[test]
fn rejects_missing_source_roots() {
    let error = parse_config(r#"{ "tests": ["t.mw"] }"#).expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::MissingSourceRoots);
}

#[test]
fn rejects_empty_source_roots() {
    let error = parse_config(r#"{ "sourceRoots": [] }"#).expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::EmptySourceRoots);
}

#[test]
fn rejects_unknown_store_backend() {
    let error = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "postgres" } }"#)
        .expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(
        error.kind,
        ConfigErrorKind::UnknownStoreBackend {
            backend: "postgres".to_string()
        }
    );
}

#[test]
fn rejects_native_store_without_data_dir() {
    let error = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "native" } }"#)
        .expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::NativeStoreMissingDataDir);

    let error = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "" } }"#,
    )
    .expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::NativeStoreEmptyDataDir);
}

#[test]
fn rejects_path_entries_that_escape_the_project_root() {
    for (json, field, value, reason) in [
        (
            r#"{ "sourceRoots": [""] }"#,
            ConfigPathField::SourceRootsEntry,
            "",
            ConfigPathViolation::Empty,
        ),
        (
            r#"{ "sourceRoots": ["/etc"] }"#,
            ConfigPathField::SourceRootsEntry,
            "/etc",
            ConfigPathViolation::Absolute,
        ),
        (
            r#"{ "sourceRoots": ["../other"] }"#,
            ConfigPathField::SourceRootsEntry,
            "../other",
            ConfigPathViolation::ParentDir,
        ),
        (
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "/var/data" } }"#,
            ConfigPathField::DataDir,
            "/var/data",
            ConfigPathViolation::Absolute,
        ),
        (
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "../data" } }"#,
            ConfigPathField::DataDir,
            "../data",
            ConfigPathViolation::ParentDir,
        ),
        (
            r#"{ "sourceRoots": ["src"], "tests": ["../tests/*.mw"] }"#,
            ConfigPathField::TestsEntry,
            "../tests/*.mw",
            ConfigPathViolation::ParentDir,
        ),
        (
            r#"{ "sourceRoots": ["src"], "tests": ["/abs/tests"] }"#,
            ConfigPathField::TestsEntry,
            "/abs/tests",
            ConfigPathViolation::Absolute,
        ),
    ] {
        let error = parse_config(json).expect_err("should reject");
        assert_eq!(error.code, "config.invalid", "{json}");
        assert_eq!(
            error.kind,
            ConfigErrorKind::InvalidPath {
                field,
                value: value.to_string(),
                reason
            }
        );
    }
}

#[test]
fn rejects_unknown_top_level_keys() {
    let error = parse_config(r#"{ "sourceRoots": ["src"], "globals": ["^x"] }"#)
        .expect_err("should reject unknown keys");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::InvalidJson);
    assert_eq!(
        error.message,
        "unknown field `globals`, expected one of `sourceRoots`, `run`, `store`, `tests` at line 1 column 35"
    );
}

#[test]
fn rejects_non_object_config_shapes() {
    for json in [
        "[]",
        r#"[["src"]]"#,
        r#"{ "sourceRoots": ["src"], "run": ["main"] }"#,
        r#"{ "sourceRoots": ["src"], "store": ["native", "db"] }"#,
    ] {
        let error = parse_config(json).expect_err("should reject non-object config shape");
        assert_eq!(error.code, "config.invalid", "{json}");
        assert_eq!(error.kind, ConfigErrorKind::InvalidJson, "{json}");
    }
}

#[test]
fn rejects_malformed_json() {
    let error = parse_config("{ not json").expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::InvalidJson);
}
