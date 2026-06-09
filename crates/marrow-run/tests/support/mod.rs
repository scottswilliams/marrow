//! Shared runtime-evaluation harness for the `marrow-run` integration tests.
//!
//! Every runtime test drives the real `check_project` pipeline over a throwaway
//! on-disk project, commits its catalog, and runs an entry against a tree store.
//! This module is the single owner of that setup: snippet checking, catalog
//! commit, saved-path construction over checked facts, store reads/writes, and
//! the run/error oracles.
//!
//! Each test binary includes this module, so not every binary exercises every
//! helper; the crate-wide `dead_code` allowance keeps the shared surface intact.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{
    CheckedProgram, CheckedRuntimeProgram, ProjectConfig, ResourceId, ResourceMemberId,
    ResourceMemberKind, check_project,
};
use marrow_run::{CheckedEntryCall, Host, RunOutput, Value, WriteDataSegment};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{SavedValue, ScalarType, decode_value, encode_value};
use marrow_syntax::parse_source;

/// Check one snippet as module `test` unless the snippet declares its own module.
pub fn checked_program(source: &str) -> CheckedRuntimeProgram {
    checked_program_with_imports(source, &[])
}

pub fn checked_program_with_imports(source: &str, imports: &[&str]) -> CheckedRuntimeProgram {
    let (path, text) = checked_source_file(source, imports);
    checked_program_files(&[(path, text)])
}

pub fn checker_rejects(source: &str, code: &str) {
    checker_rejects_with_imports(source, &[], code);
}

pub fn checker_rejects_with_imports(source: &str, imports: &[&str], code: &str) {
    let (path, text) = checked_source_file(source, imports);
    let report = check_source_files(&[(path, text)]).0;
    assert_checker_code(&report, code);
}

pub fn checker_rejects_sources(sources: &[&str], code: &str) {
    let files: Vec<(PathBuf, String)> = sources
        .iter()
        .map(|source| checked_source_file(source, &[]))
        .collect();
    let report = check_source_files(&files).0;
    assert_checker_code(&report, code);
}

pub use marrow_check::test_support::assert_checker_code;

pub fn checked_program_modules(sources: &[&str]) -> CheckedRuntimeProgram {
    let files: Vec<(PathBuf, String)> = sources
        .iter()
        .map(|source| checked_source_file(source, &[]))
        .collect();
    checked_program_files(&files)
}

pub fn checked_source_file(source: &str, imports: &[&str]) -> (PathBuf, String) {
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
    if let Some(module) = &parsed.file.module {
        assert!(
            imports.is_empty(),
            "test helper imports must be written in declared-module source"
        );
        return (module_source_path(&module.name), source.to_string());
    }

    let mut text = String::from("module test\n");
    for import in imports {
        text.push_str("use ");
        text.push_str(import);
        text.push('\n');
    }
    text.push('\n');
    text.push_str(source);
    (PathBuf::from("src/test.mw"), text)
}

pub fn checked_program_files(files: &[(PathBuf, String)]) -> CheckedRuntimeProgram {
    let root = tempfile::tempdir().expect("create checked-program project");
    for (relative, source) in files {
        write_temp_source(root.path(), relative, source);
    }
    let config = test_project_config();
    let (report, program) = check_project(root.path(), &config).expect("check project");
    assert!(
        !report.has_errors(),
        "runtime tests require a clean checked program: {:#?}",
        report.diagnostics
    );
    let program = commit_catalog(root.path(), &config, program);
    program.runtime()
}

/// Check and commit one snippet, returning both the committed checked program and
/// its runtime projection. The checked program carries the saved-place and index
/// facts an index rebuild needs; the runtime projection drives entries.
pub fn committed_program_and_runtime(source: &str) -> (CheckedProgram, CheckedRuntimeProgram) {
    let (path, text) = checked_source_file(source, &[]);
    let root = tempfile::tempdir().expect("create checked-program project");
    write_temp_source(root.path(), &path, &text);
    let config = test_project_config();
    let (report, program) = check_project(root.path(), &config).expect("check project");
    assert!(
        !report.has_errors(),
        "runtime tests require a clean checked program: {:#?}",
        report.diagnostics
    );
    let program = commit_catalog(root.path(), &config, program);
    let runtime = program.runtime();
    (program, runtime)
}

pub fn check_source_files(
    files: &[(PathBuf, String)],
) -> (marrow_check::CheckReport, CheckedRuntimeProgram) {
    let root = tempfile::tempdir().expect("create checked-program project");
    for (relative, source) in files {
        write_temp_source(root.path(), relative, source);
    }
    let config = test_project_config();
    let (report, program) = check_project(root.path(), &config).expect("check project");
    (report, program.runtime())
}

pub fn test_project_config() -> ProjectConfig {
    ProjectConfig {
        source_roots: vec!["src".into()],
        default_entry: None,
        store: None,
        tests: Vec::new(),
        accepted_catalog: "marrow.catalog.json".into(),
    }
}

pub fn commit_catalog(
    root: &Path,
    config: &ProjectConfig,
    program: CheckedProgram,
) -> CheckedProgram {
    match marrow_check::commit_pending_identity(root, config, &program)
        .expect("commit runtime test catalog")
    {
        Some((report, program)) => {
            assert!(
                !report.has_errors(),
                "committed runtime test catalog must check cleanly: {:#?}",
                report.diagnostics
            );
            program
        }
        None => program,
    }
}

pub fn write_temp_source(root: &Path, relative: &Path, source: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("source parent")).expect("create source dir");
    fs::write(path, source).expect("write source");
}

pub fn module_source_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from("src");
    let mut rest = name;
    while let Some((segment, tail)) = rest.split_once("::") {
        path.push(segment);
        rest = tail;
    }
    path.push(rest);
    path.set_extension("mw");
    path
}

pub fn empty_store() -> TreeStore {
    TreeStore::memory()
}

#[macro_export]
macro_rules! checked_entry {
    ($program:expr, $entry:expr $(, $arg:expr)* $(,)?) => {
        $crate::support::checked_entry_call($program, $entry, vec![$($arg),*])
    };
}

pub fn checked_entry_call<'p>(
    program: &'p CheckedRuntimeProgram,
    entry: &str,
    args: Vec<Value>,
) -> CheckedEntryCall<'p> {
    CheckedEntryCall::new(program, entry, args).expect("checked entry call")
}

pub fn rejected_entry_call(
    program: &CheckedRuntimeProgram,
    entry: &str,
    args: Vec<Value>,
) -> marrow_run::RuntimeError {
    CheckedEntryCall::new(program, entry, args).expect_err("entry call is rejected")
}

pub fn catalog_id(raw: &Option<String>) -> CatalogId {
    CatalogId::new(
        raw.clone()
            .expect("accepted catalog id is present in checked facts"),
    )
    .expect("checked catalog id is usable in tests")
}

pub fn store_catalog_id(program: &CheckedRuntimeProgram, root: &str) -> CatalogId {
    let store = program
        .facts()
        .stores()
        .iter()
        .find(|store| store.root == root)
        .unwrap_or_else(|| panic!("store root `{root}` is present in checked facts"));
    catalog_id(&store.catalog_id)
}

pub fn index_catalog_id(program: &CheckedRuntimeProgram, root: &str, name: &str) -> CatalogId {
    let store = program
        .facts()
        .stores()
        .iter()
        .find(|store| store.root == root)
        .unwrap_or_else(|| panic!("store root `{root}` is present in checked facts"));
    let index = program
        .facts()
        .store_indexes()
        .iter()
        .find(|index| index.store == store.id && index.name == name)
        .unwrap_or_else(|| panic!("index `{name}` is present in checked facts"));
    catalog_id(&index.catalog_id)
}

pub fn store_resource(program: &CheckedRuntimeProgram, root: &str) -> ResourceId {
    program
        .facts()
        .stores()
        .iter()
        .find(|store| store.root == root)
        .unwrap_or_else(|| panic!("store root `{root}` is present in checked facts"))
        .resource
}

pub fn enum_catalog_id(program: &CheckedRuntimeProgram, name: &str) -> CatalogId {
    let enum_fact = program
        .facts()
        .enums()
        .iter()
        .find(|enum_fact| enum_fact.name == name)
        .unwrap_or_else(|| panic!("enum `{name}` is present in checked facts"));
    catalog_id(&enum_fact.catalog_id)
}

pub fn enum_member_catalog_id(
    program: &CheckedRuntimeProgram,
    enum_name: &str,
    member_name: &str,
) -> CatalogId {
    let enum_fact = program
        .facts()
        .enums()
        .iter()
        .find(|enum_fact| enum_fact.name == enum_name)
        .unwrap_or_else(|| panic!("enum `{enum_name}` is present in checked facts"));
    let member = program
        .facts()
        .enum_members()
        .iter()
        .find(|member| member.enum_id == enum_fact.id && member.name == member_name)
        .unwrap_or_else(|| {
            panic!("enum member `{enum_name}::{member_name}` is present in checked facts")
        });
    catalog_id(&member.catalog_id)
}

pub fn member_fact(
    program: &CheckedRuntimeProgram,
    resource: ResourceId,
    parent: Option<ResourceMemberId>,
    name: &str,
) -> (ResourceMemberId, ResourceMemberKind, CatalogId) {
    let member = program
        .facts()
        .resource_members()
        .iter()
        .find(|member| {
            member.resource == resource && member.parent == parent && member.name == name
        })
        .unwrap_or_else(|| panic!("resource member `{name}` is present in checked facts"));
    (member.id, member.kind, catalog_id(&member.catalog_id))
}

pub fn data_path(
    program: &CheckedRuntimeProgram,
    root: &str,
    members: &[&str],
) -> Vec<DataPathSegment> {
    let resource = store_resource(program, root);
    let mut parent = None;
    let mut path = Vec::new();
    for name in members {
        let (member, _kind, catalog_id) = member_fact(program, resource, parent, name);
        path.push(DataPathSegment::Member(catalog_id));
        parent = Some(member);
    }
    path
}

pub fn keyed_data_path(
    program: &CheckedRuntimeProgram,
    root: &str,
    keyed_layers: &[(&str, Vec<SavedKey>)],
    members: &[&str],
) -> Vec<DataPathSegment> {
    let resource = store_resource(program, root);
    let mut parent = None;
    let mut path = Vec::new();
    for (name, keys) in keyed_layers {
        let (member, _kind, catalog_id) = member_fact(program, resource, parent, name);
        path.push(DataPathSegment::Member(catalog_id));
        path.extend(keys.iter().cloned().map(DataPathSegment::Key));
        parent = Some(member);
    }
    for name in members {
        let (member, _kind, catalog_id) = member_fact(program, resource, parent, name);
        path.push(DataPathSegment::Member(catalog_id));
        parent = Some(member);
    }
    path
}

pub fn write_data_path(path: Vec<DataPathSegment>) -> Vec<WriteDataSegment> {
    path.into_iter()
        .map(|segment| match segment {
            DataPathSegment::Member(member) => {
                WriteDataSegment::Member(member.as_str().to_string())
            }
            DataPathSegment::Key(key) => WriteDataSegment::Key(key),
        })
        .collect()
}

pub fn write_data_value(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    root: &str,
    identity: &[SavedKey],
    path: &[DataPathSegment],
    value: SavedValue,
) {
    store
        .write_data_value(
            &store_catalog_id(program, root),
            identity,
            path,
            encode_value(&value).expect("test value encodes"),
        )
        .expect("typed data write succeeds");
}

pub fn read_data_value(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    root: &str,
    identity: &[SavedKey],
    path: &[DataPathSegment],
    ty: ScalarType,
) -> Option<SavedValue> {
    let bytes = read_data_bytes(program, store, root, identity, path)?;
    decode_value(&bytes, ty)
}

pub fn read_data_bytes(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    root: &str,
    identity: &[SavedKey],
    path: &[DataPathSegment],
) -> Option<Vec<u8>> {
    store
        .read_data_value(&store_catalog_id(program, root), identity, path)
        .expect("typed data read succeeds")
}

/// Run an entry function against an empty store, returning only its value.
pub fn run(call: CheckedEntryCall<'_>) -> Result<Option<Value>, marrow_run::RuntimeError> {
    let store = empty_store();
    marrow_run::run_entry(&store, &call).map(|outcome| outcome.value)
}

/// Run an entry function against an empty store, returning its value and output.
pub fn run_full(call: CheckedEntryCall<'_>) -> Result<RunOutput, marrow_run::RuntimeError> {
    let store = empty_store();
    marrow_run::run_entry(&store, &call)
}

/// Run an entry against a caller-supplied store with no host capabilities, so any
/// effect that needs one (clock, env, log, io, host writes) fails with a capability
/// error. Tests that need a capability use the `*_with_host` variants instead.
pub fn run_entry(
    store: &TreeStore,
    call: CheckedEntryCall<'_>,
) -> Result<RunOutput, marrow_run::RuntimeError> {
    marrow_run::run_entry(store, &call)
}

pub fn assert_identity_value(value: Option<Value>, root: &str, keys: &[SavedKey]) {
    let Some(Value::Identity(identity)) = value else {
        panic!("expected identity value for {root}: {value:?}");
    };
    assert_eq!(identity.root(), root);
    assert_eq!(identity.keys(), keys);
}

pub fn run_entry_with_host(
    store: &TreeStore,
    host: &Host,
    call: CheckedEntryCall<'_>,
) -> Result<RunOutput, marrow_run::RuntimeError> {
    marrow_run::run_entry_with_host(store, host, &call)
}

/// The single oracle for a failed runtime entry call: assert the run produced a
/// runtime error carrying exactly `code`. Generic over the success payload so it
/// applies to `run`, `run_full`, `run_entry`, and `eval_source` results alike.
pub fn assert_run_error<T: std::fmt::Debug>(
    result: Result<T, marrow_run::RuntimeError>,
    code: &str,
) {
    match result {
        Err(error) => assert_eq!(error.code, code, "{error:?}"),
        Ok(value) => panic!("expected runtime error {code}, got {value:?}"),
    }
}

/// Run an entry against an empty store, expecting failure, and return the error for
/// tests that inspect its message or span beyond the code.
pub fn run_expecting_error(call: CheckedEntryCall<'_>) -> marrow_run::RuntimeError {
    run(call).expect_err("expected runtime error")
}

pub fn error_throw_fields(error: &marrow_run::RuntimeError) -> (&str, &str) {
    let Some(Value::Resource(fields)) = error.throw.as_deref() else {
        panic!("expected Error resource throw: {error:?}");
    };
    (
        resource_str_field(fields, "code"),
        resource_str_field(fields, "message"),
    )
}

pub fn resource_str_field<'a>(fields: &'a [(String, Value)], name: &str) -> &'a str {
    let Some((_, value)) = fields.iter().find(|(field, _)| field.as_str() == name) else {
        panic!("expected Error resource field `{name}`: {fields:?}");
    };
    let Value::Str(value) = value else {
        panic!("expected Error resource field `{name}` to be string: {value:?}");
    };
    value
}

/// Evaluate `entry` from a single checked `test` module against an empty store.
pub fn eval_source(
    source: &str,
    entry: &str,
    args: Vec<Value>,
) -> Result<Option<Value>, marrow_run::RuntimeError> {
    let program = checked_program(source);
    run(checked_entry_call(
        &program,
        &format!("test::{entry}"),
        args,
    ))
}

/// The sample's `add` shape: allocate an id, build a local resource field by
/// field, and save it. The runtime snippet fixtures live in the repo-root corpus,
/// so no `.mw` shape is re-declared as an inline string across crates.
pub const BOOK_ADD: &str = include_str!("../../../../fixtures/v01/runtime/books_add.mw");

/// Extract the single `mw` code block from the canonical sample, so the
/// integration test runs the exact published source.
pub fn sample_source() -> String {
    let doc = include_str!("../../../../docs/language/sample.md");
    doc.split("```mw")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("the sample document has an mw code block")
        .to_string()
}

/// A composite-identity store indexed by status. The non-unique index ends with
/// both identity keys, so traversal must descend both levels per entry and
/// reconstruct the full `Id(^enrollments)` (not just the first key component).
pub const ENROLLMENT_STATUS: &str =
    include_str!("../../../../fixtures/v01/runtime/enrollment_status.mw");

/// Iterating a primary keyed root yields identities. Two-name loops pair the
/// identity with the materialized record value. The trailing blank line lets a
/// test append its own entries after the resource block.
pub const BOOK_PRIMARY_SCHEMA: &str =
    include_str!("../../../../fixtures/v01/runtime/books_primary_schema.mw");

pub const BOOK_TAGS_SCHEMA: &str =
    include_str!("../../../../fixtures/v01/runtime/books_tags_schema.mw");

pub const BOOK_SHELF_SCHEMA: &str =
    include_str!("../../../../fixtures/v01/runtime/books_shelf_schema.mw");

pub const BOOK_ISBN_SCHEMA: &str =
    include_str!("../../../../fixtures/v01/runtime/books_isbn_schema.mw");

pub const BOOK_SHELF_INDEX_SCHEMA: &str =
    include_str!("../../../../fixtures/v01/runtime/books_shelf_index_schema.mw");

/// `count(path)` over the four presence shapes: a scalar field, a child-bearing
/// layer, and absent paths.
pub const BOOK_COUNT: &str = include_str!("../../../../fixtures/v01/runtime/books_count.mw");
