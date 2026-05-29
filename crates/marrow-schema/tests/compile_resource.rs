//! Schema compilation tests.
//!
//! The primary case is the canonical `Book` resource, which exercises the saved
//! root, required and sparse fields, a keyed-leaf
//! sequence, group and history layers, and an index. The remaining cases pin
//! the structural errors the compiler reports.

use marrow_schema::{
    LayerMember, LayerSchema, ResourceSchema, SCHEMA_DUPLICATE_MEMBER, SCHEMA_DUPLICATE_STABLE_ID,
    SCHEMA_INDEX_IN_GROUP, SCHEMA_INDEX_MISSING_IDENTITY_KEYS, SCHEMA_INDEX_REQUIRES_KEYED_ROOT,
    SCHEMA_KEY_MEMBER_COLLISION, SCHEMA_NESTED_INDEX_ARG, SCHEMA_REQUIRED_IN_UNKEYED_GROUP,
    SCHEMA_UNKNOWN_IN_SAVED, SCHEMA_UNKNOWN_INDEX_ARG, SCHEMA_UNORDERABLE_KEY, ScalarType, Type,
    compile_resource,
};
use marrow_syntax::{Declaration, ResourceDecl, parse_source};

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
    let (schema, errors) = compile_resource(&resource(source));
    assert!(errors.is_empty(), "unexpected schema errors: {errors:?}");
    schema
}

fn layer<'a>(schema: &'a ResourceSchema, name: &str) -> &'a LayerSchema {
    schema
        .layers
        .iter()
        .find(|layer| layer.name == name)
        .unwrap_or_else(|| panic!("layer `{name}` not found"))
}

/// The canonical `Book` resource.
const BOOK: &str = "\
resource Book at ^books(id: int)
    required title: string
    required author: string
    required shelf: string
    required currentVersion: int
    loanedTo: string
    tags(pos: int): string

    notes(noteId: string)
        text: string

    versions(version: int)
        required title: string
        required shelf: string
        required changedAt: instant

    index byShelf(shelf, id)
";

#[test]
fn book_saved_root_has_one_identity_key() {
    let schema = compile_ok(BOOK);
    let root = schema.saved_root.expect("Book has a saved root");
    assert_eq!(root.root, "books");
    assert_eq!(root.identity_keys.len(), 1);
    assert_eq!(root.identity_keys[0].name, "id");
    assert_eq!(root.identity_keys[0].ty, Type::Scalar(ScalarType::Int));
}

#[test]
fn book_top_level_fields() {
    let schema = compile_ok(BOOK);

    let names: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        ["title", "author", "shelf", "currentVersion", "loanedTo"]
    );

    let required: Vec<&str> = schema
        .fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(required, ["title", "author", "shelf", "currentVersion"]);

    let loaned_to = schema
        .fields
        .iter()
        .find(|f| f.name == "loanedTo")
        .expect("loanedTo field");
    assert!(!loaned_to.required, "loanedTo is sparse");
    assert_eq!(loaned_to.ty, Type::Scalar(ScalarType::Str));

    // `tags` is a keyed leaf, not a top-level field.
    assert!(!schema.fields.iter().any(|f| f.name == "tags"));
}

#[test]
fn book_tags_is_a_keyed_leaf() {
    let schema = compile_ok(BOOK);
    let tags = layer(&schema, "tags");
    assert_eq!(tags.key_params.len(), 1);
    assert_eq!(tags.key_params[0].name, "pos");
    assert_eq!(tags.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert_eq!(tags.leaf_type, Some(Type::Scalar(ScalarType::Str)));
    assert!(tags.members.is_empty(), "a keyed leaf has no members");
}

#[test]
fn book_notes_is_a_group() {
    let schema = compile_ok(BOOK);
    let notes = layer(&schema, "notes");
    assert_eq!(notes.key_params.len(), 1);
    assert_eq!(notes.key_params[0].name, "noteId");
    assert_eq!(notes.key_params[0].ty, Type::Scalar(ScalarType::Str));
    assert!(notes.leaf_type.is_none(), "a group has no leaf type");

    assert_eq!(notes.members.len(), 1);
    let LayerMember::Field(text) = &notes.members[0] else {
        panic!("notes.text should be a field");
    };
    assert_eq!(text.name, "text");
    assert_eq!(text.ty, Type::Scalar(ScalarType::Str));
    assert!(!text.required);
}

#[test]
fn book_versions_is_a_history_group() {
    let schema = compile_ok(BOOK);
    let versions = layer(&schema, "versions");
    assert_eq!(versions.key_params.len(), 1);
    assert_eq!(versions.key_params[0].name, "version");
    assert_eq!(versions.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert!(versions.leaf_type.is_none());

    let fields: Vec<(&str, bool, String)> = versions
        .members
        .iter()
        .map(|member| match member {
            LayerMember::Field(field) => {
                (field.name.as_str(), field.required, field.ty.to_string())
            }
            LayerMember::Layer(layer) => panic!("unexpected nested layer `{}`", layer.name),
        })
        .collect();
    assert_eq!(
        fields,
        [
            ("title", true, "string".to_string()),
            ("shelf", true, "string".to_string()),
            ("changedAt", true, "instant".to_string()),
        ]
    );
}

#[test]
fn book_byshelf_index() {
    let schema = compile_ok(BOOK);
    assert_eq!(schema.indexes.len(), 1);
    let by_shelf = &schema.indexes[0];
    assert_eq!(by_shelf.name, "byShelf");
    assert_eq!(by_shelf.args, ["shelf", "id"]);
    assert!(!by_shelf.unique);
}

#[test]
fn index_inside_group_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    notes(noteId: string)
        text: string
        index byText(text)
";
    let (schema, errors) = compile_resource(&resource(source));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, SCHEMA_INDEX_IN_GROUP);
    // The misplaced index is dropped, not promoted to a resource index.
    assert!(schema.indexes.is_empty());
}

#[test]
fn duplicate_member_name_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    required title: string
    title: string
";
    let (schema, errors) = compile_resource(&resource(source));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, SCHEMA_DUPLICATE_MEMBER);
    // Best-effort: every parsed member is kept in source order; the collision
    // is reported, not silently dropped. The duplicate's span points at the
    // second `title`.
    assert_eq!(schema.fields.len(), 2);
    assert_eq!(errors[0].span.line, 3);
}

/// Only this code, to keep `unknown`/collision assertions specific.
fn codes(errors: &[marrow_schema::SchemaError]) -> Vec<&'static str> {
    errors.iter().map(|error| error.code).collect()
}

#[test]
fn saved_field_typed_unknown_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    required title: string
    note: unknown
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("note"));
}

#[test]
fn saved_identity_key_typed_unknown_is_an_error() {
    let source = "\
resource Book at ^books(id: unknown)
    required title: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("id"));
}

#[test]
fn saved_keyed_leaf_typed_unknown_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    tags(pos: int): unknown
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("tags"));
}

#[test]
fn saved_nested_field_typed_unknown_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    notes(noteId: string)
        body: unknown
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("body"));
}

#[test]
fn saved_field_typed_sequence_of_unknown_is_an_error() {
    // `unknown` is rejected anywhere inside a saved type, including as the
    // element of a `sequence[...]`.
    let source = "\
resource Book at ^books(id: int)
    tags: sequence[unknown]
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("tags"));
}

#[test]
fn saved_field_typed_sequence_of_concrete_type_is_allowed() {
    // A sequence of a concrete type is an ordinary saved field; the check does
    // not over-trigger on the `sequence[...]` wrapper.
    let source = "\
resource Book at ^books(id: int)
    tags: sequence[string]
";
    let (_, errors) = compile_resource(&resource(source));
    assert!(
        errors.is_empty(),
        "sequence of a concrete type is fine: {errors:?}"
    );
}

#[test]
fn sequence_member_desugars_to_a_pos_int_keyed_leaf() {
    // `tags: sequence[string]` is sugar for `tags(pos: int): string`, so it
    // compiles to the same keyed leaf the canonical spelling produces.
    let source = "\
resource Book at ^books(id: int)
    tags: sequence[string]
";
    let schema = compile_ok(source);
    // It lives in `layers`, not as a scalar field.
    assert!(!schema.fields.iter().any(|f| f.name == "tags"));
    let tags = layer(&schema, "tags");
    assert_eq!(tags.key_params.len(), 1);
    assert_eq!(tags.key_params[0].name, "pos");
    assert_eq!(tags.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert_eq!(tags.leaf_type, Some(Type::Scalar(ScalarType::Str)));
    assert!(tags.members.is_empty(), "a keyed leaf has no members");
}

#[test]
fn sequence_member_matches_the_canonical_keyed_leaf() {
    // The desugared layer is identical to the canonical `tags(pos: int): string`.
    let sugar = layer(
        &compile_ok("resource Book at ^books(id: int)\n    tags: sequence[string]\n"),
        "tags",
    )
    .clone();
    let canonical = layer(
        &compile_ok("resource Book at ^books(id: int)\n    tags(pos: int): string\n"),
        "tags",
    )
    .clone();
    assert_eq!(sugar, canonical);
}

#[test]
fn nested_sequence_member_desugars_inside_a_group() {
    // A sequence nested inside a group desugars the same way.
    let source = "\
resource Book at ^books(id: int)
    versions(version: int)
        notes: sequence[string]
";
    let schema = compile_ok(source);
    let versions = layer(&schema, "versions");
    let LayerMember::Layer(notes) = &versions.members[0] else {
        panic!("notes should desugar to a nested keyed-leaf layer");
    };
    assert_eq!(notes.name, "notes");
    assert_eq!(notes.key_params.len(), 1);
    assert_eq!(notes.key_params[0].name, "pos");
    assert_eq!(notes.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert_eq!(notes.leaf_type, Some(Type::Scalar(ScalarType::Str)));
}

#[test]
fn keyed_leaf_key_param_typed_unknown_is_an_error() {
    // `unknown` is rejected in saved keys, including a keyed layer's own key
    // parameters, not only identity keys and value types.
    let source = "\
resource Book at ^books(id: int)
    tags(pos: unknown): string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("pos"));
}

#[test]
fn nested_group_key_param_typed_unknown_is_an_error() {
    // The check recurses into nested groups' key parameters.
    let source = "\
resource Book at ^books(id: int)
    notes(noteId: string)
        revisions(rev: unknown)
            body: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("rev"));
}

#[test]
fn local_keyed_leaf_key_param_typed_unknown_is_allowed() {
    // The saved-key rule applies only to managed saved resources; a local
    // resource (no store) may use `unknown` in a key parameter.
    let source = "\
resource Draft
    tags(pos: unknown): string
";
    let (_, errors) = compile_resource(&resource(source));
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
    let (_, errors) = compile_resource(&resource(source));
    assert!(
        errors.is_empty(),
        "local resources may use `unknown`: {errors:?}"
    );
}

#[test]
fn identity_key_name_colliding_with_field_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    required id: int
    required title: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_KEY_MEMBER_COLLISION]);
    assert!(errors[0].message.contains("id"));
}

#[test]
fn identity_key_name_colliding_with_layer_is_an_error() {
    let source = "\
resource Book at ^books(notes: int)
    notes(noteId: string)
        text: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_KEY_MEMBER_COLLISION]);
    assert!(errors[0].message.contains("notes"));
}

#[test]
fn identity_key_name_colliding_with_index_is_an_error() {
    // Identity keys, fields, layers, and index names share the resource
    // namespace, so a key may not reuse an index name.
    let source = "\
resource Book at ^books(id: int)
    required title: string
    index id(title, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_KEY_MEMBER_COLLISION]);
    assert!(errors[0].message.contains("id"));
}

#[test]
fn index_arg_naming_no_member_is_an_error() {
    // Index arguments must resolve to an identity key, field, or nested field.
    // `shelf` names nothing here.
    let source = "\
resource Book at ^books(id: int)
    required title: string
    index byShelf(shelf, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_INDEX_ARG]);
    assert!(errors[0].message.contains("shelf"));
}

#[test]
fn index_arg_naming_field_and_key_is_allowed() {
    // A top-level field and an identity key both resolve as index arguments.
    let source = "\
resource Book at ^books(id: int)
    required title: string
    index byTitle(title, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert!(
        errors.is_empty(),
        "field and identity-key args resolve: {errors:?}"
    );
}

#[test]
fn index_arg_naming_keyed_leaf_is_an_error() {
    // Index arguments do not walk keyed child layers; `tags` is a keyed leaf.
    let source = "\
resource Book at ^books(id: int)
    tags(pos: int): string
    index byTag(tags, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_INDEX_ARG]);
    assert!(errors[0].message.contains("tags"));
}

#[test]
fn index_over_a_decimal_field_is_an_error() {
    // `decimal` has no order-preserving key encoding, so the write planner could
    // never maintain an index entry for it. Reject it at compile time rather than
    // silently committing the data with no index.
    let source = "\
resource Book at ^books(id: int)
    price: decimal
    index byPrice(price, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNORDERABLE_KEY]);
    assert!(errors[0].message.contains("price"));
}

#[test]
fn keyed_leaf_with_a_decimal_key_param_is_an_error() {
    // A keyed-layer key must be an ordered key type; `decimal` is not.
    let source = "\
resource Book at ^books(id: int)
    samples(at: decimal): int
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNORDERABLE_KEY]);
    assert!(errors[0].message.contains("at"));
}

#[test]
fn identity_key_typed_decimal_is_an_error() {
    let source = "\
resource Reading at ^readings(at: decimal)
    required value: int
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNORDERABLE_KEY]);
    assert!(errors[0].message.contains("at"));
}

#[test]
fn non_unique_index_omitting_the_identity_key_is_an_error() {
    // A non-unique index must end with all identity keys so each entry is
    // distinct. `byShelf(shelf)` collapses two books on the same shelf onto one
    // entry.
    let source = "\
resource Book at ^books(id: int)
    shelf: string
    index byShelf(shelf)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_INDEX_MISSING_IDENTITY_KEYS]);
    assert!(errors[0].message.contains("byShelf"));
}

#[test]
fn non_unique_index_ending_with_identity_key_is_allowed() {
    let source = "\
resource Book at ^books(id: int)
    shelf: string
    index byShelf(shelf, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert!(
        errors.is_empty(),
        "trailing identity key resolves: {errors:?}"
    );
}

#[test]
fn non_unique_index_with_identity_key_not_last_is_an_error() {
    // The identity keys must be the trailing arguments, in declaration order.
    let source = "\
resource Book at ^books(id: int)
    shelf: string
    index byShelf(id, shelf)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_INDEX_MISSING_IDENTITY_KEYS]);
}

#[test]
fn non_unique_index_on_composite_identity_requires_all_keys_in_order() {
    // For a composite identity, a non-unique index must end with every identity
    // key in declaration order.
    let reversed = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string
    index byStatus(status, courseId, studentId)
";
    let (_, errors) = compile_resource(&resource(reversed));
    assert_eq!(codes(&errors), [SCHEMA_INDEX_MISSING_IDENTITY_KEYS]);

    let in_order = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string
    index byStatus(status, studentId, courseId)
";
    let (_, errors) = compile_resource(&resource(in_order));
    assert!(
        errors.is_empty(),
        "all identity keys in order resolve: {errors:?}"
    );
}

#[test]
fn unique_index_may_omit_the_identity_key() {
    // A unique index points to one identity, so it may omit the identity keys.
    let source = "\
resource Book at ^books(id: int)
    isbn: string
    index byIsbn(isbn) unique
";
    let (_, errors) = compile_resource(&resource(source));
    assert!(
        errors.is_empty(),
        "unique index needs no identity key: {errors:?}"
    );
}

#[test]
fn index_on_a_singleton_resource_is_an_error() {
    // A singleton saved resource has no generated identity for an index entry to
    // point to, so an index is rejected.
    let source = "\
resource Settings at ^settings
    theme: string
    index byTheme(theme)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_INDEX_REQUIRES_KEYED_ROOT]);
    assert!(errors[0].message.contains("byTheme"));
}

#[test]
fn index_on_a_local_resource_is_an_error() {
    // A local (non-saved) resource has no saved root at all, so it cannot carry a
    // declared index.
    let source = "\
resource Draft
    title: string
    index byTitle(title)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_INDEX_REQUIRES_KEYED_ROOT]);
    assert!(errors[0].message.contains("byTitle"));
}

#[test]
fn required_field_inside_an_unkeyed_group_is_an_error() {
    // The write planner does not materialize unkeyed groups: a whole-resource
    // write neither validates nor persists their fields, so a required field
    // inside an unkeyed group is a compile error rather than a silently
    // unenforced constraint. The canonical Patient
    // `name { required first; last }` shape exercises this.
    let source = "\
resource Patient at ^patients(id: string)
    name
        required first: string
        last: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_REQUIRED_IN_UNKEYED_GROUP]);
    assert!(errors[0].message.contains("first"));
}

#[test]
fn required_field_inside_a_keyed_group_is_allowed() {
    // The rejection is specific to UNKEYED groups; a keyed group (a layer the
    // planner does maintain) may hold required fields, as in the Book
    // `versions(version) { required title }` shape.
    let source = "\
resource Book at ^books(id: int)
    versions(version: int)
        required title: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert!(
        errors.is_empty(),
        "keyed-group required fields are fine: {errors:?}"
    );
}

#[test]
fn index_over_a_nested_field_is_an_error() {
    // The write planner matches index arguments by flat top-level name, so an
    // index over a field nested in an unkeyed group is silently never maintained.
    // Until nested index resolution lands, reject it.
    let source = "\
resource Book at ^books(id: int)
    pricing
        amount: int
    index byAmount(pricing.amount, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NESTED_INDEX_ARG]);
    assert!(errors[0].message.contains("pricing.amount"));
}

#[test]
fn duplicate_identity_key_name_is_an_error() {
    // Identity keys must have distinct names; two `studentId` keys are
    // unaddressable.
    let source = "\
resource Enrollment at ^enrollments(studentId: string, studentId: string)
    status: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_DUPLICATE_MEMBER]);
    assert!(errors[0].message.contains("studentId"));
}

#[test]
fn duplicate_keyed_leaf_key_param_name_is_an_error() {
    // A keyed layer's key parameters must have distinct names.
    let source = "\
resource Book at ^books(id: int)
    tags(pos: int, pos: int): string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_DUPLICATE_MEMBER]);
    assert!(errors[0].message.contains("pos"));
}

#[test]
fn duplicate_group_key_param_name_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    revisions(rev: int, rev: int)
        body: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_DUPLICATE_MEMBER]);
    assert!(errors[0].message.contains("rev"));
}

#[test]
fn duplicate_stable_id_within_resource_is_an_error() {
    // Stable IDs must be unique; within one resource the later element is the
    // error.
    let source = "\
resource Book at ^books(id: int)
    @id(\"book.x\")
    required title: string
    @id(\"book.x\")
    required author: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_DUPLICATE_STABLE_ID]);
    assert!(errors[0].message.contains("book.x"));
    // The error points at the second element, not the first.
    assert_eq!(errors[0].span.line, 5);
}

#[test]
fn clean_book_has_no_new_errors() {
    let (_, errors) = compile_resource(&resource(BOOK));
    assert!(errors.is_empty(), "Book is clean: {errors:?}");
}

#[test]
fn one_shape_compiles_as_both_local_and_saved() {
    // A resource's field and layer shape is checked through one schema whether it
    // is a saved root or a local value. Only `saved_root` differs between the two
    // compilations.
    let saved = compile_ok(
        "\
resource Book at ^books(id: int)
    required title: string
    tags(pos: int): string
",
    );
    let local = compile_ok(
        "\
resource Book
    required title: string
    tags(pos: int): string
",
    );
    assert!(saved.saved_root.is_some(), "saved Book has a root");
    assert!(local.saved_root.is_none(), "local Book has no root");
    // The stored shape is identical regardless of where the resource lives.
    assert_eq!(saved.fields, local.fields);
    assert_eq!(saved.layers, local.layers);
    assert_eq!(saved.indexes, local.indexes);
}
