//! Rejection of an optional (`T?`) value type anywhere inside a saved shape:
//! fields, keyed-leaf values, sequence elements, maybe-present records, and key
//! parameters. `?` is the code-level type a sparse read yields, not a storage
//! marker — a field is sparse by default — so a saved value type drops the `?`.

use crate::common;
use common::{assert_kind, codes, compile_source_errors};
use marrow_schema::{
    SCHEMA_NONSCALAR_KEY, SCHEMA_OPTIONAL_IN_SAVED, SchemaErrorKind, SchemaSavedPosition,
};

fn optional(target: SchemaSavedPosition, name: &str) -> SchemaErrorKind {
    SchemaErrorKind::OptionalInSaved {
        target,
        name: name.to_string(),
    }
}

#[test]
fn saved_field_typed_optional_is_an_error() {
    let source = "\
resource Book
    required title: string
    subtitle: string?
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_OPTIONAL_IN_SAVED]);
    assert_kind(&errors[0], optional(SchemaSavedPosition::Field, "subtitle"));
}

#[test]
fn saved_keyed_leaf_typed_optional_is_an_error() {
    let source = "\
resource Counter
    counts(day: string): int?
store ^counters(id: int): Counter
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_OPTIONAL_IN_SAVED]);
    assert_kind(
        &errors[0],
        optional(SchemaSavedPosition::KeyedLeaf, "counts"),
    );
}

#[test]
fn saved_positional_layer_typed_optional_is_a_sequence_element() {
    // A positional (single-`int`-keyed) layer is the sequence shape, so its optional
    // leaf is named a sequence element, not a keyed leaf.
    let source = "\
resource Book
    tags(pos: int): string?
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_OPTIONAL_IN_SAVED]);
    assert_kind(
        &errors[0],
        optional(SchemaSavedPosition::SequenceElement, "tags"),
    );
}

#[test]
fn saved_nested_field_typed_optional_is_an_error() {
    let source = "\
resource Book
    notes(noteId: string)
        body: string?
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_OPTIONAL_IN_SAVED]);
    assert_kind(&errors[0], optional(SchemaSavedPosition::Field, "body"));
}

#[test]
fn saved_field_of_optional_sequence_element_is_an_error() {
    // `?` is rejected anywhere inside a saved type, including the element of a
    // `sequence[...]`, which `embeds_optional` sees through. The rejection names the
    // sequence element that carries the `?`, not the field.
    let source = "\
resource Book
    tags: sequence[string?]
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_OPTIONAL_IN_SAVED]);
    assert_kind(
        &errors[0],
        optional(SchemaSavedPosition::SequenceElement, "tags"),
    );
}

#[test]
fn saved_field_typed_maybe_present_record_is_an_error() {
    // A maybe-present whole record `R?` is an ordinary code type, never a saved
    // field value type.
    let source = "\
resource Book
    cover: Image?
store ^books(id: int): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_OPTIONAL_IN_SAVED]);
    assert_kind(&errors[0], optional(SchemaSavedPosition::Field, "cover"));
}

#[test]
fn saved_identity_key_typed_optional_is_a_nonscalar_key() {
    // A key must be present to address a node, so an optional key is rejected
    // alongside the other non-orderable key types.
    let source = "\
resource Book
    required title: string
store ^books(id: int?): Book
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NONSCALAR_KEY]);
}
