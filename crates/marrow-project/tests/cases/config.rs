use marrow_project::{
    ConfigErrorKind, ConfigPathField, ConfigPathViolation, StoreBackend, parse_config,
};

#[test]
fn parses_the_documented_example_config() {
    let json = r#"{
        "sourceRoots": ["src"],
        "run": { "defaultEntry": "shelf::sample::main" },
        "store": { "backend": "native", "dataDir": ".marrow/data" },
        "tests": ["tests"]
    }"#;
    let config = parse_config(json).expect("valid config");
    assert_eq!(config.source_roots, ["src"]);
    assert_eq!(config.default_entry.as_deref(), Some("shelf::sample::main"));
    assert_eq!(config.store.backend, StoreBackend::Native);
    assert_eq!(config.store.data_dir.as_deref(), Some(".marrow/data"));
    assert_eq!(config.tests, ["tests"]);
}

#[test]
fn fills_optional_run_and_tests_with_defaults() {
    let config =
        parse_config(r#"{ "sourceRoots": ["src", "lib"], "store": { "backend": "memory" } }"#)
            .expect("valid config");
    assert_eq!(config.source_roots, ["src", "lib"]);
    assert_eq!(config.default_entry, None);
    assert_eq!(config.store.backend, StoreBackend::Memory);
    assert_eq!(config.store.data_dir, None);
    assert!(config.tests.is_empty());
}

#[test]
fn accepts_the_memory_backend() {
    let config = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#)
        .expect("valid config");
    assert_eq!(config.store.backend, StoreBackend::Memory);
    assert_eq!(config.store.data_dir, None);
}

#[test]
fn rejects_missing_source_roots() {
    let error = parse_config(r#"{ "tests": ["t.mw"] }"#).expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::MissingSourceRoots);
}

#[test]
fn rejects_empty_source_roots() {
    let error = parse_config(r#"{ "sourceRoots": [], "store": { "backend": "memory" } }"#)
        .expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::EmptySourceRoots);
}

#[test]
fn rejects_missing_store_block() {
    let error = parse_config(r#"{ "sourceRoots": ["src"] }"#).expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::MissingStore);
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
            r#"{ "sourceRoots": [""], "store": { "backend": "memory" } }"#,
            ConfigPathField::SourceRootsEntry,
            "",
            ConfigPathViolation::Empty,
        ),
        (
            r#"{ "sourceRoots": ["/etc"], "store": { "backend": "memory" } }"#,
            ConfigPathField::SourceRootsEntry,
            "/etc",
            ConfigPathViolation::Absolute,
        ),
        (
            r#"{ "sourceRoots": ["../other"], "store": { "backend": "memory" } }"#,
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
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["../tests"] }"#,
            ConfigPathField::TestsEntry,
            "../tests",
            ConfigPathViolation::ParentDir,
        ),
        (
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["/abs/tests"] }"#,
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
fn rejects_test_entries_with_glob_metacharacters() {
    for value in [
        "tests/*.mw",
        "tests/?_test.mw",
        "tests/[unit].mw",
        "tests/unit].mw",
        "tests/{unit}.mw",
        "tests/unit}.mw",
    ] {
        let json = format!(
            r#"{{ "sourceRoots": ["src"], "store": {{ "backend": "memory" }}, "tests": ["{value}"] }}"#
        );
        let error = parse_config(&json).expect_err("should reject glob-like test entry");
        assert_eq!(error.code, "config.invalid", "{value}");
        assert_eq!(
            error.kind,
            ConfigErrorKind::InvalidPath {
                field: ConfigPathField::TestsEntry,
                value: value.to_string(),
                reason: ConfigPathViolation::GlobMetacharacter
            }
        );
    }
}

#[test]
fn rejects_a_tests_entry_overlapping_a_source_root() {
    // Test files live outside the source roots — they are scripts, not library
    // modules. A `tests` entry that equals, sits under, or contains a source
    // root would otherwise load library modules and run their `pub fn`s as tests.
    for (json, test_entry, source_root) in [
        (
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["src"] }"#,
            "src",
            "src",
        ),
        (
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["src/cases"] }"#,
            "src/cases",
            "src",
        ),
        (
            r#"{ "sourceRoots": ["src/lib"], "store": { "backend": "memory" }, "tests": ["src"] }"#,
            "src",
            "src/lib",
        ),
        (
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["./src/smoke.mw"] }"#,
            "./src/smoke.mw",
            "src",
        ),
    ] {
        let error = parse_config(json).expect_err("should reject overlapping tests entry");
        assert_eq!(error.code, "config.invalid", "{json}");
        assert_eq!(
            error.kind,
            ConfigErrorKind::TestsOverlapSourceRoot {
                test_entry: test_entry.to_string(),
                source_root: source_root.to_string(),
            },
            "{json}"
        );
    }
}

#[test]
fn accepts_tests_paths_disjoint_from_source_roots() {
    // A `tests` entry that shares a prefix segment but is not a path-component
    // ancestor of any source root (`source` vs `src`) is disjoint and valid.
    let config = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests", "source"] }"#,
    )
    .expect("disjoint tests paths are valid");
    assert_eq!(config.tests, ["tests", "source"]);
}

#[test]
fn rejects_unknown_top_level_keys() {
    let error = parse_config(r#"{ "sourceRoots": ["src"], "globals": ["^x"] }"#)
        .expect_err("should reject unknown keys");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::InvalidJson);
}

#[test]
fn rejects_accepted_catalog_config_key() {
    let error = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "acceptedCatalog": "other.json" }"#,
    )
    .expect_err("should reject acceptedCatalog");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ConfigErrorKind::InvalidJson);
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

#[test]
fn rejects_hostile_config_json_families() {
    for (label, json) in [
        (
            "duplicate top-level key",
            r#"{ "sourceRoots": ["src"], "sourceRoots": ["other"], "store": { "backend": "memory" } }"#,
        ),
        ("type-wrong source root list", r#"{ "sourceRoots": [1] }"#),
        (
            "type-wrong store backend",
            r#"{ "sourceRoots": ["src"], "store": { "backend": 7 } }"#,
        ),
        (
            "null byte default entry",
            "{ \"sourceRoots\": [\"src\"], \"store\": { \"backend\": \"memory\" }, \"run\": { \"defaultEntry\": \"app::main\\u0000\" } }",
        ),
        (
            "null byte source root",
            "{ \"sourceRoots\": [\"src\\u0000evil\"], \"store\": { \"backend\": \"memory\" } }",
        ),
        (
            "null byte store backend",
            "{ \"sourceRoots\": [\"src\"], \"store\": { \"backend\": \"native\\u0000\", \"dataDir\": \"data\" } }",
        ),
        (
            "null byte test path",
            "{ \"sourceRoots\": [\"src\"], \"store\": { \"backend\": \"memory\" }, \"tests\": [\"tests\\u0000/unit.mw\"] }",
        ),
        (
            "null byte native data dir",
            "{ \"sourceRoots\": [\"src\"], \"store\": { \"backend\": \"native\", \"dataDir\": \"data\\u0000dir\" } }",
        ),
    ] {
        let error = parse_config(json).expect_err(label);
        assert_eq!(error.code, "config.invalid", "{label}");
        assert_eq!(error.kind, ConfigErrorKind::InvalidJson, "{label}");
    }
}
