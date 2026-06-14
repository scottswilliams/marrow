//! Saved-key type rules: a key (identity, keyed-layer parameter, or index
//! argument) must be an orderable scalar. `decimal`, bare named types,
//! resources, enums, sequences, and identity values are rejected; an orderable
//! scalar key compiles clean.

use crate::common;
use common::{assert_kind, codes};
use marrow_schema::{
    ResourceSchema, SCHEMA_NONSCALAR_KEY, SCHEMA_UNORDERABLE_KEY, ScalarType, SchemaError,
    SchemaErrorKind, SchemaKeyTarget, Type, check_saved_member_rules, compile_resource,
    compile_store, compile_stored_resource,
};
use marrow_syntax::{Declaration, ResourceDecl, StoreDecl, parse_source};

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

fn compile_store_errors(source: &str) -> Vec<SchemaError> {
    let (resource, store) = resource_and_store(source);
    let (schema, resource_errors) = compile_stored_resource(&resource);
    assert!(
        resource_errors.is_empty(),
        "unexpected resource errors: {resource_errors:?}"
    );
    let (_, store_errors) = compile_store(&store, &schema);
    store_errors
}

fn resource_and_store(source: &str) -> (ResourceDecl, StoreDecl) {
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
    (
        resource.expect("resource declaration"),
        store.expect("store declaration"),
    )
}

fn key_param(name: &str) -> SchemaKeyTarget {
    SchemaKeyTarget::KeyParam {
        name: name.to_string(),
    }
}

fn identity_key(name: &str) -> SchemaKeyTarget {
    SchemaKeyTarget::IdentityKey {
        name: name.to_string(),
    }
}

fn index_arg(index: &str, arg: &str) -> SchemaKeyTarget {
    SchemaKeyTarget::IndexArg {
        index: index.to_string(),
        arg: arg.to_string(),
    }
}

#[test]
fn index_over_a_decimal_field_is_an_error() {
    // `decimal` has no order-preserving key encoding, so the write planner could
    // never maintain an index entry for it. Reject it at compile time rather than
    // silently committing the data with no index.
    let source = "\
resource Book
    price: decimal
store ^books(id: int): Book
    index byPrice(price, id)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNORDERABLE_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::UnorderableKey {
            target: index_arg("byPrice", "price"),
            ty: Type::Scalar(ScalarType::Decimal),
        },
    );
}

#[test]
fn keyed_leaf_with_a_decimal_key_param_is_an_error() {
    // A keyed-layer key must be an ordered key type; `decimal` is not.
    let source = "\
resource Book
    samples(ts: decimal): int
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNORDERABLE_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::UnorderableKey {
            target: key_param("ts"),
            ty: Type::Scalar(ScalarType::Decimal),
        },
    );
}

#[test]
fn identity_key_typed_decimal_is_an_error() {
    let source = "\
resource Reading
    required value: int
store ^readings(ts: decimal): Reading
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNORDERABLE_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::UnorderableKey {
            target: identity_key("ts"),
            ty: Type::Scalar(ScalarType::Decimal),
        },
    );
}

#[test]
fn identity_key_typed_as_a_bare_name_is_an_error() {
    // A key must be an orderable scalar. A bare name names no scalar, so a raw
    // string or int written into that key position would be silently accepted and
    // corrupt the keyspace. The rule is structural — it needs no knowledge of what
    // the name refers to — so an enum, a resource, or a typo is caught the same way.
    let source = "\
resource Order
    required note: string
store ^orders(state: Status): Order
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NonScalarKey {
            target: identity_key("state"),
            ty: Type::Named("Status".to_string()),
        },
    );
}

#[test]
fn keyed_layer_key_param_typed_as_a_bare_name_is_an_error() {
    let source = "\
resource Order
    byState(state: Status): string
store ^orders(id: int): Order
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NonScalarKey {
            target: key_param("state"),
            ty: Type::Named("Status".to_string()),
        },
    );
}

#[test]
fn an_undeclared_or_typo_named_identity_key_is_an_error() {
    // The allowlist is structural, so a name that resolves to nothing is rejected
    // exactly like a declared one. A typo'd key would otherwise accept any value,
    // letting an int and a string coexist in one identity keyspace.
    let source = "\
resource Order
    required note: string
store ^orders(state: Stutus): Order
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NonScalarKey {
            target: identity_key("state"),
            ty: Type::Named("Stutus".to_string()),
        },
    );
}

#[test]
fn a_resource_typed_identity_key_is_an_error() {
    // A bare name that happens to be a declared resource is still not an orderable
    // scalar, so it cannot be a key. `Person` here names a local resource.
    let source = "\
resource Person
    required name: string
resource Order
    required note: string
store ^orders(owner: Person): Order
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NonScalarKey {
            target: identity_key("owner"),
            ty: Type::Named("Person".to_string()),
        },
    );
}

#[test]
fn a_sequence_typed_key_is_an_error() {
    // A sequence is not a scalar at all, so it cannot project to an orderable key.
    let source = "\
resource Order
    required note: string
store ^orders(tags: sequence[string]): Order
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NonScalarKey {
            target: identity_key("tags"),
            ty: Type::Sequence(Box::new(Type::Scalar(ScalarType::Str))),
        },
    );
}

#[test]
fn an_identity_field_index_argument_is_clean() {
    let source = "\
resource Book
    authorId: Id(^authors)
store ^books(id: int): Book
    index byAuthor(authorId, id)
";
    let errors = compile_store_errors(source);
    assert!(errors.is_empty(), "identity index argument: {errors:?}");
}

#[test]
fn an_orderable_scalar_identity_key_is_clean() {
    // The allowlist does not over-reject: an orderable scalar key at the identity,
    // a layer key param, and an index argument all compile clean.
    let source = "\
resource Order
    byTag(tag: string): string
    rank: int
store ^orders(id: int): Order
    index byRank(rank, id)
";
    let errors = compile_store_errors(source);
    assert!(errors.is_empty(), "orderable scalar keys: {errors:?}");
}

#[test]
fn an_identity_typed_key_is_an_error() {
    // A saved key must be an orderable scalar. An identity value has no supported
    // saved-key encoding yet, so reject it statically instead of deferring it.
    let source = "\
resource Edge
    required note: string
store ^edges(from: Id(^nodes)): Edge
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NonScalarKey {
            target: identity_key("from"),
            ty: Type::Identity("nodes".to_string()),
        },
    );
}

#[test]
fn a_keyed_layer_key_param_typed_as_an_identity_is_an_error() {
    let source = "\
resource Edge
    byNode(from: Id(^nodes)): string
store ^edges(id: int): Edge
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NonScalarKey {
            target: key_param("from"),
            ty: Type::Identity("nodes".to_string()),
        },
    );
}
