mod support;

use marrow_check::{CheckDiagnostic, DiagnosticPayload, RejectedSurface, check_project};
use marrow_schema::{NodeKind, ScalarType, SchemaErrorKind, SchemaKeyTarget, Type};

use support::{
    assert_clean, check_module, check_module_report, config, temp_project, with_code, write,
};

fn assert_schema_payload(diagnostic: &CheckDiagnostic, expected: SchemaErrorKind) {
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::Schema(expected),
        "{diagnostic:#?}"
    );
}

#[test]
fn split_store_applies_saved_field_schema_rules() {
    let errors = check_module(
        "split-store-saved-field",
        "module m\n\
         resource Author\n\
         \x20   required name: string\n\
         resource Book\n\
         \x20   author: Author\n\
         store ^books(id: int): Book\n",
        "schema.non_enum_named_field",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_schema_payload(
        &errors[0],
        SchemaErrorKind::NonEnumNamedField {
            field: "author".to_string(),
            ty: "Author".to_string(),
        },
    );
}

#[test]
fn typed_keyed_resource_layer_compiles_as_group_entry() {
    let root = temp_project("keyed-resource-field-schema", |root| {
        write(
            root,
            "src/blog.mw",
            "module blog\n\
             resource Comment\n\
             \x20   required body: string\n\
             \x20   meta\n\
             \x20       author: string\n\
             resource Post\n\
             \x20   required title: string\n\
             \x20   comments(seq: int): Comment\n\
             store ^posts(id: int): Post\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);

    let post = program.modules[0]
        .resources
        .iter()
        .find(|resource| resource.name == "Post")
        .expect("Post resource");
    let comments = post
        .members
        .iter()
        .find(|member| member.name == "comments")
        .expect("comments member");

    assert!(matches!(comments.kind, NodeKind::Group), "{comments:#?}");
    assert_eq!(comments.key_params.len(), 1, "{comments:#?}");
    assert_eq!(comments.key_params[0].name, "seq");
    assert_eq!(
        comments.key_params[0].ty,
        Type::Scalar(ScalarType::Int),
        "{comments:#?}"
    );
    assert!(
        comments.members.iter().any(|member| {
            member.name == "body" && matches!(member.kind, NodeKind::Slot { required: true, .. })
        }),
        "{comments:#?}"
    );
    let meta = comments
        .members
        .iter()
        .find(|member| member.name == "meta")
        .expect("meta group");
    assert!(matches!(meta.kind, NodeKind::Group), "{meta:#?}");
    assert!(
        meta.members.iter().any(|member| {
            member.name == "author" && matches!(member.kind, NodeKind::Slot { .. })
        }),
        "{meta:#?}"
    );
}

#[test]
fn typed_keyed_resource_layer_normalizes_nested_typed_layers() {
    let root = temp_project("keyed-resource-field-nested-schema", |root| {
        write(
            root,
            "src/blog.mw",
            "module blog\n\
             resource Reply\n\
             \x20   required body: string\n\
             resource Comment\n\
             \x20   required body: string\n\
             \x20   replies(seq: int): Reply\n\
             resource Post\n\
             \x20   comments(seq: int): Comment\n\
             store ^posts(id: int): Post\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);

    let post = program.modules[0]
        .resources
        .iter()
        .find(|resource| resource.name == "Post")
        .expect("Post resource");
    let comments = post
        .members
        .iter()
        .find(|member| member.name == "comments")
        .expect("comments member");
    let replies = comments
        .members
        .iter()
        .find(|member| member.name == "replies")
        .expect("replies member");

    assert!(matches!(replies.kind, NodeKind::Group), "{replies:#?}");
    assert!(
        replies.members.iter().any(|member| {
            member.name == "body" && matches!(member.kind, NodeKind::Slot { required: true, .. })
        }),
        "{replies:#?}"
    );
}

#[test]
fn typed_keyed_resource_layer_resolves_import_alias() {
    let root = temp_project("keyed-resource-field-import-alias", |root| {
        write(
            root,
            "src/blog/comments.mw",
            "module blog::comments\n\
             resource Comment\n\
             \x20   required body: string\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             use blog::comments\n\
             resource Post\n\
             \x20   comments(seq: int): comments::Comment\n\
             store ^posts(id: int): Post\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);

    let post = program
        .modules
        .iter()
        .find(|module| module.name == "app")
        .and_then(|module| {
            module
                .resources
                .iter()
                .find(|resource| resource.name == "Post")
        })
        .expect("Post resource");
    let comments = post
        .members
        .iter()
        .find(|member| member.name == "comments")
        .expect("comments member");

    assert!(matches!(comments.kind, NodeKind::Group), "{comments:#?}");
    assert_eq!(
        comments.entry_type,
        Some(Type::Named("blog::comments::Comment".into()))
    );
}

#[test]
fn typed_keyed_resource_layer_validates_entry_resource_plain_fields() {
    let errors = check_module(
        "keyed-resource-field-validates-entry-resource",
        "module m\n\
         resource Author\n\
         \x20   required name: string\n\
         resource Comment\n\
         \x20   author: Author\n\
         resource Post\n\
         \x20   comments(seq: int): Comment\n\
         store ^posts(id: int): Post\n",
        "schema.non_enum_named_field",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_schema_payload(
        &errors[0],
        SchemaErrorKind::NonEnumNamedField {
            field: "author".to_string(),
            ty: "Author".to_string(),
        },
    );
}

#[test]
fn typed_keyed_resource_layer_rejects_recursive_entry_resource_once() {
    let errors = check_module(
        "keyed-resource-field-recursive-entry",
        "module m\n\
         resource Comment\n\
         \x20   required body: string\n\
         \x20   replies(seq: int): Comment\n\
         resource Post\n\
         \x20   comments(seq: int): Comment\n\
         store ^posts(id: int): Post\n",
        "check.recursive_keyed_entry",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

/// A key must be an orderable scalar; any named type in a key position is rejected
/// structurally, without resolving the name. Each row drives the same invariant from a
/// distinct named-key source: an enum (names no scalar), a typo (resolves to nothing,
/// so it could otherwise accept any value), and a declared resource (a real type that is
/// still not a scalar) — in both the identity-key and keyed-layer-param positions.
#[test]
fn rejects_a_named_type_in_a_key_position() {
    let identity = |name: &str| SchemaKeyTarget::IdentityKey {
        name: name.to_string(),
    };
    let key_param = |name: &str| SchemaKeyTarget::KeyParam {
        name: name.to_string(),
    };
    let cases: &[(&str, &str, SchemaKeyTarget, &str)] = &[
        (
            "enum-identity-key",
            "module m\n\
             enum Status\n\
             \x20   active\n\
             \x20   archived\n\
             resource Order\n\
             \x20   required note: string\n\
             store ^orders(state: Status): Order\n",
            identity("state"),
            "Status",
        ),
        (
            "enum-layer-key",
            "module m\n\
             enum Status\n\
             \x20   active\n\
             \x20   archived\n\
             resource Order\n\
             \x20   byState(state: Status): string\n\
             store ^orders(id: int): Order\n",
            key_param("state"),
            "Status",
        ),
        (
            "typo-identity-key",
            "module m\n\
             resource Order\n\
             \x20   required note: string\n\
             store ^orders(state: Stutus): Order\n",
            identity("state"),
            "Stutus",
        ),
        (
            "typo-layer-key",
            "module m\n\
             resource Order\n\
             \x20   byState(state: Stutus): string\n\
             store ^orders(id: int): Order\n",
            key_param("state"),
            "Stutus",
        ),
        (
            "resource-identity-key",
            "module m\n\
             resource Person\n\
             \x20   required name: string\n\
             resource Order\n\
             \x20   required note: string\n\
             store ^orders(owner: Person): Order\n",
            identity("owner"),
            "Person",
        ),
    ];

    for (name, source, target, ty_name) in cases {
        let errors = check_module(name, source, "schema.nonscalar_key");
        assert_eq!(errors.len(), 1, "{name}: {errors:#?}");
        assert_schema_payload(
            &errors[0],
            SchemaErrorKind::NonScalarKey {
                target: target.clone(),
                ty: Type::Named(ty_name.to_string()),
            },
        );
    }
}

#[test]
fn typed_keyed_resource_layer_rejects_unknown_resource_type() {
    let errors = check_module(
        "keyed-resource-field-typo",
        "module m\n\
         resource Comment\n\
         \x20   required body: string\n\
         resource Post\n\
         \x20   comments(seq: int): Commet\n\
         store ^posts(id: int): Post\n",
        "check.unknown_type",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(
        errors[0].payload,
        DiagnosticPayload::UnknownType(Type::Named("Commet".into())),
        "{errors:#?}"
    );
}

#[test]
fn typed_keyed_enum_leaf_rejects_private_cross_module_enum() {
    let root = temp_project("keyed-enum-leaf-private-cross-module", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             enum Hidden\n\
             \x20   one\n\
             \x20   two\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             use a\n\
             resource Post\n\
             \x20   statuses(seq: int): a::Hidden\n\
             store ^posts(id: int): Post\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.private_enum");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::PrivateEnum("a::Hidden".into())
    );
    assert!(
        with_code(&report, "check.unknown_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn typed_keyed_leaf_rejects_unknown_named_type_inside_sequence_value() {
    let errors = check_module(
        "keyed-leaf-sequence-unknown-type",
        "module m\n\
         resource Post\n\
         \x20   tags(seq: int): sequence[Missing]\n\
         store ^posts(id: int): Post\n",
        "check.unknown_type",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(
        errors[0].payload,
        DiagnosticPayload::UnknownType(Type::Sequence(Box::new(Type::Named("Missing".into())))),
        "{errors:#?}"
    );
}

#[test]
fn typed_keyed_leaf_rejects_checker_only_error_value_type() {
    let cases = [
        (
            "keyed-leaf-error-value-type",
            "failures(seq: int): Error",
            "failures",
            "Error",
        ),
        (
            "keyed-leaf-sequence-error-value-type",
            "failures(seq: int): sequence[Error]",
            "failures",
            "Error",
        ),
        (
            "keyed-leaf-sequence-resource-value-type",
            "comments(seq: int): sequence[Comment]",
            "comments",
            "Comment",
        ),
    ];
    for (name, member, field, ty) in cases {
        let errors = check_module(
            name,
            &format!(
                "module m\n\
                 resource Comment\n\
                 \x20   required body: string\n\
                 resource Post\n\
                 \x20   {member}\n\
                 store ^posts(id: int): Post\n"
            ),
            "schema.non_enum_named_field",
        );
        assert_eq!(errors.len(), 1, "{name}: {errors:#?}");
        assert_schema_payload(
            &errors[0],
            SchemaErrorKind::NonEnumNamedField {
                field: field.to_string(),
                ty: ty.to_string(),
            },
        );
    }
}

#[test]
fn rejects_a_cross_module_qualified_enum_identity_key() {
    // A qualified `a::Status` key is structurally a non-scalar name, so it is
    // rejected without resolving which module owns it. This is the case a
    // file-local enum list could never reach.
    let root = temp_project("cross-module-enum-key", |root| {
        write(
            root,
            "src/a.mw",
            "module a\nenum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\n\
             resource Order\n    required note: string\n\
             store ^orders(state: a::Status): Order\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "schema.nonscalar_key");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_schema_payload(
        found[0],
        SchemaErrorKind::NonScalarKey {
            target: SchemaKeyTarget::IdentityKey {
                name: "state".to_string(),
            },
            ty: Type::Named("a::Status".to_string()),
        },
    );
}

#[test]
fn rejects_a_sequence_index_argument() {
    // A sequence member is a keyed layer, not a top-level scalar field, so an
    // index cannot name it as an argument.
    let errors = check_module(
        "sequence-index-arg",
        "module m\n\
         resource Order\n\
         \x20   tags: sequence[string]\n\
         store ^orders(id: int): Order\n\
         \x20   index byTags(tags, id)\n",
        "schema.unknown_index_arg",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_schema_payload(
        &errors[0],
        SchemaErrorKind::UnknownIndexArg {
            index: "byTags".to_string(),
            arg: "tags".to_string(),
        },
    );
}

#[test]
fn an_enum_field_index_argument_checks_clean() {
    let report = check_module_report(
        "enum-index-ok",
        "module m\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Order\n\
         \x20   state: Status\n\
         store ^orders(id: int): Order\n\
         \x20   index byState(state, id)\n",
    );
    assert_clean(&report);
}

#[test]
fn an_orderable_scalar_key_checks_clean() {
    // The allowlist does not over-reject an orderable scalar key alongside a
    // declared enum field on the same resource.
    let report = check_module_report(
        "scalar-key-ok",
        "module m\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Order\n\
         \x20   required state: Status\n\
         \x20   byTag(tag: string): string\n\
         store ^orders(id: int): Order\n",
    );
    assert_clean(&report);
}

#[test]
fn reports_two_stores_sharing_one_saved_root() {
    let root = temp_project("dup-root", |root| {
        // A saved root has one managed owner; two stores on `^books` collide.
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             resource Tome\n\
             \x20   required title: string\n\
             store ^books(id: int): Tome\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let owners = with_code(&report, "schema.duplicate_root_owner");
    assert_eq!(owners.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        owners[0].payload,
        DiagnosticPayload::DuplicateRootOwner {
            root: "books".into(),
            first_owner: root.join("src/shelf.mw"),
        },
        "{:#?}",
        owners[0]
    );
}

#[test]
fn split_store_may_precede_the_resource_shape() {
    let root = temp_project("store-before-resource", |root| {
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             store ^books(id: int): Book\n\
             \x20   index byTitle(title, id)\n\
             resource Book\n\
             \x20   title: string\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
    let module = program.facts.module_id("shelf").expect("shelf module");
    let resource = program.facts.resource_id(module, "Book").expect("Book");
    let store = program
        .facts
        .store_id(module, "books")
        .expect("books store");
    assert_eq!(program.facts.store(store).resource, resource);
}

#[test]
fn id_of_store_is_the_canonical_reference_type() {
    let found = check_module(
        "store-id-reference",
        "module m\n\
         resource Author\n    name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n    authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\n\
         fn put()\n    ^books(1).authorId = nextId(^authors)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn old_saved_traversal_method_shapers_are_rejected() {
    let report = check_module_report(
        "rejected-saved-traversal-shapers",
        "module m\n\
         resource Book\n    shelf: string\n\
         store ^books(id: int): Book\n    index byShelf(shelf, id)\n\n\
         fn f(token: string)\n    \
         for id in ^books.take(10)\n        print($\"{id}\")\n    \
         for id in ^books.byShelf(\"fiction\").window(size: 10)\n        print($\"{id}\")\n    \
         for id in ^books.byShelf(\"fiction\").after(1)\n        print($\"{id}\")\n    \
         for id in ^books.byShelf(\"fiction\").from(1)\n        print($\"{id}\")\n    \
         for id in ^books.byShelf(\"fiction\").until(100)\n        print($\"{id}\")\n    \
         for id in ^books.byShelf(\"fiction\").resume(token)\n        print($\"{id}\")\n    \
         for id in ^books.byShelf(\"fiction\").reverse()\n        print($\"{id}\")\n",
    );

    let found = with_code(&report, "check.rejected_surface");
    assert_eq!(found.len(), 7, "{:#?}", report.diagnostics);
    let methods: Vec<&str> = found
        .iter()
        .map(|diagnostic| match &diagnostic.payload {
            DiagnosticPayload::RejectedSurface(RejectedSurface::SavedTraversalMethod {
                method,
            }) => method.as_str(),
            payload => panic!("expected saved traversal method payload, found {payload:#?}"),
        })
        .collect();
    assert_eq!(
        methods,
        [
            "take", "window", "after", "from", "until", "resume", "reverse",
        ],
        "{found:#?}"
    );
}

#[test]
fn declared_saved_members_named_like_traversal_shapers_are_not_rejected() {
    let report = check_module_report(
        "declared-traversal-shaped-names",
        "module m\n\
         resource Book\n    shelf: string\n    window(pos: int): string\n\
         store ^books(id: int): Book\n    index take(shelf, id)\n\n\
         fn f(id: Id(^books))\n    \
         ^books(id).window(1) = \"open\"\n    \
         for found in ^books.take(\"fiction\")\n        var typed: Id(^books) = found\n",
    );

    let found = with_code(&report, "check.rejected_surface");
    assert!(found.is_empty(), "{:#?}", report.diagnostics);
    assert_clean(&report);
}
