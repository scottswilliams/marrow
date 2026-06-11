//! Member resolution and named saved fields. `field_type`/`leaf_type` resolve
//! plain fields and keyed leaves at every depth and refuse to cross the
//! field/leaf/group boundary or an unknown chain. `check_saved_named_member_fields`
//! requires a bare-named saved field or map value to be a declared enum, and
//! stays silent on unsupported or qualified names it cannot judge locally.

use marrow_schema::{
    ResourceSchema, SCHEMA_NON_ENUM_NAMED_FIELD, ScalarType, SchemaError, SchemaErrorKind, Type,
    check_saved_member_rules, check_saved_named_member_fields, compile_resource, compile_store,
    compile_stored_resource,
};
use marrow_syntax::{Declaration, ResourceDecl, parse_source};

mod common;
use common::{assert_kind, codes};

/// Parse `source` and return its single resource declaration.
fn resource(source: &str) -> ResourceDecl {
    let parsed = parse_source(source);
    assert!(
        !parsed.has_errors(),
        "source should parse cleanly: {:?}",
        parsed.diagnostics
    );
    parsed
        .file
        .declarations
        .into_iter()
        .find_map(|declaration| match declaration {
            Declaration::Resource(resource) => Some(resource),
            _ => None,
        })
        .expect("a resource declaration")
}

/// Compile `source`'s resource, asserting it produced no schema errors.
fn compile_ok(source: &str) -> ResourceSchema {
    let (schema, errors) = compile_source(source);
    assert!(errors.is_empty(), "unexpected schema errors: {errors:?}");
    schema
}

fn compile_source(source: &str) -> (ResourceSchema, Vec<SchemaError>) {
    let parsed = parse_source(source);
    assert!(
        !parsed.has_errors(),
        "source should parse cleanly: {:?}",
        parsed.diagnostics
    );
    let mut resource = None;
    let mut store = None;
    for declaration in parsed.file.declarations {
        match declaration {
            Declaration::Resource(decl) => resource = Some(decl),
            Declaration::Store(decl) => store = Some(decl),
            _ => {}
        }
    }
    let resource = resource.expect("resource declaration");
    if let Some(store) = store {
        let (schema, mut errors) = compile_stored_resource(&resource);
        let (_, store_errors) = compile_store(&store, &schema);
        errors.extend(store_errors);
        errors.extend(check_saved_member_rules(&resource.members));
        (schema, errors)
    } else {
        compile_resource(&resource)
    }
}

/// Compile every stored resource in `source` through the saved-resource schema rule
/// sequence the checker drives — the stored-resource shape, the store, the saved member
/// rules, and the named-field enum rule resolved against the module's declared enums —
/// and return the schema errors. The same-file enum set and stored-resource set are
/// derived from the parsed declarations exactly as the checker derives them, so the
/// named-field rule sees the enums a real module would.
fn compile_saved_resource_errors(source: &str) -> Vec<SchemaError> {
    let parsed = parse_source(source);
    assert!(
        !parsed.has_errors(),
        "source should parse cleanly: {:?}",
        parsed.diagnostics
    );
    let module_enums: Vec<String> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Enum(decl) => Some(decl.name.clone()),
            _ => None,
        })
        .collect();
    let stores: Vec<_> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Store(store) => Some(store.clone()),
            _ => None,
        })
        .collect();

    let mut errors = Vec::new();
    for declaration in &parsed.file.declarations {
        let Declaration::Resource(resource) = declaration else {
            continue;
        };
        let Some(store) = stores.iter().find(|store| store.resource == resource.name) else {
            continue;
        };
        let (schema, resource_errors) = compile_stored_resource(resource);
        errors.extend(resource_errors);
        let (_, store_errors) = compile_store(store, &schema);
        errors.extend(store_errors);
        errors.extend(check_saved_member_rules(&resource.members));
        errors.extend(check_saved_named_member_fields(
            &resource.members,
            &module_enums,
        ));
    }
    errors
}

/// A resource nesting a keyed-leaf layer and a field inside a group, exercising
/// the field and leaf resolvers on chains deeper than a single top-level name.
const NESTED: &str = "\
resource Catalog
    required title: string
    tags(pos: int): string

    versions(version: int)
        required note: string
        lines(pos: int): string

        comments(commentId: string)
            required body: string
store ^catalog(id: int): Catalog
    ";

#[test]
fn field_type_resolves_top_level_and_nested_fields() {
    let schema = compile_ok(NESTED);
    // A single-name chain reads a top-level field.
    assert_eq!(
        schema.field_type(&["title"]),
        Some(&Type::Scalar(ScalarType::Str))
    );
    // A field inside a group layer.
    assert_eq!(
        schema.field_type(&["versions", "note"]),
        Some(&Type::Scalar(ScalarType::Str))
    );
    // A field two group layers deep.
    assert_eq!(
        schema.field_type(&["versions", "comments", "body"]),
        Some(&Type::Scalar(ScalarType::Str))
    );
}

#[test]
fn field_type_does_not_resolve_a_keyed_leaf_layer() {
    // A keyed-leaf layer is read as a leaf, not a field: `field_type` must not
    // resolve a layer name, top-level or nested, so a bare layer read stays
    // untyped.
    let schema = compile_ok(NESTED);
    assert_eq!(schema.field_type(&["tags"]), None);
    assert_eq!(schema.field_type(&["versions", "lines"]), None);
}

#[test]
fn leaf_type_resolves_top_level_and_nested_keyed_leaves() {
    let schema = compile_ok(NESTED);
    // A top-level keyed-leaf layer of the resource.
    assert_eq!(
        schema.leaf_type(&["tags"]),
        Some(&Type::Scalar(ScalarType::Str))
    );
    // A keyed-leaf layer nested inside a group layer.
    assert_eq!(
        schema.leaf_type(&["versions", "lines"]),
        Some(&Type::Scalar(ScalarType::Str))
    );
}

#[test]
fn leaf_type_does_not_resolve_a_group_layer_or_field() {
    let schema = compile_ok(NESTED);
    // A group layer carries members, not a leaf value.
    assert_eq!(schema.leaf_type(&["versions"]), None);
    // A field is not a keyed-leaf layer.
    assert_eq!(schema.leaf_type(&["versions", "note"]), None);
}

#[test]
fn member_resolvers_reject_unknown_and_empty_chains() {
    let schema = compile_ok(NESTED);
    assert_eq!(schema.field_type(&[]), None, "an empty chain names nothing");
    assert_eq!(schema.leaf_type(&[]), None);
    assert_eq!(schema.field_type(&["missing"]), None);
    // A name under an unknown layer.
    assert_eq!(schema.field_type(&["missing", "note"]), None);
    // A real layer but an undeclared member.
    assert_eq!(schema.field_type(&["versions", "missing"]), None);
}

#[test]
fn a_bare_named_saved_field_must_be_a_declared_enum() {
    // A saved field typed by a declared enum compiles clean; the same field typed by
    // an undeclared name is the typed non-enum-named-field error.
    assert!(
        compile_saved_resource_errors(
            "\
enum Status
    active
    archived
resource Order
    required state: Status
store ^orders(id: int): Order
",
        )
        .is_empty(),
        "an enum-typed saved field is allowed"
    );
    let errors = compile_saved_resource_errors(
        "\
resource Order
    required state: Status
store ^orders(id: int): Order
",
    );
    assert_eq!(codes(&errors), [SCHEMA_NON_ENUM_NAMED_FIELD]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NonEnumNamedField {
            field: "state".to_string(),
            ty: "Status".to_string(),
        },
    );
}

#[test]
fn a_bare_named_map_value_must_be_a_declared_enum() {
    assert!(
        compile_saved_resource_errors(
            "\
enum Status
    active
    archived
resource Order
    scores: map[string, Status]
store ^orders(id: int): Order
",
        )
        .is_empty(),
        "an enum-typed map value is allowed"
    );
    let errors = compile_saved_resource_errors(
        "\
resource Order
    scores: map[string, Status]
store ^orders(id: int): Order
",
    );
    assert_eq!(codes(&errors), [SCHEMA_NON_ENUM_NAMED_FIELD]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NonEnumNamedField {
            field: "scores".to_string(),
            ty: "Status".to_string(),
        },
    );
}

#[test]
fn unsupported_map_value_is_not_checked_as_bare_named_saved_field() {
    let decl = resource(
        "\
resource Order
    scores: map[string, map[string, int]]
store ^orders(id: int): Order
",
    );
    let errors = check_saved_named_member_fields(&decl.members, &[]);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn unsupported_map_key_does_not_check_map_value_as_bare_named_saved_field() {
    let decl = resource(
        "\
resource Order
    scores: map[map[string, int], Missing]
store ^orders(id: int): Order
",
    );
    let errors = check_saved_named_member_fields(&decl.members, &[]);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn required_map_member_does_not_check_value_as_bare_named_saved_field() {
    let decl = resource(
        "\
resource Order
    required scores: map[string, Missing]
store ^orders(id: int): Order
",
    );
    let errors = check_saved_named_member_fields(&decl.members, &[]);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_qualified_named_saved_field_is_not_a_schema_local_error() {
    let short_alias = resource(
        "\
module a
use pkg::kinds
resource Saved
    required k: kinds::Color
store ^saved(id: int): Saved
",
    );
    let short_errors = check_saved_named_member_fields(&short_alias.members, &[]);
    assert!(
        short_errors.is_empty(),
        "the schema-only gate cannot reject a qualified enum name: {short_errors:#?}"
    );

    let full_path = resource(
        "\
module a
resource Saved
    required k: pkg::kinds::Color
store ^saved(id: int): Saved
",
    );
    let full_errors = check_saved_named_member_fields(&full_path.members, &[]);
    assert!(
        full_errors.is_empty(),
        "the schema-only gate cannot reject a fully qualified enum name: {full_errors:#?}"
    );
}
