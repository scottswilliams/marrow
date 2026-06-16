//! Shared helpers for the resource-compilation integration tests.
use marrow_schema::{
    Node, ResourceSchema, SchemaError, SchemaErrorKind, check_saved_member_rules, compile_resource,
    compile_store,
};
use marrow_syntax::{Declaration, ResourceDecl, StoreDecl, parse_source};

pub fn codes(errors: &[SchemaError]) -> Vec<&'static str> {
    errors.iter().map(|error| error.code).collect()
}

pub fn assert_kind(error: &SchemaError, kind: SchemaErrorKind) {
    assert_eq!(error.kind, kind);
}

/// Parse `source` and return its single resource declaration.
pub fn resource(source: &str) -> ResourceDecl {
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
pub fn compile_ok(source: &str) -> ResourceSchema {
    let (schema, errors) = compile_source(source);
    assert!(errors.is_empty(), "unexpected schema errors: {errors:?}");
    schema
}

pub fn compile_source(source: &str) -> (ResourceSchema, Vec<SchemaError>) {
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

pub fn compile_source_errors(source: &str) -> Vec<SchemaError> {
    let (_, errors) = compile_source(source);
    errors
}

pub fn resource_and_store(source: &str) -> (ResourceDecl, StoreDecl) {
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

/// The top-level node named `name` (a keyed leaf or a group).
pub fn layer<'a>(schema: &'a ResourceSchema, name: &str) -> &'a Node {
    schema
        .members
        .iter()
        .find(|node| node.name == name)
        .unwrap_or_else(|| panic!("layer `{name}` not found"))
}

/// The top-level nodes classified by the production schema API as plain fields.
pub fn top_level_fields(schema: &ResourceSchema) -> impl Iterator<Item = &Node> {
    schema.members.iter().filter(|node| node.is_plain_field())
}
