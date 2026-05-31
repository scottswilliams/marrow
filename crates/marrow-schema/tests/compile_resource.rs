//! Schema compilation tests.
//!
//! The primary case is the canonical `Book` resource, which exercises the saved
//! root, required and sparse fields, a keyed-leaf
//! sequence, group and history layers, and an index. The remaining cases pin
//! the structural errors the compiler reports.

use marrow_schema::{
    Element, Node, ResourceSchema, SCHEMA_DUPLICATE_MEMBER, SCHEMA_DUPLICATE_STABLE_ID,
    SCHEMA_INDEX_IN_GROUP, SCHEMA_INDEX_MISSING_IDENTITY_KEYS, SCHEMA_INDEX_REQUIRES_KEYED_ROOT,
    SCHEMA_KEY_MEMBER_COLLISION, SCHEMA_NESTED_INDEX_ARG, SCHEMA_NON_ENUM_NAMED_FIELD,
    SCHEMA_NONSCALAR_KEY, SCHEMA_UNKNOWN_IN_SAVED, SCHEMA_UNKNOWN_INDEX_ARG,
    SCHEMA_UNORDERABLE_KEY, SCHEMA_UNSUPPORTED_TYPE, ScalarType, Type, check_saved_named_fields,
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

/// The top-level node named `name` (a keyed leaf or a group).
fn layer<'a>(schema: &'a ResourceSchema, name: &str) -> &'a Node {
    schema
        .members
        .iter()
        .find(|node| node.name == name)
        .unwrap_or_else(|| panic!("layer `{name}` not found"))
}

/// The top-level plain-field nodes: `Slot`s with no key parameters.
fn top_level_fields(schema: &ResourceSchema) -> impl Iterator<Item = &Node> {
    schema
        .members
        .iter()
        .filter(|node| node.key_params.is_empty() && matches!(node.element, Element::Slot { .. }))
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

    let names: Vec<&str> = top_level_fields(&schema).map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        ["title", "author", "shelf", "currentVersion", "loanedTo"]
    );

    let required: Vec<&str> = top_level_fields(&schema)
        .filter(|f| matches!(f.element, Element::Slot { required, .. } if required))
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(required, ["title", "author", "shelf", "currentVersion"]);

    let loaned_to = top_level_fields(&schema)
        .find(|f| f.name == "loanedTo")
        .expect("loanedTo field");
    let Element::Slot { ty, required } = &loaned_to.element else {
        panic!("loanedTo is a slot");
    };
    assert!(!required, "loanedTo is sparse");
    assert_eq!(*ty, Type::Scalar(ScalarType::Str));

    // `tags` is a keyed leaf, not a top-level field.
    assert!(!top_level_fields(&schema).any(|f| f.name == "tags"));
}

#[test]
fn book_tags_is_a_keyed_leaf() {
    let schema = compile_ok(BOOK);
    let tags = layer(&schema, "tags");
    assert_eq!(tags.key_params.len(), 1);
    assert_eq!(tags.key_params[0].name, "pos");
    assert_eq!(tags.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert!(
        matches!(&tags.element, Element::Slot { ty, .. } if *ty == Type::Scalar(ScalarType::Str))
    );
    assert!(tags.members.is_empty(), "a keyed leaf has no members");
}

#[test]
fn book_notes_is_a_group() {
    let schema = compile_ok(BOOK);
    let notes = layer(&schema, "notes");
    assert_eq!(notes.key_params.len(), 1);
    assert_eq!(notes.key_params[0].name, "noteId");
    assert_eq!(notes.key_params[0].ty, Type::Scalar(ScalarType::Str));
    assert!(
        matches!(notes.element, Element::Group),
        "a group has no leaf type"
    );

    assert_eq!(notes.members.len(), 1);
    let text = &notes.members[0];
    let Element::Slot { ty, required } = &text.element else {
        panic!("notes.text should be a field");
    };
    assert_eq!(text.name, "text");
    assert_eq!(*ty, Type::Scalar(ScalarType::Str));
    assert!(!required);
}

#[test]
fn book_versions_is_a_history_group() {
    let schema = compile_ok(BOOK);
    let versions = layer(&schema, "versions");
    assert_eq!(versions.key_params.len(), 1);
    assert_eq!(versions.key_params[0].name, "version");
    assert_eq!(versions.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert!(matches!(versions.element, Element::Group));

    let fields: Vec<(&str, bool, String)> = versions
        .members
        .iter()
        .map(|member| match &member.element {
            Element::Slot { ty, required } => (member.name.as_str(), *required, ty.to_string()),
            Element::Group => panic!("unexpected nested group `{}`", member.name),
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
    assert_eq!(top_level_fields(&schema).count(), 2);
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
fn saved_map_member_value_typed_unknown_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    scores: map[string, unknown]
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("scores"));
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
    // It is a keyed leaf, not a plain top-level field.
    assert!(!top_level_fields(&schema).any(|f| f.name == "tags"));
    let tags = layer(&schema, "tags");
    assert_eq!(tags.key_params.len(), 1);
    assert_eq!(tags.key_params[0].name, "pos");
    assert_eq!(tags.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert!(
        matches!(&tags.element, Element::Slot { ty, .. } if *ty == Type::Scalar(ScalarType::Str))
    );
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
fn map_member_matches_the_canonical_keyed_leaf() {
    // `scores: map[string, int]` is sugar for `scores(key: string): int`.
    let sugar = layer(
        &compile_ok("resource Book at ^books(id: int)\n    scores: map[string, int]\n"),
        "scores",
    )
    .clone();
    let canonical = layer(
        &compile_ok("resource Book at ^books(id: int)\n    scores(key: string): int\n"),
        "scores",
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
    let notes = &versions.members[0];
    assert!(
        matches!(&notes.element, Element::Slot { ty, .. } if *ty == Type::Scalar(ScalarType::Str)),
        "notes should desugar to a nested keyed-leaf layer"
    );
    assert_eq!(notes.name, "notes");
    assert_eq!(notes.key_params.len(), 1);
    assert_eq!(notes.key_params[0].name, "pos");
    assert_eq!(notes.key_params[0].ty, Type::Scalar(ScalarType::Int));
}

#[test]
fn nested_map_member_desugars_inside_a_group() {
    let source = "\
resource Book at ^books(id: int)
    versions(version: int)
        scores: map[string, int]
";
    let schema = compile_ok(source);
    let versions = layer(&schema, "versions");
    let scores = &versions.members[0];
    assert!(
        matches!(&scores.element, Element::Slot { ty, .. } if *ty == Type::Scalar(ScalarType::Int)),
        "scores should desugar to a nested keyed-leaf layer"
    );
    assert_eq!(scores.name, "scores");
    assert_eq!(scores.key_params.len(), 1);
    assert_eq!(scores.key_params[0].name, "key");
    assert_eq!(scores.key_params[0].ty, Type::Scalar(ScalarType::Str));
}

#[test]
fn map_member_sugar_is_rejected_on_local_resources() {
    let source = "\
resource Draft
    scores: map[string, int]
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert!(errors[0].message.contains("scores"));
}

#[test]
fn map_type_nested_inside_sequence_is_rejected() {
    let source = "\
resource Book at ^books(id: int)
    scores: sequence[map[string, int]]
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert!(errors[0].message.contains("scores"));
}

#[test]
fn map_type_in_identity_key_is_rejected() {
    let source = "\
resource Book at ^books(id: map[string, int])
    title: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert!(errors[0].message.contains("id"));
}

#[test]
fn map_type_in_key_param_is_rejected() {
    let source = "\
resource Draft
    scores(k: map[string, int]): int
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert!(errors[0].message.contains("k"));
}

#[test]
fn map_type_as_map_key_is_rejected_once() {
    let source = "\
resource Book at ^books(id: int)
    scores: map[map[string, int], int]
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert!(errors[0].message.contains("key"));
}

#[test]
fn required_map_member_sugar_is_rejected() {
    let source = "\
resource Book at ^books(id: int)
    required scores: map[string, int]
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert!(errors[0].message.contains("scores"));
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
    // Index arguments must resolve to an identity key or top-level field.
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
fn index_arg_naming_map_member_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    scores: map[string, int]
    index byScore(scores, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_INDEX_ARG]);
    assert!(errors[0].message.contains("scores"));
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
    samples(ts: decimal): int
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNORDERABLE_KEY]);
    assert!(errors[0].message.contains("ts"));
}

#[test]
fn map_member_with_a_decimal_key_type_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    scores: map[decimal, int]
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNORDERABLE_KEY]);
    assert!(errors[0].message.contains("key"));
}

#[test]
fn identity_key_typed_decimal_is_an_error() {
    let source = "\
resource Reading at ^readings(ts: decimal)
    required value: int
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNORDERABLE_KEY]);
    assert!(errors[0].message.contains("ts"));
}

#[test]
fn identity_key_typed_as_a_bare_name_is_an_error() {
    // A key must be an orderable scalar. A bare name names no scalar, so a raw
    // string or int written into that key position would be silently accepted and
    // corrupt the keyspace. The rule is structural — it needs no knowledge of what
    // the name refers to — so an enum, a resource, or a typo is caught the same way.
    let source = "\
resource Order at ^orders(state: Status)
    required note: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert!(errors[0].message.contains("Status"));
}

#[test]
fn keyed_layer_key_param_typed_as_a_bare_name_is_an_error() {
    let source = "\
resource Order at ^orders(id: int)
    byState(state: Status): string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert!(errors[0].message.contains("Status"));
}

#[test]
fn an_undeclared_or_typo_named_identity_key_is_an_error() {
    // The allowlist is structural, so a name that resolves to nothing is rejected
    // exactly like a declared one. A typo'd key would otherwise accept any value,
    // letting an int and a string coexist in one identity keyspace.
    let source = "\
resource Order at ^orders(state: Stutus)
    required note: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert!(errors[0].message.contains("Stutus"));
}

#[test]
fn a_resource_typed_identity_key_is_an_error() {
    // A bare name that happens to be a declared resource is still not an orderable
    // scalar, so it cannot be a key. `Person` here names a local resource.
    let source = "\
resource Person
    required name: string
resource Order at ^orders(owner: Person)
    required note: string
";
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
    let order = parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Resource(resource) if resource.name == "Order" => Some(resource.clone()),
            _ => None,
        })
        .expect("Order resource");
    let (_, errors) = compile_resource(&order);
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert!(errors[0].message.contains("Person"));
}

#[test]
fn a_sequence_typed_key_is_an_error() {
    // A sequence is not a scalar at all, so it cannot project to an orderable key.
    let source = "\
resource Order at ^orders(tags: sequence[string])
    required note: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert!(errors[0].message.contains("sequence"));
}

#[test]
fn a_sequence_index_argument_is_an_error() {
    // An index argument keys on its field's stored scalar. A `sequence` has no
    // single scalar projection, so it cannot be an index key.
    let source = "\
resource Order at ^orders(id: int)
    tags: sequence[string]
    index byTags(tags, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert!(errors[0].message.contains("byTags"));
}

#[test]
fn an_identity_field_index_argument_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    authorId: Author::Id
    index byAuthor(authorId, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert!(errors[0].message.contains("authorId"));
}

#[test]
fn an_enum_field_index_argument_is_clean() {
    // An enum field stores its member ordinal as an orderable `int`, so an index
    // keys on that ordinal. This is the staged enum-field-index behavior: the index
    // position projects the stored scalar, which for an enum is `int`, not the
    // free-floating path-key case that identity and layer keys are.
    let source = "\
resource Order at ^orders(id: int)
    state: Status
    index byState(state, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert!(
        errors.is_empty(),
        "an enum-field index keys on its ordinal: {errors:?}"
    );
}

#[test]
fn an_orderable_scalar_identity_key_is_clean() {
    // The allowlist does not over-reject: an orderable scalar key at the identity,
    // a layer key param, and an index argument all compile clean.
    let source = "\
resource Order at ^orders(id: int)
    byTag(tag: string): string
    rank: int
    index byRank(rank, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert!(errors.is_empty(), "orderable scalar keys: {errors:?}");
}

#[test]
fn an_identity_typed_key_is_an_error() {
    // A saved key must be an orderable scalar. An identity value has no supported
    // saved-key encoding yet, so reject it statically instead of deferring it.
    let source = "\
resource Edge at ^edges(from: Node::Id)
    required note: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert!(errors[0].message.contains("Node::Id"));
}

#[test]
fn a_keyed_layer_key_param_typed_as_an_identity_is_an_error() {
    let source = "\
resource Edge at ^edges(id: int)
    byNode(from: Node::Id): string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
    assert!(errors[0].message.contains("Node::Id"));
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
fn required_field_inside_an_unkeyed_group_is_allowed() {
    // Unkeyed groups are structural. A required field inside one is required for
    // the containing resource, and remains marked on the nested schema node.
    let source = "\
resource Patient at ^patients(id: string)
    name
        required first: string
        last: string
";
    let schema = compile_ok(source);
    let name = layer(&schema, "name");
    let first = name
        .members
        .iter()
        .find(|node| node.name == "first")
        .expect("nested first field");
    assert!(matches!(
        first.element,
        Element::Slot { required: true, .. }
    ));
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
fn index_arg_naming_nested_leaf_is_an_error() {
    // A bare leaf inside an unkeyed group is a nested field, not an unknown name.
    let source = "\
resource Book at ^books(id: int)
    location
        shelf: string
    index byShelf(shelf, id)
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_NESTED_INDEX_ARG]);
    assert!(errors[0].message.contains("shelf"));
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
    assert_eq!(saved.members, local.members);
    assert_eq!(saved.indexes, local.indexes);
}

/// A resource nesting a keyed-leaf layer and a field inside a group, to pin the
/// field/leaf resolvers on the cases that are not a single top-level lookup.
const NESTED: &str = "\
resource Catalog at ^catalog(id: int)
    required title: string
    tags(pos: int): string

    versions(version: int)
        required note: string
        lines(pos: int): string

        comments(commentId: string)
            required body: string
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
    // untyped exactly as the checker treated it before this walk was shared.
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
    let decl = resource(
        "\
resource Order at ^orders(id: int)
    required state: Status
",
    );
    // `Status` is a declared enum: the saved field is accepted.
    assert!(
        check_saved_named_fields(&decl, &["Status".to_string()]).is_empty(),
        "an enum-typed saved field is allowed"
    );
    // With no such enum declared, the bare-named field has no stored scalar form
    // (it would otherwise silently decode as an int) and is rejected.
    let errors = check_saved_named_fields(&decl, &[]);
    assert_eq!(codes(&errors), [SCHEMA_NON_ENUM_NAMED_FIELD]);
    assert!(errors[0].message.contains("state"));

    // A local (non-saved) resource is exempt: nothing lowers into the store.
    let local = resource("resource Order\n    required state: Status\n");
    assert!(
        check_saved_named_fields(&local, &[]).is_empty(),
        "a local resource has no saved fields to reject"
    );
}

#[test]
fn a_bare_named_map_value_must_be_a_declared_enum() {
    let decl = resource(
        "\
resource Order at ^orders(id: int)
    scores: map[string, Status]
",
    );
    assert!(
        check_saved_named_fields(&decl, &["Status".to_string()]).is_empty(),
        "an enum-typed map value is allowed"
    );
    let errors = check_saved_named_fields(&decl, &[]);
    assert_eq!(codes(&errors), [SCHEMA_NON_ENUM_NAMED_FIELD]);
    assert!(errors[0].message.contains("scores"));
}

#[test]
fn unsupported_map_value_is_not_checked_as_bare_named_saved_field() {
    let decl = resource(
        "\
resource Order at ^orders(id: int)
    scores: map[string, map[string, int]]
",
    );
    let errors = check_saved_named_fields(&decl, &[]);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn unsupported_map_key_does_not_check_map_value_as_bare_named_saved_field() {
    let decl = resource(
        "\
resource Order at ^orders(id: int)
    scores: map[map[string, int], Missing]
",
    );
    let errors = check_saved_named_fields(&decl, &[]);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn required_map_member_does_not_check_value_as_bare_named_saved_field() {
    let decl = resource(
        "\
resource Order at ^orders(id: int)
    required scores: map[string, Missing]
",
    );
    let errors = check_saved_named_fields(&decl, &[]);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_qualified_named_saved_field_is_not_a_schema_local_error() {
    let short_alias = resource(
        "\
module a
use pkg::kinds
resource Saved at ^saved(id: int)
    required k: kinds::Color
",
    );
    let short_errors = check_saved_named_fields(&short_alias, &[]);
    assert!(
        short_errors.is_empty(),
        "the schema-only gate cannot reject a qualified enum name: {short_errors:#?}"
    );

    let full_path = resource(
        "\
module a
resource Saved at ^saved(id: int)
    required k: pkg::kinds::Color
",
    );
    let full_errors = check_saved_named_fields(&full_path, &[]);
    assert!(
        full_errors.is_empty(),
        "the schema-only gate cannot reject a fully qualified enum name: {full_errors:#?}"
    );
}
