use std::fs;
use std::path::Path;

#[test]
fn production_runtime_does_not_execute_syntax_bodies() {
    assert_runtime_free_of(
        "production runtime still executes syntax bodies",
        &[
            "marrow_syntax::{Argument",
            "marrow_syntax::{BinaryOp",
            "marrow_syntax::{Block",
            "marrow_syntax::{Expression",
            "marrow_syntax::{ForBinding",
            "marrow_syntax::{MatchArm",
            "marrow_syntax::{Statement",
            "&function.body",
            "Block",
            "Expression::",
            "Statement::",
        ],
    );
}

#[test]
fn checked_runtime_functions_use_checked_runtime_bodies() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let check_program = crate_parent.join("marrow-check/src/program.rs");
    let text = fs::read_to_string(&check_program).expect("checked program source");

    for forbidden in [
        "from_syntax_for_runtime",
        "&function.body",
        "function_sources",
        "CheckedFunctionSource",
        "body: Block",
    ] {
        assert!(
            !source_contains(&text, forbidden),
            "checked runtime function builder still uses syntax body bridge `{forbidden}` in {}",
            check_program.display()
        );
    }
    assert!(
        text.contains("runtime_body: Option<CheckedBody>"),
        "checked functions do not carry a checked runtime body fact"
    );
    assert!(
        !text.contains("pub runtime_body: Option<CheckedBody>")
            && !text.contains("pub body: Option<CheckedBody>"),
        "checked runtime body fields are still public syntax-construction bridges"
    );
    assert!(
        !text.contains("pub body: Block"),
        "CheckedFunction still carries a source syntax body"
    );
}

#[test]
fn checked_runtime_artifact_does_not_carry_raw_schema_copies() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let check_program = crate_parent.join("marrow-check/src/program.rs");
    let text = fs::read_to_string(&check_program).expect("checked program source");
    let runtime_module = text
        .split("pub struct CheckedRuntimeModule")
        .nth(1)
        .and_then(|tail| tail.split("impl CheckedRuntimeModule").next())
        .expect("checked runtime module struct");

    for forbidden in [
        "pub imports",
        "pub resources",
        "pub stores",
        "pub enums",
        "pub enum_public",
        "ResourceSchema",
        "StoreSchema",
        "EnumSchema",
    ] {
        assert!(
            !source_contains(runtime_module, forbidden),
            "checked runtime module still carries raw schema/import owner `{forbidden}` in {}",
            check_program.display()
        );
    }
}

#[test]
fn checked_runtime_program_exposes_facts_read_only() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let check_program = crate_parent.join("marrow-check/src/program.rs");
    let text = fs::read_to_string(&check_program).expect("checked program source");
    let runtime_program = text
        .split("pub struct CheckedRuntimeProgram")
        .nth(1)
        .and_then(|tail| tail.split("impl CheckedRuntimeProgram").next())
        .expect("checked runtime program struct");

    assert!(
        runtime_program.contains("facts: CheckedFacts"),
        "checked runtime program does not carry checked facts"
    );
    for forbidden in ["pub facts", "pub catalog", "catalog: ProgramCatalog"] {
        assert!(
            !source_contains(runtime_program, forbidden),
            "checked runtime program still exposes mutable/dead field `{forbidden}` in {}",
            check_program.display()
        );
    }
}

#[test]
fn checked_enum_refs_carry_fact_identity_not_schema_ordinals() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let executable = crate_parent.join("marrow-check/src/executable.rs");
    let text = fs::read_to_string(&executable).expect("checked executable source");
    let enum_ref = text
        .split("pub struct CheckedEnumRef")
        .nth(1)
        .and_then(|tail| tail.split("pub struct CheckedEnumMemberRef").next())
        .expect("checked enum ref struct");

    assert!(
        enum_ref.contains("pub enum_id: EnumId"),
        "checked enum refs do not carry catalog-backed fact identity"
    );
    for forbidden in ["pub module: u32", "pub enum_: u32"] {
        assert!(
            !source_contains(enum_ref, forbidden),
            "checked enum refs still carry schema ordinal `{forbidden}` in {}",
            executable.display()
        );
    }
}

#[test]
fn checked_executable_syntax_lowering_is_not_public() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let executable = crate_parent.join("marrow-check/src/executable.rs");
    let program = crate_parent.join("marrow-check/src/program.rs");
    let mut violations = Vec::new();

    collect_forbidden(
        &executable,
        &[
            "pub fn from_syntax(block",
            "pub fn from_syntax(expr",
            "pub statements: Vec<CheckedStmt>",
        ],
        &mut violations,
    );
    collect_forbidden(
        &program,
        &[
            "pub runtime_body: Option<CheckedBody>",
            "pub body: Option<CheckedBody>",
        ],
        &mut violations,
    );

    assert_no_violations(
        "checked executable syntax lowering is still public",
        violations,
    );
}

#[test]
fn checked_runtime_bodies_are_not_rebuilt_from_parsed_sources_after_checking() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let program = crate_parent.join("marrow-check/src/program.rs");
    let analysis = crate_parent.join("marrow-check/src/analysis.rs");
    let check_lib = crate_parent.join("marrow-check/src/lib.rs");
    let mut violations = Vec::new();

    collect_forbidden(
        &program,
        &[
            "rebuild_runtime_bodies_from_sources",
            "source.body.clone()",
            "resolve_block_matches",
        ],
        &mut violations,
    );
    for path in [&analysis, &check_lib] {
        collect_forbidden(
            path,
            &["rebuild_runtime_bodies_from_sources"],
            &mut violations,
        );
    }

    assert_no_violations(
        "checked runtime bodies are still rebuilt from parsed source after checking",
        violations,
    );
}

#[test]
fn checked_saved_places_do_not_embed_schema_copies() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let expr = crate_parent.join("marrow-check/src/executable/expr.rs");
    let text = fs::read_to_string(&expr).expect("checked executable expr source");
    let mut violations = Vec::new();

    for (name, end) in [
        ("CheckedSavedPlace", "pub struct CheckedSavedIndex"),
        ("CheckedSavedIndex", "pub struct CheckedSavedLayer"),
        ("CheckedSavedLayer", "pub struct CheckedSavedMember"),
        ("CheckedSavedMember", "pub enum CheckedSavedMemberKind"),
        ("CheckedSavedMemberKind", "impl CheckedSavedMember"),
        ("CheckedSavedTerminal", "pub enum CheckedExpr"),
    ] {
        let section = text
            .split(&format!("pub struct {name}"))
            .nth(1)
            .or_else(|| text.split(&format!("pub enum {name}")).nth(1))
            .and_then(|tail| tail.split(end).next())
            .unwrap_or_else(|| panic!("{name} section is present"));
        for forbidden in [
            "StoreSchema",
            "ResourceSchema",
            "IndexSchema",
            "KeyDef",
            "pub store:",
            "pub resource:",
            "schema:",
        ] {
            if source_contains(section, forbidden) {
                violations.push(format!("{name} embeds schema copy `{forbidden}`"));
            }
        }
        // `Type` is a family root: any `*Type` schema type embedded directly in a
        // saved-place descriptor (ScalarType, MarrowType, ...) is a schema copy, so it
        // is matched as a substring rather than as a whole identifier.
        if section.contains("Type") {
            violations.push(format!("{name} embeds schema copy `Type`"));
        }
    }

    assert!(
        violations.is_empty(),
        "checked saved-place descriptors still carry schema copies:\n{}",
        violations.join("\n")
    );
}

#[test]
fn public_runtime_enum_values_do_not_expose_catalog_backing_fields() {
    let value_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/value.rs");
    let text = fs::read_to_string(&value_src).expect("runtime value source");
    let enum_value = text
        .split("pub struct EnumValue")
        .nth(1)
        .and_then(|tail| tail.split("enum LeafValue").next())
        .expect("runtime enum value struct");

    for forbidden in [
        "pub enum_id",
        "pub member_id",
        "pub enum_catalog_id",
        "pub member_catalog_id",
    ] {
        assert!(
            !source_contains(enum_value, forbidden),
            "runtime enum values expose forgeable catalog backing field `{forbidden}` in {}",
            value_src.display()
        );
    }
}

#[test]
fn checker_alias_helpers_are_not_public_runtime_resolution_bridges() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let check_lib = crate_parent.join("marrow-check/src/lib.rs");

    assert_file_free_of(
        "checker alias helpers are still public runtime resolution bridges",
        &check_lib,
        &[
            "pub fn expand_module_alias",
            "pub fn build_alias_map",
            "pub fn expand_alias",
        ],
    );
}

#[test]
fn production_runtime_uses_syntax_free_program_artifact() {
    let mut violations = Vec::new();

    for path in runtime_rs_files() {
        // The evaluator spine must not see syntax-carrying artifacts; evolution apply
        // re-runs read-only discharge over the checked program and is exempt.
        if path
            .components()
            .any(|component| component.as_os_str() == "evolution")
        {
            continue;
        }
        collect_forbidden(
            &path,
            &[
                "CheckedProgram",
                "CheckedModule",
                "CheckedFunction",
                "CheckedConst",
            ],
            &mut violations,
        );
    }

    assert_no_violations(
        "production runtime still receives syntax-carrying checked artifacts",
        violations,
    );
}

/// Assert that no runtime source file contains any forbidden needle. `invariant`
/// names the architectural guarantee for the failure message; each offending file is
/// reported with the needle it still carries.
fn assert_runtime_free_of(invariant: &str, forbidden: &[&str]) {
    let mut violations = Vec::new();
    for path in runtime_rs_files() {
        collect_forbidden(&path, forbidden, &mut violations);
    }
    assert_no_violations(invariant, violations);
}

/// Assert that a single source file contains none of the forbidden needles.
fn assert_file_free_of(invariant: &str, path: &Path, forbidden: &[&str]) {
    let mut violations = Vec::new();
    collect_forbidden(path, forbidden, &mut violations);
    assert_no_violations(invariant, violations);
}

fn collect_forbidden(path: &Path, forbidden: &[&str], violations: &mut Vec<String>) {
    let text = fs::read_to_string(path).expect("source file");
    for needle in forbidden {
        if source_contains(&text, needle) {
            violations.push(format!("{} contains {needle}", path.display()));
        }
    }
}

fn assert_no_violations(invariant: &str, violations: Vec<String>) {
    assert!(
        violations.is_empty(),
        "{invariant}:\n{}",
        violations.join("\n")
    );
}

/// Match a forbidden needle against source. A needle that is a bare Rust identifier
/// is matched on identifier boundaries so it cannot hit a substring of a longer name
/// or text inside a comment word; a multi-token signature (anything carrying `:`, `(`,
/// `<`, `.`, or whitespace) is matched literally, since it cannot collide inside an
/// identifier.
fn source_contains(text: &str, needle: &str) -> bool {
    if needle.chars().all(is_rust_ident) {
        contains_rust_identifier(text, needle)
    } else {
        text.contains(needle)
    }
}

fn contains_rust_identifier(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(index, _)| {
        let before = text[..index].chars().next_back();
        let after = text[index + needle.len()..].chars().next();
        rust_boundary(before) && rust_boundary(after)
    })
}

fn rust_boundary(ch: Option<char>) -> bool {
    match ch {
        Some(ch) => !is_rust_ident(ch),
        None => true,
    }
}

fn is_rust_ident(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn runtime_rs_files() -> Vec<std::path::PathBuf> {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rust_files(&runtime_src, &mut files);
    files
}

fn collect_rust_files(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).expect("runtime source directory") {
        let entry = entry.expect("runtime source entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn production_runtime_uses_typed_tree_cell_store_boundary() {
    assert_runtime_free_of(
        "production runtime still depends on raw saved-path/backend store APIs",
        &[
            "marrow_store::backend",
            "marrow_store::path",
            "crate::path::PathSegment",
            "crate::path::encode_path",
        ],
    );
}

#[test]
fn production_runtime_enum_values_use_catalog_member_identity() {
    let mut violations = Vec::new();

    for path in runtime_rs_files() {
        let text = fs::read_to_string(&path).expect("runtime source");
        // `ordinal` is a family root: any suffix- or prefix-named reintroduction
        // (member_ordinals, subtree_ordinals, arm_ordinal) reopens ordinal-based enum
        // identity, so it must match as a substring rather than a whole identifier.
        if text.contains("ordinal") {
            violations.push(format!("{} contains ordinal", path.display()));
        }
        if source_contains(&text, "SavedValue::Int(i64::from") {
            violations.push(format!(
                "{} contains SavedValue::Int(i64::from",
                path.display()
            ));
        }
    }

    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let executable = crate_parent.join("marrow-check/src/executable.rs");
    let text = fs::read_to_string(&executable).expect("checked executable source");
    if text.contains("ordinal") {
        violations.push(format!("{} contains ordinal", executable.display()));
    }

    assert!(
        violations.is_empty(),
        "runtime enum execution still uses ordinal source identity:\n{}",
        violations.join("\n")
    );
}

#[test]
fn production_runtime_does_not_resolve_calls_from_source_strings() {
    assert_runtime_free_of(
        "production runtime still resolves calls from source strings",
        &[
            "split(\"::\")",
            "split_once(\"::\")",
            "rsplit_once(\"::\")",
            "join(\"::\")",
            "expand_alias(",
            "expand_module_alias",
            "resolve_runtime_enum",
            "MemberPathResolution",
            "walk_member_path",
            "denotes_saved_path",
            "denotes_group_base",
            "ExecExpr::SavedRoot",
            "resolve_program_function(",
            "resolve(",
            "is_std_module(",
        ],
    );
}

#[test]
fn production_runtime_has_no_reserved_ephemeral_root_execution() {
    assert_runtime_free_of(
        "production runtime contains reserved `~` ephemeral-root execution",
        &["cache ~", "ensure ~", "Id(~", "~"],
    );
}

#[test]
fn production_runtime_does_not_classify_saved_paths_locally() {
    assert_runtime_free_of(
        "production runtime still classifies saved paths locally",
        &["is_saved_path(", "classify_saved_path", "SavedPathClass"],
    );
}

#[test]
fn production_runtime_uses_checked_index_place_facts() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    for name in [
        "durable_read.rs",
        "path.rs",
        "saved_iter.rs",
        "stdlib/count.rs",
        "stdlib/index_lookup.rs",
    ] {
        collect_forbidden(
            &runtime_src.join(name),
            &[
                "fn index_branch_schema(",
                "IndexSchema",
                "find_store_resource",
                "Expression::SavedRoot",
            ],
            &mut violations,
        );
    }

    assert_no_violations(
        "production runtime still classifies index branches from syntax/schema",
        violations,
    );
}

#[test]
fn durable_reads_use_stdlib_unique_index_lookup_owner() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    assert_file_free_of(
        "durable index value reads still duplicate unique-index lookup ownership",
        &runtime_src.join("durable_read.rs"),
        &[
            "fn checked_unique_index_lookup(",
            "fn index_lookup_keys(",
            "struct IndexLookup",
            "decode_identity_payload_arity",
        ],
    );
}

#[test]
fn production_runtime_uses_checked_root_identity_facts_for_count() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    assert_file_free_of(
        "production runtime count still rediscovers root schema facts",
        &runtime_src.join("stdlib/count.rs"),
        &["find_store_resource", "let Expression::SavedRoot"],
    );
}

#[test]
fn production_runtime_uses_checked_place_facts_for_node_segments() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    assert_file_free_of(
        "production runtime node segment lowering still rediscovers saved-root facts",
        &runtime_src.join("path.rs"),
        &["root_identity_arity", "Expression::SavedRoot"],
    );
}

#[test]
fn production_runtime_uses_checked_root_identity_facts_for_iteration() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    for name in ["collection.rs", "read.rs"] {
        collect_forbidden(
            &runtime_src.join(name),
            &["root_identity_arity"],
            &mut violations,
        );
    }

    assert_no_violations(
        "production runtime iteration still rediscovers root identity arity",
        violations,
    );
}

#[test]
fn production_runtime_uses_checked_root_identity_facts_for_deletes() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    assert_file_free_of(
        "production runtime delete still rediscovers root identity arity",
        &runtime_src.join("write_dispatch.rs"),
        &["root_identity_arity"],
    );
}

#[test]
fn production_runtime_field_reads_use_checked_leaf_facts() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    assert_file_free_of(
        "production runtime field reads still rediscovers leaf facts",
        &runtime_src.join("path.rs"),
        &[
            "resource_field_leaf_kind",
            "resource_nested_member_leaf_kind",
            "fn checked_leaf_for_field",
        ],
    );
}

#[test]
fn production_runtime_does_not_classify_schema_leaves_locally() {
    assert_runtime_free_of(
        "production runtime still classifies schema leaves locally",
        &[
            "enum StoreLeafKind",
            "fn store_leaf_kind",
            "fn resource_field_leaf_kind",
            "fn resource_layer_leaf_kind_chain",
            "fn resource_nested_member_leaf_kind",
        ],
    );
}

#[test]
fn production_runtime_does_not_query_schema_facts_for_durable_places() {
    assert_runtime_free_of(
        "production runtime still queries schema facts instead of checked durable places",
        &[
            "find_store_resource",
            "find_resource",
            "marrow_schema::Node",
            "NodeKind",
            "ResourceSchema",
            "member.node",
            "group.node",
            "layer.node",
            "root_identity_arity",
            "resource_field_leaf_kind",
            "resource_nested_member_leaf_kind",
            "resource_layer_leaf_kind_chain",
            "layer_key_params",
            "store_leaf_kind",
        ],
    );
}

#[test]
fn production_runtime_resource_constructors_use_checked_contract_facts() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    assert_file_free_of(
        "production runtime still validates constructors by classifying schemas",
        &runtime_src.join("call.rs"),
        &[
            "fn check_resource_constructor_value",
            "fn runtime_type_from_schema",
            "fn runtime_resource_type_from_name",
            "fn value_matches_type",
            "fn identity_value_matches",
            "identity_key_defs",
            "identity_root",
            "runtime_enum_in",
            "ResourceSchema",
            "NodeKind",
        ],
    );
}

#[test]
fn runtime_identity_values_have_no_public_rootless_constructor() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let value = runtime_src.join("value.rs");
    let text = fs::read_to_string(&value).expect("value source");

    assert!(
        !text.contains("pub fn untrusted") && !text.contains("root: Option<String>"),
        "runtime identity values still expose rootless construction or optional root provenance"
    );
}

#[test]
fn canonical_language_docs_do_not_advertise_unsupported_edit_blocks() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let docs = crate_parent
        .parent()
        .expect("repo root")
        .join("docs/language");
    let mut violations = Vec::new();

    for entry in fs::read_dir(&docs).expect("language docs directory") {
        let path = entry.expect("language doc").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        collect_forbidden(
            &path,
            &[
                "edit_stmt",
                "\"edit\" assignable",
                "edit place",
                "`edit` block",
                "edit ^",
            ],
            &mut violations,
        );
    }

    assert_no_violations(
        "canonical docs still advertise unsupported edit syntax",
        violations,
    );
}

#[test]
fn checker_durable_path_exports_no_runtime_schema_bridge_helpers() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let check_src = crate_parent.join("marrow-check/src");
    let mut violations = Vec::new();

    let forbidden = [
        "pub fn find_store_resource",
        "pub fn find_store(",
        "pub fn find_resource(",
        "pub fn root_identity_arity",
        "pub fn root_identity_keys",
        "pub fn layer_key_params",
        "pub fn identity_root",
        "pub fn identity_key_defs",
        "pub fn resource_layer_chain_exists",
        "pub fn store_leaf_kind",
        "pub fn resource_field_leaf_kind",
        "pub fn resource_layer_leaf_kind_chain",
        "pub fn resource_nested_member_leaf_kind",
        "pub fn resource_nested_member_exists",
        "find_store_resource,",
        "find_store,",
        "find_resource,",
        "root_identity_arity,",
        "root_identity_keys,",
        "layer_key_params,",
        "identity_root,",
        "identity_key_defs,",
        "resource_layer_chain_exists,",
        "store_leaf_kind,",
        "resource_field_leaf_kind,",
        "resource_layer_leaf_kind_chain,",
        "resource_nested_member_leaf_kind,",
        "resource_nested_member_exists,",
    ];
    for name in ["durable_path.rs", "lib.rs"] {
        collect_forbidden(&check_src.join(name), &forbidden, &mut violations);
    }

    assert_no_violations(
        "checker still exposes runtime schema/path bridge helpers",
        violations,
    );
}

#[test]
fn production_runtime_has_no_local_schema_query_module() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    if runtime_src.join("schema_query.rs").exists() {
        violations.push("runtime has src/schema_query.rs".to_string());
    }

    for path in runtime_rs_files() {
        collect_forbidden(
            &path,
            &["mod schema_query", "crate::schema_query", "schema_query::"],
            &mut violations,
        );
    }

    assert_no_violations(
        "production runtime still has a local schema query module",
        violations,
    );
}

#[test]
fn production_runtime_splits_host_effect_handling() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    if !runtime_src.join("host_effects.rs").exists() {
        violations.push("runtime is missing src/host_effects.rs".to_string());
    }

    collect_forbidden(
        &runtime_src.join("stdlib.rs"),
        &[
            "RUN_CAPABILITY",
            "fn eval_clock_capability",
            "fn eval_env",
            "fn eval_log",
            "fn eval_io",
            "io_error(",
        ],
        &mut violations,
    );

    assert_no_violations(
        "production runtime still keeps host effects in stdlib dispatch",
        violations,
    );
}

#[test]
fn production_runtime_splits_index_maintenance() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    if !runtime_src.join("index_maintenance.rs").exists() {
        violations.push("runtime is missing src/index_maintenance.rs".to_string());
    }

    collect_forbidden(
        &runtime_src.join("write.rs"),
        &[
            "fn index_keys",
            "fn stored_arg_key",
            "fn stored_index_keys",
            "fn field_write_index_keys",
            "fn index_path",
            "fn index_entry_value",
            "fn encode_identity",
            "fn decode_identity",
            "fn check_unique_conflict",
        ],
        &mut violations,
    );

    assert_no_violations(
        "production runtime still keeps index maintenance in write planning",
        violations,
    );
}

#[test]
fn production_runtime_splits_checked_statement_execution() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    if !runtime_src.join("statement.rs").exists() {
        violations.push("runtime is missing src/statement.rs".to_string());
    }

    collect_forbidden(
        &runtime_src.join("exec.rs"),
        &["fn eval_statement(", "match statement"],
        &mut violations,
    );

    assert_no_violations(
        "production runtime still keeps checked statement execution in exec.rs",
        violations,
    );
}

#[test]
fn production_runtime_splits_durable_place_reads() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    if !runtime_src.join("durable_read.rs").exists() {
        violations.push("runtime is missing src/durable_read.rs".to_string());
    }

    collect_forbidden(
        &runtime_src.join("read.rs"),
        &[
            "fn eval_saved_field(",
            "fn read_saved_field(",
            "fn eval_optional_field(",
            "fn eval_index_lookup(",
            "fn eval_saved_layer_read(",
            "fn read_layer_entry(",
            "fn read_layer_entry_at(",
            "fn read_group_entry_chain(",
            "fn eval_resource_read(",
            "fn read_resource(",
            "fn materialize_resource_members(",
        ],
        &mut violations,
    );

    assert_no_violations(
        "production runtime still keeps durable-place reads in read.rs",
        violations,
    );
}

#[test]
fn production_runtime_splits_write_plan_representation() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    if !runtime_src.join("write_plan.rs").exists() {
        violations.push("runtime is missing src/write_plan.rs".to_string());
    }

    collect_forbidden(
        &runtime_src.join("write.rs"),
        &[
            "enum PlanStep",
            "enum WriteOp",
            "struct WritePlan",
            "fn apply_steps",
        ],
        &mut violations,
    );

    assert_no_violations(
        "production runtime still keeps write-plan representation in write.rs",
        violations,
    );
}

#[test]
fn production_runtime_splits_pure_std_dispatch() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    if !runtime_src.join("std_pure.rs").exists() {
        violations.push("runtime is missing src/std_pure.rs".to_string());
    }

    collect_forbidden(
        &runtime_src.join("stdlib.rs"),
        &["pub(crate) fn eval_std(", "match (module, op)"],
        &mut violations,
    );

    let std_pure = runtime_src.join("std_pure.rs");
    if let Ok(text) = fs::read_to_string(&std_pure)
        && text.contains("match (module, op)")
    {
        violations.push(format!(
            "{} contains match (module, op)",
            std_pure.display()
        ));
    }

    assert_no_violations(
        "production runtime still keeps pure std helpers in one broad dispatcher",
        violations,
    );
}

#[test]
fn production_runtime_splits_group_entry_write_dispatch() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    if !runtime_src.join("group_write.rs").exists() {
        violations.push("runtime is missing src/group_write.rs".to_string());
    }

    collect_forbidden(
        &runtime_src.join("write_dispatch.rs"),
        &[
            "pub(crate) fn eval_group_entry_write(",
            "resource_layer_leaf_kind_chain(",
            "plan_layer_group_write(",
            "plan_nested_layer_identity_leaf_write(",
            "plan_nested_layer_leaf_write(",
        ],
        &mut violations,
    );

    assert_no_violations(
        "production runtime still keeps group-entry write routing in write_dispatch.rs",
        violations,
    );
}

#[test]
fn production_runtime_splits_loop_execution() {
    let runtime_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    if !runtime_src.join("loop_exec.rs").exists() {
        violations.push("runtime is missing src/loop_exec.rs".to_string());
    }

    collect_forbidden(
        &runtime_src.join("exec.rs"),
        &[
            "pub(crate) fn eval_for(",
            "pub(crate) fn eval_while(",
            "pub(crate) fn eval_collection(",
            "enum RangeIter",
            "fn range_iter(",
        ],
        &mut violations,
    );

    assert_no_violations(
        "production runtime still keeps loop and collection iteration in exec.rs",
        violations,
    );
}

#[test]
fn checked_runtime_entry_lookup_is_precomputed() {
    let crate_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent");
    let check_program = crate_parent.join("marrow-check/src/program.rs");
    let executable = crate_parent.join("marrow-check/src/executable.rs");
    let text = fs::read_to_string(&check_program).expect("checked program source");
    let executable_text = fs::read_to_string(&executable).expect("checked executable source");

    assert!(
        !text.contains("entry.rsplit_once(\"::\")"),
        "checked runtime entry lookup still parses source entry strings in {}",
        check_program.display()
    );
    assert!(
        text.contains("entry_functions: HashMap<String, CheckedEntryFunction>"),
        "checked runtime program does not carry precomputed entry resolutions"
    );
    for forbidden in [
        "pub modules: Vec<CheckedRuntimeModule>",
        "pub entry_functions: HashMap<String, CheckedFunctionRef>",
        "pub entry_functions: HashMap<String, CheckedEntryFunction>",
        "pub functions: Vec<CheckedRuntimeFunction>",
        "pub entry_params: Vec<CheckedRuntimeParam>",
    ] {
        assert!(
            !source_contains(&text, forbidden),
            "checked runtime artifact still exposes mutable or fallback entry shape `{forbidden}` in {}",
            check_program.display()
        );
    }
    for forbidden in ["PrivateFunction", "UnsupportedCallee", "Unresolved { name"] {
        assert!(
            !source_contains(&executable_text, forbidden),
            "checked executable call target still carries fallback variant `{forbidden}` in {}",
            executable.display()
        );
    }

    let run_cli = crate_parent.join("marrow/src/cmd_run.rs");
    let text = fs::read_to_string(&run_cli).expect("run CLI source");
    assert!(
        !text.contains("fn resolve_entry("),
        "run CLI still pre-resolves entries from syntax-carrying checked program in {}",
        run_cli.display()
    );
}

#[test]
fn public_runtime_entrypoints_take_checked_entry_calls() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let entry = src.join("entry.rs");
    let call = src.join("call.rs");
    let entry_text = fs::read_to_string(&entry).expect("runtime entry source");
    let call_text = fs::read_to_string(&call).expect("runtime call source");

    assert!(
        entry_text.contains("pub struct CheckedEntryCall"),
        "runtime entry calls are not represented by a checked boundary object"
    );
    assert!(
        !call_text.contains("pub struct CheckedEntryCall"),
        "checked entry boundary still lives in the generic call dispatcher"
    );
    for forbidden in [
        "entry: &str,\n    args: &[Value]",
        "pub fn args(&self) -> &[Value]",
        "entry: String",
        "entry.to_string()",
        "entry_target(program, &call.entry)",
    ] {
        assert!(
            !source_contains(&entry_text, forbidden),
            "public runtime entrypoint still exposes raw argument shape `{forbidden}` in {}",
            entry.display()
        );
    }
    assert!(
        entry_text.contains("target: CheckedFunctionRef"),
        "checked entry calls should carry the resolved checked function target"
    );
    assert!(
        entry_text.contains("program: &'p CheckedRuntimeProgram"),
        "checked entry calls should be tied to the checked runtime program they were built from"
    );
}

#[test]
fn runtime_eval_helpers_follow_checked_entry_call_shape() {
    let forbidden = [
        "fn run(\n    _program: &CheckedRuntimeProgram",
        "fn run_full(\n    _program: &CheckedRuntimeProgram",
        "fn run_entry(\n    _program: &CheckedRuntimeProgram",
        "fn run_entry_with_host(\n    _program: &CheckedRuntimeProgram",
        "fn run_entry_with_debugger(\n    _program: &CheckedRuntimeProgram",
        "run(&program,",
        "run_full(&program,",
        "run_entry(&program,",
        "run_entry_with_host(&program,",
        "run_entry_with_debugger(&program,",
    ];

    let mut violations = Vec::new();
    for path in runtime_eval_sources() {
        collect_forbidden(&path, &forbidden, &mut violations);
    }

    assert_no_violations(
        "runtime eval tests still preserve obsolete checked entry helper shape",
        violations,
    );
}

/// The runtime-eval suite files: the shared harness and every `eval_*.rs` split.
fn runtime_eval_sources() -> Vec<std::path::PathBuf> {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut sources = vec![tests.join("support/mod.rs")];
    for entry in fs::read_dir(&tests).expect("runtime tests directory") {
        let path = entry.expect("runtime test entry").path();
        let is_eval = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("eval_") && name.ends_with(".rs"));
        if is_eval {
            sources.push(path);
        }
    }
    sources
}

#[test]
fn runtime_tests_do_not_hand_build_checked_function_syntax_bodies() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut violations = Vec::new();

    for entry in fs::read_dir(&tests).expect("runtime tests directory") {
        let entry = entry.expect("runtime test entry");
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some("architecture.rs") {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        collect_forbidden(
            &path,
            &[
                "CheckedBody::from_syntax",
                "body: function.body",
                "CheckedFunction {",
                "entry: &str,\n    args: &[Value]",
                "args: &[Value]",
                "run_entry(program, store, entry, args)",
            ],
            &mut violations,
        );
    }

    assert_no_violations(
        "runtime tests still hand-build checked functions from syntax bodies",
        violations,
    );
}
