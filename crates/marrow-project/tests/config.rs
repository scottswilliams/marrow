use marrow_project::{StoreBackend, parse_config};

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
    assert!(error.message.contains("sourceRoots"), "{}", error.message);
}

#[test]
fn rejects_empty_source_roots() {
    let error = parse_config(r#"{ "sourceRoots": [] }"#).expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
}

#[test]
fn rejects_unknown_store_backend() {
    let error = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "postgres" } }"#)
        .expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert!(error.message.contains("postgres"), "{}", error.message);
}

#[test]
fn rejects_native_store_without_data_dir() {
    // The native backend cannot open without a data directory, so a native store
    // missing (or with an empty) `dataDir` is invalid here, not later at open time.
    let error = parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "native" } }"#)
        .expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert!(error.message.contains("dataDir"), "{}", error.message);

    let error = parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "" } }"#,
    )
    .expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
    assert!(error.message.contains("dataDir"), "{}", error.message);
}

#[test]
fn rejects_path_entries_that_escape_the_project_root() {
    // sourceRoots, dataDir, and tests entries are joined onto the project root,
    // so an empty, absolute, or `..`-bearing value would escape it. Each is
    // rejected with a message naming the offending value.
    for (json, offender) in [
        (r#"{ "sourceRoots": [""] }"#, ""),
        (r#"{ "sourceRoots": ["/etc"] }"#, "/etc"),
        (r#"{ "sourceRoots": ["../other"] }"#, "../other"),
        (
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "/var/data" } }"#,
            "/var/data",
        ),
        (
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "../data" } }"#,
            "../data",
        ),
        (
            r#"{ "sourceRoots": ["src"], "tests": ["../tests/*.mw"] }"#,
            "../tests/*.mw",
        ),
        (
            r#"{ "sourceRoots": ["src"], "tests": ["/abs/tests"] }"#,
            "/abs/tests",
        ),
    ] {
        let error = parse_config(json).expect_err("should reject");
        assert_eq!(error.code, "config.invalid", "{json}");
        assert!(
            error.message.contains(offender)
                || (offender.is_empty() && error.message.contains("empty")),
            "message {:?} should name offender {:?}",
            error.message,
            offender
        );
    }
}

#[test]
fn rejects_unknown_top_level_keys() {
    let error = parse_config(r#"{ "sourceRoots": ["src"], "globals": ["^x"] }"#)
        .expect_err("should reject unknown keys");
    assert_eq!(error.code, "config.invalid");
}

#[test]
fn rejects_malformed_json() {
    let error = parse_config("{ not json").expect_err("should reject");
    assert_eq!(error.code, "config.invalid");
}
