//! Index resolution and uniqueness rules: an index argument must resolve to an
//! identity key or a top-level scalar field (not a keyed leaf, sequence, or
//! nested field), index names are distinct and may not collide with
//! identity keys, a non-unique index must end with all identity keys in order, a
//! unique index may omit them, and an index requires a keyed root.

use crate::common;
use common::{assert_kind, codes, resource_and_store};
use marrow_schema::{
    SCHEMA_DUPLICATE_MEMBER, SCHEMA_INDEX_MISSING_IDENTITY_KEYS, SCHEMA_INDEX_REQUIRES_KEYED_ROOT,
    SCHEMA_KEY_MEMBER_COLLISION, SCHEMA_NESTED_INDEX_ARG, SCHEMA_UNKNOWN_INDEX_ARG,
    SchemaDuplicateTarget, SchemaError, SchemaErrorKind, SchemaNameCollision, compile_resource,
    compile_store,
};

fn compile_store_errors(source: &str) -> Vec<SchemaError> {
    let (resource, store) = resource_and_store(source);
    let (schema, resource_errors) = compile_resource(&resource);
    assert!(
        resource_errors.is_empty(),
        "unexpected resource errors: {resource_errors:?}"
    );
    let (_, store_errors) = compile_store(&store, &schema);
    store_errors
}

#[test]
fn identity_key_name_colliding_with_index_is_an_error() {
    // Identity keys and index names share the store namespace, so a key may not
    // reuse an index name.
    let source = "\
resource Book
    required title: string
store ^books(id: int): Book
    index id(title, id)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_KEY_MEMBER_COLLISION]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::KeyMemberCollision {
            collision: SchemaNameCollision::IdentityKeyWithIndex {
                key: "id".to_string(),
                index: "id".to_string(),
            },
        },
    );
}

#[test]
fn duplicate_index_name_is_an_error() {
    let source = "\
resource Book
    required title: string
    shelf: string
store ^books(id: int): Book
    index byShelf(shelf, id)
    index byShelf(title, id)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_DUPLICATE_MEMBER], "{errors:#?}");
    assert_kind(
        &errors[0],
        SchemaErrorKind::DuplicateMember {
            target: SchemaDuplicateTarget::Index,
            name: "byShelf".to_string(),
        },
    );
}

#[test]
fn index_arg_naming_no_member_is_an_error() {
    // Index arguments must resolve to an identity key or top-level field.
    // `shelf` names nothing here.
    let source = "\
resource Book
    required title: string
store ^books(id: int): Book
    index byShelf(shelf, id)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_INDEX_ARG]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::UnknownIndexArg {
            index: "byShelf".to_string(),
            arg: "shelf".to_string(),
        },
    );
}

#[test]
fn index_arg_naming_field_and_key_is_allowed() {
    // A top-level field and an identity key both resolve as index arguments.
    let source = "\
resource Book
    required title: string
store ^books(id: int): Book
    index byTitle(title, id)
";
    let errors = compile_store_errors(source);
    assert!(
        errors.is_empty(),
        "field and identity-key args resolve: {errors:?}"
    );
}

#[test]
fn index_arg_naming_keyed_leaf_is_an_error() {
    // Index arguments do not walk keyed child layers; `tags` is a keyed leaf.
    let source = "\
resource Book
    tags(pos: int): string
store ^books(id: int): Book
    index byTag(tags, id)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_INDEX_ARG]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::UnknownIndexArg {
            index: "byTag".to_string(),
            arg: "tags".to_string(),
        },
    );
}

#[test]
fn a_sequence_index_argument_is_an_error() {
    // An index argument keys on its field's stored scalar. A `sequence` has no
    // single scalar projection, so it cannot be an index key.
    let source = "\
resource Order
    tags: sequence[string]
store ^orders(id: int): Order
    index byTags(tags, id)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_INDEX_ARG]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::UnknownIndexArg {
            index: "byTags".to_string(),
            arg: "tags".to_string(),
        },
    );
}

#[test]
fn an_enum_field_index_argument_is_clean() {
    // Schema admits an enum-typed top-level field as an index argument. Project
    // checking attaches the catalog-member key meaning once the enum identity is
    // known.
    let source = "\
resource Order
    state: Status
store ^orders(id: int): Order
    index byState(state, id)
";
    let errors = compile_store_errors(source);
    assert!(
        errors.is_empty(),
        "an enum-field index argument should be accepted: {errors:?}"
    );
}

#[test]
fn non_unique_index_omitting_the_identity_key_is_an_error() {
    // A non-unique index must end with all identity keys so each entry is
    // distinct. `byShelf(shelf)` collapses two books on the same shelf onto one
    // entry.
    let source = "\
resource Book
    shelf: string
store ^books(id: int): Book
    index byShelf(shelf)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_INDEX_MISSING_IDENTITY_KEYS]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::IndexMissingIdentityKeys {
            index: "byShelf".to_string(),
        },
    );
}

#[test]
fn non_unique_index_ending_with_identity_key_is_allowed() {
    let source = "\
resource Book
    shelf: string
store ^books(id: int): Book
    index byShelf(shelf, id)
";
    let errors = compile_store_errors(source);
    assert!(
        errors.is_empty(),
        "trailing identity key resolves: {errors:?}"
    );
}

#[test]
fn non_unique_index_with_identity_key_not_last_is_an_error() {
    // The identity keys must be the trailing arguments, in declaration order.
    let source = "\
resource Book
    shelf: string
store ^books(id: int): Book
    index byShelf(id, shelf)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_INDEX_MISSING_IDENTITY_KEYS]);
}

#[test]
fn non_unique_index_on_composite_identity_requires_all_keys_in_order() {
    // For a composite identity, a non-unique index must end with every identity
    // key in declaration order.
    let reversed = "\
resource Enrollment
    status: string
store ^enrollments(studentId: string, courseId: string): Enrollment
    index byStatus(status, courseId, studentId)
";
    let errors = compile_store_errors(reversed);
    assert_eq!(codes(&errors), [SCHEMA_INDEX_MISSING_IDENTITY_KEYS]);

    let in_order = "\
resource Enrollment
    status: string
store ^enrollments(studentId: string, courseId: string): Enrollment
    index byStatus(status, studentId, courseId)
";
    let errors = compile_store_errors(in_order);
    assert!(
        errors.is_empty(),
        "all identity keys in order resolve: {errors:?}"
    );
}

#[test]
fn unique_index_may_omit_the_identity_key() {
    // A unique index points to one identity, so it may omit the identity keys.
    let source = "\
resource Book
    isbn: string
store ^books(id: int): Book
    index byIsbn(isbn) unique
";
    let errors = compile_store_errors(source);
    assert!(
        errors.is_empty(),
        "unique index needs no identity key: {errors:?}"
    );
}

#[test]
fn index_on_a_singleton_store_is_an_error() {
    // A singleton store has no generated identity for an index entry to
    // point to, so an index is rejected.
    let source = "\
resource Settings
    theme: string
store ^settings: Settings
    index byTheme(theme)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_INDEX_REQUIRES_KEYED_ROOT]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::IndexRequiresKeyedRoot {
            index: "byTheme".to_string(),
        },
    );
}

#[test]
fn index_over_a_nested_field_is_an_error() {
    let source = "\
resource Book
    pricing
        amount: int
store ^books(id: int): Book
    index byAmount(pricing.amount, id)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NESTED_INDEX_ARG]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NestedIndexArg {
            index: "byAmount".to_string(),
            arg: "pricing.amount".to_string(),
        },
    );
}

#[test]
fn index_arg_naming_nested_leaf_is_an_error() {
    let source = "\
resource Book
    location
        shelf: string
store ^books(id: int): Book
    index byShelf(shelf, id)
";
    let errors = compile_store_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_NESTED_INDEX_ARG]);
    assert_kind(
        &errors[0],
        SchemaErrorKind::NestedIndexArg {
            index: "byShelf".to_string(),
            arg: "shelf".to_string(),
        },
    );
}
