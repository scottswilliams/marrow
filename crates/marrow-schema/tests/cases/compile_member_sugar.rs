//! Member sugar: `sequence[T]` desugars to a keyed-leaf layer identical to its
//! canonical spelling, top-level and nested.

use crate::common;
use common::{compile_ok, layer, top_level_fields};
use marrow_schema::{NodeKind, ScalarType, Type};

#[test]
fn sequence_member_desugars_to_a_pos_int_keyed_leaf() {
    // `tags: sequence[string]` is sugar for `tags(pos: int): string`, so it
    // compiles to the same keyed leaf the canonical spelling produces.
    let source = "\
resource Book
    tags: sequence[string]
store ^books(id: int): Book
";
    let schema = compile_ok(source);
    // It is a keyed leaf, not a plain top-level field.
    assert!(!top_level_fields(&schema).any(|f| f.name == "tags"));
    let tags = layer(&schema, "tags");
    assert_eq!(tags.key_params.len(), 1);
    assert_eq!(tags.key_params[0].name, "pos");
    assert_eq!(tags.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert!(
        matches!(&tags.kind, NodeKind::Slot { ty, .. } if *ty == Type::Scalar(ScalarType::Str))
    );
    assert!(tags.members.is_empty(), "a keyed leaf has no members");
}

#[test]
fn sequence_member_matches_the_canonical_keyed_leaf() {
    // The desugared layer is identical to the canonical `tags(pos: int): string`.
    let sugar = layer(
        &compile_ok("resource Book\n    tags: sequence[string]\nstore ^books(id: int): Book\n"),
        "tags",
    )
    .clone();
    let canonical = layer(
        &compile_ok("resource Book\n    tags(pos: int): string\nstore ^books(id: int): Book\n"),
        "tags",
    )
    .clone();
    assert_eq!(sugar, canonical);
}

#[test]
fn nested_sequence_member_desugars_inside_a_group() {
    // A sequence nested inside a group desugars the same way.
    let source = "\
resource Book
    versions(version: int)
        notes: sequence[string]
store ^books(id: int): Book
";
    let schema = compile_ok(source);
    let versions = layer(&schema, "versions");
    let notes = &versions.members[0];
    assert!(
        matches!(&notes.kind, NodeKind::Slot { ty, .. } if *ty == Type::Scalar(ScalarType::Str)),
        "notes should desugar to a nested keyed-leaf layer"
    );
    assert_eq!(notes.name, "notes");
    assert_eq!(notes.key_params.len(), 1);
    assert_eq!(notes.key_params[0].name, "pos");
    assert_eq!(notes.key_params[0].ty, Type::Scalar(ScalarType::Int));
}
