//! Duplicate and colliding member/key names, and the unkeyed-group required-field
//! rule: a member name may not repeat, an identity key may not collide with a
//! member or layer name, key parameters must be distinct, and a required field is
//! allowed inside both unkeyed and keyed groups.

use crate::common;
use common::{assert_kind, codes};
use marrow_schema::{
    Node, NodeKind, ResourceSchema, SCHEMA_DUPLICATE_MEMBER, SCHEMA_KEY_MEMBER_COLLISION,
    SchemaDuplicateTarget, SchemaError, SchemaErrorKind, SchemaNameCollision,
    check_saved_member_rules, compile_resource, compile_store, compile_stored_resource,
};
use marrow_syntax::{Declaration, parse_source};

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

fn compile_source_errors(source: &str) -> Vec<SchemaError> {
    let (_, errors) = compile_source(source);
    errors
}

/// The top-level node named `name` (a keyed leaf or a group).
fn layer<'a>(schema: &'a ResourceSchema, name: &str) -> &'a Node {
    schema
        .members
        .iter()
        .find(|node| node.name == name)
        .unwrap_or_else(|| panic!("layer `{name}` not found"))
}

/// The top-level nodes classified by the production schema API as plain fields.
fn top_level_fields(schema: &ResourceSchema) -> impl Iterator<Item = &Node> {
    schema.members.iter().filter(|node| node.is_plain_field())
}

#[test]
fn duplicate_member_name_is_an_error() {
    let source = "\
resource Book
    required title: string
    title: string
store ^books(id: int): Book
";
    let (schema, errors) = compile_source(source);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, SCHEMA_DUPLICATE_MEMBER);
    // Best-effort: every parsed member is kept in source order; the collision
    // is reported, not silently dropped. The duplicate's span points at the
    // second `title`.
    assert_eq!(top_level_fields(&schema).count(), 2);
    assert_eq!(errors[0].span.line, 3);
}

#[test]
fn identity_key_name_colliding_with_field_is_an_error() {
    let source = "\
resource Book
    required id: int
    required title: string
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_KEY_MEMBER_COLLISION]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::KeyMemberCollision {
            collision: SchemaNameCollision::IdentityKeyWithMember {
                key: "id".to_string(),
            },
        },
    );
}

#[test]
fn identity_key_name_colliding_with_layer_is_an_error() {
    let source = "\
resource Book
    notes(noteId: string)
        text: string
store ^books(notes: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_KEY_MEMBER_COLLISION]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::KeyMemberCollision {
            collision: SchemaNameCollision::IdentityKeyWithMember {
                key: "notes".to_string(),
            },
        },
    );
}

#[test]
fn duplicate_identity_key_name_is_an_error() {
    // Identity keys must have distinct names; two `studentId` keys are
    // unaddressable.
    let source = "\
resource Enrollment
    status: string
store ^enrollments(studentId: string, studentId: string): Enrollment
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_DUPLICATE_MEMBER]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::DuplicateMember {
            target: SchemaDuplicateTarget::KeyParam,
            name: "studentId".to_string(),
        },
    );
}

#[test]
fn duplicate_keyed_leaf_key_param_name_is_an_error() {
    // A keyed layer's key parameters must have distinct names.
    let source = "\
resource Book
    tags(pos: int, pos: int): string
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_DUPLICATE_MEMBER]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::DuplicateMember {
            target: SchemaDuplicateTarget::KeyParam,
            name: "pos".to_string(),
        },
    );
}

#[test]
fn duplicate_group_key_param_name_is_an_error() {
    let source = "\
resource Book
    revisions(rev: int, rev: int)
        body: string
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_DUPLICATE_MEMBER]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::DuplicateMember {
            target: SchemaDuplicateTarget::KeyParam,
            name: "rev".to_string(),
        },
    );
}

#[test]
fn required_field_inside_an_unkeyed_group_is_allowed() {
    // Unkeyed groups are structural. A required field inside one is required for
    // the containing resource, and remains marked on the nested schema node.
    let source = "\
resource Patient
    name
        required first: string
        last: string
store ^patients(id: string): Patient
";
    let schema = compile_ok(source);
    let name = layer(&schema, "name");
    let first = name
        .members
        .iter()
        .find(|node| node.name == "first")
        .expect("nested first field");
    assert!(matches!(first.kind, NodeKind::Slot { required: true, .. }));
}

#[test]
fn required_field_inside_a_keyed_group_is_allowed() {
    // The rejection is specific to UNKEYED groups; a keyed group (a layer the
    // planner does maintain) may hold required fields, as in the Book
    // `versions(version) { required title }` shape.
    let source = "\
resource Book
    versions(version: int)
        required title: string
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert!(
        errors.is_empty(),
        "keyed-group required fields are fine: {errors:?}"
    );
}
