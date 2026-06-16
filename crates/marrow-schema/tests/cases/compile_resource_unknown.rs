//! Rejection of `unknown` anywhere inside a saved type: fields, identity keys,
//! keyed-leaf values, nested fields, sequence elements, and key parameters at
//! every depth. Local (store-free) resources may still use `unknown`.

use crate::common;
use common::{assert_kind, codes};
use marrow_schema::{
    ResourceSchema, SCHEMA_UNKNOWN_IN_SAVED, SchemaError, SchemaErrorKind,
    SchemaSavedUnknownTarget, check_saved_member_rules, compile_resource, compile_store,
};
use marrow_syntax::{Declaration, parse_source};

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
        let (schema, mut errors) = compile_resource(&resource);
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

fn unknown(target: SchemaSavedUnknownTarget, name: &str) -> SchemaErrorKind {
    SchemaErrorKind::UnknownInSaved {
        target,
        name: name.to_string(),
    }
}

#[test]
fn saved_field_typed_unknown_is_an_error() {
    let source = "\
resource Book
    required title: string
    note: unknown
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert_kind(&errors[0], unknown(SchemaSavedUnknownTarget::Field, "note"));
}

#[test]
fn saved_identity_key_typed_unknown_is_an_error() {
    let source = "\
resource Book
    required title: string
store ^books(id: unknown): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert_kind(
        &errors[0],
        unknown(SchemaSavedUnknownTarget::IdentityKey, "id"),
    );
}

#[test]
fn saved_keyed_leaf_typed_unknown_is_an_error() {
    let source = "\
resource Book
    tags(pos: int): unknown
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert_kind(
        &errors[0],
        unknown(SchemaSavedUnknownTarget::KeyedLeaf, "tags"),
    );
}

#[test]
fn saved_nested_field_typed_unknown_is_an_error() {
    let source = "\
resource Book
    notes(noteId: string)
        body: unknown
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert_kind(&errors[0], unknown(SchemaSavedUnknownTarget::Field, "body"));
}

#[test]
fn saved_field_typed_sequence_of_unknown_is_an_error() {
    // `unknown` is rejected anywhere inside a saved type, including as the
    // element of a `sequence[...]`.
    let source = "\
resource Book
    tags: sequence[unknown]
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert_kind(&errors[0], unknown(SchemaSavedUnknownTarget::Field, "tags"));
}

#[test]
fn saved_field_typed_sequence_of_concrete_type_is_allowed() {
    // A sequence of a concrete type is an ordinary saved field; the check does
    // not over-trigger on the `sequence[...]` wrapper.
    let source = "\
resource Book
    tags: sequence[string]
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert!(
        errors.is_empty(),
        "sequence of a concrete type is fine: {errors:?}"
    );
}

#[test]
fn keyed_leaf_key_param_typed_unknown_is_an_error() {
    // `unknown` is rejected in saved keys, including a keyed layer's own key
    // parameters, not only identity keys and value types.
    let source = "\
resource Book
    tags(pos: unknown): string
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert_kind(&errors[0], unknown(SchemaSavedUnknownTarget::Key, "pos"));
}

#[test]
fn nested_group_key_param_typed_unknown_is_an_error() {
    // The check recurses into nested groups' key parameters.
    let source = "\
resource Book
    notes(noteId: string)
        revisions(rev: unknown)
            body: string
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert_kind(&errors[0], unknown(SchemaSavedUnknownTarget::Key, "rev"));
}

#[test]
fn local_keyed_leaf_key_param_typed_unknown_is_allowed() {
    // The saved-key rule applies only to managed saved resources; a local
    // resource (no store) may use `unknown` in a key parameter.
    let source = "\
resource Draft
    tags(pos: unknown): string
";
    let errors = compile_source_errors(source);
    assert!(
        errors.is_empty(),
        "local resources may use `unknown` in keys: {errors:?}"
    );
}

#[test]
fn local_field_typed_unknown_is_allowed() {
    let source = "\
resource Draft
    title: string
    note: unknown
";
    let errors = compile_source_errors(source);
    assert!(
        errors.is_empty(),
        "local resources may use `unknown`: {errors:?}"
    );
}
