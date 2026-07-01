use marrow_schema::{
    ScalarType, SchemaDuplicateTarget, SchemaErrorKind, SchemaKeyTarget, SchemaNameCollision,
    SchemaSavedPosition, SchemaStoreInvalidation, Type,
};

fn index(name: &str) -> Option<SchemaStoreInvalidation> {
    Some(SchemaStoreInvalidation::Index {
        name: name.to_string(),
    })
}

#[test]
fn store_invalidation_mapping_is_pinned() {
    let cases = [
        (
            SchemaErrorKind::DuplicateMember {
                target: SchemaDuplicateTarget::ResourceMember,
                name: "title".into(),
            },
            None,
        ),
        (
            SchemaErrorKind::DuplicateMember {
                target: SchemaDuplicateTarget::EnumMember,
                name: "draft".into(),
            },
            None,
        ),
        (
            SchemaErrorKind::DuplicateMember {
                target: SchemaDuplicateTarget::KeyParam,
                name: "id".into(),
            },
            Some(SchemaStoreInvalidation::Store),
        ),
        (
            SchemaErrorKind::DuplicateMember {
                target: SchemaDuplicateTarget::Index,
                name: "byTitle".into(),
            },
            index("byTitle"),
        ),
        (
            SchemaErrorKind::CategoryLeaf {
                member: "status".into(),
            },
            None,
        ),
        (
            SchemaErrorKind::ParentNotCategory {
                member: "status".into(),
            },
            None,
        ),
        (
            SchemaErrorKind::UnknownInSaved {
                target: SchemaSavedPosition::Field,
                name: "title".into(),
            },
            None,
        ),
        (
            SchemaErrorKind::UnknownInSaved {
                target: SchemaSavedPosition::IdentityKey,
                name: "id".into(),
            },
            Some(SchemaStoreInvalidation::Store),
        ),
        (
            SchemaErrorKind::UnknownInSaved {
                target: SchemaSavedPosition::Key,
                name: "pos".into(),
            },
            None,
        ),
        (
            SchemaErrorKind::UnknownInSaved {
                target: SchemaSavedPosition::KeyedLeaf,
                name: "tag".into(),
            },
            None,
        ),
        (
            SchemaErrorKind::KeyMemberCollision {
                collision: SchemaNameCollision::IdentityKeyWithMember { key: "id".into() },
            },
            Some(SchemaStoreInvalidation::Store),
        ),
        (
            SchemaErrorKind::KeyMemberCollision {
                collision: SchemaNameCollision::IdentityKeyWithIndex {
                    key: "id".into(),
                    index: "byId".into(),
                },
            },
            index("byId"),
        ),
        (
            SchemaErrorKind::UnknownIndexArg {
                index: "byTitle".into(),
                arg: "title".into(),
            },
            index("byTitle"),
        ),
        (
            SchemaErrorKind::UnorderableKey {
                target: SchemaKeyTarget::IdentityKey { name: "id".into() },
                ty: Type::Scalar(ScalarType::Decimal),
            },
            Some(SchemaStoreInvalidation::Store),
        ),
        (
            SchemaErrorKind::UnorderableKey {
                target: SchemaKeyTarget::KeyParam { name: "pos".into() },
                ty: Type::Scalar(ScalarType::Decimal),
            },
            None,
        ),
        (
            SchemaErrorKind::UnorderableKey {
                target: SchemaKeyTarget::IndexArg {
                    index: "byTitle".into(),
                    arg: "title".into(),
                },
                ty: Type::Scalar(ScalarType::Decimal),
            },
            index("byTitle"),
        ),
        (
            SchemaErrorKind::IndexMissingIdentityKeys {
                index: "byTitle".into(),
            },
            index("byTitle"),
        ),
        (
            SchemaErrorKind::IndexRequiresKeyedRoot {
                index: "byTitle".into(),
            },
            index("byTitle"),
        ),
        (
            SchemaErrorKind::NestedIndexArg {
                index: "byTitle".into(),
                arg: "meta".into(),
            },
            index("byTitle"),
        ),
        (
            SchemaErrorKind::NonEnumNamedField {
                field: "status".into(),
                ty: "Status".into(),
            },
            None,
        ),
        (
            SchemaErrorKind::NonScalarKey {
                target: SchemaKeyTarget::IdentityKey { name: "id".into() },
                ty: Type::Unknown,
            },
            Some(SchemaStoreInvalidation::Store),
        ),
        (
            SchemaErrorKind::NonScalarKey {
                target: SchemaKeyTarget::KeyParam { name: "pos".into() },
                ty: Type::Unknown,
            },
            None,
        ),
        (
            SchemaErrorKind::NonScalarKey {
                target: SchemaKeyTarget::IndexArg {
                    index: "byTitle".into(),
                    arg: "title".into(),
                },
                ty: Type::Unknown,
            },
            index("byTitle"),
        ),
    ];

    for (kind, expected) in cases {
        assert_eq!(kind.store_invalidation(), expected, "{kind:?}");
    }
}
