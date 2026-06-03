//! Evaluate pure scalar functions: arithmetic, comparison, logical operators,
//! locals, and conditionals over integer and boolean values.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use marrow_check::{
    CheckedProgram, CheckedRuntimeProgram, FileId, ProjectConfig, ResourceId, ResourceMemberId,
    ResourceMemberKind, check_project,
};
use marrow_run::{
    CheckedEntryCall, Host, RUN_ABSENT, RUN_ASSERT, RUN_CAPABILITY, RUN_DECIMAL_OVERFLOW,
    RUN_DIVIDE_BY_ZERO, RUN_OVERFLOW, RUN_TRAVERSAL, RUN_TYPE, RUN_UNCAUGHT_THROW,
    RUN_UNKNOWN_FUNCTION, RUN_UNSUPPORTED, RunOutput, Value, WriteDataSegment, WriteTarget,
};
use marrow_store::Decimal;
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{DataPathSegment, TreeStore, decode_tree_enum_member};
use marrow_store::value::{SavedValue, ScalarType, decode_value, encode_value};
use marrow_syntax::parse_source;

/// Check one snippet as module `test` unless the snippet declares its own module.
fn checked_program(source: &str) -> CheckedRuntimeProgram {
    checked_program_with_imports(source, &[])
}

fn checked_program_typed(source: &str) -> CheckedRuntimeProgram {
    checked_program(source)
}

fn checked_program_with_imports(source: &str, imports: &[&str]) -> CheckedRuntimeProgram {
    let (path, text) = checked_source_file(source, imports);
    checked_program_files(&[(path, text)])
}

fn checker_rejects(source: &str, code: &str) {
    checker_rejects_with_imports(source, &[], code);
}

fn checker_rejects_with_imports(source: &str, imports: &[&str], code: &str) {
    let (path, text) = checked_source_file(source, imports);
    let report = check_source_files(&[(path, text)]).0;
    assert_checker_code(&report, code);
}

fn checker_rejects_sources(sources: &[&str], code: &str) {
    let files: Vec<(PathBuf, String)> = sources
        .iter()
        .map(|source| checked_source_file(source, &[]))
        .collect();
    let report = check_source_files(&files).0;
    assert_checker_code(&report, code);
}

fn assert_checker_code(report: &marrow_check::CheckReport, code: &str) {
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code),
        "expected checker diagnostic {code}, got {:#?}",
        report.diagnostics
    );
}

fn checked_program_modules(sources: &[&str]) -> CheckedRuntimeProgram {
    let files: Vec<(PathBuf, String)> = sources
        .iter()
        .map(|source| checked_source_file(source, &[]))
        .collect();
    checked_program_files(&files)
}

fn checked_source_file(source: &str, imports: &[&str]) -> (PathBuf, String) {
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
    text.push_str(&public_test_functions(source));
    (PathBuf::from("src/test.mw"), text)
}

fn public_test_functions(source: &str) -> String {
    let mut text = String::new();
    for line in source.lines() {
        if line.starts_with("fn ") {
            text.push_str("pub ");
        }
        text.push_str(line);
        text.push('\n');
    }
    if !source.ends_with('\n') {
        text.pop();
    }
    text
}

fn checked_program_files(files: &[(PathBuf, String)]) -> CheckedRuntimeProgram {
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
    let program = accept_catalog(root.path(), &config, program);
    program.runtime()
}

fn check_source_files(
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

fn test_project_config() -> ProjectConfig {
    ProjectConfig {
        source_roots: vec!["src".into()],
        default_entry: None,
        store: None,
        tests: Vec::new(),
        accepted_catalog: "marrow.catalog.json".into(),
    }
}

fn accept_catalog(root: &Path, config: &ProjectConfig, program: CheckedProgram) -> CheckedProgram {
    match marrow_check::accept_catalog_proposal(root, config, &program)
        .expect("accept runtime test catalog")
    {
        Some((report, program)) => {
            assert!(
                !report.has_errors(),
                "accepted runtime test catalog must check cleanly: {:#?}",
                report.diagnostics
            );
            program
        }
        None => program,
    }
}

#[test]
#[should_panic(expected = "runtime tests require a clean checked program")]
fn runtime_test_helper_rejects_checker_error_programs() {
    checked_program_with_imports(
        "fn stamp(s: string): string\n    return clock::formatDate(clock::parseDate(s))\n",
        &[],
    );
}

fn write_temp_source(root: &Path, relative: &Path, source: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("source parent")).expect("create source dir");
    fs::write(path, source).expect("write source");
}

fn module_source_path(name: &str) -> PathBuf {
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

fn empty_store() -> TreeStore {
    TreeStore::memory()
}

macro_rules! checked_entry {
    ($program:expr, $entry:expr $(, $arg:expr)* $(,)?) => {
        checked_entry_call($program, $entry, vec![$($arg),*])
    };
}

fn checked_entry_call<'p>(
    program: &'p CheckedRuntimeProgram,
    entry: &str,
    args: Vec<Value>,
) -> CheckedEntryCall<'p> {
    CheckedEntryCall::new(program, entry, args).expect("checked entry call")
}

fn rejected_entry_call(
    program: &CheckedRuntimeProgram,
    entry: &str,
    args: Vec<Value>,
) -> marrow_run::RuntimeError {
    CheckedEntryCall::new(program, entry, args).expect_err("entry call is rejected")
}

fn catalog_id(raw: &str) -> CatalogId {
    CatalogId::new(raw.to_string()).expect("checked catalog id is usable in tests")
}

fn store_catalog_id(program: &CheckedRuntimeProgram, root: &str) -> CatalogId {
    let store = program
        .facts()
        .stores()
        .iter()
        .find(|store| store.root == root)
        .unwrap_or_else(|| panic!("store root `{root}` is present in checked facts"));
    catalog_id(&store.catalog_id)
}

fn index_catalog_id(program: &CheckedRuntimeProgram, root: &str, name: &str) -> CatalogId {
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

fn store_resource(program: &CheckedRuntimeProgram, root: &str) -> ResourceId {
    program
        .facts()
        .stores()
        .iter()
        .find(|store| store.root == root)
        .unwrap_or_else(|| panic!("store root `{root}` is present in checked facts"))
        .resource
}

fn enum_catalog_id(program: &CheckedRuntimeProgram, name: &str) -> CatalogId {
    let enum_fact = program
        .facts()
        .enums()
        .iter()
        .find(|enum_fact| enum_fact.name == name)
        .unwrap_or_else(|| panic!("enum `{name}` is present in checked facts"));
    catalog_id(&enum_fact.catalog_id)
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

fn member_fact(
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

fn data_path(
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

fn keyed_data_path(
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

fn write_data_path(path: Vec<DataPathSegment>) -> Vec<WriteDataSegment> {
    path.into_iter()
        .map(|segment| match segment {
            DataPathSegment::Member(member) => {
                WriteDataSegment::Member(member.as_str().to_string())
            }
            DataPathSegment::Key(key) => WriteDataSegment::Key(key),
        })
        .collect()
}

fn write_data_value(
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

fn read_data_value(
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

fn read_data_bytes(
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
fn run(call: CheckedEntryCall<'_>) -> Result<Option<Value>, marrow_run::RuntimeError> {
    let store = empty_store();
    marrow_run::run_entry(&store, &call).map(|outcome| outcome.value)
}

/// Run an entry function against an empty store, returning its value and output.
fn run_full(call: CheckedEntryCall<'_>) -> Result<RunOutput, marrow_run::RuntimeError> {
    let store = empty_store();
    marrow_run::run_entry(&store, &call)
}

fn run_entry(
    store: &TreeStore,
    call: CheckedEntryCall<'_>,
) -> Result<RunOutput, marrow_run::RuntimeError> {
    marrow_run::run_entry(store, &call)
}

fn run_entry_with_host(
    store: &TreeStore,
    host: &Host,
    call: CheckedEntryCall<'_>,
) -> Result<RunOutput, marrow_run::RuntimeError> {
    marrow_run::run_entry_with_host(store, host, &call)
}

fn run_entry_with_debugger(
    store: &TreeStore,
    host: &Host,
    hook: &mut dyn StepHook,
    call: CheckedEntryCall<'_>,
) -> Result<RunOutput, marrow_run::RuntimeError> {
    marrow_run::run_entry_with_debugger(store, host, hook, &call)
}

/// Evaluate `entry` from a single checked `test` module against an empty store.
fn eval_source(
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

#[test]
fn evaluates_arithmetic_over_parameters() {
    assert_eq!(
        eval_source(
            "fn add(a: int, b: int): int\n    return a + b\n",
            "add",
            vec![Value::Int(2), Value::Int(40)]
        ),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn respects_arithmetic_precedence() {
    // 2 + 3 * 4 == 14, not 20.
    assert_eq!(
        eval_source("fn f(): int\n    return 2 + 3 * 4\n", "f", Vec::new()),
        Ok(Some(Value::Int(14)))
    );
}

#[test]
fn evaluates_decimal_literals_and_arithmetic() {
    // Decimal `+`, `*`, and `-` over decimal operands, rendered to text.
    let program = checked_program(
        "pub fn f(): string\n    return $\"{1.5 + 2.5} {1.5 * 2.0} {5.5 - 0.5}\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("4 3 5".into()))
    );
}

#[test]
fn negates_a_decimal() {
    // Unary `-` on a decimal, and a subtraction that produces a negative decimal.
    let program = checked_program("pub fn f(): string\n    return $\"{-1.5} {0.0 - 2.5}\"\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("-1.5 -2.5".into()))
    );
}

#[test]
fn division_yields_a_decimal() {
    // `/` always yields a decimal, even for integer operands (1/2 = 0.5).
    let program =
        checked_program("pub fn f(): string\n    return $\"{1 / 2} {7 / 2} {1.0 / 4.0}\"\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("0.5 3.5 0.25".into()))
    );
}

#[test]
fn decimal_division_rounds_half_even() {
    // 1/3 rounds half-even to 34 significant digits.
    let program = checked_program("pub fn f(): string\n    return $\"{1 / 3}\"\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str(format!("0.{}", "3".repeat(34))))
    );
}

#[test]
fn decimal_multiplication_must_fit_exactly() {
    let program = checked_program(
        "pub fn f(): decimal\n    return 0.123456789012345678 * 0.123456789012345678\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap_err().code,
        RUN_DECIMAL_OVERFLOW
    );
}

#[test]
fn decimal_division_by_zero_is_a_runtime_error() {
    let program = checked_program("pub fn f(): decimal\n    return 1.0 / 0.0\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap_err().code,
        RUN_DIVIDE_BY_ZERO
    );
}

#[test]
fn compares_decimal_values() {
    // Ordering and equality compare by value (1.50 equals 1.5).
    let program = checked_program(
        "pub fn f(): string\n    return $\"{1.5 < 2.0} {1.50 == 1.5} {2.5 > 3.0}\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("true true false".into()))
    );
}

#[test]
fn decimal_round_trips_through_saved_data() {
    // A decimal field saves and loads unchanged.
    let program = checked_program(
        "resource Account at ^accts(id: int)\n\
         \x20   balance: decimal\n\
         \n\
         pub fn seed()\n\
         \x20   ^accts(1).balance = 9.99\n\
         \n\
         pub fn balance(): string\n\
         \x20   return $\"{^accts(1).balance ?? 0.0}\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::balance"))
            .unwrap()
            .value,
        Some(Value::Str("9.99".into()))
    );
}

#[test]
fn evaluates_bytes_literals_and_equality() {
    let program = checked_program(
        "pub fn same(): bool\n    return b\"abc\" == b\"abc\"\n\n\
         pub fn different(): bool\n    return b\"abc\" == b\"abd\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::same")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::different")).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn bytes_escapes_are_decoded() {
    let program = checked_program(
        "pub fn f(): bytes\n    return b\"slash \\\\ quote \\\" line\\n carriage\\r tab\\t hex \\x00\\x7f\\xff café\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Bytes(
            b"slash \\ quote \" line\n carriage\r tab\t hex \x00\x7f\xff caf\xc3\xa9".to_vec()
        ))
    );
}

#[test]
fn malformed_bytes_escapes_are_rejected() {
    for source in [
        "pub fn f(): bytes\n    return b\"\\q\"\n",
        "pub fn f(): bytes\n    return b\"\\x0g\"\n",
        "pub fn f(): bytes\n    return b\"\\x0\"\n",
    ] {
        let program = checked_program(source);
        let result = run(checked_entry!(&program, "test::f"));
        assert!(
            matches!(result, Err(ref error) if error.code == RUN_UNSUPPORTED),
            "{result:?}"
        );
    }
}

#[test]
fn compares_bytes_by_byte_order() {
    let program = checked_program(
        "pub fn f(): bool\n    return b\"a\" < b\"b\"\n\n\
         pub fn g(): bool\n    return b\"ab\" > b\"a\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::g")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn bytes_round_trip_through_saved_data() {
    let program = checked_program(
        "resource Blob at ^blobs(id: int)\n\
         \x20   data: bytes\n\
         \n\
         pub fn seed()\n\
         \x20   ^blobs(1).data = b\"xy\"\n\
         \n\
         pub fn matches(): bool\n\
         \x20   return (^blobs(1).data ?? b\"\") == b\"xy\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::matches"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn converts_string_to_bytes_and_measures_length() {
    let program = checked_program(
        "pub fn short(): int\n    return std::bytes::length(bytes(\"hi\"))\n\n\
         pub fn utf8(): int\n    return std::bytes::length(bytes(\"café\"))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::short")).unwrap(),
        Some(Value::Int(2))
    );
    // `café` is 4 characters but 5 UTF-8 bytes; std::bytes::length counts bytes.
    assert_eq!(
        run(checked_entry!(&program, "test::utf8")).unwrap(),
        Some(Value::Int(5))
    );
}

#[test]
fn bytes_conversion_equals_a_bytes_literal() {
    let program = checked_program("pub fn f(): bool\n    return bytes(\"xy\") == b\"xy\"\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn base64_encodes_with_padding() {
    let program = checked_program(
        "pub fn a(): string\n    return std::bytes::base64Encode(b\"hello\")\n\n\
         pub fn b(): string\n    return std::bytes::base64Encode(b\"a\")\n\n\
         pub fn c(): string\n    return std::bytes::base64Encode(b\"ab\")\n\n\
         pub fn d(): string\n    return std::bytes::base64Encode(b\"abc\")\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::a")).unwrap(),
        Some(Value::Str("aGVsbG8=".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::b")).unwrap(),
        Some(Value::Str("YQ==".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::c")).unwrap(),
        Some(Value::Str("YWI=".into()))
    );
    // An exact 3-byte group needs no padding.
    assert_eq!(
        run(checked_entry!(&program, "test::d")).unwrap(),
        Some(Value::Str("YWJj".into()))
    );
}

#[test]
fn base64_decodes_and_round_trips() {
    let program = checked_program(
        "pub fn known(): bool\n    return std::bytes::base64Decode(\"aGVsbG8=\") == b\"hello\"\n\n\
         pub fn round(): bool\n    return std::bytes::base64Decode(std::bytes::base64Encode(b\"hi there\")) == b\"hi there\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::known")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::round")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn base64_decode_rejects_invalid_text() {
    // Invalid characters, and `=` padding outside the final group.
    let program = checked_program(
        "pub fn bad_chars(): bytes\n    return std::bytes::base64Decode(\"!!!!\")\n\n\
         pub fn early_pad(): bytes\n    return std::bytes::base64Decode(\"AAA=AAAA\")\n",
    );
    assert!(run(checked_entry!(&program, "test::bad_chars")).is_err());
    assert!(run(checked_entry!(&program, "test::early_pad")).is_err());
}

#[test]
fn splits_a_string_and_iterates_the_sequence() {
    // `std::text::split` yields a sequence the `for` loop iterates in order.
    let program = checked_program(
        "pub fn f(): string\n\
         \x20   var result = \"\"\n\
         \x20   for word in std::text::split(\"a,b,c\", \",\")\n\
         \x20       result = result _ word\n\
         \x20   return result\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("abc".into()))
    );
}

#[test]
fn iterates_a_sequence_counting_its_elements() {
    let program = checked_program(
        "pub fn split_count(): int\n\
         \x20   var n = 0\n\
         \x20   for word in std::text::split(\"a,b,c,d\", \",\")\n\
         \x20       n = n + 1\n\
         \x20   return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::split_count")).unwrap(),
        Some(Value::Int(4))
    );
}

#[test]
fn append_on_a_local_sequence_is_checker_rejected() {
    checker_rejects(
        "pub fn grow(): int\n\
         \x20   var order: sequence[int]\n\
         \x20   const first: int = append(order, 10)\n\
         \x20   const second: int = append(order, 20)\n\
         \x20   var total = first * 100 + second * 10 + count(order)\n\
         \x20   for value in order\n\
         \x20       total = total + value\n\
         \x20   return total\n",
        "check.untyped_value",
    );
}

#[test]
fn reads_and_writes_a_local_sequence_by_position() {
    let program = checked_program(
        "pub fn seq_index(): int\n\
         \x20   var xs: sequence[int]\n\
         \x20   xs(1) = 10\n\
         \x20   xs(1) = xs(1) + 5\n\
         \x20   return xs(1)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seq_index")).unwrap(),
        Some(Value::Int(15))
    );
}

#[test]
fn local_keyed_trees_are_checker_rejected() {
    checker_rejects(
        "pub fn keyed(): int\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   var total = count(scores)\n\
         \x20   for value in values(scores)\n\
         \x20       total = total * 10 + value\n\
         \x20   return total + scores(\"p2\")\n",
        "check.untyped_value",
    );
}

#[test]
fn reads_and_writes_a_multi_key_local_tree() {
    let program = checked_program(
        "pub fn keyed(day: date): int\n\
         \x20   var counts(day: date, category: string): int\n\
         \x20   counts(day, \"open\") = 3\n\
         \x20   return counts(day, \"open\")\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed", Value::Date(1))).unwrap(),
        Some(Value::Int(3))
    );
}

#[test]
fn std_math_decimal_helpers() {
    // absDecimal yields a decimal; floor rounds toward negative infinity to an int.
    let program = checked_program(
        "pub fn a(): string\n    return $\"{std::math::absDecimal(-2.5)}\"\n\n\
         pub fn up(): int\n    return std::math::floor(2.7)\n\n\
         pub fn down(): int\n    return std::math::floor(-2.7)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::a")).unwrap(),
        Some(Value::Str("2.5".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::up")).unwrap(),
        Some(Value::Int(2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::down")).unwrap(),
        Some(Value::Int(-3))
    );
}

#[test]
fn formats_and_parses_instants() {
    // An instant round-trips through its canonical UTC text.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatInstant(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("2026-05-28T12:00:00Z".into()))
    );
}

#[test]
fn parse_instant_rejects_invalid_text() {
    let program = checked_program(
        "pub fn f(): instant\n    return std::clock::parseInstant(\"not a time\")\n",
    );
    assert!(run(checked_entry!(&program, "test::f")).is_err());
}

#[test]
fn formats_and_parses_dates() {
    // A date round-trips through its canonical YYYY-MM-DD text (leap day).
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatDate(std::clock::parseDate(\"2024-02-29\"))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("2024-02-29".into()))
    );
}

#[test]
fn formats_and_parses_durations() {
    // A duration round-trips through its canonical PT<seconds>S text.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatDuration(std::clock::parseDuration(\"PT90S\"))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("PT90S".into()))
    );
}

#[test]
fn duration_literals_evaluate_to_their_fixed_spans() {
    // Each unit literal evaluates to the same duration value as parsing its
    // canonical PT<seconds>S text.
    let cases = [
        ("1.second", "PT1S"),
        ("1.minute", "PT60S"),
        ("1.hour", "PT3600S"),
        ("1.day", "PT86400S"),
        ("1.week", "PT604800S"),
        ("2.days", "PT172800S"),
        ("3.hours", "PT10800S"),
    ];
    for (literal, canonical) in cases {
        let program = checked_program(&format!("pub fn f(): duration\n    return {literal}\n"));
        let reference = checked_program(&format!(
            "pub fn f(): duration\n    return std::clock::parseDuration(\"{canonical}\")\n"
        ));
        assert_eq!(
            run(checked_entry!(&program, "test::f")).unwrap(),
            run(checked_entry!(&reference, "test::f")).unwrap(),
            "{literal} should equal duration(\"{canonical}\")"
        );
    }
}

#[test]
fn duration_literal_is_usable_where_a_duration_value_is() {
    // A duration literal flows into `add(instant, duration)` like any duration.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatInstant(std::clock::add(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"), 1.hour))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("2026-05-28T13:00:00Z".into()))
    );
}

#[test]
fn clock_add_offsets_an_instant_by_a_duration() {
    // add(instant, duration): one hour after noon UTC is 13:00.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatInstant(std::clock::add(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"), std::clock::parseDuration(\"PT3600S\")))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("2026-05-28T13:00:00Z".into()))
    );
}

#[test]
fn clock_today_reads_the_host_clock_capability() {
    // `today()` is the host clock's UTC calendar date.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatDate(std::clock::today())\n",
    );
    let store = TreeStore::memory();
    // 2023-11-14T22:13:20Z.
    let host = Host::new().with_clock(1_700_000_000_000_000_000);
    let outcome =
        run_entry_with_host(&store, &host, checked_entry!(&program, "test::f")).expect("today");
    assert_eq!(outcome.value, Some(Value::Str("2023-11-14".into())));
}

#[test]
fn clock_today_without_a_clock_capability_is_a_capability_error() {
    let program = checked_program("fn t(): date\n    return std::clock::today()\n");
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::t"));
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

#[test]
fn a_date_round_trips_through_saved_data() {
    // A `date` value saves and loads through a managed field write and read.
    let program = checked_program(
        "resource Event at ^events(id: int)\n    on: date\n\nfn record(id: int, text: string)\n    ^events(id).on = std::clock::parseDate(text)\n\nfn dateOf(id: int): string\n    return std::clock::formatDate(^events(id).on ?? std::clock::parseDate(\"1970-01-01\"))\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::record",
            Value::Int(1),
            Value::Str("2024-02-29".into())
        ),
    )
    .expect("record");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::dateOf", Value::Int(1)),
    )
    .expect("read");
    assert_eq!(outcome.value, Some(Value::Str("2024-02-29".into())));
}

#[test]
fn temporal_values_order_and_equate() {
    // Dates, instants, and durations compare by their underlying counts, matching
    // the ordered/equatable types the checker already advertises.
    let program = checked_program(
        "fn dateBefore(a: string, b: string): bool\n    return std::clock::parseDate(a) < std::clock::parseDate(b)\nfn dateSame(a: string, b: string): bool\n    return std::clock::parseDate(a) == std::clock::parseDate(b)\nfn instantBefore(a: string, b: string): bool\n    return std::clock::parseInstant(a) < std::clock::parseInstant(b)\nfn durationBefore(a: string, b: string): bool\n    return std::clock::parseDuration(a) < std::clock::parseDuration(b)\n",
    );
    let call = |entry: &str, a: &str, b: &str| {
        run(checked_entry!(
            &program,
            entry,
            Value::Str(a.into()),
            Value::Str(b.into())
        ))
    };
    assert_eq!(
        call("test::dateBefore", "2024-01-01", "2024-12-31"),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        call("test::dateBefore", "2024-12-31", "2024-01-01"),
        Ok(Some(Value::Bool(false)))
    );
    assert_eq!(
        call("test::dateSame", "2024-02-29", "2024-02-29"),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        call(
            "test::instantBefore",
            "2026-05-28T12:00:00Z",
            "2026-05-28T13:00:00Z"
        ),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        call("test::durationBefore", "PT60S", "PT3600S"),
        Ok(Some(Value::Bool(true)))
    );
}

/// A short-form `clock::formatDate(clock::parseDate(s))` lowers through the
/// checker to std call targets and then runs without runtime alias expansion.
#[test]
fn short_form_std_call_runs() {
    let program = checked_program_with_imports(
        "fn roundtrip(s: string): string\n    return clock::formatDate(clock::parseDate(s))\n",
        &["std::clock"],
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::roundtrip",
            Value::Str("2024-02-29".into())
        )),
        Ok(Some(Value::Str("2024-02-29".into())))
    );
}

#[test]
fn scalar_conversions_validate_a_dynamic_value() {
    // A conversion builtin asserts a dynamically-typed value is the target type
    // and returns it (the `unknown` → concrete bridge).
    let program = checked_program(
        "fn asInt(v: int): int\n    return int(v)\nfn asString(v: string): string\n    return string(v)\nfn asBool(v: bool): bool\n    return bool(v)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::asInt", Value::Int(42))),
        Ok(Some(Value::Int(42)))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::asString",
            Value::Str("hi".into())
        )),
        Ok(Some(Value::Str("hi".into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::asBool", Value::Bool(true))),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn a_conversion_rejects_a_value_of_the_wrong_type() {
    // `int(...)` validates; a string value is not an int.
    let program = checked_program("fn f(v: int): int\n    return int(v)\n");
    let error = rejected_entry_call(&program, "test::f", vec![Value::Str("x".into())]);
    assert_eq!(error.code, RUN_TYPE);
}

#[test]
fn temporal_conversions_validate_their_values() {
    // `date`/`instant`/`duration` validate canonical temporal values (here built
    // via the std::clock parsers), returning them unchanged.
    let program = checked_program(
        "fn d(t: string): string\n    return std::clock::formatDate(date(std::clock::parseDate(t)))\nfn span(t: string): string\n    return std::clock::formatDuration(duration(std::clock::parseDuration(t)))\n",
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::d",
            Value::Str("2024-02-29".into())
        )),
        Ok(Some(Value::Str("2024-02-29".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::span",
            Value::Str("PT90S".into())
        )),
        Ok(Some(Value::Str("PT90S".into())))
    );
}

#[test]
fn bool_conversion_accepts_canonical_int_forms() {
    // `bool(...)` accepts only the canonical integer forms at runtime: 0 and 1.
    let program = checked_program("fn b(v: int): bool\n    return bool(v)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::b", Value::Int(0))),
        Ok(Some(Value::Bool(false)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::b", Value::Int(1))),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn bool_conversion_rejects_non_canonical_values() {
    let program = checked_program("fn b(v: int): bool\n    return bool(v)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::b", Value::Int(2)))
            .unwrap_err()
            .code,
        RUN_TYPE
    );
    checker_rejects(
        "fn bs(v: string): bool\n    return bool(v)\n",
        "check.call_argument",
    );
}

#[test]
fn int_conversion_parses_canonical_text() {
    let program = checked_program("fn n(v: string): int\n    return int(v)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::n", Value::Str("12".into()))),
        Ok(Some(Value::Int(12)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::n", Value::Str("-7".into()))),
        Ok(Some(Value::Int(-7)))
    );
}

#[test]
fn decimal_conversion_parses_canonical_text() {
    // `decimal("1.5")` parses to a decimal; rendered back through interpolation it
    // round-trips to its canonical text.
    let program = checked_program("fn d(v: string): string\n    return $\"{decimal(v)}\"\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::d",
            Value::Str("1.5".into())
        )),
        Ok(Some(Value::Str("1.5".into())))
    );
}

#[test]
fn error_code_conversion_validates_and_returns_text() {
    let program = checked_program(
        "fn code(): string\n\
         \x20   const code: ErrorCode = ErrorCode(\"x.y\")\n\
         \x20   if code == ErrorCode(\"x.y\")\n\
         \x20       return string(code)\n\
         \x20   return \"mismatch\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::code")),
        Ok(Some(Value::Str("x.y".into())))
    );
}

#[test]
fn error_code_conversion_accepts_dynamic_text() {
    let program =
        checked_program("fn code(raw: string): string\n    return string(ErrorCode(raw))\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::code",
            Value::Str("app.missing".into())
        )),
        Ok(Some(Value::Str("app.missing".into())))
    );
}

#[test]
fn error_code_conversion_failures_are_catchable_type_errors() {
    let program = checked_program(
        "fn code(raw: string): string\n\
         \x20   try\n\
         \x20       return string(ErrorCode(raw))\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n",
    );
    for raw in ["Bad.Code", "missing_dot"] {
        assert_eq!(
            run(checked_entry!(
                &program,
                "test::code",
                Value::Str(raw.into())
            )),
            Ok(Some(Value::Str(RUN_TYPE.into())))
        );
    }
}

#[test]
fn conversion_builtins_accept_documented_sources() {
    let program = checked_program(
        "fn intFromDecimal(v: decimal): int\n    return int(v)\n\
         fn decimalFromInt(v: int): string\n    return string(decimal(v))\n\
         fn stringFromInt(v: int): string\n    return string(v)\n\
         fn stringFromDecimal(v: decimal): string\n    return string(v)\n\
         fn stringFromBool(v: bool): string\n    return string(v)\n\
         fn stringFromBytes(v: bytes): string\n    return string(v)\n\
         fn bytesFromBytes(v: bytes): bytes\n    return bytes(v)\n\
         fn dateFromText(v: string): string\n    return string(date(v))\n\
         fn instantFromText(v: string): string\n    return string(instant(v))\n\
         fn durationFromText(v: string): string\n    return string(duration(v))\n",
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::intFromDecimal",
            Value::Decimal(Decimal::parse("42").expect("decimal"))
        )),
        Ok(Some(Value::Int(42)))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::decimalFromInt",
            Value::Int(42)
        )),
        Ok(Some(Value::Str("42".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::stringFromInt",
            Value::Int(-7)
        )),
        Ok(Some(Value::Str("-7".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::stringFromDecimal",
            Value::Decimal(Decimal::parse("1.5").expect("decimal"))
        )),
        Ok(Some(Value::Str("1.5".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::stringFromBool",
            Value::Bool(true)
        )),
        Ok(Some(Value::Str("true".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::stringFromBytes",
            Value::Bytes("snow".as_bytes().to_vec())
        )),
        Ok(Some(Value::Str("snow".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::bytesFromBytes",
            Value::Bytes(vec![0, 1, 2])
        )),
        Ok(Some(Value::Bytes(vec![0, 1, 2])))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::dateFromText",
            Value::Str("2024-02-29".into())
        )),
        Ok(Some(Value::Str("2024-02-29".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::instantFromText",
            Value::Str("1970-01-01T00:00:00Z".into())
        )),
        Ok(Some(Value::Str("1970-01-01T00:00:00Z".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::durationFromText",
            Value::Str("PT90S".into())
        )),
        Ok(Some(Value::Str("PT90S".into())))
    );
}

#[test]
fn documented_conversions_reject_invalid_dynamic_values() {
    let program = checked_program(
        "fn intFromDecimal(v: decimal): int\n    return int(v)\n\
         fn stringFromBytes(v: bytes): string\n    return string(v)\n\
         fn dateFromText(v: string): date\n    return date(v)\n",
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::intFromDecimal",
            Value::Decimal(Decimal::parse("1.5").expect("decimal"))
        ))
        .unwrap_err()
        .code,
        RUN_TYPE
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::stringFromBytes",
            Value::Bytes(vec![0xff])
        ))
        .unwrap_err()
        .code,
        RUN_TYPE
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::dateFromText",
            Value::Str("2021-02-29".into())
        ))
        .unwrap_err()
        .code,
        RUN_TYPE
    );
}

#[test]
fn documented_conversion_failures_are_catchable_type_errors() {
    let program = checked_program(
        "fn code(v: bytes): string\n    try\n        return string(v)\n    catch err: Error\n        return err.code\n",
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::code",
            Value::Bytes(vec![0xff])
        )),
        Ok(Some(Value::Str(RUN_TYPE.into())))
    );
}

#[test]
fn a_numeric_conversion_rejects_malformed_text() {
    // Malformed text is a typed numeric error, not a silent zero.
    let program = checked_program("fn n(v: string): int\n    return int(v)\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::n",
            Value::Str("nope".into())
        ))
        .unwrap_err()
        .code,
        RUN_TYPE
    );
    let program = checked_program("fn d(v: string): decimal\n    return decimal(v)\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::d",
            Value::Str("1.2.3".into())
        ))
        .unwrap_err()
        .code,
        RUN_TYPE
    );
}

#[test]
fn decimal_conversion_distinguishes_malformed_text_from_envelope_overflow() {
    let program = checked_program(
        "fn d(v: string): decimal\n    return decimal(v)\n\
         fn caught(v: string): string\n    try\n        var d: decimal = decimal(v)\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::d",
            Value::Str("1.2.3".into())
        ))
        .unwrap_err()
        .code,
        RUN_TYPE
    );
    for raw in [
        "99999999999999999999999999999999999",
        "0.11111111111111111111111111111111111",
    ] {
        let error = run(checked_entry!(&program, "test::d", Value::Str(raw.into()))).unwrap_err();
        assert_eq!(error.code, RUN_DECIMAL_OVERFLOW);
        assert!(
            error.message.contains("decimal arithmetic exceeded"),
            "{}",
            error.message
        );
        assert_eq!(
            run(checked_entry!(
                &program,
                "test::caught",
                Value::Str(raw.into())
            )),
            Ok(Some(Value::Str(RUN_DECIMAL_OVERFLOW.into())))
        );
    }
}

#[test]
fn a_conversion_error_message_is_grammar_independent() {
    // The message must not embed an article, so it reads correctly for
    // vowel-initial type names (not 'requires a int value').
    let program = checked_program("fn n(v: string): int\n    return int(v)\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::n",
            Value::Str("nope".into())
        ))
        .unwrap_err()
        .message,
        "cannot convert this value to int"
    );
}

#[test]
fn evaluates_conditionals() {
    let source = "fn max(a: int, b: int): int\n    if a > b\n        return a\n    return b\n";
    assert_eq!(
        eval_source(source, "max", vec![Value::Int(7), Value::Int(3)]),
        Ok(Some(Value::Int(7)))
    );
    assert_eq!(
        eval_source(source, "max", vec![Value::Int(3), Value::Int(7)]),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn std_assert_is_true_passes_and_fails() {
    let program = checked_program("pub fn ok()\n    std::assert::isTrue(1 == 1)\n");
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));

    let program = checked_program("pub fn bad()\n    std::assert::isTrue(1 == 2)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::bad")).unwrap_err().code,
        RUN_ASSERT
    );
}

#[test]
fn std_assert_is_false_passes_and_fails() {
    let program = checked_program("pub fn ok()\n    std::assert::isFalse(1 == 2)\n");
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));

    let program = checked_program("pub fn bad()\n    std::assert::isFalse(1 == 1)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::bad")).unwrap_err().code,
        RUN_ASSERT
    );
}

#[test]
fn std_assert_fail_raises_with_its_message() {
    let program = checked_program("pub fn bad()\n    std::assert::fail(\"boom\")\n");
    let error = run(checked_entry!(&program, "test::bad")).unwrap_err();
    assert_eq!(error.code, RUN_ASSERT);
    assert!(error.message.contains("boom"), "{}", error.message);
}

#[test]
fn std_assert_absent_passes_when_nothing_is_saved() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\
         \n\
         pub fn ok()\n    std::assert::absent(^books(1))\n",
    );
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));
}

#[test]
fn std_assert_absent_fails_when_a_value_is_present() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\
         \n\
         pub fn bad()\n    std::assert::absent(^books(1))\n",
    );
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["title"]),
        SavedValue::Str("present".into()),
    );
    let error = run_entry(&store, checked_entry!(&program, "test::bad")).unwrap_err();
    assert_eq!(error.code, RUN_ASSERT);
}

#[test]
fn std_assert_rejects_misused_arguments() {
    checker_rejects(
        "pub fn bad()\n    std::assert::isTrue(1)\n",
        "check.call_argument",
    );
    checker_rejects(
        "pub fn bad()\n    std::assert::fail(42)\n",
        "check.call_argument",
    );
}

#[test]
fn a_passing_assert_lets_execution_continue() {
    // A passing assertion produces no value and falls through to later statements.
    let program =
        checked_program("pub fn ok(): int\n    std::assert::isTrue(1 == 1)\n    return 7\n");
    assert_eq!(
        run(checked_entry!(&program, "test::ok")),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn a_whole_group_entry_write_creates_the_entry() {
    // `^books(1).versions(2) = b` writes the whole group entry from a resource
    // value; the runtime matches its fields against the group's members by name.
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var b: Book\n\
         \x20\x20\x20\x20b.title = \"v2\"\n\
         \x20\x20\x20\x20^books(1).versions(2) = b\n\
         \n\
         pub fn version_title(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::version_title"))
            .unwrap()
            .value,
        Some(Value::Str("v2".into()))
    );
}

#[test]
fn a_nested_group_field_round_trips() {
    // `versions(version)` entries hold a nested `comments(pos)` group; writing and
    // reading `^books(1).versions(2).comments(3).text` exercises a saved-tree path
    // deeper than one keyed layer.
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20comments(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20required text: string\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20^books(1).title = \"root\"\n\
         \x20\x20\x20\x20^books(1).versions(2).title = \"version\"\n\
         \x20\x20\x20\x20^books(1).versions(2).comments(3).text = \"deep\"\n\
         \n\
         pub fn comment(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).comments(3).text ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::comment"))
            .unwrap()
            .value,
        Some(Value::Str("deep".into()))
    );
}

#[test]
fn a_whole_group_entry_can_be_read_and_copied() {
    // `^books(1).versions(2) = ^books(1).versions(1)` reads the whole entry as a
    // value (RHS) and writes it to another key (LHS).
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var b: Book\n\
         \x20\x20\x20\x20b.title = \"v1\"\n\
         \x20\x20\x20\x20^books(1).versions(1) = b\n\
         \x20\x20\x20\x20if exists(^books(1).versions(1))\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^books(1).versions(2) = ^books(1).versions(1)\n\
         \n\
         pub fn copied_title(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::copied_title"))
            .unwrap()
            .value,
        Some(Value::Str("v1".into()))
    );
}

#[test]
fn std_text_builtins_operate_on_strings() {
    // `length` counts Unicode scalar values, not bytes ("café" is 4 scalars).
    let program = checked_program("pub fn f(): int\n    return std::text::length(\"café\")\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(4)))
    );

    let program = checked_program("pub fn f(): string\n    return std::text::trim(\"  hi  \")\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Str("hi".into())))
    );

    let program =
        checked_program("pub fn f(): bool\n    return std::text::contains(\"hello\", \"ell\")\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn std_math_builtins_compute_over_integers() {
    let program = checked_program("pub fn f(): int\n    return std::math::absInt(0 - 7)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(7)))
    );

    // remainder is truncated (sign of the dividend): -7 rem 3 = -1.
    let program = checked_program("pub fn f(): int\n    return std::math::remainder(0 - 7, 3)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(-1)))
    );

    // modulo is floored (sign of the divisor): -7 mod 3 = 2.
    let program = checked_program("pub fn f(): int\n    return std::math::modulo(0 - 7, 3)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn std_math_modulo_by_zero_is_a_runtime_error() {
    let program = checked_program("pub fn f(): int\n    return std::math::modulo(7, 0)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap_err().code,
        RUN_DIVIDE_BY_ZERO
    );
}

#[test]
fn std_builtins_reject_wrong_argument_types() {
    checker_rejects(
        "pub fn f(): int\n    return std::text::length(42)\n",
        "check.call_argument",
    );
    checker_rejects(
        "pub fn f(): int\n    return std::math::absInt(\"x\")\n",
        "check.call_argument",
    );
}

#[test]
fn throw_surfaces_as_an_uncaught_error() {
    let program = checked_program(
        "pub fn bad()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
    );
    let error = run(checked_entry!(&program, "test::bad")).unwrap_err();
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    // The rendered message is byte-identical to the `uncaught error [code]: msg`
    // formula, pinning the format the CLI surfaces for an uncaught throw.
    assert_eq!(error.message, "uncaught error [book.absent]: no book");
}

#[test]
fn error_constructor_requires_code_and_message() {
    let program = checked_program("pub fn bad()\n    throw Error(code: \"x.y\")\n");
    assert_eq!(
        run(checked_entry!(&program, "test::bad")).unwrap_err().code,
        RUN_TYPE
    );
}

#[test]
fn throw_is_an_error_value() {
    checker_rejects("pub fn bad()\n    throw 7\n", "check.throw_type");
}

#[test]
fn catch_binds_the_thrown_error_and_recovers() {
    let program = checked_program(
        "pub fn safe(): string\n    try\n        throw Error(code: \"x.y\", message: \"boom\")\n    catch err: Error\n        return err.message\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::safe")),
        Ok(Some(Value::Str("boom".into())))
    );
}

#[test]
fn a_try_that_succeeds_skips_catch() {
    let program = checked_program(
        "pub fn ok(): int\n    try\n        return 1\n    catch err: Error\n        return 2\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::ok")),
        Ok(Some(Value::Int(1)))
    );
}

#[test]
fn finally_runs_on_success_and_on_throw() {
    let program = checked_program(
        "pub fn run_it(do_throw: bool)\n    try\n        if do_throw\n            throw Error(code: \"x.y\", message: \"b\")\n    catch err: Error\n        write(\"caught \")\n    finally\n        write(\"cleanup\")\n",
    );
    let out = |b| {
        run_full(checked_entry!(&program, "test::run_it", Value::Bool(b)))
            .unwrap()
            .output
    };
    assert_eq!(out(false), "cleanup");
    assert_eq!(out(true), "caught cleanup");
}

#[test]
fn a_runtime_fault_in_try_is_caught() {
    let program = checked_program(
        "pub fn f(): int\n    try\n        const boom = 1 / 0\n    catch err: Error\n        return 2\n    return 0\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn numeric_parse_and_range_faults_are_catchable_with_specific_codes() {
    let program = checked_program(
        "pub fn overflow_code(): string\n\
         \x20   try\n\
         \x20       var x: int = 9223372036854775807\n\
         \x20       x = x + 1\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn remainder_code(): string\n\
         \x20   try\n\
         \x20       var z: int = 0\n\
         \x20       var y: int = 5 % z\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn parse_date_code(): string\n\
         \x20   try\n\
         \x20       var d: date = std::clock::parseDate(\"2023-02-29\")\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn parse_duration_code(): string\n\
         \x20   try\n\
         \x20       var d: duration = std::clock::parseDuration(\"nonsense\")\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn duration_conversion_code(): string\n\
         \x20   try\n\
         \x20       var raw: string = \"nonsense\"\n\
         \x20       var d: duration = duration(raw)\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn instant_range_code(): string\n\
         \x20   try\n\
         \x20       var text: string = std::clock::formatInstant(std::clock::add(std::clock::parseInstant(\"9999-12-31T23:59:59Z\"), 1.day))\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn decimal_overflow_code(): string\n\
         \x20   try\n\
         \x20       var x: decimal = 9999999999999999999999999999999999.0 * 9999999999999999999999999999999999.0\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::overflow_code")),
        Ok(Some(Value::Str(RUN_OVERFLOW.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::remainder_code")),
        Ok(Some(Value::Str(RUN_DIVIDE_BY_ZERO.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::parse_date_code")),
        Ok(Some(Value::Str(RUN_TYPE.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::parse_duration_code")),
        Ok(Some(Value::Str(RUN_TYPE.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::duration_conversion_code")),
        Ok(Some(Value::Str(RUN_TYPE.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::instant_range_code")),
        Ok(Some(Value::Str("value.range".into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::decimal_overflow_code")),
        Ok(Some(Value::Str("run.decimal_overflow".into())))
    );
}

#[test]
fn a_throw_from_a_callee_is_caught_by_the_caller() {
    // An Error thrown inside a called function unwinds through the call and is
    // caught by the caller.
    let program = checked_program(
        "fn boom()\n    throw Error(code: \"x.y\", message: \"deep\")\npub fn safe(): string\n    try\n        boom()\n    catch err: Error\n        return err.message\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::safe")),
        Ok(Some(Value::Str("deep".into())))
    );
}

#[test]
fn an_expression_position_call_throw_is_caught_like_a_statement_throw() {
    // The throw rides one channel regardless of position: a throw from a call used
    // mid-expression (`var x = boom() + 1`) unwinds on the same `Err` channel a
    // bare `throw` statement does, so the same `catch` binds it. This pins the A32
    // goal that expression- and statement-position throws agree.
    let program = checked_program(
        "fn boom(): int\n    throw Error(code: \"x.y\", message: \"mid\")\npub fn safe(): string\n    try\n        var total: int = boom() + 1\n    catch err: Error\n        return err.message\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::safe")),
        Ok(Some(Value::Str("mid".into())))
    );
}

#[test]
fn a_throw_propagates_through_intermediate_calls() {
    // a -> b -> c; c throws, a catches. The Error crosses two call boundaries.
    let program = checked_program(
        "fn c()\n    throw Error(code: \"deep.fail\", message: \"from c\")\nfn b()\n    c()\npub fn a(): string\n    try\n        b()\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::a")),
        Ok(Some(Value::Str("deep.fail".into())))
    );
}

#[test]
fn a_callee_throw_rolls_back_the_enclosing_transaction() {
    // A transaction writes, then a called function throws. The throw escapes the
    // transaction, so it rolls back and the write never lands.
    let program = checked_program(
        "resource Account at ^accts(id: int)\n    balance: int\n\nfn fail()\n    throw Error(code: \"x\", message: \"boom\")\n\npub fn run_it()\n    transaction\n        ^accts(1).balance = 5\n        fail()\n\npub fn read(): int\n    return ^accts(1).balance ?? -1\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::run_it"));
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNCAUGHT_THROW),
        "{result:?}"
    );
    let after = run_entry(&store, checked_entry!(&program, "test::read"))
        .expect("read")
        .value;
    assert_eq!(after, Some(Value::Int(-1)));
}

#[test]
fn a_caught_callee_throw_does_not_leak_into_a_later_fault() {
    // After a caller catches a callee's throw, the pending throw is cleared, so a
    // later fault is caught with its own Error value rather than the stale throw.
    let program = checked_program(
        "fn callee()\n    throw Error(code: \"e1\", message: \"boom\")\npub fn check(): int\n    try\n        callee()\n    catch err: Error\n        write(\"caught\")\n    try\n        const boom = 1 / 0\n    catch err: Error\n        return 99\n    return 0\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::check")),
        Ok(Some(Value::Int(99)))
    );
}

#[test]
fn a_throwing_finally_does_not_leak_a_pending_throw() {
    // A `finally` throwing over a call-propagated throw must not leave that throw
    // stashed: after an outer `catch` swallows the finally throw, a later fault is
    // caught with its own Error value rather than the stale throw.
    let program = checked_program(
        "fn callee()\n    throw Error(code: \"e1\", message: \"from call\")\npub fn leak(): int\n    try\n        try\n            callee()\n        finally\n            throw Error(code: \"e2\", message: \"from finally\")\n    catch err: Error\n        write(\"swallowed\")\n    try\n        const boom = 1 / 0\n    catch err: Error\n        return 99\n    return 0\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::leak")),
        Ok(Some(Value::Int(99)))
    );
}

#[test]
fn a_throw_from_a_call_in_finally_propagates() {
    // A `finally` whose own called function throws: that throw replaces the
    // outcome and is caught by an outer handler.
    let program = checked_program(
        "fn boom()\n    throw Error(code: \"deep\", message: \"x\")\npub fn run_it(): string\n    try\n        try\n            write(\"body\")\n        finally\n            boom()\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::run_it")),
        Ok(Some(Value::Str("deep".into())))
    );
}

#[test]
fn a_clean_finally_preserves_a_propagated_call_throw() {
    // A clean `finally` (no throw of its own) over a call-propagated throw must
    // restore the pending throw so an outer `catch` still sees it.
    let program = checked_program(
        "fn boom()\n    throw Error(code: \"deep\", message: \"x\")\npub fn run_it(): string\n    try\n        try\n            boom()\n        finally\n            write(\"cleanup\")\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    let outcome = run_full(checked_entry!(&program, "test::run_it")).expect("caught");
    assert_eq!(outcome.value, Some(Value::Str("deep".into())));
    assert_eq!(outcome.output, "cleanup");
}

#[test]
fn an_out_parameter_writes_back_to_a_local() {
    // The callee fills an `out` parameter, and the caller's local sees the
    // written value.
    let program = checked_program(
        "fn give(out value: int)\n    value = 42\npub fn main(): int\n    var n: int = 0\n    give(out n)\n    return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn an_uninitialized_scalar_var_starts_at_its_zero() {
    // A typed `var` without an initializer is a writable place that starts at its
    // type's default, so plain declaration-then-use works.
    let program = checked_program("pub fn main(): int\n    var n: int\n    return n\n");
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(0)))
    );
}

#[test]
fn an_out_parameter_writes_back_to_an_uninitialized_var() {
    // The documented `out` pattern declares the place without a value:
    // `var n: int` then `give(out n)`.
    let program = checked_program(
        "fn give(out value: int)\n    value = 42\npub fn main(): int\n    var n: int\n    give(out n)\n    return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn an_out_parameter_ignores_the_caller_value_and_overwrites_it() {
    // `out` does not read the caller's value; whatever the callee assigns wins.
    let program = checked_program(
        "fn give(out value: int)\n    value = 42\npub fn main(): int\n    var n: int = 99\n    give(out n)\n    return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn an_inout_parameter_reads_then_writes_a_local() {
    // `inout` seeds the parameter from the caller's value, then writes back.
    let program = checked_program(
        "fn bump(inout n: int)\n    n = n + 1\npub fn main(): int\n    var n: int = 41\n    bump(inout n)\n    return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn an_inout_parameter_mutates_a_local_resource() {
    // Mutating a field of a local resource passed `inout` is visible to the
    // caller.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\nfn setTitle(inout book: Book)\n    book.title = \"Small Gods\"\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    setTitle(inout book)\n    return book.title\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Str("Small Gods".into())))
    );
}

#[test]
fn an_uninitialized_qualified_resource_var_starts_empty() {
    let program = checked_program_modules(&[
        "module library\nresource Book\n    title: string\n",
        "module app\nuse library\npub fn main(): string\n    var book: library::Book\n    book.title = \"draft\"\n    return book.title\n",
    ]);
    assert_eq!(
        run(checked_entry!(&program, "app::main")),
        Ok(Some(Value::Str("draft".into())))
    );
}

#[test]
fn uninitialized_bare_foreign_resource_var_is_not_project_wide() {
    checker_rejects_sources(
        &[
            "module library\nresource Book\n    title: string\n",
            "module app\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    return book.title\n",
        ],
        "check.unknown_type",
    );
}

#[test]
fn an_inout_parameter_writes_back_to_a_local_resource_field() {
    // A field of a local resource, `book.title`, is an assignable place; passing it
    // `inout` reads it to seed the parameter and writes the result back.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\nfn upper(inout s: string)\n    s = \"UPPER\"\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    upper(inout book.title)\n    return book.title\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Str("UPPER".into())))
    );
}

#[test]
fn an_out_parameter_writes_back_to_a_local_resource_field() {
    // `out` on a local resource field fills it without reading it first.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\nfn fill(out s: string)\n    s = \"FILLED\"\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    fill(out book.title)\n    return book.title\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Str("FILLED".into())))
    );
}

#[test]
fn write_back_is_skipped_when_the_callee_throws() {
    // A callee that mutates an `inout` parameter then throws must not write back:
    // the caller's local keeps its pre-call value.
    let program = checked_program(
        "fn bad(inout n: int)\n    n = 99\n    throw Error(code: \"x\", message: \"boom\")\npub fn main(): int\n    var n: int = 1\n    try\n        bad(inout n)\n    catch err: Error\n        write(\"caught\")\n    return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(1)))
    );
}

#[test]
fn an_argument_mode_must_match_the_parameter_mode() {
    checker_rejects(
        "fn plain(n: int): int\n    return n\npub fn main(): int\n    var n: int = 1\n    return plain(out n)\n",
        "check.call_argument",
    );
}

/// A program exercising the four `std::io` file builtins.
const IO_SAMPLE: &str = "\
fn saveText(path: string, text: string)
    std::io::writeText(path, text)

fn loadText(path: string): string
    return std::io::readText(path)

fn saveBytes(path: string, data: bytes)
    std::io::writeBytes(path, data)

fn loadBytes(path: string): bytes
    return std::io::readBytes(path)

fn loadOrCode(path: string): string
    try
        return std::io::readText(path)
    catch err: Error
        return err.code
";

#[test]
fn io_round_trips_text_through_a_file() {
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("note.txt").to_string_lossy().into_owned();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(
            &program,
            "test::saveText",
            Value::Str(path.clone()),
            Value::Str("hello".into())
        ),
    )
    .expect("write");
    let loaded = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::loadText", Value::Str(path)),
    )
    .expect("read")
    .value;
    assert_eq!(loaded, Some(Value::Str("hello".into())));
}

#[test]
fn irreversible_host_effects_inside_a_transaction_are_rejected_before_the_effect() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("effect.txt");
    let program = checked_program(
        "pub fn write_in_txn(path: string)\n    transaction\n        std::io::writeText(path, \"leaked\")\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let error = run_entry_with_host(
        &store,
        &host,
        checked_entry!(
            &program,
            "test::write_in_txn",
            Value::Str(path.to_string_lossy().into_owned())
        ),
    )
    .expect_err("host write inside transaction must fail");

    assert_eq!(error.code, RUN_CAPABILITY);
    assert!(
        !path.exists(),
        "host write must be rejected before creating the file"
    );
}

#[test]
fn output_inside_a_transaction_is_rejected_before_the_effect() {
    let program =
        checked_program("pub fn print_in_txn()\n    transaction\n        print(\"leaked\")\n");
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::print_in_txn"));
    let error = result.expect_err("print inside transaction must fail");

    assert_eq!(error.code, RUN_CAPABILITY);
}

#[test]
fn log_inside_a_transaction_is_rejected_before_the_effect() {
    let program = checked_program(
        "pub fn log_in_txn()\n    transaction\n        std::log::info(\"leaked\")\n",
    );
    let store = TreeStore::memory();
    let log = Rc::new(RefCell::new(String::new()));
    let host = Host::new().with_log_sink(Rc::clone(&log));
    let error = run_entry_with_host(&store, &host, checked_entry!(&program, "test::log_in_txn"))
        .expect_err("log inside transaction must fail");

    assert_eq!(error.code, RUN_CAPABILITY);
    assert_eq!(log.borrow().as_str(), "");
}

#[test]
fn io_round_trips_bytes_through_a_file() {
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("blob.bin").to_string_lossy().into_owned();
    let data = Value::Bytes(vec![0, 1, 2, 255, 128]);
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(
            &program,
            "test::saveBytes",
            Value::Str(path.clone()),
            data.clone()
        ),
    )
    .expect("write");
    let loaded = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::loadBytes", Value::Str(path)),
    )
    .expect("read")
    .value;
    assert_eq!(loaded, Some(data));
}

#[test]
fn io_without_a_filesystem_capability_is_a_capability_error() {
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    // Plain `run_entry` provides no host capabilities.
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::loadText", Value::Str("x".into())),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

#[test]
fn an_io_error_raises_a_catchable_error() {
    // Reading a missing file (with the capability present) raises a typed Error
    // the program can `catch`, not a runtime fault.
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let dir = tempfile::tempdir().expect("temp dir");
    let missing = dir.path().join("absent.txt").to_string_lossy().into_owned();
    let code = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::loadOrCode", Value::Str(missing)),
    )
    .expect("caught")
    .value;
    assert_eq!(code, Some(Value::Str("io.read".into())));
}

/// A resource and helper exercising `out` write-back to saved places.
const SAVED_MODE_SAMPLE: &str = "\
resource Account at ^accts(id: int)
    balance: int

fn give(out n: int)
    n = 7

pub fn produce()
    give(out ^accts(1).balance)

pub fn balanceOf(): int
    return ^accts(1).balance ?? 0
";

#[test]
fn out_creates_a_saved_field() {
    let program = checked_program(SAVED_MODE_SAMPLE);
    let store = TreeStore::memory();
    // `out` never reads the place, so the field need not exist beforehand.
    run_entry(&store, checked_entry!(&program, "test::produce")).expect("produce");
    let balance = run_entry(&store, checked_entry!(&program, "test::balanceOf"))
        .expect("read")
        .value;
    assert_eq!(balance, Some(Value::Int(7)));
}

/// A resource with a `versions(version)` group layer, for `out` into a field
/// inside a keyed group entry.
const GROUP_FIELD_MODE_SAMPLE: &str = "\
resource Book at ^books(id: int)
    title: string
    versions(version: int)
        title: string

fn makeTitle(out t: string)
    t = \"made\"

pub fn produce()
    makeTitle(out ^books(1).versions(3).title)

pub fn producedTitle(): string
    return ^books(1).versions(3).title ?? \"\"
";

#[test]
fn out_creates_a_group_entry_field() {
    // `out` never reads the place, so the group-entry field need not exist first.
    let program = checked_program(GROUP_FIELD_MODE_SAMPLE);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::produce")).expect("produce");
    let title = run_entry(&store, checked_entry!(&program, "test::producedTitle"))
        .expect("read")
        .value;
    assert_eq!(title, Some(Value::Str("made".into())));
}

#[test]
fn finally_runs_after_a_fault_and_can_replace_it() {
    // The try body faults (not catchable); finally still runs and its throw
    // replaces the fault, proving finally ran.
    let program = checked_program(
        "pub fn f(): int\n    try\n        const boom = 1 / 0\n    finally\n        throw Error(code: \"cleanup.failed\", message: \"x\")\n    return 0\n",
    );
    let error = run(checked_entry!(&program, "test::f")).unwrap_err();
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    assert!(
        error.message.contains("cleanup.failed"),
        "{}",
        error.message
    );
}

#[test]
fn an_uncaught_throw_without_a_catch_propagates_through_finally() {
    let program = checked_program(
        "pub fn f()\n    try\n        throw Error(code: \"x.y\", message: \"boom\")\n    finally\n        write(\"cleanup\")\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap_err().code,
        RUN_UNCAUGHT_THROW
    );
}

#[test]
fn a_throw_in_finally_replaces_the_outcome() {
    let program = checked_program(
        "pub fn f(): int\n    try\n        return 1\n    finally\n        throw Error(code: \"from.finally\", message: \"x\")\n",
    );
    let error = run(checked_entry!(&program, "test::f")).unwrap_err();
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    assert!(error.message.contains("from.finally"), "{}", error.message);
}

#[test]
fn a_clean_finally_preserves_a_return() {
    // A finally that completes normally lets the try's `return` through.
    let program = checked_program(
        "pub fn f(): int\n    try\n        return 7\n    finally\n        write(\"cleanup\")\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn a_throw_caught_inside_a_transaction_commits() {
    // The throw is handled within the transaction, so the body completes normally
    // and the catch's write commits.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn safe(id: int)\n    transaction\n        try\n            throw Error(code: \"x.y\", message: \"b\")\n        catch err: Error\n            ^books(id).title = \"recovered\"\n\n\
         pub fn title(id: int): string\n    return ^books(id).title\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::safe", Value::Int(1)),
    )
    .expect("safe runs");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title", Value::Int(1))
        )
        .unwrap()
        .value,
        Some(Value::Str("recovered".into()))
    );
}

#[test]
fn throw_inside_a_transaction_rolls_back() {
    // An escaping throw rolls the transaction back, like any other escape.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn risky(id: int)\n    transaction\n        ^books(id).title = \"staged\"\n        throw Error(code: \"x.y\", message: \"boom\")\n\n\
         pub fn has_book(id: int): bool\n    return exists(^books(id))\n",
    );
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::risky", Value::Int(1))
        )
        .unwrap_err()
        .code,
        RUN_UNCAUGHT_THROW
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .unwrap()
        .value,
        Some(Value::Bool(false))
    );
}

#[test]
fn evaluates_locals_and_reassignment() {
    assert_eq!(
        eval_source(
            "fn f(n: int): int\n    var total = n\n    total = total + 1\n    return total\n",
            "f",
            vec![Value::Int(41)]
        ),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn evaluates_boolean_logic() {
    let source = "fn f(a: bool, b: bool): bool\n    return a and not b\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Bool(true), Value::Bool(false)]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        eval_source(source, "f", vec![Value::Bool(true), Value::Bool(true)]),
        Ok(Some(Value::Bool(false)))
    );
}

#[test]
fn equality_compares_values() {
    // Marrow spells equality `==` (and inequality `!=`); assignment is the
    // single `=`, so equality in expression position uses `==`.
    let source = "fn f(a: int, b: int): bool\n    return a == b\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(5), Value::Int(5)]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(5), Value::Int(6)]),
        Ok(Some(Value::Bool(false)))
    );
}

#[test]
fn a_function_that_returns_nothing_yields_none() {
    // Falls off the end with no `return`.
    assert_eq!(
        eval_source(
            "fn f(a: int)\n    const x = a + 1\n",
            "f",
            vec![Value::Int(1)]
        ),
        Ok(None)
    );
}

#[test]
fn rejects_division_by_zero() {
    let result = eval_source(
        "fn f(a: int): int\n    const boom = a / 0\n    return 0\n",
        "f",
        vec![Value::Int(10)],
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_DIVIDE_BY_ZERO),
        "{result:?}"
    );
}

#[test]
fn integer_remainder_by_zero_reports_one_consistent_message() {
    // The `%` operator and `std::math::remainder`/`modulo` are the same integer
    // remainder, so a zero divisor must report the same divide-by-zero message.
    let result = eval_source(
        "fn f(a: int): int\n    return a % 0\n",
        "f",
        vec![Value::Int(10)],
    );
    let Err(error) = result else {
        panic!("expected an error, got {result:?}");
    };
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.message, "integer remainder by zero");

    // std::math::modulo routes through the same integer-remainder path.
    let program = checked_program("pub fn g(): int\n    return std::math::modulo(7, 0)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::g"))
            .unwrap_err()
            .message,
        "integer remainder by zero"
    );
}

#[test]
fn detects_integer_overflow() {
    let result = eval_source(
        "fn f(a: int): int\n    return a * a\n",
        "f",
        vec![Value::Int(i64::MAX)],
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_OVERFLOW),
        "{result:?}"
    );
}

#[test]
fn detects_an_over_range_integer_literal() {
    checker_rejects(
        "fn f(): int\n    return 99999999999999999999999999\n",
        "check.literal_range",
    );
}

#[test]
fn detects_an_over_envelope_decimal_literal() {
    checker_rejects(
        "fn f(): decimal\n    return 9.9999999999999999999999999999999999\n",
        "check.literal_range",
    );
}

#[test]
fn rejects_an_unbound_name() {
    checker_rejects("fn f(): int\n    return x\n", "check.unresolved_name");
}

#[test]
fn rejects_assignment_to_an_immutable_binding() {
    let result = eval_source(
        "fn f(): int\n    const x = 1\n    x = 2\n    return x\n",
        "f",
        Vec::new(),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn a_local_const_binds_a_runtime_computed_value() {
    // `const` is the immutable local binding. Unlike a module constant, its
    // initializer may be any expression — here a call resolved at run time.
    let program = checked_program(
        "fn double(n: int): int\n    return n * 2\nfn f(): int\n    const x = double(5)\n    return x\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(10)))
    );
}

#[test]
fn rejects_an_argument_count_mismatch() {
    let program = checked_program("fn add(a: int, b: int): int\n    return a + b\n");
    let error = rejected_entry_call(&program, "test::add", vec![Value::Int(1)]);
    assert_eq!(error.code, RUN_TYPE);
}

#[test]
fn reports_an_unsupported_construct() {
    checker_rejects("fn f(): int\n    return 1..3\n", "check.range_value");
}

#[test]
fn an_if_condition_must_be_boolean() {
    checker_rejects(
        "fn f(a: int): int\n    if a\n        return 1\n    return 0\n",
        "check.condition_type",
    );
}

#[test]
fn an_inner_scope_shadows_then_restores_an_outer_binding() {
    // `const x = 1` inside the if-block shadows only within that block; after it,
    // the outer `x` (99) is what `return x` sees.
    assert_eq!(
        eval_source(
            "fn f(): int\n    const x = 99\n    if true\n        const x = 1\n    return x\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Int(99)))
    );
}

#[test]
fn an_else_if_chain_selects_the_matching_branch() {
    let source = "fn grade(n: int): int\n    if n > 90\n        return 1\n    else if n > 80\n        return 2\n    else\n        return 3\n";
    assert_eq!(
        eval_source(source, "grade", vec![Value::Int(95)]),
        Ok(Some(Value::Int(1)))
    );
    assert_eq!(
        eval_source(source, "grade", vec![Value::Int(85)]),
        Ok(Some(Value::Int(2)))
    );
    assert_eq!(
        eval_source(source, "grade", vec![Value::Int(50)]),
        Ok(Some(Value::Int(3)))
    );
}

#[test]
fn detects_min_over_negative_one_overflow() {
    // `i64::MIN % -1` overflows. (`/` now yields a decimal, so `%` is the only
    // integer-division-family operator that can overflow this way.)
    let result = eval_source(
        "fn f(a: int, b: int): int\n    return a % b\n",
        "f",
        vec![Value::Int(i64::MIN), Value::Int(-1)],
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_OVERFLOW),
        "{result:?}"
    );
}

#[test]
fn evaluates_a_while_loop() {
    let source = "fn sum(n: int): int\n    var total = 0\n    var i = 1\n    while i <= n\n        total = total + i\n        i = i + 1\n    return total\n";
    assert_eq!(
        eval_source(source, "sum", vec![Value::Int(5)]),
        Ok(Some(Value::Int(15)))
    );
}

#[test]
fn evaluates_an_inclusive_for_range() {
    let source = "fn sum(n: int): int\n    var total = 0\n    for i in 1..=n\n        total = total + i\n    return total\n";
    assert_eq!(
        eval_source(source, "sum", vec![Value::Int(5)]),
        Ok(Some(Value::Int(15)))
    );
}

#[test]
fn an_exclusive_for_range_stops_before_the_end() {
    let source = "fn range_count(n: int): int\n    var c = 0\n    for i in 0..n\n        c = c + 1\n    return c\n";
    assert_eq!(
        eval_source(source, "range_count", vec![Value::Int(5)]),
        Ok(Some(Value::Int(5)))
    );
}

#[test]
fn an_int_range_steps_by_a_positive_by_value() {
    // `1..10 by 2` yields 1, 3, 5, 7, 9 (exclusive end), summing to 25.
    assert_eq!(
        eval_source(
            "fn f(): int\n    var total = 0\n    for i in 1..10 by 2\n        total = total + i\n    return total\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Int(25)))
    );
}

#[test]
fn an_int_range_steps_down_with_a_negative_by_value() {
    // `10..1 by -1` counts down 10..2 (exclusive end) — ten iterations from 10 to 2.
    let source = "fn f(): int\n    var last = 0\n    var count = 0\n    for i in 10..1 by -1\n        last = i\n        count = count + 1\n    return count * 100 + last\n";
    // 9 iterations (10 down to 2), last value 2.
    assert_eq!(
        eval_source(source, "f", Vec::new()),
        Ok(Some(Value::Int(902)))
    );
}

#[test]
fn an_inclusive_descending_range_reaches_its_end() {
    // `10..=1 by -1` includes 1, so the final bound is reached.
    let source = "fn f(): int\n    var last = 99\n    for i in 10..=1 by -1\n        last = i\n    return last\n";
    assert_eq!(
        eval_source(source, "f", Vec::new()),
        Ok(Some(Value::Int(1)))
    );
}

#[test]
fn a_wrong_direction_variable_step_is_an_empty_loop() {
    // A runtime wrong-direction step never loops forever: it iterates zero times.
    // `1..10 by step` with step = -1 runs the body never.
    let source = "fn f(step: int): int\n    var count = 0\n    for i in 1..10 by step\n        count = count + 1\n    return count\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(-1)]),
        Ok(Some(Value::Int(0)))
    );
}

#[test]
fn a_default_wrong_direction_range_is_an_empty_loop() {
    // `lo..hi` with lo > hi and the default +1 step iterates zero times.
    let source = "fn f(lo: int, hi: int): int\n    var count = 0\n    for i in lo..hi\n        count = count + 1\n    return count\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(10), Value::Int(1)]),
        Ok(Some(Value::Int(0)))
    );
}

#[test]
fn a_runtime_zero_step_faults() {
    // A zero step would never progress; a non-literal zero faults rather than hangs.
    let source =
        "fn f(step: int): int\n    for i in 1..10 by step\n        return i\n    return 0\n";
    let result = eval_source(source, "f", vec![Value::Int(0)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn a_decimal_range_steps_by_a_decimal() {
    // `0.0..1.0 by 0.25` yields 0.0, 0.25, 0.50, 0.75 (exclusive end): four values.
    assert_eq!(
        eval_source(
            "fn f(): int\n    var count = 0\n    for x in 0.0..1.0 by 0.25\n        count = count + 1\n    return count\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Int(4)))
    );
}

#[test]
fn a_date_range_steps_one_calendar_day_across_a_leap_boundary() {
    // 2024-02-27..=2024-03-02 by 1.day lands on Feb 28, 29, Mar 1, 2 in a leap year:
    // calendar arithmetic, not 30-day months.
    let program = checked_program(
        "pub fn f(): string\n    var acc = \"\"\n    for d in std::clock::parseDate(\"2024-02-27\")..=std::clock::parseDate(\"2024-03-02\") by 1.day\n        acc = acc _ std::clock::formatDate(d) _ \";\"\n    return acc\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str(
            "2024-02-27;2024-02-28;2024-02-29;2024-03-01;2024-03-02;".into()
        ))
    );
}

#[test]
fn a_date_range_rejects_a_sub_day_step() {
    checker_rejects(
        "pub fn f(): int\n    var count = 0\n    for d in std::clock::parseDate(\"2024-01-01\")..std::clock::parseDate(\"2024-01-10\") by 1.hour\n        count = count + 1\n    return count\n",
        "check.range",
    );
}

#[test]
fn an_instant_range_steps_by_a_duration_in_utc() {
    // Stepping an instant range by 1.hour from noon to 3pm UTC yields noon, 1pm, 2pm
    // (exclusive end): three instants.
    let program = checked_program(
        "pub fn f(): string\n    var acc = \"\"\n    for t in std::clock::parseInstant(\"2024-03-10T12:00:00Z\")..std::clock::parseInstant(\"2024-03-10T15:00:00Z\") by 1.hour\n        acc = acc _ std::clock::formatInstant(t) _ \";\"\n    return acc\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str(
            "2024-03-10T12:00:00Z;2024-03-10T13:00:00Z;2024-03-10T14:00:00Z;".into()
        ))
    );
}

#[test]
fn break_exits_the_loop() {
    let source = "fn f(n: int): int\n    var i = 0\n    while true\n        if i > n\n            break\n        i = i + 1\n    return i\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(3)]),
        Ok(Some(Value::Int(4)))
    );
}

#[test]
fn continue_skips_to_the_next_iteration() {
    let source = "fn f(n: int): int\n    var c = 0\n    for i in 1..=n\n        if i == 1\n            continue\n        c = c + 1\n    return c\n";
    // The first iteration is skipped; the rest count.
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(3)]),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn a_labeled_break_exits_the_outer_loop() {
    let source = "fn f(): int\n    var count = 0\n    outer: for i in 1..=3\n        for j in 1..=3\n            if j == 2\n                break outer\n            count = count + 1\n    return count\n";
    // i=1: j=1 counts (1), j=2 breaks the outer loop entirely.
    assert_eq!(
        eval_source(source, "f", Vec::new()),
        Ok(Some(Value::Int(1)))
    );
}

#[test]
fn an_unlabeled_break_exits_only_the_inner_loop() {
    let source = "fn f(): int\n    var count = 0\n    for i in 1..=2\n        for j in 1..=3\n            if j == 2\n                break\n            count = count + 1\n    return count\n";
    // Each outer iteration counts j=1 then breaks the inner loop: 2 total.
    assert_eq!(
        eval_source(source, "f", Vec::new()),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn break_outside_a_loop_is_an_error() {
    checker_rejects("fn f()\n    break\n", "check.loop_control_flow");
}

#[test]
fn returns_a_string_literal() {
    assert_eq!(
        eval_source("fn f(): string\n    return \"hello\"\n", "f", Vec::new()),
        Ok(Some(Value::Str("hello".into())))
    );
}

#[test]
fn concatenates_strings() {
    // Marrow spells string concatenation `_`.
    assert_eq!(
        eval_source(
            "fn greet(name: string): string\n    return \"Hello, \" _ name\n",
            "greet",
            vec![Value::Str("World".into())]
        ),
        Ok(Some(Value::Str("Hello, World".into())))
    );
}

#[test]
fn compares_strings_for_equality_and_order() {
    assert_eq!(
        eval_source(
            "fn eq(a: string, b: string): bool\n    return a == b\n",
            "eq",
            vec![Value::Str("x".into()), Value::Str("x".into())]
        ),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        eval_source(
            "fn lt(a: string, b: string): bool\n    return a < b\n",
            "lt",
            vec![Value::Str("apple".into()), Value::Str("banana".into())]
        ),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn string_escapes_are_decoded() {
    assert_eq!(
        eval_source(
            "fn f(): string\n    return \"slash \\\\ quote \\\" line\\n carriage\\r tab\\t\"\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Str(
            "slash \\ quote \" line\n carriage\r tab\t".into()
        )))
    );
}

#[test]
fn unknown_string_escapes_are_rejected() {
    let result = eval_source("fn f(): string\n    return \"\\q\"\n", "f", Vec::new());
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNSUPPORTED),
        "{result:?}"
    );
}

#[test]
fn concatenation_requires_strings() {
    checker_rejects(
        "fn f(): string\n    return \"x\" _ 5\n",
        "check.operator_type",
    );
}

#[test]
fn evaluates_string_interpolation() {
    assert_eq!(
        eval_source(
            "fn f(n: int): string\n    return $\"n is {n}\"\n",
            "f",
            vec![Value::Int(5)]
        ),
        Ok(Some(Value::Str("n is 5".into())))
    );
}

#[test]
fn interpolation_renders_several_values() {
    assert_eq!(
        eval_source(
            "fn f(name: string, ok: bool): string\n    return $\"{name}={ok}\"\n",
            "f",
            vec![Value::Str("ready".into()), Value::Bool(true)]
        ),
        Ok(Some(Value::Str("ready=true".into())))
    );
}

#[test]
fn interpolation_unescapes_literal_braces() {
    assert_eq!(
        eval_source("fn f(): string\n    return $\"a {{ b\"\n", "f", Vec::new()),
        Ok(Some(Value::Str("a { b".into())))
    );
}

#[test]
fn interpolation_text_decodes_string_escapes() {
    assert_eq!(
        eval_source(
            "fn f(name: string): string\n    return $\"slash \\\\ quote \\\" {{\\n{name}\\r\\t}}\"\n",
            "f",
            vec![Value::Str("Ada".into())]
        ),
        Ok(Some(Value::Str("slash \\ quote \" {\nAda\r\t}".into())))
    );
}

#[test]
fn unknown_interpolation_escapes_are_rejected() {
    let result = eval_source("fn f(): string\n    return $\"\\q\"\n", "f", Vec::new());
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNSUPPORTED),
        "{result:?}"
    );
}

#[test]
fn interpolation_rejects_later_bad_escapes_before_evaluating_holes() {
    let program = checked_program(
        "fn boom(): decimal\n    return 1.0 / 0.0\n\n\
         fn f(): string\n    return $\"{boom()}\\q\"\n",
    );
    let result = run(checked_entry!(&program, "test::f"));
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNSUPPORTED),
        "{result:?}"
    );
}

#[test]
fn run_entry_evaluates_a_function_by_qualified_name() {
    let program = checked_program("fn add(a: int, b: int): int\n    return a + b\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::add",
            Value::Int(2),
            Value::Int(3)
        )),
        Ok(Some(Value::Int(5)))
    );
}

#[test]
fn run_entry_rejects_host_values_that_do_not_match_checked_parameters() {
    let program = checked_program("fn needs_int(n: int)\n    print(\"ran\")\n");
    let error = rejected_entry_call(&program, "test::needs_int", vec![Value::Str("x".into())]);

    assert_eq!(error.code, RUN_TYPE);
    assert!(error.message.contains("entry argument `n`"));
}

#[test]
fn run_entry_rejects_private_functions_as_entries() {
    let program = checked_program_modules(&["module a\n\nfn secret(): int\n    return 1\n"]);
    let error = rejected_entry_call(&program, "a::secret", vec![]);

    assert_eq!(error.code, "run.private_function");
}

#[test]
fn run_entry_rejects_ambiguous_bare_entries() {
    let program = checked_program_modules(&[
        "module a\n\npub fn widget(): int\n    return 1\n",
        "module b\n\npub fn widget(): int\n    return 2\n",
    ]);
    let error = rejected_entry_call(&program, "widget", vec![]);

    assert_eq!(error.code, "run.ambiguous_function");
}

#[test]
fn run_entry_rejects_host_values_for_moded_parameters() {
    let program = checked_program("fn fill(out n: int)\n    n = 1\n");
    let error = rejected_entry_call(&program, "test::fill", vec![Value::Int(0)]);

    assert_eq!(error.code, RUN_TYPE);
    assert!(error.message.contains("out/inout"));
}

#[test]
fn run_entry_rejects_host_values_for_identity_parameters() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         fn load(id: Id(^books))\n    print(\"ran\")\n",
    );
    let error = rejected_entry_call(&program, "test::load", vec![Value::Int(1)]);

    assert_eq!(error.code, RUN_TYPE);
    assert!(error.message.contains("entry argument `id`"));
}

#[test]
fn run_entry_rejects_host_values_for_resource_parameters() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         fn show(book: Book)\n    print(\"ran\")\n",
    );
    let error = rejected_entry_call(&program, "test::show", vec![Value::Resource(vec![])]);

    assert_eq!(error.code, RUN_TYPE);
    assert!(error.message.contains("entry argument `book`"));
}

#[test]
fn a_function_can_call_another() {
    let program = checked_program(
        "fn double(n: int): int\n    return n + n\n\nfn quad(n: int): int\n    return double(n) + double(n)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::quad", Value::Int(3))),
        Ok(Some(Value::Int(12)))
    );
}

#[test]
fn functions_recurse() {
    let program = checked_program(
        "fn fact(n: int): int\n    if n <= 1\n        return 1\n    return n * fact(n - 1)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::fact", Value::Int(5))),
        Ok(Some(Value::Int(120)))
    );
}

#[test]
fn a_void_call_runs_as_a_statement() {
    let program = checked_program(
        "fn note(n: int)\n    const doubled = n + n\n\nfn caller(): int\n    note(3)\n    return 2\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::caller")),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn using_a_void_call_as_a_value_is_rejected() {
    checker_rejects(
        "fn note(n: int)\n    const doubled = n + n\n\nfn caller(): int\n    return note(3)\n",
        "check.untyped_value",
    );
}

#[test]
fn an_unknown_function_is_rejected() {
    let program = checked_program("fn f(): int\n    return 1\n");
    let error = rejected_entry_call(&program, "test::missing", Vec::new());
    assert_eq!(error.code, RUN_UNKNOWN_FUNCTION);
}

#[test]
fn values_and_entries_over_an_index_branch_are_unsupported() {
    let resource = "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\n";
    for builtin in ["values", "entries"] {
        checker_rejects(
            &format!("{resource}fn f()\n    {builtin}(^books.byShelf(\"x\"))\n"),
            "check.collection_unsupported",
        );
    }
}

#[test]
fn a_unique_index_lookup_loop_skips_an_absent_entry() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\nfn f()\n    for id in ^books.byIsbn(\"978-0\")\n        print($\"{id}\")\n",
    );

    let outcome = run_full(checked_entry!(&program, "test::f")).expect("run");
    assert_eq!(outcome.output, "");
}

#[test]
fn print_writes_a_line_to_output() {
    let program = checked_program("fn main()\n    print($\"hello {1}\")\n");
    let outcome = run_full(checked_entry!(&program, "test::main")).expect("run");
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "hello 1\n");
}

#[test]
fn write_does_not_add_a_newline() {
    let program = checked_program("fn main()\n    write(\"a\")\n    write(\"b\")\n");
    let outcome = run_full(checked_entry!(&program, "test::main")).expect("run");
    assert_eq!(outcome.output, "ab");
}

#[test]
fn output_accumulates_across_calls() {
    let program = checked_program(
        "fn greet(name: string)\n    print($\"hi {name}\")\n\nfn main()\n    greet(\"a\")\n    greet(\"b\")\n",
    );
    let outcome = run_full(checked_entry!(&program, "test::main")).expect("run");
    assert_eq!(outcome.output, "hi a\nhi b\n");
}

#[test]
fn print_takes_one_argument() {
    let program = checked_program("fn main()\n    print()\n");
    let result = run_full(checked_entry!(&program, "test::main"));
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

/// A program with a saved `Book` resource and functions that read a title.
const BOOK_READER: &str = "\
resource Book at ^books(id: int)
    required title: string

fn title_of(id: int): string
    return ^books(id).title

fn show(id: int)
    print($\"title: {^books(id).title}\")
";

fn store_with_title(program: &CheckedRuntimeProgram, id: i64, title: &str) -> TreeStore {
    let store = empty_store();
    write_data_value(
        program,
        &store,
        "books",
        &[SavedKey::Int(id)],
        &data_path(program, "books", &["title"]),
        SavedValue::Str(title.into()),
    );
    store
}

#[test]
fn reads_a_scalar_field_from_saved_data() {
    let program = checked_program(BOOK_READER);
    let store = store_with_title(&program, 1, "Mort");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::title_of", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(outcome.value, Some(Value::Str("Mort".into())));
}

#[test]
fn reading_an_absent_field_is_an_error() {
    let program = checked_program(BOOK_READER);
    let store = TreeStore::memory(); // empty: the title is absent
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::title_of", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_ABSENT),
        "{result:?}"
    );
}

#[test]
fn a_saved_read_interpolates_and_prints() {
    let program = checked_program(BOOK_READER);
    let store = store_with_title(&program, 7, "Mort");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::show", Value::Int(7)),
    )
    .expect("run");
    assert_eq!(outcome.output, "title: Mort\n");
}

#[test]
fn whole_resource_read_rejects_missing_required_durable_fields() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    required shelf: string\n\nfn read(id: int): Book\n    var fallback: Book\n    fallback.title = \"\"\n    fallback.shelf = \"\"\n    return ^books(id) ?? fallback\n",
    );
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["shelf"]),
        SavedValue::Str("fiction".into()),
    );
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::read", Value::Int(1)),
    );

    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

/// A program that writes and reads a `Book` title.
const BOOK_WRITER: &str = "\
resource Book at ^books(id: int)
    required title: string

fn set_title(id: int, t: string)
    ^books(id).title = t

fn title_of(id: int): string
    return ^books(id).title
";

#[test]
fn a_field_write_updates_saved_data() {
    let program = checked_program(BOOK_WRITER);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_title",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("write");
    // Read it back through the runtime against the same store.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::title_of", Value::Int(1)),
    )
    .expect("read");
    assert_eq!(outcome.value, Some(Value::Str("Mort".into())));
}

#[test]
fn out_of_transaction_field_write_rejects_partial_required_record() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n\
         \x20   required name: string\n\
         \x20   shelf: string\n\n\
         fn set_shelf(id: int)\n\
         \x20   ^items(id).shelf = \"fiction\"\n\n\
         fn has_item(id: int): bool\n\
         \x20   return exists(^items(id))\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::set_shelf", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_absent"),
        "{result:?}"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("presence check")
        .value,
        Some(Value::Bool(false)),
        "the rejected sparse write must leave no partial record"
    );
}

#[test]
fn out_of_transaction_group_field_write_rejects_partial_required_record() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   binding\n\
         \x20       cover: string\n\n\
         fn set_cover(id: int)\n\
         \x20   ^books(id).binding.cover = \"hard\"\n\n\
         fn has_book(id: int): bool\n\
         \x20   return exists(^books(id))\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::set_cover", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_absent"),
        "{result:?}"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .expect("presence check")
        .value,
        Some(Value::Bool(false)),
        "the rejected group-field write must leave no partial record"
    );
}

#[test]
fn transaction_commit_rejects_partial_required_record() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n\
         \x20   required name: string\n\
         \x20   shelf: string\n\n\
         fn set_shelf(id: int)\n\
         \x20   transaction\n\
         \x20       ^items(id).shelf = \"fiction\"\n\n\
         fn has_item(id: int): bool\n\
         \x20   return exists(^items(id))\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::set_shelf", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_absent"),
        "{result:?}"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("presence check")
        .value,
        Some(Value::Bool(false)),
        "the rejected transaction must roll back the partial record"
    );
}

#[test]
fn transaction_required_field_checks_cross_helper_calls() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n\
         \x20   required name: string\n\
         \x20   shelf: string\n\n\
         fn set_shelf(id: int)\n\
         \x20   ^items(id).shelf = \"fiction\"\n\n\
         fn create(id: int)\n\
         \x20   transaction\n\
         \x20       set_shelf(id)\n\
         \x20       ^items(id).name = \"Mort\"\n\n\
         fn name_of(id: int): string\n\
         \x20   return ^items(id).name\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::create", Value::Int(1)),
    )
    .expect("commit");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::name_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into()))
    );
}

#[test]
fn nested_transaction_defers_required_check_until_outer_commit() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n\
         \x20   required name: string\n\
         \x20   shelf: string\n\n\
         fn create(id: int)\n\
         \x20   transaction\n\
         \x20       transaction\n\
         \x20           ^items(id).shelf = \"fiction\"\n\
         \x20       ^items(id).name = \"Mort\"\n\n\
         fn name_of(id: int): string\n\
         \x20   return ^items(id).name\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::create", Value::Int(1)),
    )
    .expect("commit");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::name_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into()))
    );
}

#[test]
fn a_mistyped_field_write_is_rejected() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn bad(id: int)\n    ^books(id).title = 5\n",
        "check.assignment_type",
    );
}

/// A program that queries saved `Book` data with `exists` and the `??`
/// absence-default.
const BOOK_QUERY: &str = "\
resource Book at ^books(id: int)
    required title: string
    subtitle: string

fn has_book(id: int): bool
    return exists(^books(id))

fn has_title(id: int): bool
    return exists(^books(id).title)

fn subtitle_or(id: int, fallback: string): string
    return ^books(id).subtitle ?? fallback
";

#[test]
fn exists_reports_record_and_field_presence() {
    let program = checked_program(BOOK_QUERY);
    let store = store_with_title(&program, 1, "Mort");
    let value = |entry, id| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(id)))
            .expect("run")
            .value
    };
    // Record 1 exists (it has the title child); record 2 does not.
    assert_eq!(value("test::has_book", 1), Some(Value::Bool(true)));
    assert_eq!(value("test::has_book", 2), Some(Value::Bool(false)));
    // Its title field is present; its sparse subtitle is not.
    assert_eq!(value("test::has_title", 1), Some(Value::Bool(true)));
}

#[test]
fn coalesce_returns_the_default_for_an_absent_field() {
    let program = checked_program(BOOK_QUERY);
    let store = store_with_title(&program, 1, "Mort"); // subtitle is absent
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::subtitle_or",
            Value::Int(1),
            Value::Str("(none)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(none)".into())));
}

#[test]
fn coalesce_returns_the_value_when_present() {
    let program = checked_program(BOOK_QUERY);
    let store = store_with_title(&program, 1, "Mort");
    // Populate the sparse subtitle directly.
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["subtitle"]),
        SavedValue::Str("A Discworld Novel".into()),
    );
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::subtitle_or",
            Value::Int(1),
            Value::Str("(none)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("A Discworld Novel".into())));
}

/// A `Patient` with an unkeyed `name` group, queried with a `?.` chain defaulted
/// by `??`. The chain `^patients(id)?.name?.first` reads the first name when the
/// whole record, the `name` group, and the field are all populated, and is absent
/// (caught by `??`) when any step along the way is missing.
const PATIENT_CHAIN: &str = "\
resource Patient at ^patients(id: int)
    name
        first: string
        last: string

fn first_name_or(id: int, fallback: string): string
    return ^patients(id)?.name?.first ?? fallback
";

fn store_with_first_name(program: &CheckedRuntimeProgram, id: i64, first: &str) -> TreeStore {
    let store = empty_store();
    write_data_value(
        program,
        &store,
        "patients",
        &[SavedKey::Int(id)],
        &data_path(program, "patients", &["name", "first"]),
        SavedValue::Str(first.into()),
    );
    store
}

#[test]
fn optional_chain_with_default_reads_a_present_value() {
    let program = checked_program(PATIENT_CHAIN);
    let store = store_with_first_name(&program, 1, "Granny");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::first_name_or",
            Value::Int(1),
            Value::Str("(unknown)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("Granny".into())));
}

#[test]
fn optional_chain_defaults_when_the_record_is_absent() {
    let program = checked_program(PATIENT_CHAIN);
    // Record 2 was never written: the whole record is absent, so the chain
    // short-circuits and `??` supplies the default.
    let store = store_with_first_name(&program, 1, "Granny");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::first_name_or",
            Value::Int(2),
            Value::Str("(unknown)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(unknown)".into())));
}

#[test]
fn optional_chain_defaults_when_an_intermediate_field_is_absent() {
    let program = checked_program(PATIENT_CHAIN);
    // The record exists (it has a `last` name) but `name.first` does not: the
    // final hop short-circuits the chain, and `??` supplies the default.
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "patients",
        &[SavedKey::Int(1)],
        &data_path(&program, "patients", &["name", "last"]),
        SavedValue::Str("Weatherwax".into()),
    );
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::first_name_or",
            Value::Int(1),
            Value::Str("(unknown)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(unknown)".into())));
}

#[test]
fn an_unguarded_optional_chain_that_ends_absent_is_rejected() {
    checker_rejects(
        "resource Patient at ^patients(id: int)\n    name\n        first: string\n        last: string\n\nfn first_name(id: int): string\n    return ^patients(id)?.name?.first\n",
        "check.bare_maybe_present_read",
    );
}

#[test]
fn next_id_allocates_past_the_highest_record() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn fresh(): Id(^books)\n    return nextId(^books)\n",
    );
    let store = empty_store();
    // Empty root: the next id is 1.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::fresh"))
            .expect("run")
            .value,
        Some(Value::Int(1))
    );
    // Seed records 1 and 4; the next id is one past the highest.
    for id in [1, 4] {
        write_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(id)],
            &data_path(&program, "books", &["title"]),
            SavedValue::Str("t".into()),
        );
    }
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::fresh"))
            .expect("run")
            .value,
        Some(Value::Int(5))
    );
}

#[test]
fn next_id_skips_ahead_after_restore() {
    // After a restore the store may hold records far above any contiguous run.
    // `nextId` chooses one past the highest existing key, never reusing a gap.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn fresh(): Id(^books)\n    return nextId(^books)\n",
    );
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(900)],
        &data_path(&program, "books", &["title"]),
        SavedValue::Str("t".into()),
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::fresh"))
            .expect("run")
            .value,
        Some(Value::Int(901))
    );
}

/// `nextId` over a composite-identity root faults with `write.next_id_unsupported`
/// rather than inventing a bogus `Int(1)`: composite identities have no default
/// allocation policy.
#[test]
fn next_id_over_a_composite_root_faults() {
    checker_rejects(
        "resource Enrollment at ^enrollments(studentId: int, courseId: int)\n    required grade: string\n\nfn fresh(): int\n    return nextId(^enrollments)\n",
        "check.next_id_requires_single_int",
    );
}

/// `nextId` over a keyless singleton root faults: a singleton has no generated
/// identity to allocate.
#[test]
fn next_id_over_a_singleton_root_faults() {
    checker_rejects(
        "resource Settings at ^settings\n    required theme: string\n\nfn fresh(): int\n    return nextId(^settings)\n",
        "check.next_id_requires_single_int",
    );
}

/// `nextId` over a single non-integer (string) identity key faults: only an
/// `int` identity has the default policy.
#[test]
fn next_id_over_a_string_keyed_root_faults() {
    checker_rejects(
        "resource Tag at ^tags(slug: string)\n    required name: string\n\nfn fresh(): int\n    return nextId(^tags)\n",
        "check.next_id_requires_single_int",
    );
}

/// `nextId` of a saved root no store declares is a `run.unsupported`: there is
/// no schema to decide an allocation policy (mirrors `eval_append`'s unknown-root
/// path).
#[test]
fn next_id_over_an_undeclared_root_is_unsupported() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn fresh(): int\n    return nextId(^bogus)\n",
        "check.untyped_value",
    );
}

#[test]
fn delete_removes_a_record() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn set_title(id: int, t: string)\n    ^books(id).title = t\n\nfn remove(id: int)\n    delete ^books(id)\n\nfn has_book(id: int): bool\n    return exists(^books(id))\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_title",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(true))
    );
    run_entry(
        &store,
        checked_entry!(&program, "test::remove", Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(false)),
        "the record is gone after delete"
    );
}

#[test]
fn delete_removes_a_sparse_field_and_leaves_a_sibling() {
    // `delete ^books(id).subtitle` removes that field; a sibling field survives.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    subtitle: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).subtitle = \"A Discworld Novel\"\n\nfn drop_subtitle(id: int)\n    delete ^books(id).subtitle\n\nfn has_subtitle(id: int): bool\n    return exists(^books(id).subtitle)\n\nfn title_of(id: int): string\n    return ^books(id).title\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_subtitle", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(true))
    );
    run_entry(
        &store,
        checked_entry!(&program, "test::drop_subtitle", Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_subtitle", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(false)),
        "the field is gone after delete"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title_of", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Str("Mort".into())),
        "the sibling field survives"
    );
}

#[test]
fn deleting_an_indexed_field_removes_its_index_entry() {
    // `delete ^books(id).shelf` where `shelf` feeds `byShelf` tears down the entry,
    // so a later `keys(^books.byShelf(...))` no longer yields the record.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\nfn add(id: int, t: string, s: string)\n    ^books(id).title = t\n    ^books(id).shelf = s\n\nfn drop_shelf(id: int)\n    delete ^books(id).shelf\n\nfn count_on(shelf: string): int\n    var c = 0\n    for id in keys(^books.byShelf(shelf))\n        c = c + 1\n    return c\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("fiction".into()),
        ),
    )
    .expect("add");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::count_on", Value::Str("fiction".into()))
        )
        .expect("run")
        .value,
        Some(Value::Int(1))
    );
    run_entry(
        &store,
        checked_entry!(&program, "test::drop_shelf", Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::count_on", Value::Str("fiction".into()))
        )
        .expect("run")
        .value,
        Some(Value::Int(0)),
        "the index entry the deleted field fed is gone"
    );
}

#[test]
fn deleting_a_required_field_is_rejected() {
    // A required field can only go away when its entry/resource is deleted.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn drop_title(id: int)\n    delete ^books(id).title\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::drop_title", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_field"),
        "{result:?}"
    );
}

#[test]
fn deleting_a_layer_entry_leaves_other_entries() {
    // `delete ^books(id).versions(v)` removes one group-entry subtree; siblings
    // survive. Read each entry's `.title` to prove it: the deleted entry's title
    // falls back to the `??` default, the survivor's stays intact.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n    versions(version: int)\n        required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).versions(1).title = \"first\"\n    ^books(id).versions(2).title = \"second\"\n\nfn drop_version(id: int, v: int)\n    delete ^books(id).versions(v)\n\nfn version_title(id: int, v: int): string\n    return ^books(id).versions(v).title ?? \"<gone>\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(&program, "test::drop_version", Value::Int(1), Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::version_title",
                Value::Int(1),
                Value::Int(1)
            )
        )
        .expect("run")
        .value,
        Some(Value::Str("<gone>".into())),
        "the deleted version's subtree is gone"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::version_title",
                Value::Int(1),
                Value::Int(2)
            )
        )
        .expect("run")
        .value,
        Some(Value::Str("second".into())),
        "the other version survives"
    );
}

#[test]
fn deleting_a_keyed_leaf_entry_leaves_other_entries() {
    // `delete ^books(id).tags(pos)` removes one keyed-leaf entry; siblings survive.
    // `count(^books(id).tags)` counts the remaining entries; reading the deleted
    // one is an absent-element error while the survivor reads back.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).tags(1) = \"fiction\"\n    ^books(id).tags(2) = \"funny\"\n\nfn drop_tag(id: int, pos: int)\n    delete ^books(id).tags(pos)\n\nfn tag_count(id: int): int\n    return count(^books(id).tags)\n\nfn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(&program, "test::drop_tag", Value::Int(1), Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_count", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Int(1)),
        "one tag remains after deleting one of two"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(1), Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str(String::new()))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(1), Value::Int(2))
        )
        .expect("run")
        .value,
        Some(Value::Str("funny".into())),
        "the other tag survives"
    );
}

#[test]
fn a_transaction_commits_on_normal_exit() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn save(id: int)\n    transaction\n        ^books(id).title = \"kept\"\n\nfn title_of(id: int): string\n    return ^books(id).title\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::save", Value::Int(1)),
    )
    .expect("commit");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title_of", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Str("kept".into()))
    );
}

#[test]
fn a_transaction_rolls_back_on_an_escaping_error() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn risky(id: int)\n    transaction\n        ^books(id).title = \"staged\"\n        const x = 1 / 0\n\nfn has_book(id: int): bool\n    return exists(^books(id))\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::risky", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_DIVIDE_BY_ZERO),
        "{result:?}"
    );
    // The write staged before the error was rolled back.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(false)),
        "the staged write rolled back with the transaction"
    );
}

/// A `Book` with a unique `isbn` index plus helpers that seed a record, attempt
/// a conflicting write under `try`/`catch`, and read a field back. Used by the
/// recoverable-write-fault tests.
const UNIQUE_RECOVERY: &str = "\
resource Book at ^books(id: int)
    required title: string
    isbn: string

    index byIsbn(isbn) unique

fn seed(id: int, t: string, isbn: string)
    ^books(id).title = t
    ^books(id).isbn = isbn

fn claimOrCode(id: int, isbn: string): string
    try
        ^books(id).isbn = isbn
    catch err: Error
        return err.code
    return \"written\"

fn claim(id: int, isbn: string)
    ^books(id).isbn = isbn

fn recover(id: int, isbn: string, fallback: string): string
    try
        ^books(id).isbn = isbn
    catch err: Error
        ^books(id).title = fallback
    return ^books(id).title ?? \"\"

fn titleOf(id: int): string
    return ^books(id).title ?? \"\"

fn isbnOf(id: int): string
    return ^books(id).isbn ?? \"\"

fn ownerOf(isbn: string): Id(^books)
    for id in ^books.byIsbn(isbn)
        return id
    throw Error(code: \"test.missing_isbn\", message: \"missing isbn\")
";

#[test]
fn a_unique_conflict_is_catchable_and_binds_the_dotted_code() {
    // A unique-index conflict surfaces as a catchable Error, so a `try`/`catch`
    // inside the writing function binds it by its `write.unique_conflict` code
    // and the function continues normally.
    let program = checked_program(UNIQUE_RECOVERY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    // Book 2 tries to claim book 1's isbn: a unique conflict the catch binds.
    let caught = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::claimOrCode",
            Value::Int(2),
            Value::Str("978-0".into())
        ),
    )
    .expect("caught")
    .value;
    assert_eq!(caught, Some(Value::Str("write.unique_conflict".into())));
}

#[test]
fn a_caught_unique_conflict_lets_following_code_run_and_did_not_write() {
    // After catching the conflict, code keeps running (writes a fallback) and the
    // rejected write left no effect: book 2 still owns its original isbn.
    let program = checked_program(UNIQUE_RECOVERY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    let title = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::recover",
            Value::Int(2),
            Value::Str("978-0".into()),
            Value::Str("fallback".into()),
        ),
    )
    .expect("recovered")
    .value;
    assert_eq!(title, Some(Value::Str("fallback".into())), "catch body ran");
    // The rejected write left no effect: book 2 still has its original isbn and the
    // unique index still maps the conflicting isbn to book 1, not book 2.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::isbnOf", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("978-9".into())),
        "book 2's isbn was not overwritten",
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::ownerOf", Value::Str("978-0".into()))
        )
        .expect("read")
        .value,
        Some(Value::Int(1)),
        "the unique index still points at book 1",
    );
}

#[test]
fn an_uncaught_unique_conflict_keeps_its_dotted_code() {
    // Preserve uncaught behavior: a conflict that escapes the entry surfaces with
    // its own `write.unique_conflict` code (not run.uncaught_error), exactly as
    // before it became catchable.
    let program = checked_program(UNIQUE_RECOVERY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::claim",
            Value::Int(2),
            Value::Str("978-0".into())
        ),
    );
    assert_eq!(result.expect_err("conflict").code, "write.unique_conflict",);
}

#[test]
fn a_unique_conflict_inside_a_transaction_can_be_caught_and_continue() {
    // A conflict caught inside a transaction has no effect, and the transaction
    // continues and commits its other writes.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\nfn seed(id: int, t: string, isbn: string)\n    ^books(id).title = t\n    ^books(id).isbn = isbn\n\nfn run_it(id: int, isbn: string, t: string)\n    transaction\n        try\n            ^books(id).isbn = isbn\n        catch err: Error\n            ^books(id).title = t\n\nfn titleOf(id: int): string\n    return ^books(id).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::run_it",
            Value::Int(2),
            Value::Str("978-0".into()),
            Value::Str("after".into()),
        ),
    )
    .expect("transaction commits after catching");
    // The transaction's other write (the title) committed.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::titleOf", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("after".into())),
    );
}

#[test]
fn a_caught_write_fault_does_not_leak_into_a_later_fault() {
    // After a `try` catches a write fault, the stashed Error is cleared, so a later
    // genuine fault (divide-by-zero) still faults rather than being miscaught.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\nfn seed(id: int, t: string, isbn: string)\n    ^books(id).title = t\n    ^books(id).isbn = isbn\n\nfn run_it(): int\n    try\n        ^books(2).isbn = \"978-0\"\n    catch err: Error\n        write(\"caught\")\n    const boom = 1 / 0\n    return 0\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::run_it"))
            .unwrap_err()
            .code,
        RUN_DIVIDE_BY_ZERO,
    );
}

#[test]
fn an_unguarded_absent_element_read_is_rejected() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    title: string\n\nfn titleOrCode(id: int): string\n    try\n        return ^books(id).title\n    catch err: Error\n        return err.code\n",
        "check.bare_maybe_present_read",
    );
}

#[test]
fn reads_inside_a_transaction_see_earlier_writes() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn rww(id: int): string\n    transaction\n        ^books(id).title = \"fresh\"\n        return ^books(id).title\n",
    );
    let store = TreeStore::memory();
    let outcome =
        run_entry(&store, checked_entry!(&program, "test::rww", Value::Int(1))).expect("run");
    assert_eq!(outcome.value, Some(Value::Str("fresh".into())));
}

#[test]
fn append_writes_at_the_next_position() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n",
    );
    let store = TreeStore::memory();
    let appended = |t: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add_tag",
                Value::Int(5),
                Value::Str(t.into())
            ),
        )
        .expect("run")
        .value
    };
    // Successive appends take positions 1 then 2 (no hole-filling).
    assert_eq!(appended("a"), Some(Value::Int(1)));
    assert_eq!(appended("b"), Some(Value::Int(2)));
    // The values landed at `^books(5).tags(1)` and `tags(2)`.
    let tag = |pos: i64| -> Option<SavedValue> {
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(5)],
            &keyed_data_path(
                &program,
                "books",
                &[("tags", vec![SavedKey::Int(pos)])],
                &[],
            ),
            ScalarType::Str,
        )
    };
    assert_eq!(tag(1), Some(SavedValue::Str("a".into())));
    assert_eq!(tag(2), Some(SavedValue::Str("b".into())));
}

#[test]
fn appends_then_reads_back_keyed_leaf_entries() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n\nfn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_tag",
            Value::Int(5),
            Value::Str("a".into())
        ),
    )
    .expect("append");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_tag",
            Value::Int(5),
            Value::Str("b".into())
        ),
    )
    .expect("append");
    let tag = |pos: i64| {
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(5), Value::Int(pos)),
        )
        .expect("read")
        .value
    };
    assert_eq!(tag(1), Some(Value::Str("a".into())));
    assert_eq!(tag(2), Some(Value::Str("b".into())));
    assert_eq!(tag(3), Some(Value::Str(String::new())));
}

#[test]
fn explicit_keyed_leaf_write_then_reads_back() {
    // `^books(id).tags(pos) = value` writes one keyed-leaf entry directly, and a
    // string-keyed leaf `scores(key) = value` writes through the same path.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n    scores(key: string): int\n\nfn set_tag(id: int, pos: int, t: string)\n    ^books(id).tags(pos) = t\n\nfn set_score(id: int, key: string, n: int)\n    ^books(id).scores(key) = n\n\nfn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n\nfn score_at(id: int, key: string): int\n    return ^books(id).scores(key) ?? 0\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_tag",
            Value::Int(5),
            Value::Int(3),
            Value::Str("fiction".into())
        ),
    )
    .expect("explicit keyed-leaf write");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_score",
            Value::Int(5),
            Value::Str("alice".into()),
            Value::Int(7)
        ),
    )
    .expect("string-keyed leaf write");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(5), Value::Int(3))
        )
        .expect("read")
        .value,
        Some(Value::Str("fiction".into()))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::score_at",
                Value::Int(5),
                Value::Str("alice".into())
            )
        )
        .expect("read")
        .value,
        Some(Value::Int(7))
    );
}

#[test]
fn explicit_keyed_leaf_write_creates_a_hole_that_append_skips() {
    // An explicit write past the dense range leaves a hole; append chooses one
    // past the highest positive key, not the first gap.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn set_tag(id: int, pos: int, t: string)\n    ^books(id).tags(pos) = t\n\nfn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n\nfn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n",
    );
    let store = TreeStore::memory();
    // Write position 5 directly, leaving 1..=4 as holes.
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_tag",
            Value::Int(9),
            Value::Int(5),
            Value::Str("hi".into())
        ),
    )
    .expect("explicit write");
    // Append lands at 6 (one past the highest positive key), skipping the holes.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add_tag",
                Value::Int(9),
                Value::Str("next".into())
            )
        )
        .expect("append")
        .value,
        Some(Value::Int(6))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(9), Value::Int(6))
        )
        .expect("read")
        .value,
        Some(Value::Str("next".into()))
    );
}

/// A program that indexes books by shelf and traverses the index with `keys`.
const BOOK_SHELF: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

fn count_on(shelf: string): int
    var c = 0
    for id in keys(^books.byShelf(shelf))
        c = c + 1
    return c

fn count_via_bare_index(): int
    var c = 0
    for shelf in ^books.byShelf
        for id in ^books.byShelf(shelf)
            c = c + 1
    return c

fn reshelve_while_iterating()
    for id in keys(^books.byShelf(\"fiction\"))
        ^books(id).shelf = \"history\"

fn reshelve_while_iterating_direct()
    for id in ^books.byShelf(\"fiction\")
        ^books(id).shelf = \"history\"

fn titles_on(shelf: string)
    for id in ^books.byShelf(shelf)
        print(^books(id).title)
";

#[test]
fn iterates_index_keys() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ),
        )
        .expect("add");
    };
    add(1, "Mort", "fiction");
    add(2, "Sourcery", "fiction");
    add(3, "Guards", "history");

    let count = |shelf: &str| {
        run_entry(
            &store,
            checked_entry!(&program, "test::count_on", Value::Str(shelf.into())),
        )
        .expect("count")
        .value
    };
    assert_eq!(count("fiction"), Some(Value::Int(2)));
    assert_eq!(count("history"), Some(Value::Int(1)));
    assert_eq!(count("romance"), Some(Value::Int(0)));
}

#[test]
fn bare_index_iteration_yields_first_level_keys() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ),
        )
        .expect("add");
    };
    add(1, "Mort", "fiction");
    add(2, "Sourcery", "fiction");
    add(3, "Guards", "history");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::count_via_bare_index"),
    )
    .expect("run");
    assert_eq!(outcome.value, Some(Value::Int(3)));
}

#[test]
fn updating_an_indexed_field_while_iterating_that_index_faults() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    for (id, title) in [(1, "Mort"), (2, "Sourcery")] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str("fiction".into()),
            ),
        )
        .expect("add");
    }

    let error = run_entry(
        &store,
        checked_entry!(&program, "test::reshelve_while_iterating"),
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_TRAVERSAL, "{error:?}");
    let remaining = run_entry(
        &store,
        checked_entry!(&program, "test::count_on", Value::Str("fiction".into())),
    )
    .expect("count")
    .value;
    assert_eq!(remaining, Some(Value::Int(2)));
}

#[test]
fn updating_an_indexed_field_while_directly_iterating_that_index_faults() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("fiction".into()),
        ),
    )
    .expect("add");

    let error = run_entry(
        &store,
        checked_entry!(&program, "test::reshelve_while_iterating_direct"),
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_TRAVERSAL, "{error:?}");
}

#[test]
fn prints_titles_in_index_key_order() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery", "fiction");
    add(1, "Mort", "fiction");

    // The index yields ids in key order (1 then 2), regardless of insert order.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::titles_on", Value::Str("fiction".into())),
    )
    .expect("run");
    assert_eq!(outcome.output, "Mort\nSourcery\n");
}

/// A program that reads, copies, and reads back whole `Book` resources.
const BOOK_COPY: &str = "\
resource Book at ^books(id: int)
    required title: string
    required shelf: string

fn read(id: int): Book
    var fallback: Book
    fallback.title = \"\"
    fallback.shelf = \"\"
    return ^books(id) ?? fallback

fn copy(from: int, to: int)
    var fallback: Book
    fallback.title = \"\"
    fallback.shelf = \"\"
    ^books(to) = ^books(from) ?? fallback

fn title_of(id: int): string
    return ^books(id).title ?? \"\"

fn shelf_of(id: int): string
    return ^books(id).shelf ?? \"\"
";

fn seed_field(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
    field: &str,
    value: &str,
) {
    write_data_value(
        program,
        store,
        "books",
        &[SavedKey::Int(id)],
        &data_path(program, "books", &[field]),
        SavedValue::Str(value.into()),
    );
}

#[test]
fn reads_a_whole_resource() {
    let program = checked_program(BOOK_COPY);
    let store = TreeStore::memory();
    seed_field(&program, &store, 1, "title", "Mort");
    seed_field(&program, &store, 1, "shelf", "fiction");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::read", Value::Int(1)),
    )
    .expect("read");
    // Present fields, in schema order.
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![
            ("title".into(), Value::Str("Mort".into())),
            ("shelf".into(), Value::Str("fiction".into())),
        ]))
    );
}

#[test]
fn constructs_a_resource_value() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20shelf: string\n\n\
         fn draft(): Book\n\
         \x20\x20\x20\x20return Book(title: \"Mort\", shelf: \"fiction\")\n",
    );
    let store = TreeStore::memory();
    let outcome = run_entry(&store, checked_entry!(&program, "test::draft")).expect("draft");
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![
            ("title".into(), Value::Str("Mort".into())),
            ("shelf".into(), Value::Str("fiction".into())),
        ]))
    );
}

#[test]
fn constructs_a_resource_value_with_a_local_resource_field() {
    let program = checked_program(
        "resource Address\n\
         \x20\x20\x20\x20city: string\n\n\
         resource Person\n\
         \x20\x20\x20\x20required name: string\n\
         \x20\x20\x20\x20address: Address\n\n\
         fn city(): string\n\
         \x20\x20\x20\x20const person = Person(name: \"Sam\", address: Address(city: \"Paris\"))\n\
         \x20\x20\x20\x20return person.address.city\n",
    );
    let store = TreeStore::memory();
    let outcome = run_entry(&store, checked_entry!(&program, "test::city")).expect("city");
    assert_eq!(outcome.value, Some(Value::Str("Paris".into())));
}

#[test]
fn constructs_a_qualified_resource_value() {
    let program = checked_program_modules(&[
        "module library\n\
         resource Book\n\
         \x20\x20\x20\x20title: string\n",
        "module app\n\
         use library\n\
         pub fn draft(): unknown\n\
         \x20\x20\x20\x20return library::Book(title: \"Mort\")\n",
    ]);
    let store = TreeStore::memory();
    let outcome = run_entry(&store, checked_entry!(&program, "app::draft")).expect("draft");
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![(
            "title".into(),
            Value::Str("Mort".into())
        )]))
    );
}

#[test]
fn constructor_field_with_qualified_resource_type_rejects_scalar() {
    checker_rejects_sources(
        &[
            "module library\n\
             resource Address\n\
             \x20\x20\x20\x20city: string\n",
            "module app\n\
             use library\n\
             resource Person\n\
             \x20\x20\x20\x20address: library::Address\n\
             fn make(): unknown\n\
             \x20\x20\x20\x20return Person(address: 1)\n",
        ],
        "check.call_argument",
    );
}

#[test]
fn resource_constructor_value_can_be_saved() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20author: string\n\n\
         fn save(): int\n\
         \x20\x20\x20\x20const draft = Book(title: \"Small Gods\", author: \"Pratchett\")\n\
         \x20\x20\x20\x20^books(1) = draft\n\
         \x20\x20\x20\x20return count(^books)\n\n\
         fn title(): string\n\
         \x20\x20\x20\x20return ^books(1).title\n",
    );
    let store = TreeStore::memory();
    let saved = run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    assert_eq!(saved.value, Some(Value::Int(1)));
    let title = run_entry(&store, checked_entry!(&program, "test::title")).expect("title");
    assert_eq!(title.value, Some(Value::Str("Small Gods".into())));
}

#[test]
fn resource_constructor_optional_coalesce_is_checker_rejected() {
    checker_rejects(
        "resource Profile\n\
         \x20\x20\x20\x20email: string\n\n\
         fn email(): string\n\
         \x20\x20\x20\x20return Profile()?.email ?? \"none\"\n",
        "check.operator_type",
    );
}

#[test]
fn copies_a_whole_resource() {
    let program = checked_program(BOOK_COPY);
    let store = TreeStore::memory();
    seed_field(&program, &store, 1, "title", "Mort");
    seed_field(&program, &store, 1, "shelf", "fiction");
    run_entry(
        &store,
        checked_entry!(&program, "test::copy", Value::Int(1), Value::Int(2)),
    )
    .expect("copy");
    let read = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(2)))
            .expect("run")
            .value
    };
    assert_eq!(read("test::title_of"), Some(Value::Str("Mort".into())));
    assert_eq!(read("test::shelf_of"), Some(Value::Str("fiction".into())));
}

/// A resource declaring an unkeyed nested group (`name`). Whole-resource reads
/// and writes materialize the structural group as a nested resource value.
const PATIENT_WITH_GROUP: &str = "\
resource Patient at ^patients(id: int)
    mrn: string
    name
        required first: string
        last: string

fn read(id: int): Patient
    var fallback: Patient
    fallback.mrn = \"\"
    return ^patients(id) ?? fallback

fn copy(from: int, to: int)
    var fallback: Patient
    fallback.mrn = \"\"
    ^patients(to) = ^patients(from) ?? fallback

fn first_of(id: int): string
    return ^patients(id)?.name?.first ?? \"\"
";

#[test]
fn whole_resource_read_materializes_unkeyed_groups() {
    let program = checked_program(PATIENT_WITH_GROUP);
    let store = TreeStore::memory();
    seed_patient_field(&program, &store, 1, "mrn", "A1");
    seed_patient_name_field(&program, &store, 1, "first", "Sam");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::read", Value::Int(1)),
    )
    .expect("read");
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![
            ("mrn".into(), Value::Str("A1".into())),
            (
                "name".into(),
                Value::Resource(vec![("first".into(), Value::Str("Sam".into()))])
            ),
        ]))
    );
}

#[test]
fn whole_resource_write_copies_unkeyed_group_fields() {
    let program = checked_program(PATIENT_WITH_GROUP);
    let store = TreeStore::memory();
    seed_patient_field(&program, &store, 1, "mrn", "A1");
    seed_patient_name_field(&program, &store, 1, "first", "Sam");
    run_entry(
        &store,
        checked_entry!(&program, "test::copy", Value::Int(1), Value::Int(2)),
    )
    .expect("copy");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::first_of", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("Sam".into()))
    );
}

#[test]
fn whole_resource_write_from_local_value_accepts_resources_with_unkeyed_groups() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   binding\n\
         \x20       cover: string\n\
         \x20       spine: string\n\n\
         fn save(id: int)\n\
         \x20   var book: Book\n\
         \x20   book.title = \"Small Gods\"\n\
         \x20   ^books(id) = book\n\n\
         fn title_of(id: int): string\n\
         \x20   return ^books(id).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::save", Value::Int(1)),
    )
    .expect("write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Small Gods".into()))
    );
}

fn seed_patient_field(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
    field: &str,
    value: &str,
) {
    write_data_value(
        program,
        store,
        "patients",
        &[SavedKey::Int(id)],
        &data_path(program, "patients", &[field]),
        SavedValue::Str(value.into()),
    );
}

fn seed_patient_name_field(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
    field: &str,
    value: &str,
) {
    write_data_value(
        program,
        store,
        "patients",
        &[SavedKey::Int(id)],
        &data_path(program, "patients", &["name", field]),
        SavedValue::Str(value.into()),
    );
}

/// The sample's `add` shape: allocate an id, build a local resource field by
/// field, and save it.
const BOOK_ADD: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

fn add(title: string, shelf: string): Id(^books)
    const id = nextId(^books)
    var book: Book
    book.title = title
    book.shelf = shelf
    ^books(id) = book
    return id

fn title_of(id: int): string
    return ^books(id).title ?? \"\"

fn shelf_of(id: int): string
    return ^books(id).shelf ?? \"\"
";

#[test]
fn builds_a_local_resource_and_saves_it() {
    let program = checked_program(BOOK_ADD);
    let store = TreeStore::memory();
    let id = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Str("Mort".into()),
            Value::Str("fiction".into())
        ),
    )
    .expect("add")
    .value;
    assert_eq!(id, Some(Value::Int(1)));
    let read = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(1)))
            .expect("run")
            .value
    };
    assert_eq!(read("test::title_of"), Some(Value::Str("Mort".into())));
    assert_eq!(read("test::shelf_of"), Some(Value::Str("fiction".into())));
}

#[test]
fn reads_a_local_resource_field() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\nfn echo(t: string): string\n    var book: Book\n    book.title = t\n    return book.title\n",
    );
    let store = TreeStore::memory();
    let value = run_entry(
        &store,
        checked_entry!(&program, "test::echo", Value::Str("Mort".into())),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
}

/// A program that records the run's clock instant into a saved `instant` field
/// and reads it back, exercising `std::clock::now()` through `const` and a managed
/// write.
const CLOCK_SAMPLE: &str = "\
resource Event at ^events(id: int)
    required changedAt: instant

fn record(id: int)
    const now: instant = std::clock::now()
    ^events(id).changedAt = now

fn changed_at_of(id: int): instant
    return ^events(id).changedAt
";

#[test]
fn clock_now_reads_the_host_clock_capability() {
    let program = checked_program(CLOCK_SAMPLE);
    let store = TreeStore::memory();
    // 1970-01-01T00:00:01Z, one second after the epoch.
    let host = Host::new().with_clock(1_000_000_000);
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::record", Value::Int(1)),
    )
    .expect("record");
    // The instant round-trips through the managed write and a typed read.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::changed_at_of", Value::Int(1)),
    )
    .expect("read");
    assert_eq!(outcome.value, Some(Value::Instant(1_000_000_000)));
}

#[test]
fn clock_now_without_a_clock_capability_is_a_capability_error() {
    let program = checked_program("fn t(): instant\n    return std::clock::now()\n");
    let store = TreeStore::memory();
    // Plain `run_entry` supplies no host capabilities.
    let result = run_entry(&store, checked_entry!(&program, "test::t"));
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

/// A program that reads environment variables through the three `std::env`
/// builtins: presence, lookup with a default, and a required lookup.
const ENV_SAMPLE: &str = "\
fn has(name: string): bool
    return std::env::exists(name)

fn read(name: string, fallback: string): string
    return std::env::get(name, fallback)

fn must(name: string): string
    return std::env::require(name)
";

/// A host whose environment is the test's fixed variables.
fn env_host() -> Host {
    Host::new().with_environment(HashMap::from([
        ("HOME".to_string(), "/home/marrow".to_string()),
        ("EMPTY".to_string(), String::new()),
    ]))
}

#[test]
fn env_reads_variables_from_the_host_capability() {
    let program = checked_program(ENV_SAMPLE);
    let store = TreeStore::memory();
    let host = env_host();
    let call = |entry: CheckedEntryCall| {
        run_entry_with_host(&store, &host, entry)
            .expect("env call")
            .value
    };
    // `exists` reports presence, including a present-but-empty variable.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::has",
            Value::Str("HOME".into())
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::has",
            Value::Str("EMPTY".into())
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::has",
            Value::Str("MISSING".into())
        )),
        Some(Value::Bool(false))
    );
    // `require` returns a present variable's value.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::must",
            Value::Str("HOME".into())
        )),
        Some(Value::Str("/home/marrow".into()))
    );
}

#[test]
fn env_get_falls_back_to_the_default_when_absent() {
    let program = checked_program(ENV_SAMPLE);
    let store = TreeStore::memory();
    let host = env_host();
    let call = |name: &str, fallback: &str| {
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(
                &program,
                "test::read",
                Value::Str(name.into()),
                Value::Str(fallback.into())
            ),
        )
        .expect("env get")
        .value
    };
    // A present variable wins over the default; an empty one is still present.
    assert_eq!(
        call("HOME", "fallback"),
        Some(Value::Str("/home/marrow".into()))
    );
    assert_eq!(call("EMPTY", "fallback"), Some(Value::Str(String::new())));
    // An absent variable falls back to the default.
    assert_eq!(
        call("MISSING", "fallback"),
        Some(Value::Str("fallback".into()))
    );
}

#[test]
fn env_require_missing_variable_is_an_absent_error() {
    let program = checked_program(ENV_SAMPLE);
    let store = TreeStore::memory();
    let host = env_host();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::must", Value::Str("MISSING".into())),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_ABSENT),
        "{result:?}"
    );
}

#[test]
fn env_without_an_environment_capability_is_a_capability_error() {
    let program = checked_program(ENV_SAMPLE);
    let store = TreeStore::memory();
    // Plain `run_entry` supplies no host capabilities, so the whole module is
    // unavailable — even presence checks.
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::has", Value::Str("HOME".into())),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

/// A program that logs at each level, including an `Error` value.
const LOG_SAMPLE: &str = "\
fn note(m: string)
    std::log::info(m)

fn careful(m: string)
    std::log::warn(m)

fn boom()
    std::log::error(Error(code: \"E_BOOM\", message: \"kaboom\"))
";

#[test]
fn log_writes_each_level_to_the_host_sink() {
    let program = checked_program(LOG_SAMPLE);
    let store = TreeStore::memory();
    let sink = Rc::new(RefCell::new(String::new()));
    let host = Host::new().with_log_sink(Rc::clone(&sink));
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::note", Value::Str("hello".into())),
    )
    .expect("info");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::careful", Value::Str("watch out".into())),
    )
    .expect("warn");
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::boom")).expect("error");
    assert_eq!(
        sink.borrow().as_str(),
        "INFO hello\nWARN watch out\nERROR [E_BOOM] kaboom\n"
    );
}

#[test]
fn log_without_a_log_capability_is_a_capability_error() {
    let program = checked_program(LOG_SAMPLE);
    let store = TreeStore::memory();
    // Plain `run_entry` supplies no host capabilities.
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::note", Value::Str("hi".into())),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

#[test]
fn log_error_requires_an_error_value() {
    checker_rejects(
        "fn t()\n    std::log::error(\"not an error\")\n",
        "check.call_argument",
    );
}

#[test]
fn a_group_entry_field_write_lands_in_saved_data() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n    notes(noteId: string)\n        text: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn add_note(id: int, note: string, t: string)\n    ^books(id).notes(note).text = t\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(5)),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_note",
            Value::Int(5),
            Value::Str("n1".into()),
            Value::Str("hello".into()),
        ),
    )
    .expect("group-entry write");
    let value = read_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(5)],
        &keyed_data_path(
            &program,
            "books",
            &[("notes", vec![SavedKey::Str("n1".into())])],
            &["text"],
        ),
        ScalarType::Str,
    );
    assert_eq!(value, Some(SavedValue::Str("hello".into())));
}

fn book_group_field(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
    layer: &str,
    key: SavedKey,
    field: &str,
) -> Option<SavedValue> {
    read_data_value(
        program,
        store,
        "books",
        &[SavedKey::Int(id)],
        &keyed_data_path(program, "books", &[(layer, vec![key])], &[field]),
        ScalarType::Str,
    )
}
#[test]
fn group_entry_field_writes_compose_in_a_transaction() {
    // The sample's `add` shape: a whole-record write plus group-entry history
    // writes, all inside one transaction.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n    versions(version: int)\n        required title: string\n        required shelf: string\n\nfn add(id: int, t: string, s: string)\n    transaction\n        ^books(id).title = t\n        ^books(id).versions(1).title = t\n        ^books(id).versions(1).shelf = s\n\nfn title_of(id: int): string\n    return ^books(id).title\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("fiction".into()),
        ),
    )
    .expect("transactional group writes");
    // The top-level field reads back through the runtime.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into()))
    );
    // The group-entry members committed alongside it.
    let version_member =
        |field: &str| book_group_field(&program, &store, 1, "versions", SavedKey::Int(1), field);
    assert_eq!(
        version_member("title"),
        Some(SavedValue::Str("Mort".into()))
    );
    assert_eq!(
        version_member("shelf"),
        Some(SavedValue::Str("fiction".into()))
    );
}

#[test]
fn a_call_binds_named_arguments_by_name() {
    // Named arguments may appear in any order; they bind by name, not position.
    // `sub(b: 10, a: 3)` is `3 - 10`, not `10 - 3`.
    let program = checked_program(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(b: 10, a: 3)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::go")),
        Ok(Some(Value::Int(-7)))
    );
}

#[test]
fn a_call_mixes_positional_then_named_arguments() {
    let program = checked_program(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(10, b: 3)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::go")),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn a_call_with_an_unknown_parameter_name_is_rejected() {
    checker_rejects(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(a: 1, c: 2)\n",
        "check.call_argument",
    );
}

#[test]
fn a_call_missing_an_argument_is_rejected() {
    checker_rejects(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(a: 1)\n",
        "check.call_argument",
    );
}

#[test]
fn a_call_supplying_a_parameter_twice_is_rejected() {
    checker_rejects(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(1, a: 2)\n",
        "check.call_argument",
    );
}

// Note: positional-after-named (`sub(b: 1, 2)`) is now rejected by the PARSER
// (parse.syntax), so it cannot reach the runtime via a parsed program; the
// `bind_arguments` guard remains as defensive depth. The parser owns this rule
// and tests it in marrow-syntax.

/// Extract the single `mw` code block from the canonical sample, so the
/// integration test runs the exact published source.
fn sample_source() -> String {
    let doc = include_str!("../../../docs/language/sample.md");
    doc.split("```mw")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("the sample document has an mw code block")
        .to_string()
}

#[test]
fn the_reference_sample_runs_end_to_end() {
    // The canonical sample must run on the in-memory store: add a book in a
    // transaction (whole-resource + history group writes),
    // tag it, and print the fiction shelf via index traversal.
    let program = checked_program(&sample_source());
    let store = TreeStore::memory();
    let host = Host::new().with_clock(1_700_000_000_000_000_000); // 2023-11-14T22:13:20Z
    let outcome = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "shelf::sample::main"),
    )
    .expect("the sample's main runs end-to-end");
    // `main` returns nothing and prints the one fiction book it added.
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "1: Small Gods\n");
}

#[test]
fn the_reference_sample_runs_on_native_storage() {
    // Step 9's done-criterion: the same sample runs unchanged on the native redb
    // backend, with output identical to the in-memory run.
    let program = checked_program(&sample_source());
    let dir = tempfile::tempdir().expect("create a temp dir");
    let store = TreeStore::open(&dir.path().join("sample.redb")).expect("open redb");
    let host = Host::new().with_clock(1_700_000_000_000_000_000);
    let outcome = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "shelf::sample::main"),
    )
    .expect("the sample's main runs on native storage");
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "1: Small Gods\n");
}

const BOOK_VERSIONS: &str = "\
resource Book at ^books(id: int)
    required title: string

    versions(version: int)
        required title: string

fn seed(id: int, t: string)
    ^books(id).title = t

fn set_version_title(id: int, v: int, t: string)
    ^books(id).versions(v).title = t

fn version_title(id: int, v: int): string
    return ^books(id).versions(v).title
";

#[test]
fn reads_a_field_from_a_group_entry() {
    let program = checked_program(BOOK_VERSIONS);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("root".into())
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_version_title",
            Value::Int(1),
            Value::Int(2),
            Value::Str("Mort".into())
        ),
    )
    .expect("write");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::version_title",
            Value::Int(1),
            Value::Int(2)
        ),
    )
    .expect("read")
    .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
}

#[test]
fn reading_an_absent_group_field_is_an_error() {
    let program = checked_program(BOOK_VERSIONS);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::version_title",
            Value::Int(1),
            Value::Int(2)
        ),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_ABSENT),
        "{result:?}"
    );
}

#[test]
fn the_sample_update_functions_run() {
    // Drive the reference sample's mutating API beyond `main`: add a book, add a
    // note (group write guarded by `exists`), and move it between shelves (a
    // field write that also moves its generated index entry).
    let source = format!(
        "{}\n\n\
         pub fn exerciseUpdates(changedAt: instant): bool\n\
         \x20   const id = add(\n\
         \x20       title: \"Small Gods\",\n\
         \x20       author: \"Terry Pratchett\",\n\
         \x20       shelf: \"fiction\",\n\
         \x20       changedAt: changedAt,\n\
         \x20   )\n\
         \x20   const missing = nextId(^books)\n\
         \x20   const note = addNote(id, \"n1\", \"first\")\n\
         \x20   const missingNote = addNote(missing, \"n2\", \"missing\")\n\
         \x20   moveToShelf(id, \"history\", changedAt)\n\
         \x20   return note and not missingNote\n",
        sample_source()
    );
    let program = checked_program(&source);
    let store = TreeStore::memory();
    let when = Value::Instant(1_700_000_000_000_000_000);
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "shelf::sample::exerciseUpdates", when)
        )
        .expect("exercise sample updates")
        .value,
        Some(Value::Bool(true))
    );
    let shelf = |name: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "shelf::sample::printShelf",
                Value::Str(name.into())
            ),
        )
        .expect("printShelf")
        .output
    };
    assert_eq!(shelf("history"), "1: Small Gods\n", "moved to history");
    assert_eq!(shelf("fiction"), "", "and left fiction");
}

// --- Resource-identity values ---

#[test]
fn multiple_stores_over_one_resource_keep_runtime_roots_separate() {
    let program = checked_program(
        "resource Book\n\
         \x20   title: string\n\
         \n\
         store ^books(id: int): Book\n\
         store ^archivedBooks(id: int): Book\n\
         \n\
         fn seed()\n\
         \x20   ^books(1).title = \"live\"\n\
         \x20   ^archivedBooks(1).title = \"archived\"\n\
         \n\
         fn live(): string\n\
         \x20   return ^books(1).title ?? \"\"\n\
         \n\
         fn archived(): string\n\
         \x20   return ^archivedBooks(1).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let live = run_entry(&store, checked_entry!(&program, "test::live"))
        .expect("live")
        .value;
    assert_eq!(live, Some(Value::Str("live".into())));

    let archived = run_entry(&store, checked_entry!(&program, "test::archived"))
        .expect("archived")
        .value;
    assert_eq!(archived, Some(Value::Str("archived".into())));
}

/// A single-key store identity from `nextId(^books)` can be passed to saved reads
/// and writes. The identity carries the lowered key so `^books(id)` reads the
/// same record `^books(1)` does.
const BOOK_IDENTITY: &str = "\
resource Book at ^books(id: int)
    required title: string

fn save(t: string)
    const id = nextId(^books)
    ^books(id).title = t

fn title(): string
    for id in ^books
        return ^books(id).title
    return \"\"
";

#[test]
fn allocates_and_uses_a_single_key_store_identity() {
    let program = checked_program(BOOK_IDENTITY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::save", Value::Str("Mort".into())),
    )
    .expect("save");
    let value = run_entry(&store, checked_entry!(&program, "test::title"))
        .expect("title")
        .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
    // The identity lowered to the same key a plain int does: `^books(1)`.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Mort".into()))
    );
}

#[test]
fn a_plain_int_identity_still_works() {
    // The bare int path remains the executable single-key store identity path.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn save()\n    ^books(1).title = \"a\"\n\nfn read(): string\n    return ^books(1).title\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    let value = run_entry(&store, checked_entry!(&program, "test::read"))
        .expect("read")
        .value;
    assert_eq!(value, Some(Value::Str("a".into())));
}

/// A composite-key resource can still be addressed directly by its declared store
/// key order. Composite identity values come from traversal and indexes.
const ENROLLMENT_IDENTITY: &str = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string

fn enroll(s: string, c: string, st: string)
    ^enrollments(s, c).status = st

fn statusOf(s: string, c: string): string
    return ^enrollments(s, c).status ?? \"\"
";

#[test]
fn constructs_and_uses_a_composite_identity_round_trips() {
    let program = checked_program(ENROLLMENT_IDENTITY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::enroll",
            Value::Str("student-1".into()),
            Value::Str("course-9".into()),
            Value::Str("active".into()),
        ),
    )
    .expect("enroll");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::statusOf",
            Value::Str("student-1".into()),
            Value::Str("course-9".into()),
        ),
    )
    .expect("statusOf")
    .value;
    assert_eq!(value, Some(Value::Str("active".into())));
    // Keys lowered in declared order: studentId then courseId.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "enrollments",
            &[
                SavedKey::Str("student-1".into()),
                SavedKey::Str("course-9".into()),
            ],
            &data_path(&program, "enrollments", &["status"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("active".into()))
    );
}

#[test]
fn composite_root_keys_write_in_declaration_order() {
    let program = checked_program(
        "resource Enrollment at ^enrollments(studentId: string, courseId: string)\n    status: string\n\nfn enroll()\n    ^enrollments(\"s\", \"c\").status = \"active\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::enroll")).expect("enroll");
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "enrollments",
            &[SavedKey::Str("s".into()), SavedKey::Str("c".into())],
            &data_path(&program, "enrollments", &["status"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("active".into()))
    );
}

#[test]
fn whole_resource_read_through_an_identity() {
    // The primary-root iterator yields a composite identity that can be used for
    // a whole-resource read.
    let program = checked_program(
        "resource Enrollment at ^enrollments(studentId: string, courseId: string)\n    status: string\n\nfn statusOf(): string\n    for id in ^enrollments\n        var e: Enrollment = ^enrollments(id)\n        return e.status\n    return \"\"\n",
    );
    let store = TreeStore::memory();
    write_data_value(
        &program,
        &store,
        "enrollments",
        &[SavedKey::Str("s".into()), SavedKey::Str("c".into())],
        &data_path(&program, "enrollments", &["status"]),
        SavedValue::Str("active".into()),
    );
    let value = run_entry(&store, checked_entry!(&program, "test::statusOf"))
        .expect("statusOf")
        .value;
    assert_eq!(value, Some(Value::Str("active".into())));
}

// --- Singleton stores end-to-end ---

/// A singleton store (no identity keys). Field
/// read/write address the root directly, and whole read/write materialize and
/// replace the root as a resource value.
const SETTINGS: &str = "\
resource Settings at ^settings
    required theme: string
    required maxLoans: int

fn init(t: string, n: int)
    transaction
        ^settings.theme = t
        ^settings.maxLoans = n

fn setMaxLoans(n: int)
    ^settings.maxLoans = n

fn setTheme(t: string)
    ^settings.theme = t

fn theme(): string
    return ^settings.theme ?? \"\"

fn snapshot(): Settings
    var fallback: Settings
    fallback.theme = \"\"
    fallback.maxLoans = 0
    return ^settings ?? fallback

fn restore(s: Settings)
    ^settings = s

fn restoreFixture(theme: string, maxLoans: int)
    var s: Settings
    s.theme = theme
    s.maxLoans = maxLoans
    restore(s)
";

#[test]
fn singleton_field_read_and_write() {
    let program = checked_program(SETTINGS);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::init",
            Value::Str("light".into()),
            Value::Int(3)
        ),
    )
    .expect("init");
    run_entry(
        &store,
        checked_entry!(&program, "test::setMaxLoans", Value::Int(5)),
    )
    .expect("setMaxLoans");
    run_entry(
        &store,
        checked_entry!(&program, "test::setTheme", Value::Str("dark".into())),
    )
    .expect("setTheme");
    let value = run_entry(&store, checked_entry!(&program, "test::theme"))
        .expect("theme")
        .value;
    assert_eq!(value, Some(Value::Str("dark".into())));
    // The field landed at `^settings.theme`, no record key in between.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "settings",
            &[],
            &data_path(&program, "settings", &["theme"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("dark".into()))
    );
}

#[test]
fn singleton_whole_read_and_write_round_trip() {
    let program = checked_program(SETTINGS);
    let store = TreeStore::memory();
    // Seed the singleton's fields directly.
    write_data_value(
        &program,
        &store,
        "settings",
        &[],
        &data_path(&program, "settings", &["theme"]),
        SavedValue::Str("light".into()),
    );
    write_data_value(
        &program,
        &store,
        "settings",
        &[],
        &data_path(&program, "settings", &["maxLoans"]),
        SavedValue::Int(5),
    );
    // A whole read materializes the singleton's present fields.
    let snapshot = run_entry(&store, checked_entry!(&program, "test::snapshot"))
        .expect("snapshot")
        .value;
    assert_eq!(
        snapshot,
        Some(Value::Resource(vec![
            ("theme".into(), Value::Str("light".into())),
            ("maxLoans".into(), Value::Int(5)),
        ]))
    );
    // A whole write replaces it; read it back via the field reader.
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::restoreFixture",
            Value::Str("solar".into()),
            Value::Int(9)
        ),
    )
    .expect("restore");
    let value = run_entry(&store, checked_entry!(&program, "test::theme"))
        .expect("theme")
        .value;
    assert_eq!(value, Some(Value::Str("solar".into())));
}

// --- Unkeyed-group field read/write through a saved path ---

/// A resource with an unkeyed nested group (`name { first; last }`). Its fields
/// are addressed `^patients(p).name.first` — a `.field` off a `.field` off the
/// record, with no keyed layer in between.
#[test]
fn a_whole_read_of_a_keyed_root_without_an_identity_is_rejected() {
    checker_rejects(
        "module test\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         pub fn read()\n\
         \x20   var b: Book = ^books\n",
        "check.untyped_value",
    );
}

#[test]
fn a_field_read_off_a_keyed_root_without_an_identity_is_rejected() {
    checker_rejects(
        "module test\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         pub fn read(): string\n\
         \x20   return ^books.title\n",
        "check.untyped_value",
    );
}

const PATIENT_UNKEYED_GROUP: &str = "\
resource Patient at ^patients(id: int)
    mrn: string
    name
        first: string
        last: string

fn setName(id: int, f: string, l: string)
    ^patients(id).name.first = f
    ^patients(id).name.last = l

fn firstOf(id: int): string
    return ^patients(id)?.name?.first ?? \"\"

fn lastOf(id: int): string
    return ^patients(id)?.name?.last ?? \"\"
";

#[test]
fn unkeyed_group_field_write_then_read_round_trips() {
    let program = checked_program(PATIENT_UNKEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::setName",
            Value::Int(7),
            Value::Str("Terry".into()),
            Value::Str("Pratchett".into()),
        ),
    )
    .expect("setName");
    let read = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(7)))
            .expect("read")
            .value
    };
    assert_eq!(read("test::firstOf"), Some(Value::Str("Terry".into())));
    assert_eq!(read("test::lastOf"), Some(Value::Str("Pratchett".into())));
    // The field landed under the group layer `^patients(7).name.first`.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "patients",
            &[SavedKey::Int(7)],
            &data_path(&program, "patients", &["name", "first"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Terry".into()))
    );
}

#[test]
fn an_absent_unkeyed_group_field_read_uses_the_default() {
    let program = checked_program(PATIENT_UNKEYED_GROUP);
    let store = TreeStore::memory();
    let value = run_entry(
        &store,
        checked_entry!(&program, "test::firstOf", Value::Int(1)),
    )
    .expect("default")
    .value;
    assert_eq!(value, Some(Value::Str(String::new())));
}

// --- Unique-index identity reads ---

/// A book with a unique index on `isbn`. `register` stores the book, and
/// `titleByIsbn` reads the identity back from the unique-index lookup path and
/// uses it to address the record.
const BOOK_ISBN: &str = "\
resource Book at ^books(id: int)
    required title: string
    isbn: string

    index byIsbn(isbn) unique

fn register(id: int, t: string, isbn: string)
    ^books(id).title = t
    ^books(id).isbn = isbn

fn titleByIsbnKey(isbn: string, fallback: int): string
    for id in ^books.byIsbn(isbn)
        return ^books(id).title ?? \"\"
    return ^books(fallback).title ?? \"\"

fn hasIsbn(isbn: string): bool
    return exists(^books.byIsbn(isbn))

fn countIsbn(isbn: string): int
    return count(^books.byIsbn(isbn))

fn iterTitlesByIsbn(isbn: string)
    for id in ^books.byIsbn(isbn)
        print(^books(id).title ?? \"\")

fn changeIsbn(id: Id(^books))
    ^books(id).isbn = \"978-1\"

fn changeIsbnThroughHelper(isbn: string)
    for id in ^books.byIsbn(isbn)
        changeIsbn(id)
";

#[test]
fn reads_an_identity_from_a_unique_index() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titleByIsbnKey",
            Value::Str("978-0".into()),
            Value::Int(42)
        ),
    )
    .expect("titleByIsbn")
    .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
}

#[test]
fn a_unique_index_value_read_rejects_the_wrong_arity_at_runtime() {
    let resource = "resource Book at ^books(id: int)\n    required title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\n";
    checker_rejects(
        &format!("{resource}fn badIsbnMissing()\n    return ^books.byIsbn()\n"),
        "check.key_type",
    );
    checker_rejects(
        &format!("{resource}fn badIsbnExtra(isbn: string)\n    return ^books.byIsbn(isbn, 1)\n"),
        "check.key_type",
    );
}

#[test]
fn an_absent_unique_index_lookup_uses_the_fallback_identity() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(99),
            Value::Str("Fallback".into()),
            Value::Str("fallback-isbn".into()),
        ),
    )
    .expect("register fallback");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titleByIsbnKey",
            Value::Str("missing".into()),
            Value::Int(99)
        ),
    )
    .expect("fallback")
    .value;
    assert_eq!(value, Some(Value::Str("Fallback".into())));
}

#[test]
fn unique_index_presence_and_count_follow_the_lookup_value() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");

    let call = |entry: &str, isbn: &str| {
        run_entry(
            &store,
            checked_entry!(&program, entry, Value::Str(isbn.into())),
        )
        .expect(entry)
        .value
    };
    assert_eq!(call("test::hasIsbn", "978-0"), Some(Value::Bool(true)));
    assert_eq!(call("test::hasIsbn", "missing"), Some(Value::Bool(false)));
    assert_eq!(call("test::countIsbn", "978-0"), Some(Value::Int(1)));
    assert_eq!(call("test::countIsbn", "missing"), Some(Value::Int(0)));
}

#[test]
fn unique_index_lookup_iteration_yields_the_stored_identity() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");

    let present = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::iterTitlesByIsbn",
            Value::Str("978-0".into())
        ),
    )
    .expect("present unique lookup iterates");
    assert_eq!(present.output, "Mort\n");

    let absent = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::iterTitlesByIsbn",
            Value::Str("missing".into())
        ),
    )
    .expect("absent unique lookup is an empty iteration");
    assert_eq!(absent.output, "");
}

#[test]
fn helper_call_mutating_a_traversed_unique_index_faults() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");

    let error = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::changeIsbnThroughHelper",
            Value::Str("978-0".into())
        ),
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_TRAVERSAL, "{error:?}");
}

#[test]
fn keys_over_a_unique_index_lookup_is_not_a_collection() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\nfn register(id: int, t: string, isbn: string)\n    ^books(id).title = t\n    ^books(id).isbn = isbn\n\nfn countKeysByIsbn(isbn: string): int\n    var c = 0\n    for id in keys(^books.byIsbn(isbn))\n        c = c + 1\n    return c\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");

    let error = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::countKeysByIsbn",
            Value::Str("978-0".into())
        ),
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
}

#[test]
fn unique_index_prefix_branch_presence_count_and_iteration_agree() {
    checker_rejects(
        "resource Item at ^items(id: int)\n    required title: string\n    series: string\n    code: string\n\n    index bySeriesCode(series, code) unique\n\nfn countSeries(series: string): int\n    return count(^items.bySeriesCode(series))\n",
        "check.key_type",
    );
}

#[test]
fn unique_index_prefix_branch_loops_are_rejected_by_the_checker() {
    let source = "resource Item at ^items(id: int)\n    required title: string\n    series: string\n    code: string\n\n    index bySeriesCode(series, code) unique\n\nfn titlesInSeries(series: string)\n    for id in ^items.bySeriesCode(series)\n        print(^items(id).title ?? \"\")\n";
    checker_rejects(source, "check.key_type");

    let source = "resource Item at ^items(id: int)\n    required title: string\n    series: string\n    code: string\n\n    index bySeriesCode(series, code) unique\n\nfn titlesInAnySeries()\n    for id in ^items.bySeriesCode\n        print(^items(id).title ?? \"\")\n";
    checker_rejects(source, "check.key_type");
}

/// A non-unique index in value position has no single identity to yield; the
/// runtime rejects it and points the reader at `keys(...)`.
const BOOK_SHELF_VALUE: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

fn firstOnShelf(shelf: string): Id(^books)
    return ^books.byShelf(shelf)
";

#[test]
fn a_non_unique_index_in_value_position_is_rejected() {
    checker_rejects(BOOK_SHELF_VALUE, "check.untyped_value");
}

// --- Composite-identity index traversal ---

/// A composite-identity store indexed by status. The non-unique index ends with
/// both identity keys, so traversal must descend both levels per entry and
/// reconstruct the full `Id(^enrollments)` (not just the first key component).
const ENROLLMENT_STATUS: &str = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    required status: string
    required student: string
    required course: string

    index byStatus(status, studentId, courseId)

fn enroll(s: string, c: string, st: string)
    var enrollment: Enrollment
    enrollment.status = st
    enrollment.student = s
    enrollment.course = c
    ^enrollments(s, c) = enrollment

fn activeStatuses()
    for id in keys(^enrollments.byStatus(\"active\"))
        print(^enrollments(id).status)

fn activeEnrollmentsDirect()
    for id in ^enrollments.byStatus(\"active\")
        print($\"{^enrollments(id).student}:{^enrollments(id).course}\")

fn activeCoursesForStudent(student: string)
    for id in ^enrollments.byStatus(\"active\", student)
        print(^enrollments(id).course)

fn activeCourseExact(student: string, course: string)
    for id in ^enrollments.byStatus(\"active\", student, course)
        print(^enrollments(id).course)

fn activeCourseExactPair(student: string, course: string)
    for id, enrollment in ^enrollments.byStatus(\"active\", student, course)
        print($\"{^enrollments(id).course}:{enrollment.course}\")

fn activeCourseExactKeys(student: string, course: string)
    for id in keys(^enrollments.byStatus(\"active\", student, course))
        print(^enrollments(id).course)

fn activeCourseExactCount(student: string, course: string): int
    return count(^enrollments.byStatus(\"active\", student, course))

";

#[test]
fn traverses_a_composite_identity_index() {
    let program = checked_program(ENROLLMENT_STATUS);
    let store = TreeStore::memory();
    let enroll = |s: &str, c: &str, st: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::enroll",
                Value::Str(s.into()),
                Value::Str(c.into()),
                Value::Str(st.into()),
            ),
        )
        .expect("enroll");
    };
    enroll("student-1", "course-8", "active");
    enroll("student-1", "course-9", "active");
    enroll("student-1", "course-7", "dropped");

    // Each reconstructed identity addresses its record: every active enrollment
    // reads back `active`. Two such entries exist, in (studentId, courseId) order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::activeStatuses")).expect("run");
    assert_eq!(outcome.output, "active\nactive\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCoursesForStudent",
            Value::Str("student-1".into())
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "course-8\ncourse-9\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExact",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "course-8\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExactPair",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "course-8:course-8\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExactKeys",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "course-8\n");

    let exact_count = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExactCount",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
        ),
    )
    .expect("count")
    .value;
    assert_eq!(exact_count, Some(Value::Int(1)));

    let inactive_count = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExactCount",
            Value::Str("student-1".into()),
            Value::Str("course-7".into()),
        ),
    )
    .expect("count")
    .value;
    assert_eq!(inactive_count, Some(Value::Int(0)));
}

#[test]
fn helper_mutating_a_traversed_composite_index_faults_at_runtime() {
    let program = checked_program(
        "resource Enrollment at ^enrollments(studentId: string, courseId: string)\n    required status: string\n    required student: string\n    required course: string\n\n    index byStatus(status, studentId, courseId)\n\nfn enroll(s: string, c: string, st: string)\n    var enrollment: Enrollment\n    enrollment.status = st\n    enrollment.student = s\n    enrollment.course = c\n    ^enrollments(s, c) = enrollment\n\nfn markInactive(id: Id(^enrollments))\n    ^enrollments(id).status = \"inactive\"\n\nfn deactivateExact(student: string, course: string)\n    for id in ^enrollments.byStatus(\"active\", student, course)\n        markInactive(id)\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::enroll",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
            Value::Str("active".into()),
        ),
    )
    .expect("enroll");
    let error = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::deactivateExact",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
        ),
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_TRAVERSAL, "{error:?}");
}

#[test]
fn direct_composite_identity_index_loop_yields_identities() {
    let program = checked_program(ENROLLMENT_STATUS);
    let store = TreeStore::memory();
    let enroll = |s: &str, c: &str, st: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::enroll",
                Value::Str(s.into()),
                Value::Str(c.into()),
                Value::Str(st.into()),
            ),
        )
        .expect("enroll");
    };
    enroll("student-1", "course-8", "active");
    enroll("student-1", "course-9", "active");
    enroll("student-1", "course-7", "dropped");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::activeEnrollmentsDirect"),
    )
    .expect("run");
    assert_eq!(outcome.output, "student-1:course-8\nstudent-1:course-9\n");
}

// --- Unified saved-layer enumeration ---

/// Iterating a primary keyed root yields identities. Two-name loops pair the
/// identity with the materialized record value.
const BOOK_PRIMARY: &str = "\
resource Book at ^books(id: int)
    required title: string

fn add(id: int, t: string)
    ^books(id).title = t

fn titles()
    for id in ^books
        print(^books(id).title)

fn directIds()
    for id in ^books
        print($\"{id}\")

fn idsAndElementTitles()
    for id, book in ^books
        print($\"{id}: {book.title}\")

fn reversedElementTitles()
    for id, book in reversed(^books)
        print(book.title)

fn reversedIdsAsValue()
    const ids = reversed(^books)
    for id in ids
        print($\"{id}\")

fn ids()
    const all = keys(^books)
    for id in all
        print($\"{id}\")
";

#[test]
fn iterates_a_primary_keyed_root() {
    let program = checked_program(BOOK_PRIMARY);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    // Direct root iteration yields ids in key order, each addressing its record.
    let outcome = run_entry(&store, checked_entry!(&program, "test::titles")).expect("run");
    assert_eq!(outcome.output, "Mort\nSourcery\n");
}

#[test]
fn primary_root_loop_yields_identities() {
    let program = checked_program(BOOK_PRIMARY);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    let outcome = run_entry(&store, checked_entry!(&program, "test::directIds")).expect("run");
    assert_eq!(outcome.output, "1\n2\n");
}

#[test]
fn two_name_primary_root_loop_yields_id_and_resource() {
    let program = checked_program(BOOK_PRIMARY);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::idsAndElementTitles"),
    )
    .expect("run");
    assert_eq!(outcome.output, "1: Mort\n2: Sourcery\n");
}

#[test]
fn reversed_two_name_primary_root_loop_yields_resources() {
    let program = checked_program(BOOK_PRIMARY);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::reversedElementTitles"),
    )
    .expect("run");
    assert_eq!(outcome.output, "Sourcery\nMort\n");
}

#[test]
fn reversed_primary_root_as_a_value_is_rejected() {
    let program = checked_program(BOOK_PRIMARY);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    let error =
        run_entry(&store, checked_entry!(&program, "test::reversedIdsAsValue")).unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
}

#[test]
fn keys_of_a_primary_root_as_a_value_is_rejected() {
    let program = checked_program(BOOK_PRIMARY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("add");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(2),
            Value::Str("Sourcery".into())
        ),
    )
    .expect("add");

    let error = run_entry(&store, checked_entry!(&program, "test::ids")).unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
}

#[test]
fn iterating_a_singleton_root_is_a_type_error() {
    // A keyless singleton has no identities to enumerate; iterating it is a
    // type error, not a silent empty loop.
    let program = checked_program(
        "resource Settings at ^settings\n    theme: string\n\nfn each()\n    for s in ^settings\n        print(\"x\")\n",
    );
    let store = TreeStore::memory();
    let error = run_entry(&store, checked_entry!(&program, "test::each")).unwrap_err();
    assert_eq!(error.code, RUN_TYPE, "{error:?}");
}

/// Iterating a composite primary root yields materialized records in identity order.
const ENROLLMENT_PRIMARY: &str = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string

fn enroll(s: string, c: string, st: string)
    ^enrollments(s, c).status = st

fn statuses()
    for id, enrollment in ^enrollments
        print(enrollment.status)
";

#[test]
fn iterates_a_composite_primary_root() {
    let program = checked_program(ENROLLMENT_PRIMARY);
    let store = TreeStore::memory();
    let enroll = |s: &str, c: &str, st: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::enroll",
                Value::Str(s.into()),
                Value::Str(c.into()),
                Value::Str(st.into()),
            ),
        )
        .expect("enroll");
    };
    enroll("student-1", "course-9", "active");
    enroll("student-2", "course-1", "dropped");

    // Two-name iteration reads each materialized enrollment record.
    let outcome = run_entry(&store, checked_entry!(&program, "test::statuses")).expect("run");
    assert_eq!(outcome.output, "active\ndropped\n");
}

/// Iterating a sequence/keyed child layer yields positions. Two-name loops pair
/// each position with its value.
const BOOK_TAGS: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags: sequence[string]

pub fn seed()
    ^books(1).title = \"Mort\"
    const a: int = append(^books(1).tags, \"fiction\")
    const b: int = append(^books(1).tags, \"funny\")

fn positions()
    for pos in ^books(1).tags
        print($\"{pos}\")

fn tagValues()
    for pos, tag in ^books(1).tags
        print(tag)

fn tagEntries()
    for pos, tag in ^books(1).tags
        print($\"{pos}={tag}\")

fn tagValuesDescending()
    for pos, tag in reversed(^books(1).tags)
        print(tag)

fn positionsDescending()
    for pos in reversed(keys(^books(1).tags))
        print($\"{pos}\")

fn keysOf()
    for pos in keys(^books(1).tags)
        print($\"{pos}\")
";

#[test]
fn iterates_a_sequence_child_layer() {
    let program = checked_program(BOOK_TAGS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    // Direct layer iteration yields 1-based positions in key order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::positions")).expect("run");
    assert_eq!(outcome.output, "1\n2\n");

    // `keys(^books(1).tags)` yields the same positions.
    let outcome = run_entry(&store, checked_entry!(&program, "test::keysOf")).expect("run");
    assert_eq!(outcome.output, "1\n2\n");
}

#[test]
fn sequence_child_layer_two_name_loop_yields_element_values() {
    let program = checked_program(BOOK_TAGS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::tagValues")).expect("run");
    assert_eq!(outcome.output, "fiction\nfunny\n");
}

#[test]
fn two_name_sequence_child_layer_loop_yields_key_and_value() {
    let program = checked_program(BOOK_TAGS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::tagEntries")).expect("run");
    assert_eq!(outcome.output, "1=fiction\n2=funny\n");
}

#[test]
fn reversed_sequence_child_layer_loop_yields_values_descending() {
    let program = checked_program(BOOK_TAGS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::tagValuesDescending"),
    )
    .expect("run");
    assert_eq!(outcome.output, "funny\nfiction\n");
}

#[test]
fn reversed_sequence_child_layer_keys_loop_yields_positions_descending() {
    let program = checked_program(BOOK_TAGS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::positionsDescending"),
    )
    .expect("run");
    assert_eq!(outcome.output, "2\n1\n");
}

/// A keyed (non-sequence) child tree iterates keys; two-name loops pair keys
/// with values. Seeded through the store directly to keep the focus on order.
const PLAYER_SCORES: &str = "\
resource Game at ^games(id: int)
    scores(playerId: string): int

fn players()
    for p in keys(^games(1).scores)
        print(p)

fn scores()
    for player, score in ^games(1).scores
        print($\"{score}\")
";

#[test]
fn iterates_a_keyed_child_tree() {
    let program = checked_program(PLAYER_SCORES);
    let store = TreeStore::memory();
    let score = |player: &str, n: i64| {
        write_data_value(
            &program,
            &store,
            "games",
            &[SavedKey::Int(1)],
            &keyed_data_path(
                &program,
                "games",
                &[("scores", vec![SavedKey::Str(player.into())])],
                &[],
            ),
            SavedValue::Int(n),
        );
    };
    score("bob", 7);
    score("alice", 10);

    // Keys iterate in sorted key order (alice before bob).
    let outcome = run_entry(&store, checked_entry!(&program, "test::players")).expect("run");
    assert_eq!(outcome.output, "alice\nbob\n");

    // Two-name child-layer iteration yields values in key order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::scores")).expect("run");
    assert_eq!(outcome.output, "10\n7\n");
}

/// A keyed group layer iterates keys, with two-name loops preserving the group
/// address alongside the entry value.
const BOOK_VERSION_LOOPS: &str = "\
resource Book at ^books(id: int)
    required title: string

    versions(v: int)
        required title: string

pub fn seed()
    ^books(1).title = \"Mort\"
    ^books(1).versions(2).title = \"second\"
    ^books(1).versions(1).title = \"first\"

fn versionTitles()
    for v, version in ^books(1).versions
        print(version.title)

fn versionEntries()
    for v, version in ^books(1).versions
        print($\"{v}: {version.title}\")
";

#[test]
fn keyed_group_layer_loop_yields_materialized_entries() {
    let program = checked_program(BOOK_VERSION_LOOPS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::versionTitles")).expect("run");
    assert_eq!(outcome.output, "first\nsecond\n");
}

#[test]
fn two_name_keyed_group_layer_loop_yields_key_and_entry() {
    let program = checked_program(BOOK_VERSION_LOOPS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(&store, checked_entry!(&program, "test::versionEntries")).expect("run");
    assert_eq!(outcome.output, "1: first\n2: second\n");
}

#[test]
fn deleting_a_record_while_traversing_the_root_is_a_traversal_fault() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\nfn clear()\n    for id in keys(^books)\n        delete ^books(id)\n",
        "check.loop_mutates_traversed_layer",
    );
}

#[test]
fn traversal_faults_are_not_catchable_errors() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\nfn clear(): string\n    try\n        for id in keys(^books)\n            delete ^books(id)\n        return \"completed\"\n    catch error: Error\n        return error.code\n",
        "check.loop_mutates_traversed_layer",
    );
}

#[test]
fn appending_to_the_sequence_being_traversed_is_a_traversal_fault() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn seed()\n    ^books(1).title = \"a\"\n    const p: int = append(^books(1).tags, \"x\")\n\nfn grow()\n    for tag in ^books(1).tags\n        const p: int = append(^books(1).tags, \"y\")\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let faulted = run_entry(&store, checked_entry!(&program, "test::grow"));
    assert!(
        matches!(faulted, Err(ref error) if error.code == RUN_TRAVERSAL),
        "{faulted:?}"
    );
}

#[test]
fn helper_appending_to_the_sequence_being_traversed_is_a_traversal_fault() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn seed()\n    ^books(1).title = \"a\"\n    append(^books(1).tags, \"x\")\n\nfn grow()\n    append(^books(1).tags, \"y\")\n\nfn walk()\n    for tag in ^books(1).tags\n        grow()\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let faulted = run_entry(&store, checked_entry!(&program, "test::walk"));
    assert!(
        matches!(faulted, Err(ref error) if error.code == RUN_TRAVERSAL),
        "{faulted:?}"
    );
}

#[test]
fn helper_deleting_from_the_root_being_traversed_is_a_traversal_fault() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\nfn remove(id: int)\n    delete ^books(id)\n\nfn walk()\n    for id in keys(^books)\n        remove(id)\n",
        "check.call_argument",
    );
}

#[test]
fn field_write_creating_a_record_in_the_traversed_root_is_a_traversal_fault() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\nfn grow()\n    for id in ^books\n        ^books(99).title = \"new\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let faulted = run_entry(&store, checked_entry!(&program, "test::grow"));
    assert!(
        matches!(faulted, Err(ref error) if error.code == RUN_TRAVERSAL),
        "{faulted:?}"
    );
}

#[test]
fn collecting_saved_keys_as_a_local_snapshot_is_rejected() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\nfn clear()\n    const ids = keys(^books)\n    for id in ids\n        delete ^books(id)\n\nfn remaining(): int\n    return count(^books)\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let error = run_entry(&store, checked_entry!(&program, "test::clear")).unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::remaining"))
            .expect("count")
            .value,
        Some(Value::Int(2))
    );
}

#[test]
fn mutating_a_different_record_layer_while_traversing_is_allowed() {
    // Traversing `^books(1).tags` and appending to `^books(2).tags` touches a
    // different record's layer, so it is not a traversal fault.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n    const p: int = append(^books(1).tags, \"x\")\n\nfn copy()\n    for tag in ^books(1).tags\n        const p: int = append(^books(2).tags, \"y\")\n\nfn tags2(): int\n    return count(^books(2).tags)\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry(&store, checked_entry!(&program, "test::copy")).expect("copy");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::tags2"))
            .expect("count")
            .value,
        Some(Value::Int(1))
    );
}

// --- Ordered navigation: reversed / next / prev ---

/// A primary keyed root with a keyed child layer, used to exercise reverse
/// iteration and stored-neighbor seeks over both record identities and layer keys.
const NAV_BOOKS: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags(pos: int): string

fn add(id: int, t: string)
    ^books(id).title = t

fn delId(id: int)
    delete ^books(id)

fn tag(id: int, t: string)
    const p: int = append(^books(id).tags, t)

fn idsDescending()
    for id in reversed(keys(^books))
        print($\"{id}\")

fn keysReversedValue()
    const r = reversed(keys(^books))
    for id in r
        print($\"{id}\")

fn titlesDescending()
    for id, book in reversed(^books)
        print(book.title)

fn nextOfKey(id: int, fallback: string): string
    const wanted: string = ^books(id).title ?? \"\"
    for current in keys(^books)
        if (^books(current).title ?? \"\") == wanted
            const neighbor: Id(^books) = next(^books(current)) ?? current
            if neighbor == current
                return fallback
            return ^books(neighbor).title ?? fallback
    return fallback

fn prevOfKey(id: int, fallback: string): string
    const wanted: string = ^books(id).title ?? \"\"
    for current in keys(^books)
        if (^books(current).title ?? \"\") == wanted
            const neighbor: Id(^books) = prev(^books(current)) ?? current
            if neighbor == current
                return fallback
            return ^books(neighbor).title ?? fallback
    return fallback

fn firstIdKey(fallback: string): string
    for current in keys(^books)
        const first: Id(^books) = next(^books) ?? current
        return ^books(first).title ?? fallback
    return fallback

fn lastIdKey(fallback: string): string
    for current in keys(^books)
        const last: Id(^books) = prev(^books) ?? current
        return ^books(last).title ?? fallback
    return fallback

fn nextOrDefaultKey(id: int, fallback: string): string
    return nextOfKey(id, fallback)

fn nextTitleKey(id: int): string
    return nextOfKey(id, \"\")

fn breakAfterFirst(): int
    var seen = 0
    for id in reversed(^books)
        seen = seen + 1
        break
    return seen
";

#[test]
fn reversed_layer_iterates_descending_and_skips_a_hole() {
    let program = checked_program(NAV_BOOKS);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into())
            ),
        )
        .expect("add");
    };
    add(1, "Mort");
    add(2, "Sourcery");
    add(3, "Reaper");

    // `reversed(keys(^books))` yields ids in descending key order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::idsDescending")).expect("run");
    assert_eq!(outcome.output, "3\n2\n1\n");

    // Materializing the same durable reversed key collection as a value is rejected.
    let error = run_entry(&store, checked_entry!(&program, "test::keysReversedValue")).unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");

    // Bare reversed root iteration yields records in descending key order.
    let outcome =
        run_entry(&store, checked_entry!(&program, "test::titlesDescending")).expect("run");
    assert_eq!(outcome.output, "Reaper\nSourcery\nMort\n");

    // Deleting the middle record leaves a hole; reverse iteration skips it,
    // visiting only stored entries.
    run_entry(
        &store,
        checked_entry!(&program, "test::delId", Value::Int(2)),
    )
    .expect("del");
    let outcome = run_entry(&store, checked_entry!(&program, "test::idsDescending")).expect("run");
    assert_eq!(outcome.output, "3\n1\n");
}

#[test]
fn next_and_prev_skip_a_deleted_hole() {
    let program = checked_program(NAV_BOOKS);
    let store = TreeStore::memory();
    let add = |id: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(id.to_string())
            ),
        )
        .expect("add");
    };
    add(1);
    add(2);
    add(5);
    // Delete the middle stored key, leaving a gap between 1 and 5.
    run_entry(
        &store,
        checked_entry!(&program, "test::delId", Value::Int(2)),
    )
    .expect("del");

    // `next(^books(1))` is the nearest *stored* successor, skipping the gap at 2.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::nextOfKey",
                Value::Int(1),
                Value::Str("missing".into())
            )
        )
        .expect("next")
        .value,
        Some(Value::Str("5".into()))
    );
    // `prev(^books(5))` mirrors it: the nearest stored predecessor is 1.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::prevOfKey",
                Value::Int(5),
                Value::Str("missing".into())
            )
        )
        .expect("prev")
        .value,
        Some(Value::Str("1".into()))
    );
}

#[test]
fn next_of_bare_layer_is_first_and_prev_is_last() {
    let program = checked_program(NAV_BOOKS);
    let store = TreeStore::memory();
    let add = |id: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(id.to_string())
            ),
        )
        .expect("add");
    };
    add(4);
    add(2);
    add(9);

    // `next(^books)` (a bare layer) is the first stored entry; `prev(^books)` the
    // last — in key order, regardless of insertion order.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::firstIdKey", Value::Str("missing".into()))
        )
        .expect("first")
        .value,
        Some(Value::Str("2".into()))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::lastIdKey", Value::Str("missing".into()))
        )
        .expect("last")
        .value,
        Some(Value::Str("9".into()))
    );
}

#[test]
fn prev_of_first_is_absent_and_composes_with_coalesce() {
    let program = checked_program(NAV_BOOKS);
    let store = TreeStore::memory();
    let add = |id: i64| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(id.to_string())
            ),
        )
        .expect("add");
    };
    add(1);
    add(2);

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::prevOfKey",
                Value::Int(1),
                Value::Str("-1".into())
            )
        )
        .expect("prev default")
        .value,
        Some(Value::Str("-1".into()))
    );

    // `next` of the last stored key is likewise absent, and `?? -1` recovers it —
    // the edge fault composes with `??`.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::nextOrDefaultKey",
                Value::Int(2),
                Value::Str("-1".into())
            )
        )
        .expect("coalesce")
        .value,
        Some(Value::Str("-1".into()))
    );
}

#[test]
fn next_neighbor_identity_reads_a_field() {
    let program = checked_program(NAV_BOOKS);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("add");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(2),
            Value::Str("Sourcery".into())
        ),
    )
    .expect("add");

    // `^books(next(^books(1))).title` reads the neighbor record's field through its
    // returned identity.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::nextTitleKey", Value::Int(1))
        )
        .expect("nextTitle")
        .value,
        Some(Value::Str("Sourcery".into()))
    );
}

#[test]
fn reversed_iteration_supports_early_break() {
    let program = checked_program(NAV_BOOKS);
    let store = TreeStore::memory();
    for id in 1..=3 {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str("t".into())
            ),
        )
        .expect("add");
    }
    // A `break` on the first reversed element stops the loop after one iteration.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::breakAfterFirst"))
            .expect("break")
            .value,
        Some(Value::Int(1))
    );
}

#[test]
fn next_on_a_keyed_child_layer_position() {
    // `next(^books(1).tags(1))` seeks among the layer's integer positions.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn seed()\n    ^books(1).title = \"a\"\n    const x: int = append(^books(1).tags, \"p\")\n    const y: int = append(^books(1).tags, \"q\")\n    const z: int = append(^books(1).tags, \"r\")\n\nfn nextPos(p: int): int\n    return next(^books(1).tags(p)) ?? 0\n\nfn firstPos(): int\n    return next(^books(1).tags) ?? 0\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    // Positions are 1, 2, 3; the successor of 1 is 2.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::nextPos", Value::Int(1))
        )
        .expect("nextPos")
        .value,
        Some(Value::Int(2))
    );
    // `next(^books(1).tags)` (a bare layer) is the first stored position.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::firstPos"))
            .expect("firstPos")
            .value,
        Some(Value::Int(1))
    );
}

#[test]
fn reversed_over_an_in_memory_sequence_reverses_directly() {
    // `reversed(std::text::split(...))` reverses the in-memory sequence — no store
    // involved — so a `for` over it yields the elements back-to-front.
    let program = checked_program(
        "fn rev()\n    for word in reversed(std::text::split(\"a,b,c\", \",\"))\n        print(word)\n",
    );
    let store = TreeStore::memory();
    let outcome = run_entry(&store, checked_entry!(&program, "test::rev")).expect("run");
    assert_eq!(outcome.output, "c\nb\na\n");
}

#[test]
fn reversed_respects_the_traversed_layer_write_guard() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\nfn clear()\n    for id in reversed(keys(^books))\n        delete ^books(id)\n",
        "check.loop_mutates_traversed_layer",
    );
}

#[test]
fn reversed_over_a_composite_root_is_a_true_reverse() {
    // A composite identity reverses at every level, so the whole identity stream is
    // the exact reverse of the ascending one — not just the outermost component.
    let program = checked_program(ENROLLMENT_PRIMARY);
    let store = TreeStore::memory();
    let enroll = |s: &str, c: &str, st: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::enroll",
                Value::Str(s.into()),
                Value::Str(c.into()),
                Value::Str(st.into()),
            ),
        )
        .expect("enroll");
    };
    enroll("s1", "c1", "active");
    enroll("s1", "c2", "dropped");
    enroll("s2", "c1", "active");

    // Ascending identity order is (s1,c1),(s1,c2),(s2,c1); reverse is the mirror.
    let program = checked_program(&format!(
        "{ENROLLMENT_PRIMARY}\nfn revStatuses()\n    for id, enrollment in reversed(^enrollments)\n        print(enrollment.status)\n"
    ));
    let outcome = run_entry(&store, checked_entry!(&program, "test::revStatuses")).expect("run");
    assert_eq!(outcome.output, "active\ndropped\nactive\n");
}

/// A non-unique index branch, iterated forward and reversed: the entries enumerate
/// in identity-key order, and `reversed(...)` walks the same branch backward.
const BOOK_SHELF_NAV: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

fn onShelfReversed(shelf: string)
    for id in reversed(^books.byShelf(shelf))
        print($\"{id}\")
";

#[test]
fn reversed_over_an_index_branch_descends() {
    // `reversed(^books.byShelf(\"x\"))` walks a declared index branch backward,
    // yielding the matching identities in descending id order.
    let program = checked_program(BOOK_SHELF_NAV);
    let store = TreeStore::memory();
    let add = |id: i64, s: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str("t".into()),
                Value::Str(s.into())
            ),
        )
        .expect("add");
    };
    add(1, "x");
    add(2, "x");
    add(3, "y");
    add(4, "x");

    // Only shelf-"x" ids (1, 2, 4) match, enumerated in descending key order.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::onShelfReversed", Value::Str("x".into())),
    )
    .expect("run");
    assert_eq!(outcome.output, "4\n2\n1\n");
}

/// `next`/`prev` over a keyed root that *also* declares an index. The index is
/// stored as a named child of the root, sorting after the record-key children, so
/// the edge seek must skip it: stepping off the last record raises the catchable
/// `run.absent_element`, never an uncatchable `run.unsupported`, and `prev(^books)`
/// returns the last *record*, not the index.
const BOOK_SHELF_NEIGHBOR: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

fn nextOrDefaultKey(id: int, fallback: string): string
    const wanted: string = ^books(id).title ?? \"\"
    for current in keys(^books)
        if (^books(current).title ?? \"\") == wanted
            const neighbor: Id(^books) = next(^books(current)) ?? current
            if neighbor == current
                return fallback
            return ^books(neighbor).title ?? fallback
    return fallback

fn nextOrSelfKey(id: int): string
    return nextOrDefaultKey(id, ^books(id).title ?? \"\")

fn lastIdKey(fallback: string): string
    for current in keys(^books)
        const last: Id(^books) = prev(^books) ?? current
        return ^books(last).title ?? fallback
    return fallback
";

#[test]
fn neighbor_at_an_indexed_root_edge_skips_the_index_on_both_backends() {
    // The declared `index byShelf` is a named child of `^books`, stored after the
    // record-key children. The edge seek must skip it: `next` past the last record
    // is a catchable absent-element (so `??` recovers), and `prev(^books)` lands on
    // the last record, not the index name. Both must hold in memory and redb.
    let program = checked_program(BOOK_SHELF_NEIGHBOR);
    let dir = tempfile::tempdir().expect("temp dir");
    let mem = TreeStore::memory();
    let redb = TreeStore::open(&dir.path().join("nav.redb")).expect("open redb");
    let stores: [&TreeStore; 2] = [&mem, &redb];

    for store in stores {
        let add = |id: i64, s: &str| {
            run_entry(
                store,
                checked_entry!(
                    &program,
                    "test::add",
                    Value::Int(id),
                    Value::Str(id.to_string()),
                    Value::Str(s.into())
                ),
            )
            .expect("add");
        };
        add(1, "x");
        add(2, "x");
        add(3, "y");
        add(4, "x");

        // `next(^books(4)) ?? -1`: 4 is the last record, so `next` steps off the
        // edge with a catchable absent-element that `?? -1` recovers — it must not
        // abort with `run.unsupported` by landing on the `byShelf` index name.
        assert_eq!(
            run_entry(
                store,
                checked_entry!(
                    &program,
                    "test::nextOrDefaultKey",
                    Value::Int(4),
                    Value::Str("-1".into())
                )
            )
            .expect("next ?? -1")
            .value,
            Some(Value::Str("-1".into()))
        );
        // The same edge with a different default proves `??` is reached, not bypassed.
        assert_eq!(
            run_entry(
                store,
                checked_entry!(&program, "test::nextOrSelfKey", Value::Int(4))
            )
            .expect("next ?? id")
            .value,
            Some(Value::Str("4".into()))
        );
        // `prev(^books)` is the last *record* (4), not the trailing index name.
        assert_eq!(
            run_entry(
                store,
                checked_entry!(&program, "test::lastIdKey", Value::Str("missing".into()))
            )
            .expect("prev")
            .value,
            Some(Value::Str("4".into()))
        );
    }
}

/// `count(path)` over the four presence shapes: a scalar field, a child-bearing
/// layer, and absent paths.
const BOOK_COUNT: &str = "\
resource Book at ^books(id: int)
    required title: string
    subtitle: string
    tags: sequence[string]

fn seed()
    ^books(1).title = \"Mort\"
    const a: int = append(^books(1).tags, \"fiction\")
    const b: int = append(^books(1).tags, \"funny\")

fn countTitle(): int
    return count(^books(1).title)

fn countTags(): int
    return count(^books(1).tags)

fn countMissingField(): int
    return count(^books(1).subtitle)

fn countMissingTags(): int
    return count(^books(2).tags)
";

#[test]
fn count_reports_scalar_presence_and_child_counts() {
    let program = checked_program(BOOK_COUNT);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let count = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry))
            .expect("count")
            .value
    };
    assert_eq!(count("test::countTitle"), Some(Value::Int(1)));
    assert_eq!(count("test::countTags"), Some(Value::Int(2)));
    assert_eq!(count("test::countMissingField"), Some(Value::Int(0)));
    assert_eq!(count("test::countMissingTags"), Some(Value::Int(0)));
}

#[test]
fn count_of_a_path_with_both_value_and_children_counts_children() {
    // When a path has BOTH a value and children, `count` returns the number of
    // immediate children, not children-plus-one.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags: sequence[string]\n\nfn n(): int\n    return count(^books(1).tags)\n",
    );
    let store = TreeStore::memory();
    // Seed a value at `^books(1).tags` itself and two children below it.
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["tags"]),
        SavedValue::Str("self".into()),
    );
    for (pos, value) in [(1, "a"), (2, "b")] {
        write_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &keyed_data_path(
                &program,
                "books",
                &[("tags", vec![SavedKey::Int(pos)])],
                &[],
            ),
            SavedValue::Str(value.into()),
        );
    }
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::n"))
            .expect("run")
            .value,
        Some(Value::Int(2)),
    );
}

/// `count` over a declared index branch returns the number of entries under that
/// branch, exactly as `keys(...)` over the same branch would yield. The branch is
/// a non-unique index so several entries share one query key.
const BOOK_COUNT_INDEX: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string
    tags: sequence[string]

    index byShelf(shelf, id)

fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

fn tag(id: int, t: string): int
    return append(^books(id).tags, t)

fn countBranch(shelf: string): int
    return count(^books.byShelf(shelf))

fn keysBranch(shelf: string): int
    var c = 0
    for id in keys(^books.byShelf(shelf))
        c = c + 1
    return c

fn countRoot(): int
    return count(^books)

fn countLayer(id: int): int
    return count(^books(id).tags)

fn countScalar(id: int): int
    return count(^books(id).title)

fn countRecord(id: int): int
    return count(^books(id))
";

#[test]
fn count_over_an_index_branch_matches_branch_entry_count() {
    let program = checked_program(BOOK_COUNT_INDEX);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ),
        )
        .expect("add");
    };
    add(1, "Mort", "fiction");
    add(2, "Sourcery", "fiction");
    add(3, "Guards", "history");

    let call = |entry: CheckedEntryCall| run_entry(&store, entry).expect("count").value;
    // Two tags on book 1, so its keyed/sequence layer has two entries.
    call(checked_entry!(
        &program,
        "test::tag",
        Value::Int(1),
        Value::Str("a".into())
    ));
    call(checked_entry!(
        &program,
        "test::tag",
        Value::Int(1),
        Value::Str("b".into())
    ));

    // `count(^books.byShelf(shelf))` returns the entry count under that index
    // branch, matching `keys(...)` over the same branch.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::countBranch",
            Value::Str("fiction".into())
        )),
        Some(Value::Int(2))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::keysBranch",
            Value::Str("fiction".into())
        )),
        Some(Value::Int(2))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::countBranch",
            Value::Str("history".into())
        )),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::keysBranch",
            Value::Str("history".into())
        )),
        Some(Value::Int(1))
    );
    // An empty branch counts as zero, like `keys(...)` of it.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::countBranch",
            Value::Str("romance".into())
        )),
        Some(Value::Int(0))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::keysBranch",
            Value::Str("romance".into())
        )),
        Some(Value::Int(0))
    );

    // The previously-correct count shapes stay byte-identical: a keyed/sequence
    // layer counts its entries, a scalar counts as 1, and a whole record counts
    // its populated immediate children. These all keep the read/child-keys path.
    assert_eq!(
        call(checked_entry!(&program, "test::countLayer", Value::Int(1))),
        Some(Value::Int(2))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::countLayer", Value::Int(3))),
        Some(Value::Int(0))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::countScalar", Value::Int(1))),
        Some(Value::Int(1))
    );
    assert!(matches!(
        call(checked_entry!(&program, "test::countRecord", Value::Int(1))),
        Some(Value::Int(n)) if n >= 1
    ));
    // A primary root counts the record identities that direct iteration yields,
    // not generated index branches stored beside the records.
    assert_eq!(
        call(checked_entry!(&program, "test::countRoot")),
        Some(Value::Int(3))
    );
}

#[test]
fn count_over_an_indexed_root_ignores_populated_index_branches() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n    isbn: string\n\n    index byShelf(shelf, id)\n    index byIsbn(isbn) unique\n\nfn add(id: int, t: string, s: string)\n    ^books(id).title = t\n    ^books(id).shelf = s\n\nfn addIsbn(id: int, isbn: string)\n    ^books(id).isbn = isbn\n\nfn countRoot(): int\n    return count(^books)\n\nfn iterRoot(): int\n    var n = 0\n    for book in ^books\n        n = n + 1\n    return n\n",
    );
    let store = TreeStore::memory();
    let call = |entry: CheckedEntryCall| run_entry(&store, entry).expect("run").value;

    assert_eq!(
        call(checked_entry!(&program, "test::countRoot")),
        Some(Value::Int(0))
    );
    call(checked_entry!(
        &program,
        "test::add",
        Value::Int(1),
        Value::Str("Mort".into()),
        Value::Str("fiction".into())
    ));
    assert_eq!(
        call(checked_entry!(&program, "test::countRoot")),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::iterRoot")),
        Some(Value::Int(1))
    );

    call(checked_entry!(
        &program,
        "test::addIsbn",
        Value::Int(1),
        Value::Str("ISBN-1".into())
    ));
    assert_eq!(
        call(checked_entry!(&program, "test::countRoot")),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::iterRoot")),
        Some(Value::Int(1))
    );
}

#[test]
fn count_over_a_direct_saved_root_matches_written_records() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"A\"\n    ^books(2).title = \"B\"\n    ^books(3).title = \"C\"\n\nfn countRoot(): int\n    return count(^books)\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countRoot"))
            .expect("count")
            .value,
        Some(Value::Int(3))
    );
}

#[test]
fn keys_saved_root_loop_returns_ids_in_store_order() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"A\"\n    ^books(2).title = \"B\"\n    ^books(3).title = \"C\"\n\nfn idOrder()\n    for id in keys(^books)\n        print($\"{id}\")\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::idOrder"))
            .expect("id order")
            .output,
        "1\n2\n3\n"
    );
}

#[test]
fn direct_saved_root_loop_streams_ids_and_reads_current_values() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"A\"\n    ^books(2).title = \"B\"\n\nfn mutateFutureValue(): int\n    var total = 0\n    for id in ^books\n        if ^books(id).title == \"A\"\n            total = total * 10 + 1\n            ^books(2).title = \"Z\"\n        else if ^books(id).title == \"B\"\n            total = total * 10 + 2\n        else if ^books(id).title == \"Z\"\n            total = total * 10 + 9\n    return total\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::mutateFutureValue"))
            .expect("loop")
            .value,
        Some(Value::Int(19))
    );
}

#[test]
fn direct_saved_root_loop_returns_before_later_records() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = $\"Book {id}\"\n\nfn printFirstId()\n    for id in keys(^books)\n        print($\"{id}\")\n        return\n",
    );
    let store = TreeStore::memory();
    for id in 1..=129 {
        run_entry(
            &store,
            checked_entry!(&program, "test::seed", Value::Int(id)),
        )
        .expect("seed");
    }

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::printFirstId"))
            .expect("first id")
            .output,
        "1\n"
    );
}

#[test]
fn direct_values_loop_returns_before_reading_later_records() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    note: string\n\nfn seed()\n    ^books(1).title = \"Mort\"\n\nfn firstTitle(): string\n    for book in values(^books)\n        return book.title\n    return \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(2)],
        &data_path(&program, "books", &["note"]),
        SavedValue::Str("incomplete row".into()),
    );

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::firstTitle"))
            .expect("first title")
            .value,
        Some(Value::Str("Mort".into()))
    );
}

#[test]
fn direct_entries_loop_returns_before_reading_later_records() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    note: string\n\nfn seed()\n    ^books(1).title = \"Mort\"\n\nfn firstTitle(): string\n    for id, book in ^books\n        return $\"{id}: {book.title}\"\n    return \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(2)],
        &data_path(&program, "books", &["note"]),
        SavedValue::Str("incomplete row".into()),
    );

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::firstTitle"))
            .expect("first title")
            .value,
        Some(Value::Str("1: Mort".into()))
    );
}

#[test]
fn keys_saved_layer_loops_return_keys_in_order() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn seed()\n    ^books(1).title = \"A\"\n    ^books(1).tags(1) = \"x\"\n    ^books(1).tags(2) = \"y\"\n    ^books(1).tags(3) = \"z\"\n\nfn tagKeys(): int\n    var total = 0\n    for pos in keys(^books(1).tags)\n        total = total * 10 + pos\n    return total\n\nfn tagKeysRev(): int\n    var total = 0\n    for pos in reversed(keys(^books(1).tags))\n        total = total * 10 + pos\n    return total\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::tagKeys"))
            .expect("tag keys")
            .value,
        Some(Value::Int(123))
    );

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::tagKeysRev"))
            .expect("tag keys")
            .value,
        Some(Value::Int(321))
    );
}

#[test]
fn count_over_a_composite_root_matches_direct_iteration() {
    let program = checked_program(
        "resource Cell at ^cells(x: int, y: int)\n    required value: int\n\nfn put(x: int, y: int, value: int)\n    ^cells(x, y).value = value\n\nfn countRoot(): int\n    return count(^cells)\n\nfn iterRoot(): int\n    var n = 0\n    for cell in ^cells\n        n = n + 1\n    return n\n",
    );
    let store = TreeStore::memory();
    for (x, y, value) in [(1, 1, 11), (1, 2, 12), (2, 1, 21)] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::put",
                Value::Int(x),
                Value::Int(y),
                Value::Int(value)
            ),
        )
        .expect("put");
    }
    let call = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry))
            .expect(entry)
            .value
    };
    assert_eq!(call("test::countRoot"), Some(Value::Int(3)));
    assert_eq!(call("test::iterRoot"), Some(Value::Int(3)));
}

/// A resource carrying both a keyed-leaf layer (`tags(pos: int): string`) and a
/// GROUP layer (`versions(version: int)` with member fields). Used to prove that
/// `exists`, `count`, and `std::assert::absent` agree with the actual stored path
/// for a keyed layer entry — the paths a record/field read or write lowers to.
const BOOK_KEYED_PRESENCE: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags(pos: int): string
    versions(version: int)
        required note: string

fn seed()
    ^books(1).title = \"Mort\"
    ^books(1).tags(1) = \"fiction\"
    ^books(1).versions(2).note = \"draft\"

fn tagExists(id: int, pos: int): bool
    return exists(^books(id).tags(pos))

fn versionExists(id: int, ver: int): bool
    return exists(^books(id).versions(ver))

fn tagCount(id: int): int
    return count(^books(id).tags)

fn versionFieldCount(id: int, ver: int): int
    return count(^books(id).versions(ver))

fn topLevelExists(id: int): bool
    return exists(^books(id).title)

fn topLevelCount(id: int): int
    return count(^books(id).title)

fn assertTagAbsent(id: int, pos: int)
    std::assert::absent(^books(id).tags(pos))

fn assertVersionAbsent(id: int, ver: int)
    std::assert::absent(^books(id).versions(ver))
";

// A keyed-leaf layer entry and a group-layer entry are stored under
// `ChildLayer`/`IndexKey` segments — the same shape a normal read or write
// lowers to. `exists`, `count`, and `std::assert::absent` must read that same
// path, not a record-key mis-encoding, so they agree byte-for-byte with what is
// actually stored. (Before routing these through the one canonical lowering, the
// schema-unaware escape hatch encoded `tags(pos)` as a record-key lookup, so
// `exists` mis-reported absent and `assert::absent` passed over a written entry.)
#[test]
fn exists_count_and_assert_absent_agree_over_a_present_keyed_layer_entry() {
    let program = checked_program(BOOK_KEYED_PRESENCE);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let call = |entry: CheckedEntryCall| run_entry(&store, entry).expect("run").value;

    // A present keyed-leaf entry `^books(1).tags(1)` exists, and the layer counts
    // its one entry.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::tagExists",
            Value::Int(1),
            Value::Int(1)
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(&program, "test::tagCount", Value::Int(1))),
        Some(Value::Int(1))
    );

    // A present group entry `^books(1).versions(2)` exists (it carries a `note`
    // child), and counting it counts that one populated member.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::versionExists",
            Value::Int(1),
            Value::Int(2)
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::versionFieldCount",
            Value::Int(1),
            Value::Int(2)
        )),
        Some(Value::Int(1))
    );

    // `std::assert::absent` over either written entry is a failed assertion, not a
    // silent pass: the entry is present.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::assertTagAbsent",
                Value::Int(1),
                Value::Int(1)
            )
        )
        .unwrap_err()
        .code,
        RUN_ASSERT
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::assertVersionAbsent",
                Value::Int(1),
                Value::Int(2)
            )
        )
        .unwrap_err()
        .code,
        RUN_ASSERT
    );

    // An absent keyed-leaf entry and an absent group entry report absent: `exists`
    // is false and `std::assert::absent` passes.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::tagExists",
            Value::Int(1),
            Value::Int(9)
        )),
        Some(Value::Bool(false))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::versionExists",
            Value::Int(1),
            Value::Int(9)
        )),
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::assertTagAbsent",
                Value::Int(1),
                Value::Int(9)
            )
        )
        .expect("absent tag passes")
        .value,
        None
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::assertVersionAbsent",
                Value::Int(1),
                Value::Int(9)
            )
        )
        .expect("absent version passes")
        .value,
        None
    );

    // The already-correct top-level-field shapes stay green: a present field
    // exists and counts as one.
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::topLevelExists",
            Value::Int(1)
        )),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(checked_entry!(
            &program,
            "test::topLevelCount",
            Value::Int(1)
        )),
        Some(Value::Int(1))
    );
}

const NESTED_KEYED_LAYERS: &str = "\
resource Table at ^tables(name: string)
    rows(row: int)
        fields(col: int): string
        cells(col: int)
            required value: string

fn setField(table: string, row: int, col: int, value: string)
    ^tables(table).rows(row).fields(col) = value

fn addField(table: string, row: int, value: string): int
    return append(^tables(table).rows(row).fields, value)

fn fieldAt(table: string, row: int, col: int): string
    return ^tables(table).rows(row).fields(col) ?? \"\"

fn seedCells()
    ^tables(\"t\").rows(1).cells(1).value = \"a\"
    ^tables(\"t\").rows(1).cells(2).value = \"b\"

fn countCells(): int
    return count(^tables(\"t\").rows(1).cells)

fn iterateCells(): int
    var total: int = 0
    for cell in ^tables(\"t\").rows(1).cells
        total = total + 1
    return total

fn cellEntries()
    for col, cell in entries(^tables(\"t\").rows(1).cells)
        print($\"{col}={cell.value}\")
";

#[test]
fn nested_keyed_leaf_entries_write_append_and_read_back() {
    let program = checked_program(NESTED_KEYED_LAYERS);
    let store = TreeStore::memory();

    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::setField",
            Value::Str("t".into()),
            Value::Int(1),
            Value::Int(1),
            Value::Str("a".into()),
        ),
    )
    .expect("nested keyed-leaf write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::fieldAt",
                Value::Str("t".into()),
                Value::Int(1),
                Value::Int(1)
            )
        )
        .expect("read nested keyed leaf")
        .value,
        Some(Value::Str("a".into()))
    );

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::addField",
                Value::Str("t".into()),
                Value::Int(1),
                Value::Str("b".into())
            )
        )
        .expect("append nested keyed leaf")
        .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::fieldAt",
                Value::Str("t".into()),
                Value::Int(1),
                Value::Int(2)
            )
        )
        .expect("read appended nested keyed leaf")
        .value,
        Some(Value::Str("b".into()))
    );
}

#[test]
fn nested_keyed_group_layers_iterate_and_materialize_entries() {
    let program = checked_program(NESTED_KEYED_LAYERS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seedCells")).expect("seed cells");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countCells"))
            .expect("count nested cells")
            .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::iterateCells"))
            .expect("iterate nested cells")
            .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::cellEntries"))
            .expect("entries over nested cells")
            .output,
        "1=a\n2=b\n"
    );
}

#[test]
fn writing_a_nested_keyed_leaf_while_traversing_it_is_a_traversal_fault() {
    checker_rejects(
        "resource Table at ^tables(name: string)\n    rows(row: int)\n        fields(col: int): string\n\nfn mutateNestedLeafDuringTraversal()\n    for col in keys(^tables(\"t\").rows(1).fields)\n        ^tables(\"t\").rows(1).fields(3) = \"c\"\n",
        "check.loop_mutates_traversed_layer",
    );
}

/// `values`/`entries` over a primary root materialize whole records; over a
/// keyed/sequence layer they materialize each entry's value. `entries` feeds the
/// two-name `for id, x in entries(...)` binding.
const BOOK_VALUES: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags: sequence[string]

fn add(id: int, t: string)
    ^books(id).title = t

fn tag(id: int, t: string): int
    return append(^books(id).tags, t)

fn titles()
    for book in values(^books)
        print(book.title)

fn idsAndTitles()
    for id, book in entries(^books)
        print($\"{id}: {book.title}\")

fn tagValues(id: int)
    for tag in values(^books(id).tags)
        print(tag)

fn tagEntries(id: int)
    for pos, tag in entries(^books(id).tags)
        print($\"{pos}={tag}\")

fn titlesReversed()
    for book in reversed(values(^books))
        print(book.title)

fn idsAndTitlesReversed()
    for id, book in reversed(entries(^books))
        print($\"{id}: {book.title}\")

fn tagValuesReversed(id: int)
    for tag in reversed(values(^books(id).tags))
        print(tag)

fn tagEntriesReversed(id: int)
    for pos, tag in reversed(entries(^books(id).tags))
        print($\"{pos}={tag}\")

fn titlesValue()
    const books = values(^books)
    for book in books
        print(book.title)

fn titleEntriesValue()
    const books = entries(^books)
    for id, book in books
        print($\"{id}: {book.title}\")

fn tagValuesValue(id: int)
    const tags = values(^books(id).tags)
    for tag in tags
        print(tag)

fn tagEntriesValue(id: int)
    const tags = entries(^books(id).tags)
    for pos, tag in tags
        print($\"{pos}={tag}\")
";

#[test]
fn values_and_entries_materialize_whole_records_over_a_primary_root() {
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    let add = |id: i64, t: &str| {
        run_entry(
            &store,
            checked_entry!(&program, "test::add", Value::Int(id), Value::Str(t.into())),
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    // `values(^books)` yields each whole record, in key order, with field access.
    let titles = run_entry(&store, checked_entry!(&program, "test::titles")).expect("run");
    assert_eq!(titles.output, "Mort\nSourcery\n");

    // `entries(^books)` binds the identity and the materialized record together.
    let pairs = run_entry(&store, checked_entry!(&program, "test::idsAndTitles")).expect("run");
    assert_eq!(pairs.output, "1: Mort\n2: Sourcery\n");
}

#[test]
fn values_and_entries_materialize_entries_over_a_keyed_layer() {
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("add");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::tag",
            Value::Int(1),
            Value::Str("fiction".into())
        ),
    )
    .expect("tag");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::tag",
            Value::Int(1),
            Value::Str("funny".into())
        ),
    )
    .expect("tag");

    // `values(^books(1).tags)` yields each leaf value in key order.
    let values = run_entry(
        &store,
        checked_entry!(&program, "test::tagValues", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(values.output, "fiction\nfunny\n");

    // `entries(...)` binds each 1-based position to its leaf value.
    let entries = run_entry(
        &store,
        checked_entry!(&program, "test::tagEntries", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(entries.output, "1=fiction\n2=funny\n");
}

#[test]
fn saved_values_and_entries_as_values_are_rejected() {
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("add");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::tag",
            Value::Int(1),
            Value::Str("fiction".into())
        ),
    )
    .expect("tag");

    let error = run_entry(&store, checked_entry!(&program, "test::titlesValue")).unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
    let error = run_entry(&store, checked_entry!(&program, "test::titleEntriesValue")).unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
    let error = run_entry(
        &store,
        checked_entry!(&program, "test::tagValuesValue", Value::Int(1)),
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
    let error = run_entry(
        &store,
        checked_entry!(&program, "test::tagEntriesValue", Value::Int(1)),
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
}

#[test]
fn reversed_values_and_entries_bind_values_and_pairs_descending() {
    // `for x in reversed(values(L))` must bind whole values descending — not the
    // bare child keys. Likewise `for k, v in reversed(entries(L))` binds (key,
    // value) pairs descending, not key-only segments (which would runtime-error).
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    let add = |id: i64, t: &str| {
        run_entry(
            &store,
            checked_entry!(&program, "test::add", Value::Int(id), Value::Str(t.into())),
        )
        .expect("add");
    };
    add(1, "Mort");
    add(2, "Sourcery");

    // `reversed(values(^books))` yields whole records in descending key order.
    let titles = run_entry(&store, checked_entry!(&program, "test::titlesReversed")).expect("run");
    assert_eq!(titles.output, "Sourcery\nMort\n");

    // `reversed(entries(^books))` binds (identity, record) pairs descending.
    let pairs = run_entry(
        &store,
        checked_entry!(&program, "test::idsAndTitlesReversed"),
    )
    .expect("run");
    assert_eq!(pairs.output, "2: Sourcery\n1: Mort\n");
}

#[test]
fn reversed_values_and_entries_over_a_keyed_layer_descend() {
    // The same shaping over a keyed/sequence child layer: values and (pos, value)
    // pairs descend by key, rather than collapsing to bare position keys.
    let program = checked_program(BOOK_VALUES);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("add");
    for tag in ["fiction", "funny", "fantasy"] {
        run_entry(
            &store,
            checked_entry!(&program, "test::tag", Value::Int(1), Value::Str(tag.into())),
        )
        .expect("tag");
    }

    // `reversed(values(^books(1).tags))` yields each leaf value in descending order.
    let values = run_entry(
        &store,
        checked_entry!(&program, "test::tagValuesReversed", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(values.output, "fantasy\nfunny\nfiction\n");

    // `reversed(entries(...))` binds each position to its value, descending.
    let entries = run_entry(
        &store,
        checked_entry!(&program, "test::tagEntriesReversed", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(entries.output, "3=fantasy\n2=funny\n1=fiction\n");
}

const BOOK_ISBN_SAVE: &str = "\
module test
resource Book at ^books(id: int)
    isbn: string
    index byIsbn(isbn) unique
fn save(i: int, code: string)
    ^books(i).isbn = code
";

#[test]
fn a_recoverable_write_fault_is_catchable_across_a_call_boundary() {
    // A write fault raised in a CALLED function must be catchable by the caller's
    // try/catch (the transaction-recovery contract), not only within the same frame.
    let program = checked_program(&format!(
        "{BOOK_ISBN_SAVE}\
         pub fn run(): string\n\
         \x20   save(1, \"x\")\n\
         \x20   try\n\
         \x20       save(2, \"x\")\n\
         \x20       return \"uncaught\"\n\
         \x20   catch e: Error\n\
         \x20       return e.code\n"
    ));
    let store = TreeStore::memory();
    let value = run_entry(&store, checked_entry!(&program, "test::run"))
        .expect("run")
        .value;
    assert_eq!(value, Some(Value::Str("write.unique_conflict".into())));
}

#[test]
fn an_uncaught_cross_boundary_write_fault_keeps_its_dotted_code() {
    // Crossing a call boundary must not collapse an uncaught fault to
    // run.uncaught_error: it surfaces with its own dotted code (and exit code).
    let program = checked_program(&format!(
        "{BOOK_ISBN_SAVE}\
         pub fn run()\n\
         \x20   save(1, \"x\")\n\
         \x20   save(2, \"x\")\n"
    ));
    let store = TreeStore::memory();
    let error = run_entry(&store, checked_entry!(&program, "test::run")).unwrap_err();
    assert_eq!(error.code, "write.unique_conflict", "{error:?}");
}

const PATIENT_SPARSE_GROUP: &str = "\
module test
resource Patient at ^patients(id: string)
    name
        first: string
        last: string
";

const PATIENT_REQUIRED_GROUP: &str = "\
module test
resource Patient at ^patients(id: string)
    name
        required first: string
        last: string
";

const PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP: &str = "\
module test
resource Patient at ^patients(id: string)
    visits(pos: int)
        name
            required first: string
            last: string

pub fn seed()
    ^patients(\"p1\").visits(1).name.first = \"Sam\"
    ^patients(\"p1\").visits(1).name.last = \"Vimes\"

pub fn drop()
    delete ^patients(\"p1\").visits(1).name

pub fn visit_first(): string
    return ^patients(\"p1\").visits(1).name.first ?? \"\"
";

#[test]
fn deleting_a_sparse_field_inside_an_unkeyed_group_is_allowed() {
    // Field delete descends unkeyed-group layers. Sparse descendants may still be
    // deleted independently.
    let program = checked_program(&format!(
        "{PATIENT_SPARSE_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name.last\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::drop"))
        .expect("sparse group-field delete is a no-op");
}

#[test]
fn deleting_a_required_field_inside_an_unkeyed_group_is_rejected() {
    let program = checked_program(&format!(
        "{PATIENT_REQUIRED_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name.first\n"
    ));
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::drop"));
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_field"),
        "{result:?}"
    );
}

#[test]
fn deleting_an_unkeyed_group_with_required_descendants_is_rejected() {
    let program = checked_program(&format!(
        "{PATIENT_REQUIRED_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name\n"
    ));
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::drop"));
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_field"),
        "{result:?}"
    );
}

#[test]
fn deleting_a_nested_unkeyed_group_with_required_descendants_is_rejected() {
    let program = checked_program(PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let result = run_entry(&store, checked_entry!(&program, "test::drop"));
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_field"),
        "{result:?}"
    );
}

#[test]
fn maintenance_can_delete_a_nested_unkeyed_group_with_required_descendants() {
    let program = checked_program(PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let host = Host::new().with_maintenance();
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::drop")).expect("drop");
    for field in ["first", "last"] {
        assert_eq!(
            read_data_bytes(
                &program,
                &store,
                "patients",
                &[SavedKey::Str("p1".into())],
                &keyed_data_path(
                    &program,
                    "patients",
                    &[("visits", vec![SavedKey::Int(1)])],
                    &["name", field],
                ),
            ),
            None,
            "{field} removed under maintenance"
        );
    }
}

#[test]
fn keyed_group_entry_read_materializes_unkeyed_group_descendants() {
    let program = checked_program(PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let outcome = run_entry(&store, checked_entry!(&program, "test::visit_first")).expect("read");
    assert_eq!(outcome.value, Some(Value::Str("Sam".into())));
}

// --- Maintenance mode & managed-root protection ---

/// A two-key books program with an index, reused by the maintenance tests below:
/// it can seed records, drop the whole `^books` root, and count remaining records
/// and index entries so a root drop's effect is observable.
const MAINTENANCE_BOOKS: &str = "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\nfn seed()\n    ^books(1).title = \"Mort\"\n    ^books(1).shelf = \"fiction\"\n    ^books(2).title = \"Guards\"\n    ^books(2).shelf = \"fiction\"\n\nfn drop_root()\n    delete ^books\n\nfn drop_root_while_iterating_index()\n    for id in keys(^books.byShelf(\"fiction\"))\n        delete ^books\n\nfn record_count(): int\n    var c = 0\n    for book in ^books\n        c = c + 1\n    return c\n\nfn shelf_count(s: string): int\n    var c = 0\n    for id in keys(^books.byShelf(s))\n        c = c + 1\n    return c\n";

#[test]
fn deleting_a_whole_root_without_maintenance_is_rejected() {
    // `delete ^books` on a keyed root is maintenance work; with no maintenance
    // capability the run is rejected with `write.requires_maintenance`.
    let program = checked_program(MAINTENANCE_BOOKS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let result = run_entry(&store, checked_entry!(&program, "test::drop_root"));
    assert!(
        matches!(result, Err(ref error) if error.code == "write.requires_maintenance"),
        "{result:?}"
    );
    // The records still exist: the rejected delete did not touch the store.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::record_count"))
            .expect("count")
            .value,
        Some(Value::Int(2))
    );
}

#[test]
fn deleting_a_whole_root_under_maintenance_drops_records_and_indexes() {
    // With the maintenance capability, `delete ^books` drops the entire managed
    // root subtree: no records and no index entries remain.
    let program = checked_program(MAINTENANCE_BOOKS);
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::drop_root"))
        .expect("drop root");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::record_count")
        )
        .expect("count")
        .value,
        Some(Value::Int(0)),
        "no records remain after the root drop"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_count", Value::Str("fiction".into()))
        )
        .expect("count")
        .value,
        Some(Value::Int(0)),
        "no index entries remain after the root drop"
    );
}

#[test]
fn maintenance_root_delete_while_iterating_an_index_is_a_traversal_fault() {
    let program = checked_program(MAINTENANCE_BOOKS);
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::seed")).expect("seed");
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::drop_root_while_iterating_index"),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TRAVERSAL),
        "{result:?}"
    );
}

#[test]
fn whole_identity_delete_stays_ungated_under_no_maintenance() {
    // `delete ^books(1)` is ordinary whole-identity work: it must still succeed
    // with no maintenance capability, leaving the sibling record in place.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"Mort\"\n    ^books(2).title = \"Guards\"\n\nfn drop_one()\n    delete ^books(1)\n\nfn record_count(): int\n    var c = 0\n    for book in ^books\n        c = c + 1\n    return c\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry(&store, checked_entry!(&program, "test::drop_one"))
        .expect("ordinary identity delete");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::record_count"))
            .expect("count")
            .value,
        Some(Value::Int(1)),
        "the sibling record survives an ordinary identity delete"
    );
}

#[test]
fn deleting_a_required_field_under_maintenance_succeeds() {
    // A required-field delete is rejected without maintenance (existing behavior),
    // but a maintenance run lifts the guard and actually removes the field.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn drop_title(id: int)\n    delete ^books(id).title\n\nfn has_title(id: int): bool\n    return exists(^books(id).title)\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::drop_title", Value::Int(1)),
    )
    .expect("maintenance lifts the required-field guard");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "the required field is gone after a maintenance delete"
    );
}

#[test]
fn maintenance_transaction_can_delete_required_field_after_a_field_write() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).shelf = \"fiction\"\n\nfn repair(id: int)\n    transaction\n        ^books(id).shelf = \"legacy\"\n        delete ^books(id).title\n\nfn has_title(id: int): bool\n    return exists(^books(id).title)\n\nfn shelf_of(id: int): string\n    return ^books(id).shelf ?? \"\"\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::repair", Value::Int(1)),
    )
    .expect("maintenance transaction");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "maintenance still permits required-field repair deletes"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("legacy".into())),
        "the transaction's ordinary field write still committed"
    );
}

#[test]
fn maintenance_transaction_still_rejects_new_partial_required_record() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n    required name: string\n    shelf: string\n\nfn create(id: int)\n    transaction\n        ^items(id).shelf = \"legacy\"\n\nfn has_item(id: int): bool\n    return exists(^items(id))\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_absent"),
        "{result:?}"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "maintenance does not make a sparse field write a partial-record creator"
    );
}

#[test]
fn maintenance_transaction_noop_required_delete_does_not_permit_partial_record() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n    required name: string\n    shelf: string\n\nfn create(id: int)\n    transaction\n        ^items(id).shelf = \"legacy\"\n        delete ^items(id).name\n\nfn has_item(id: int): bool\n    return exists(^items(id))\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_absent"),
        "{result:?}"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "a no-op required delete is not a license to create partial data"
    );
}

#[test]
fn maintenance_transaction_staged_required_delete_does_not_permit_partial_record() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n    required name: string\n    shelf: string\n\nfn create(id: int)\n    transaction\n        ^items(id).name = \"temporary\"\n        ^items(id).shelf = \"legacy\"\n        delete ^items(id).name\n\nfn has_item(id: int): bool\n    return exists(^items(id))\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_absent"),
        "{result:?}"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "staged-only required data cannot mint a maintenance exemption"
    );
}

#[test]
fn maintenance_transaction_whole_resource_required_delete_does_not_permit_partial_record() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n    required name: string\n    shelf: string\n\nfn create(id: int)\n    var item: Item\n    item.name = \"temporary\"\n    item.shelf = \"legacy\"\n    transaction\n        ^items(id) = item\n        delete ^items(id).name\n\nfn has_item(id: int): bool\n    return exists(^items(id))\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_absent"),
        "{result:?}"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "a whole-resource write cannot create then delete required data to bypass validation"
    );
}

#[test]
fn maintenance_transaction_whole_group_required_delete_does_not_permit_partial_entry() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    versions(version: int)\n        required title: string\n        note: string\n\nfn seed(id: int)\n    ^books(id).title = \"root\"\n\nfn create_version(id: int)\n    var version: Book\n    version.title = \"temporary\"\n    version.note = \"legacy\"\n    transaction\n        ^books(id).versions(1) = version\n        delete ^books(id).versions(1).title\n\nfn has_version(id: int): bool\n    return exists(^books(id).versions(1))\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create_version", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_absent"),
        "{result:?}"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_version", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "a whole group-entry write cannot create then delete required data to bypass validation"
    );
}

#[test]
fn maintenance_outer_delete_of_inner_created_required_field_is_rejected() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n    required name: string\n    shelf: string\n\nfn create(id: int)\n    transaction\n        transaction\n            ^items(id).name = \"temporary\"\n            ^items(id).shelf = \"legacy\"\n        delete ^items(id).name\n\nfn has_item(id: int): bool\n    return exists(^items(id))\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_absent"),
        "{result:?}"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "an outer transaction must still validate entries created by an inner commit"
    );
}

#[test]
fn maintenance_required_delete_exemption_covers_parent_validation_for_child_writes() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n    notes(note: string)\n        required text: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).shelf = \"fiction\"\n\nfn repair(id: int)\n    transaction\n        delete ^books(id).title\n        ^books(id).notes(\"n1\").text = \"indexed\"\n\nfn has_title(id: int): bool\n    return exists(^books(id).title)\n\nfn note_text(id: int): string\n    return ^books(id).notes(\"n1\").text ?? \"\"\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::repair", Value::Int(1)),
    )
    .expect("parent maintenance delete should not block child validation");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::note_text", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("indexed".into()))
    );
}

#[test]
fn maintenance_required_delete_exemption_crosses_nested_transaction_commit() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).shelf = \"fiction\"\n\nfn inner_delete(id: int)\n    transaction\n        ^books(id).shelf = \"outer\"\n        transaction\n            delete ^books(id).title\n\nfn outer_delete(id: int)\n    transaction\n        delete ^books(id).title\n        transaction\n            ^books(id).shelf = \"inner\"\n\nfn has_title(id: int): bool\n    return exists(^books(id).title)\n\nfn shelf_of(id: int): string\n    return ^books(id).shelf ?? \"\"\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::inner_delete", Value::Int(1)),
    )
    .expect("inner maintenance delete should satisfy outer validation");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("outer".into()))
    );

    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(2)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::outer_delete", Value::Int(2)),
    )
    .expect("outer maintenance delete should satisfy inner validation");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_of", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("inner".into()))
    );
}

#[test]
fn quoted_undeclared_saved_field_write_is_unknown_field_without_maintenance() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn quoted_write(id: int)\n    ^books(id).\"old-title\" = \"legacy\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::quoted_write", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.unknown_field"),
        "{result:?}"
    );
}

#[test]
fn quoted_undeclared_saved_field_write_is_unknown_field_under_maintenance() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn quoted_write(id: int, v: string)\n    ^books(id).\"old-title\" = v\n\nfn quoted_read(id: int): string\n    return ^books(id).\"old-title\"\n",
        "check.untyped_value",
    );
}

#[test]
fn quoted_undeclared_saved_field_write_does_not_get_raw_type_rules() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn quoted_write(id: int, n: int)\n    ^books(id).\"count\" = n\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::quoted_write", Value::Int(1), Value::Int(5)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.unknown_field"),
        "{result:?}"
    );
}

#[test]
fn unquoted_undeclared_field_stays_unknown_field_even_under_maintenance() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn typo(id: int)\n    ^books(id).nope = \"x\"\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::typo", Value::Int(1)),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == "write.unknown_field"),
        "{result:?}"
    );
}

#[test]
fn quoted_declared_saved_field_write_uses_managed_path_under_maintenance() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).shelf = \"fiction\"\n\nfn quoted_update(id: int)\n    ^books(id).\"shelf\" = \"history\"\n\nfn shelf_at(s: string): int\n    var c = 0\n    for id in keys(^books.byShelf(s))\n        c = c + 1\n    return c\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::quoted_update", Value::Int(1)),
    )
    .expect("quoted declared field write uses the managed path");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_at", Value::Str("fiction".into()))
        )
        .expect("count")
        .value,
        Some(Value::Int(0)),
        "the managed write moved the record off the old shelf index entry"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_at", Value::Str("history".into()))
        )
        .expect("count")
        .value,
        Some(Value::Int(1)),
        "the managed write added the new shelf index entry"
    );
}

#[test]
fn quoted_undeclared_saved_field_read_is_managed_field_error_under_maintenance() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn quoted_read(id: int): string\n    return ^books(id).\"old-title\"\n",
        "check.untyped_value",
    );
}

#[test]
fn managed_write_to_a_declared_field_is_unaffected() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).shelf = \"fiction\"\n\nfn move_shelf(id: int, s: string)\n    ^books(id).shelf = s\n\nfn shelf_at(s: string): int\n    var c = 0\n    for id in keys(^books.byShelf(s))\n        c = c + 1\n    return c\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(
            &program,
            "test::move_shelf",
            Value::Int(1),
            Value::Str("history".into())
        ),
    )
    .expect("managed write to a declared field");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_at", Value::Str("history".into()))
        )
        .expect("count")
        .value,
        Some(Value::Int(1)),
        "the managed write moved the record's index entry to the new shelf"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_at", Value::Str("fiction".into()))
        )
        .expect("count")
        .value,
        Some(Value::Int(0)),
        "no stale entry remains on the old shelf"
    );
}

#[test]
fn quoted_declared_saved_field_write_uses_managed_path_without_maintenance() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\nfn quoted_update(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).\"shelf\" = \"history\"\n\nfn shelf(id: int): string\n    return ^books(id).shelf ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::quoted_update", Value::Int(1)),
    )
    .expect("quoted declared field write uses the managed path");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::shelf", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("history".into()))
    );
}

// --- Saved-path lowering (the one `lower` pass) ---
//
// These pin the equivalence-risk corners of the unified lowering: the identity
// splice versus raw keys, the keyed-root arity message, the unkeyed-group hop
// versus keyed-layer distinction, and the index-branch terminal classification.

/// A single-key store identity splices in as the whole key, addressing the same
/// record a bare int key does.
#[test]
fn an_identity_argument_splices_in_as_the_record_key() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         fn save()\n    const id = nextId(^books)\n    ^books(id).title = \"a\"\n\n\
         fn read(): string\n    return ^books(1).title\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read")
            .value,
        Some(Value::Str("a".into()))
    );
}

#[test]
fn a_wrong_typed_key_faults_at_lowering_and_does_not_write() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         fn save(bad: string)\n    ^books(bad).title = \"a\"\n",
        "check.key_type",
    );
}

/// A single-key identity from a string-keyed store cannot be spliced into an
/// int-keyed root; lowering rejects the scalar mismatch before writing.
#[test]
fn a_wrong_scalar_spliced_identity_faults_and_does_not_write() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         resource Magazine at ^magazines(issn: string)\n    required title: string\n\n\
         fn seed()\n    ^magazines(\"issn\").title = \"m\"\n\n\
         fn save()\n    for id in ^magazines\n        ^books(id).title = \"a\"\n",
        "check.key_type",
    );
}

/// A single-key identity produced from the target store still writes through the
/// saved path lowering.
#[test]
fn a_single_key_store_identity_splice_still_writes() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         fn seed()\n    ^books(7).title = \"seed\"\n\n\
         fn save()\n    for id in ^books\n        ^books(id).title = \"a\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry(&store, checked_entry!(&program, "test::save"))
        .expect("store identity splice writes");
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(7)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("a".into()))
    );
}

/// A composite identity cannot be one component among raw keys: `^pairs(id, 5)`
/// mixing the spliced identity with a trailing raw key is rejected as unsupported.
#[test]
fn an_identity_mixed_with_a_raw_key_is_rejected() {
    checker_rejects(
        "resource Pair at ^pairs(a: int, b: int)\n    required title: string\n\n\
         fn seed()\n    ^pairs(7, 8).title = \"seed\"\n\n\
         fn save()\n    for id in ^pairs\n        ^pairs(id, 5).title = \"a\"\n",
        "check.key_type",
    );
}

/// Addressing a keyed root without an identity is a type error naming the
/// expected key count, not a silent read of the keyless path.
#[test]
fn a_keyed_root_without_an_identity_is_a_type_error() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         fn read(): string\n    return ^books.title\n",
        "check.untyped_value",
    );
}

/// An unkeyed group hop (`^patients(id).name.first`) lowers `name` as a zero-key
/// group layer, landing the field under a `ChildLayer`, not as a top-level field.
#[test]
fn an_unkeyed_group_hop_lowers_to_a_child_layer() {
    let program = checked_program(
        "resource Patient at ^patients(id: int)\n    mrn: string\n    name\n        first: string\n\n\
         fn save()\n    ^patients(1).name.first = \"Sam\"\n\n\
         fn read(): string\n    return ^patients(1)?.name?.first ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read")
            .value,
        Some(Value::Str("Sam".into()))
    );
    // The field landed under the `name` group layer, not beside `mrn`.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "patients",
            &[SavedKey::Int(1)],
            &data_path(&program, "patients", &["name", "first"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Sam".into()))
    );
}

// ---------------------------------------------------------------------------
// Opt-in statement debugger hook (`StepHook` / `Frame` / `run_entry_with_debugger`).
// ---------------------------------------------------------------------------

use marrow_run::{Frame, RuntimeError, StepHook};
use marrow_syntax::SourceSpan;

/// A test hook that records, for each statement it is offered, the statement's
/// line and the sorted `name=display_debug` of its visible locals plus the
/// activation depth. Optionally aborts at a given line to exercise the
/// terminate-by-Err contract.
#[derive(Default)]
struct Recorder {
    steps: Vec<(u32, Vec<String>, usize)>,
    abort_at_line: Option<u32>,
}

impl StepHook for Recorder {
    fn before_statement(
        &mut self,
        span: SourceSpan,
        frame: Frame<'_, '_>,
    ) -> Result<(), RuntimeError> {
        let mut locals: Vec<String> = frame
            .locals()
            .map(|(name, value)| format!("{name}={}", value.display_debug()))
            .collect();
        locals.sort();
        self.steps.push((span.line, locals, frame.depth()));
        if self.abort_at_line == Some(span.line) {
            return Err(RuntimeError {
                code: marrow_run::RUN_UNSUPPORTED,
                message: "debugger terminate".into(),
                span,
                throw: None,
                origin: None,
            });
        }
        Ok(())
    }
}

#[test]
fn hook_observes_each_statement_with_its_locals_and_depth() {
    // Three statements on consecutive lines, each adding one local; the hook is
    // offered each before it runs, so it sees the locals bound by earlier ones.
    let program = checked_program(
        "fn compute(a: int): int\n\
         \x20\x20\x20\x20const b = a + 1\n\
         \x20\x20\x20\x20var c = b * 2\n\
         \x20\x20\x20\x20return c\n",
    );
    let store = TreeStore::memory();
    let mut hook = Recorder::default();
    let outcome = run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::compute", Value::Int(10)),
    )
    .expect("debugged run completes");
    assert_eq!(outcome.value, Some(Value::Int(22)));

    // The helper writes snippets as `module test`, so the body statements land
    // on lines 4..=6 in the checked source file.
    assert_eq!(
        hook.steps,
        vec![
            (4, vec!["a=10".to_string()], 1),
            (5, vec!["a=10".to_string(), "b=11".to_string()], 1),
            (
                6,
                vec!["a=10".to_string(), "b=11".to_string(), "c=22".to_string()],
                1,
            ),
        ],
    );
}

#[test]
fn hook_depth_tracks_nested_activations() {
    // A call deepens the activation; the callee's statements report a greater
    // depth than the caller's, so step-over/step-out can be expressed by depth.
    let program = checked_program(
        "fn inner(): int\n\
         \x20\x20\x20\x20return 1\n\
         \n\
         fn outer(): int\n\
         \x20\x20\x20\x20const x = inner()\n\
         \x20\x20\x20\x20return x\n",
    );
    let store = TreeStore::memory();
    let mut hook = Recorder::default();
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::outer"),
    )
    .expect("debugged run completes");

    let depths: Vec<usize> = hook.steps.iter().map(|(_, _, depth)| *depth).collect();
    // outer's `const x = inner()` (depth 1), inner's `return 1` (depth 2 during
    // the nested call), then outer's `return x` (back at depth 1).
    assert_eq!(depths, vec![1, 2, 1]);
}

#[test]
fn hook_store_handle_sees_prior_writes() {
    // `Frame::store()` is the live handle, so a write made by an earlier statement
    // is visible to the hook when it inspects a later one (read-your-writes).
    let program = checked_program(
        "resource Account at ^accts(id: int)\n\
         \x20\x20\x20\x20balance: int\n\
         \n\
         fn seed(): int\n\
         \x20\x20\x20\x20^accts(1).balance = 7\n\
         \x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();

    struct StorePeeker<'a> {
        program: &'a CheckedRuntimeProgram,
        balance_seen_at_return: Option<i64>,
    }
    impl StepHook for StorePeeker<'_> {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            // The `return 0` is the second body statement (line 8). By then the
            // first statement has written the balance, so the live store reflects it.
            if span.line == 8 {
                self.balance_seen_at_return = match read_data_value(
                    self.program,
                    frame.store(),
                    "accts",
                    &[SavedKey::Int(1)],
                    &data_path(self.program, "accts", &["balance"]),
                    ScalarType::Int,
                ) {
                    Some(SavedValue::Int(n)) => Some(n),
                    _ => None,
                };
            }
            Ok(())
        }
    }

    let mut hook = StorePeeker {
        program: &program,
        balance_seen_at_return: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::seed"),
    )
    .expect("debugged run completes");
    assert_eq!(hook.balance_seen_at_return, Some(7));
}

#[test]
fn hook_error_aborts_the_run() {
    // Returning Err from `before_statement` terminates the run; the error
    // surfaces and later statements never execute.
    let program = checked_program(
        "fn compute(): int\n\
         \x20\x20\x20\x20const a = 1\n\
         \x20\x20\x20\x20const b = 2\n\
         \x20\x20\x20\x20return a + b\n",
    );
    let store = TreeStore::memory();
    let mut hook = Recorder {
        steps: Vec::new(),
        abort_at_line: Some(5),
    };
    let error = run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::compute"),
    )
    .expect_err("the hook aborts the run");
    assert_eq!(error.code, marrow_run::RUN_UNSUPPORTED);
    assert_eq!(error.message, "debugger terminate");
    // Only the statements up to and including the aborting one were offered.
    let lines: Vec<u32> = hook.steps.iter().map(|(line, _, _)| *line).collect();
    assert_eq!(lines, vec![4, 5]);
}

/// A hook recording each managed write it is offered: its operation, the human
/// path, whether a value was present, and the activation depth. Statement events
/// are ignored, so the recorded sequence is exactly the run's managed writes in
/// commit order.
#[derive(Default)]
struct WriteRecorder {
    writes: Vec<(marrow_run::WriteOp, WriteTarget, bool, usize)>,
}

impl StepHook for WriteRecorder {
    fn before_statement(
        &mut self,
        _span: SourceSpan,
        _frame: Frame<'_, '_>,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn before_write(
        &mut self,
        op: marrow_run::WriteOp,
        target: &WriteTarget,
        value: Option<&[u8]>,
        depth: usize,
    ) {
        self.writes
            .push((op, target.clone(), value.is_some(), depth));
    }
}

#[test]
fn hook_observes_each_managed_write_in_commit_order() {
    // A field write, then a whole-record delete: the field write stages one
    // `Write` (the field). `delete ^books(1)` clears the record's subtree with one
    // `Delete` of the identity path. The hook sees each `PlanStep` as a
    // `before_write` event, in commit order, at the statement's activation depth.
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
         \n\
         fn seed(): int\n\
         \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
         \x20\x20\x20\x20delete ^books(1)\n\
         \x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();
    let mut hook = WriteRecorder::default();
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::seed"),
    )
    .expect("debugged run completes");

    assert_eq!(
        hook.writes,
        vec![
            (
                marrow_run::WriteOp::Write,
                WriteTarget::Data {
                    store: store_catalog_id(&program, "books").as_str().to_string(),
                    identity: vec![SavedKey::Int(1)],
                    path: write_data_path(data_path(&program, "books", &["title"])),
                },
                true,
                1
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store_catalog_id(&program, "books").as_str().to_string(),
                    identity: vec![SavedKey::Int(1)],
                    path: Vec::new(),
                },
                false,
                1
            ),
        ],
    );
}

#[test]
fn hook_observes_each_managed_delete_shape() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
         \n\
         \x20\x20\x20\x20details\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20meta\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20label: string\n\
         \n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \n\
         fn seed()\n\
         \x20\x20\x20\x20^books(1).details.note = \"top\"\n\
         \x20\x20\x20\x20^books(2).details.meta.label = \"nested group\"\n\
         \x20\x20\x20\x20^books(3).details.meta.label = \"nested scalar\"\n\
         \x20\x20\x20\x20^books(4).versions(1).note = \"entry\"\n\
         \n\
         fn dropAll()\n\
         \x20\x20\x20\x20delete ^books(1).details\n\
         \x20\x20\x20\x20delete ^books(2).details.meta\n\
         \x20\x20\x20\x20delete ^books(3).details.meta.label\n\
         \x20\x20\x20\x20delete ^books(4).versions(1)\n",
    );
    let store = TreeStore::memory();
    run_entry_with_host(&store, &Host::new(), checked_entry!(&program, "test::seed"))
        .expect("seed runs");
    let mut hook = WriteRecorder::default();
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::dropAll"),
    )
    .expect("delete run completes");

    let store = store_catalog_id(&program, "books").as_str().to_string();
    assert_eq!(
        hook.writes,
        vec![
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store.clone(),
                    identity: vec![SavedKey::Int(1)],
                    path: write_data_path(data_path(&program, "books", &["details"])),
                },
                false,
                1,
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store.clone(),
                    identity: vec![SavedKey::Int(2)],
                    path: write_data_path(data_path(&program, "books", &["details", "meta"])),
                },
                false,
                1,
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store.clone(),
                    identity: vec![SavedKey::Int(3)],
                    path: write_data_path(data_path(
                        &program,
                        "books",
                        &["details", "meta", "label"],
                    )),
                },
                false,
                1,
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store,
                    identity: vec![SavedKey::Int(4)],
                    path: write_data_path(keyed_data_path(
                        &program,
                        "books",
                        &[("versions", vec![SavedKey::Int(1)])],
                        &[],
                    )),
                },
                false,
                1,
            ),
        ],
    );
}

#[test]
fn hook_observes_maintenance_whole_root_deletes() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
         \x20\x20\x20\x20shelf: string\n\
         \n\
         \x20\x20\x20\x20index byShelf(shelf, id)\n\
         \n\
         fn seed()\n\
         \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
         \x20\x20\x20\x20^books(1).shelf = \"fiction\"\n\
         \n\
         fn dropRoot()\n\
         \x20\x20\x20\x20delete ^books\n",
    );
    let store = TreeStore::memory();
    run_entry_with_host(&store, &Host::new(), checked_entry!(&program, "test::seed"))
        .expect("seed runs");
    let mut hook = WriteRecorder::default();
    run_entry_with_debugger(
        &store,
        &Host::new().with_maintenance(),
        &mut hook,
        checked_entry!(&program, "test::dropRoot"),
    )
    .expect("maintenance root drop runs");

    assert_eq!(
        hook.writes,
        vec![
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store_catalog_id(&program, "books").as_str().to_string(),
                    identity: Vec::new(),
                    path: Vec::new(),
                },
                false,
                1,
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Index {
                    index: index_catalog_id(&program, "books", "byShelf")
                        .as_str()
                        .to_string(),
                    keys: Vec::new(),
                    identity: Vec::new(),
                },
                false,
                1,
            ),
        ],
    );
}

#[test]
fn no_hook_run_pays_no_write_observation() {
    // Regression: a write with no hook installed runs through the non-debugged
    // entry exactly as before. The default `before_write` is never reached, so the
    // managed write still lands and the run completes.
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
         \n\
         fn seed(): int\n\
         \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
         \x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();
    run_entry_with_host(&store, &Host::new(), checked_entry!(&program, "test::seed"))
        .expect("run completes");
    let title = match read_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["title"]),
        ScalarType::Str,
    ) {
        Some(SavedValue::Str(s)) => Some(s),
        _ => None,
    };
    assert_eq!(title, Some("Mort".to_string()));
}

#[test]
fn display_debug_renders_scalars_and_structured_previews() {
    // Scalars render like the normal renderer.
    assert_eq!(Value::Int(42).display_debug(), "42");
    assert_eq!(Value::Bool(true).display_debug(), "true");
    assert_eq!(Value::Str("hi".into()).display_debug(), "hi");

    // Bytes and a sequence get a total, never-faulting preview (the normal
    // renderer refuses these).
    assert_eq!(Value::Bytes(vec![1, 2, 3]).display_debug(), "bytes[3]");
    assert_eq!(
        Value::Sequence(vec![Value::Int(1), Value::Int(2)]).display_debug(),
        "sequence[2]"
    );

    // A resource previews its present field names; an identity previews its keys.
    assert_eq!(
        Value::Resource(vec![
            ("title".into(), Value::Str("v2".into())),
            ("pages".into(), Value::Int(3)),
        ])
        .display_debug(),
        "resource{title, pages}"
    );
    assert_eq!(
        Value::Identity(vec![SavedKey::Int(17)]).display_debug(),
        "identity(17)"
    );
}

#[test]
fn an_ordinary_run_with_host_is_unchanged_by_the_hook_machinery() {
    // Sanity: the same program through the non-debugged entry behaves exactly as
    // before — no hook installed, no behavior change.
    let program = checked_program(
        "fn compute(a: int): int\n\
         \x20\x20\x20\x20const b = a + 1\n\
         \x20\x20\x20\x20return b\n",
    );
    let store = TreeStore::memory();
    let outcome = run_entry_with_host(
        &store,
        &Host::new(),
        checked_entry!(&program, "test::compute", Value::Int(4)),
    )
    .expect("run completes");
    assert_eq!(outcome.value, Some(Value::Int(5)));
}

// --- W1 unified resolver: module-aware, visibility-aware runtime dispatch ---

/// Build a checked program from `(module_name, source)` pairs, one source file
/// per pair, so module-aware dispatch and file ids use the production checker
/// artifact.
fn multi_module_program(modules: &[(&str, &str)]) -> CheckedRuntimeProgram {
    let files: Vec<(PathBuf, String)> = modules
        .iter()
        .map(|(name, source)| {
            let path = module_source_path(name);
            let text = format!("module {name}\n\n{source}");
            let parsed = parse_source(&text);
            assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
            (path, text)
        })
        .collect();
    checked_program_files(&files)
}

#[test]
fn bare_call_resolves_in_own_module_not_a_foreign_one() {
    // The proven P1 bug: two modules each declare `fn greet` returning a distinct
    // value. `zzz::run` calls a bare `greet()`. A bare name resolves in its own
    // module first, so it must run `zzz::greet` (2) — never the foreign
    // `aaa::greet` (1) the old first-match-across-all-modules resolver reached.
    let program = multi_module_program(&[
        ("aaa", "pub fn greet(): int\n    return 1\n"),
        (
            "zzz",
            "fn greet(): int\n    return 2\npub fn run(): int\n    return greet()\n",
        ),
    ]);
    assert_eq!(
        run(checked_entry!(&program, "zzz::run")),
        Ok(Some(Value::Int(2))),
        "a bare call must run the calling module's own function"
    );
}

#[test]
fn cross_module_call_to_a_private_fn_is_a_visibility_error() {
    checker_rejects_sources(
        &[
            "module aaa\nfn secret(): int\n    return 1\n",
            "module zzz\nfn run(): int\n    return aaa::secret()\n",
        ],
        "check.private_function",
    );
}

/// An index branch is not an assignable place: `out ^books.byShelf(s)` is
/// rejected, the same unsupported-path classification the lowering gives it.
#[test]
fn an_index_branch_is_not_an_assignable_place() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\n\
         fn give(out s: string)\n    s = \"x\"\n\n\
         fn run_it()\n    give(out ^books.byShelf(\"a\"))\n",
        "check.untyped_value",
    );
}

/// An enum-typed field round-trips through saved data and reads back to the same
/// member. The test observes the language behavior instead of the storage codec.
#[test]
fn an_enum_field_round_trips_through_saved_data() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n    banned\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         fn seed(id: int)\n    ^orders(id).state = Status::archived\n\n\
         fn matches_archived(id: int): bool\n    return (^orders(id).state ?? Status::active) == Status::archived\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(7)),
    )
    .expect("seed");
    let bytes = read_data_bytes(
        &program,
        &store,
        "orders",
        &[SavedKey::Int(7)],
        &data_path(&program, "orders", &["state"]),
    )
    .expect("enum leaf is written");
    assert!(
        decode_value(&bytes, ScalarType::Int).is_none(),
        "enum leaves must not use the old scalar ordinal codec"
    );
    let stored = decode_tree_enum_member(&bytes).expect("enum leaf uses tree enum member codec");
    assert_eq!(stored.enum_id(), &enum_catalog_id(&program, "Status"));
    assert_eq!(
        stored.member_id(),
        &enum_member_catalog_id(&program, "Status", "archived")
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::matches_archived", Value::Int(7))
        )
        .expect("compare")
        .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn an_entry_enum_argument_uses_checked_catalog_identity() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         fn give(): Status\n    return Status::active\n\n\
         fn save(state: Status)\n    ^orders(1).state = state\n\n\
         fn read(): bool\n    return (^orders(1).state ?? Status::archived) == Status::active\n",
    );
    let store = TreeStore::memory();
    let state = run_entry(&store, checked_entry!(&program, "test::give"))
        .expect("give")
        .value
        .expect("enum value");

    run_entry(&store, checked_entry!(&program, "test::save", state)).expect("save enum argument");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read")
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn an_enum_index_uses_catalog_member_keys() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n    banned\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n    index byState(state, id)\n\n\
         fn seed()\n    ^orders(1).state = Status::archived\n    ^orders(2).state = Status::active\n    ^orders(3).state = Status::archived\n\n\
         fn countArchived(): int\n    var count = 0\n    for id in ^orders.byState(Status::archived)\n        count = count + 1\n    return count\n\n\
         fn countActive(): int\n    var count = 0\n    for id in ^orders.byState(Status::active)\n        count = count + 1\n    return count\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed enum-index fixture");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countArchived"))
            .expect("count archived")
            .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countActive"))
            .expect("count active")
            .value,
        Some(Value::Int(1))
    );
}

#[test]
fn a_singleton_keyed_enum_leaf_can_be_matched_after_read() {
    let program = checked_program_typed(
        "enum Kind\n    number\n    plus\n\n\
         resource Session at ^session\n    required cursor: int\n    kinds(pos: int): Kind\n\n\
         pub fn readBack(): int\n    \
         ^session.cursor = 1\n    \
         ^session.kinds(1) = Kind::plus\n    \
         match ^session.kinds(1) ?? Kind::number\n        number\n            return 0\n        plus\n            return 1\n",
    );
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::readBack"))
            .expect("keyed enum leaf match runs")
            .value,
        Some(Value::Int(1))
    );
}

/// Nominal `==` on enum values: comparing the same member is true, comparing two
/// different members of the same enum is false.
#[test]
fn enum_equality_is_true_for_the_same_member_and_false_otherwise() {
    let program = checked_program(
        "enum Color\n    red\n    green\n    blue\n\n\
         fn same(): bool\n    return Color::green == Color::green\n\n\
         fn different(): bool\n    return Color::green == Color::blue\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::same")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::different")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// `match` dispatches to the arm for the scrutinee's member. Each arm returns a
/// distinct marker, so the returned value names the chosen arm.
#[test]
fn match_dispatches_to_the_arm_for_the_scrutinees_member() {
    let program = checked_program_typed(
        "enum Status\n    active\n    archived\n    banned\n\n\
         fn label(s: Status): int\n    \
         match s\n        active\n            return 10\n        \
         archived\n            return 20\n        banned\n            return 30\n\n\
         fn labelActive(): int\n    return label(Status::active)\n\n\
         fn labelArchived(): int\n    return label(Status::archived)\n\n\
         fn labelBanned(): int\n    return label(Status::banned)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelActive")).unwrap(),
        Some(Value::Int(10))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelArchived")).unwrap(),
        Some(Value::Int(20))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelBanned")).unwrap(),
        Some(Value::Int(30))
    );
}

/// `match` dispatches by the scrutinee's resolved enum, not by the arm member
/// set. Two enums share member names `x` and `y` but in opposite declaration
/// order, so dispatching by the arm set alone would pick `A` and invert the
/// result for `B`.
#[test]
fn match_dispatches_by_the_scrutinees_enum_not_the_arm_set() {
    let program = checked_program_typed(
        "enum A\n    x\n    y\n\n\
         enum B\n    y\n    x\n\n\
         fn label(s: B): int\n    \
         match s\n        x\n            return 1\n        y\n            return 2\n\n\
         fn labelX(): int\n    return label(B::x)\n\n\
         fn labelY(): int\n    return label(B::y)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelX")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelY")).unwrap(),
        Some(Value::Int(2))
    );
}

/// Nested enum member paths resolve to distinct concrete members.
#[test]
fn a_nested_member_path_resolves_to_the_right_member() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         fn bengalMatches(): bool\n    return Cat::bengal == Cat::tiger::bengal\n\n\
         fn bengalIsHousecat(): bool\n    return Cat::bengal == Cat::housecat\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalMatches")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalIsHousecat")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// `pet is Cat::tiger` is true for any value at or under `tiger` (a `bengal`),
/// false for one outside it (a `housecat`).
#[test]
fn is_tests_subtree_membership() {
    let program = checked_program_typed(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         fn bengalIsTiger(): bool\n    return Cat::bengal is Cat::tiger\n\n\
         fn housecatIsTiger(): bool\n    return Cat::housecat is Cat::tiger\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalIsTiger")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::housecatIsTiger")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// `is` against a concrete leaf is exact equality: a `bengal` value is
/// `Cat::bengal` but not `Cat::siberian`.
#[test]
fn is_against_a_leaf_is_exact() {
    let program = checked_program_typed(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         fn bengalIsBengal(): bool\n    return Cat::bengal is Cat::bengal\n\n\
         fn siberianIsBengal(): bool\n    return Cat::siberian is Cat::bengal\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalIsBengal")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::siberianIsBengal")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// A `match` runs the category arm for any descendant: a `bengal` or `siberian`
/// value both take the `tiger` arm, a `housecat` takes its own.
#[test]
fn match_runs_the_category_arm_for_any_descendant() {
    let program = checked_program_typed(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         fn label(pet: Cat): int\n    \
         match pet\n        tiger\n            return 1\n        \
         housecat\n            return 2\n\n\
         fn labelBengal(): int\n    return label(Cat::bengal)\n\n\
         fn labelSiberian(): int\n    return label(Cat::siberian)\n\n\
         fn labelHousecat(): int\n    return label(Cat::housecat)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelBengal")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelSiberian")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelHousecat")).unwrap(),
        Some(Value::Int(2))
    );
}

/// Two `paw`s under different parents are distinct members: the full member paths
/// `Cat::tiger::paw` and `Cat::lion::paw` are not aliases.
#[test]
fn a_duplicated_member_resolves_by_its_full_path_to_a_distinct_member() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n\n\
         fn sameTigerPaw(): bool\n    return Cat::tiger::paw == Cat::tiger::paw\n\n\
         fn differentPaws(): bool\n    return Cat::tiger::paw == Cat::lion::paw\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::sameTigerPaw")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::differentPaws")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// A `match` with qualified arms over duplicated leaves dispatches each value to
/// the correct arm by walking the arm path against the scrutinee enum — the two
/// `paw`s take different arms even though they share a name.
#[test]
fn match_with_qualified_arms_dispatches_each_duplicated_paw_to_its_own_arm() {
    let program = checked_program_typed(
        "enum Cat\n    category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n\n\
         fn label(pet: Cat): int\n    \
         match pet\n        \
         tiger::bengal\n            return 1\n        \
         tiger::paw\n            return 2\n        \
         lion::paw\n            return 3\n        \
         lion::mane\n            return 4\n\n\
         fn labelTigerBengal(): int\n    return label(Cat::tiger::bengal)\n\n\
         fn labelTigerPaw(): int\n    return label(Cat::tiger::paw)\n\n\
         fn labelLionPaw(): int\n    return label(Cat::lion::paw)\n\n\
         fn labelLionMane(): int\n    return label(Cat::lion::mane)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelTigerPaw")).unwrap(),
        Some(Value::Int(2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelLionPaw")).unwrap(),
        Some(Value::Int(3))
    );
    // The other leaves still dispatch to their arms.
    assert_eq!(
        run(checked_entry!(&program, "test::labelTigerBengal")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelLionMane")).unwrap(),
        Some(Value::Int(4))
    );
}

/// `is` with a full member path is exact over the right leaf, and a category right
/// operand is the subtree test — the same `is_descendant` walk, now reachable for
/// a duplicated leaf via its qualifying path.
#[test]
fn is_with_a_full_path_to_a_duplicated_leaf_is_exact() {
    let program = checked_program_typed(
        "enum Cat\n    category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n\n\
         fn tigerPawIsTigerPaw(): bool\n    return Cat::tiger::paw is Cat::tiger::paw\n\n\
         fn lionPawIsTigerPaw(): bool\n    return Cat::lion::paw is Cat::tiger::paw\n\n\
         fn tigerPawIsTiger(): bool\n    return Cat::tiger::paw is Cat::tiger\n\n\
         fn lionPawIsTiger(): bool\n    return Cat::lion::paw is Cat::tiger\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::tigerPawIsTigerPaw")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lionPawIsTigerPaw")).unwrap(),
        Some(Value::Bool(false))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::tigerPawIsTiger")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lionPawIsTiger")).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn uncaught_fault_in_entry_module_carries_its_file_id() {
    // A divide-by-zero raised in the entry module's own body stamps that
    // module's file id (index 0), so a renderer can name the file it lives in.
    let program = multi_module_program(&[(
        "a",
        "pub fn boom(): int\n    const boom = 1 / 0\n    return 0\n",
    )]);
    let error = run(checked_entry!(&program, "a::boom")).unwrap_err();
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(0)));
    assert!(
        program
            .file_path(FileId(0))
            .is_some_and(|path| path.ends_with("src/a.mw"))
    );
}

#[test]
fn uncaught_fault_in_cross_module_callee_carries_the_callee_file_id() {
    // The entry `a::run` calls `b::boom`, which divides by zero. The fault is
    // uncaught, so its origin must be `b`'s file (index 1), the frame that
    // raised it, not the entry's `a`.
    let program = multi_module_program(&[
        ("a", "pub fn run(): int\n    return b::boom()\n"),
        (
            "b",
            "pub fn boom(): int\n    const boom = 1 / 0\n    return 0\n",
        ),
    ]);
    let error = run(checked_entry!(&program, "a::run")).unwrap_err();
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(1)));
    assert!(
        program
            .file_path(FileId(1))
            .is_some_and(|path| path.ends_with("src/b.mw"))
    );
}

#[test]
fn uncaught_throw_from_cross_module_callee_carries_the_raising_frame_file_id() {
    // A language `throw` is catchable, so it re-spans at each call boundary on its
    // way out — the same path catchable faults take. Thrown in callee `b` and
    // never caught, its origin must stay `b`'s file (index 1), the frame that
    // first raised it, not the entry `a` it surfaces through.
    let program = multi_module_program(&[
        ("a", "pub fn run(): int\n    return b::boom()\n"),
        (
            "b",
            "pub fn boom(): int\n    throw Error(code: \"x.y\", message: \"bad\")\n",
        ),
    ]);
    let error = run(checked_entry!(&program, "a::run")).unwrap_err();
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    assert_eq!(error.origin, Some(FileId(1)));
}

#[test]
fn checked_program_entry_fault_carries_origin() {
    let error = eval_source(
        "fn f(): int\n    const boom = 1 / 0\n    return 0\n",
        "f",
        Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(0)));
}

#[test]
fn outer_frame_does_not_overwrite_inner_origin() {
    // `a::outer` calls `a::mid` calls `b::boom`. The uncaught fault crosses
    // three frames in two modules; the deepest (`b`, index 1) wins and the outer
    // `a` frames must not overwrite it.
    let program = multi_module_program(&[
        (
            "a",
            "pub fn outer(): int\n    return mid()\n\n\
             fn mid(): int\n    return b::boom()\n",
        ),
        (
            "b",
            "pub fn boom(): int\n    const boom = 1 / 0\n    return 0\n",
        ),
    ]);
    let error = run(checked_entry!(&program, "a::outer")).unwrap_err();
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(1)));
}

/// A program with an `Author` resource and a `Book` whose `authorId` is a typed
/// reference to `Author`. `seed` writes a reference; `read` reads it back.
fn typed_ref_program() -> CheckedRuntimeProgram {
    checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         resource Book at ^books(id: int)\n\
         \x20   authorId: Id(^authors)\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   ^books(1).authorId = author\n\
         \n\
         pub fn read(): bool\n\
         \x20   for author in keys(^authors)\n\
         \x20       const stored: Id(^authors) = ^books(1).authorId ?? author\n\
         \x20       return stored == author\n\
         \x20   return false\n",
    )
}

#[test]
fn an_identity_field_round_trips_through_saved_data() {
    // A `Book.authorId: Id(^authors)` field stores an identity and reads it back as
    // the same identity value produced by the author store.
    let program = typed_ref_program();
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn a_stored_identity_field_reads_back_the_identity_value() {
    // The stored leaf carries the referenced identity's key segments, not a plain
    // scalar field encoding.
    let program = checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         resource Book at ^books(id: int)\n\
         \x20   authorId: Id(^authors)\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   ^books(1).authorId = author\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    // The stored leaf is the canonical identity encoding — the same
    // order-preserving key bytes a unique index entry stores — not a scalar
    // `encode_value`.
    let stored = read_data_bytes(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["authorId"]),
    );
    assert_eq!(
        stored,
        Some(encode_identity_payload(&[SavedKey::Int(1)])),
        "identity field stored as its canonical key encoding"
    );
}

#[test]
fn an_identity_field_assigned_via_next_id_round_trips() {
    // Constructing the reference from `nextId(^authors)` (the first allocated id is
    // `1` on an empty root) round-trips through the saved identity field.
    let program = checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         resource Book at ^books(id: int)\n\
         \x20   authorId: Id(^authors)\n\
         \n\
         pub fn seed()\n\
         \x20   const a = nextId(^authors)\n\
         \x20   ^authors(a).name = \"Ada\"\n\
         \x20   ^books(1).authorId = a\n\
         \n\
         pub fn read(): bool\n\
         \x20   for author in keys(^authors)\n\
         \x20       const stored: Id(^authors) = ^books(1).authorId ?? author\n\
         \x20       return stored == author\n\
         \x20   return false\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn a_self_referencing_identity_field_round_trips() {
    // A field of the same resource (`managerId: Id(^people)` on `Person`) is a valid
    // self-reference that stores and reads back like any other typed reference.
    let program = checked_program(
        "resource Person at ^people(id: int)\n\
         \x20   managerId: Id(^people)\n\
         \n\
         pub fn seed(): bool\n\
         \x20   const person = nextId(^people)\n\
         \x20   ^people(person).managerId = person\n\
         \x20   const manager = nextId(^people)\n\
         \x20   ^people(person).managerId = manager\n\
         \x20   return read(manager)\n\
         \n\
         fn read(expected: Id(^people)): bool\n\
         \x20   const stored: Id(^people) = ^people(1).managerId ?? expected\n\
         \x20   return stored == expected\n",
    );
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::seed"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn equality_on_two_identities_of_the_same_store_evaluates() {
    // `==` on two identities of the same store is value equality of their keys:
    // equal keys are `true`, differing keys are `false`.
    let program = checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         pub fn same(): bool\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   return author == author\n\
         \n\
         pub fn different(): bool\n\
         \x20   const ada = nextId(^authors)\n\
         \x20   ^authors(ada).name = \"Ada\"\n\
         \x20   const grace = nextId(^authors)\n\
         \x20   ^authors(grace).name = \"Grace\"\n\
         \x20   return ada == grace\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::same")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::different")).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn single_key_store_identity_behaves_like_other_identity_origins() {
    let program = checked_program(
        "resource Doc at ^docs(id: int)\n\
         \x20   title: string\n\
         \n\
         pub fn idValue(): Id(^docs)\n\
         \x20   const id = nextId(^docs)\n\
         \x20   ^docs(id).title = \"T\"\n\
         \x20   for doc in keys(^docs)\n\
         \x20       return doc\n\
         \x20   return id\n\
         \n\
         pub fn mixedEq(): bool\n\
         \x20   const id = nextId(^docs)\n\
         \x20   ^docs(id).title = \"T\"\n\
         \x20   for doc in keys(^docs)\n\
         \x20       return id == doc\n\
         \x20   return false\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::idValue")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::mixedEq")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn unique_index_identity_compares_with_the_allocated_identity() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required isbn: string\n\
         \x20   index byIsbn(isbn) unique\n\
         \n\
         pub fn seed(): bool\n\
         \x20   var b: Book\n\
         \x20   b.title = \"T\"\n\
         \x20   b.isbn = \"I-1\"\n\
         \x20   const id = nextId(^books)\n\
         \x20   ^books(id) = b\n\
         \x20   const found: Id(^books) = ^books.byIsbn(\"I-1\") ?? id\n\
         \x20   return id == found\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seed")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn a_whole_resource_write_with_an_identity_field_round_trips() {
    // A whole-record write `^books(1) = b` carrying an identity-typed field stores
    // the reference, and a whole-record read reads it back.
    let program = checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   authorId: Id(^authors)\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   var b: Book\n\
         \x20   b.title = \"Mort\"\n\
         \x20   b.authorId = author\n\
         \x20   ^books(1) = b\n\
         \n\
         pub fn read(): bool\n\
         \x20   if exists(^books(1))\n\
         \x20       const b = ^books(1)\n\
         \x20       for author in keys(^authors)\n\
         \x20           return b.authorId == author\n\
         \x20   return false\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}
