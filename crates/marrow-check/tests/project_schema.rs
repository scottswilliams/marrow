mod support;

use marrow_check::{CheckDiagnostic, DiagnosticPayload, RejectedSurface, check_project};
use marrow_schema::{SchemaErrorKind, SchemaKeyTarget, Type};

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
             resource Order at ^orders(state: Status)\n\
             \x20   required note: string\n",
            identity("state"),
            "Status",
        ),
        (
            "enum-layer-key",
            "module m\n\
             enum Status\n\
             \x20   active\n\
             \x20   archived\n\
             resource Order at ^orders(id: int)\n\
             \x20   byState(state: Status): string\n",
            key_param("state"),
            "Status",
        ),
        (
            "typo-identity-key",
            "module m\n\
             resource Order at ^orders(state: Stutus)\n\
             \x20   required note: string\n",
            identity("state"),
            "Stutus",
        ),
        (
            "typo-layer-key",
            "module m\n\
             resource Order at ^orders(id: int)\n\
             \x20   byState(state: Stutus): string\n",
            key_param("state"),
            "Stutus",
        ),
        (
            "resource-identity-key",
            "module m\n\
             resource Person\n\
             \x20   required name: string\n\
             resource Order at ^orders(owner: Person)\n\
             \x20   required note: string\n",
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
             resource Order at ^orders(state: a::Status)\n    required note: string\n",
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
         resource Order at ^orders(id: int)\n\
         \x20   tags: sequence[string]\n\
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
         resource Order at ^orders(id: int)\n\
         \x20   state: Status\n\
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
         resource Order at ^orders(id: int)\n\
         \x20   required state: Status\n\
         \x20   byTag(tag: string): string\n",
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
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             resource Tome at ^books(id: int)\n\
             \x20   required title: string\n",
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
fn saved_inout_through_resource_reference_is_rejected() {
    let report = check_module_report(
        "saved-inout-resource-reference",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn normalize(inout book: Book)\n    return\n\
         fn f(id: int)\n    var local = Book(title: \"local\")\n    normalize(inout local)\n    normalize(inout ^books(id))\n",
    );

    let found = with_code(&report, "check.rejected_surface");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::RejectedSurface(RejectedSurface::SavedInout),
        "{found:#?}"
    );
}

#[test]
fn saved_inout_through_index_entry_is_rejected_surface() {
    let report = check_module_report(
        "rejected-index-inout",
        "module m\n\
         resource Book at ^books(id: int)\n    shelf: string\n    index byShelf(shelf, id)\n\n\
         fn touch(inout id: Id(^books))\n    return\n\
         fn f(id: int)\n    touch(inout ^books.byShelf(\"fiction\")(id))\n",
    );

    let found = with_code(&report, "check.rejected_surface");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::RejectedSurface(RejectedSurface::SavedInout),
        "{found:#?}"
    );
}

#[test]
fn malformed_saved_inout_through_keyed_root_field_is_rejected() {
    let report = check_module_report(
        "malformed-saved-inout-keyed-root-field",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn touch(inout value: unknown)\n    value = \"x\"\n\
         fn f()\n    touch(inout ^books.title)\n",
    );

    let found = with_code(&report, "check.rejected_surface");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::RejectedSurface(RejectedSurface::SavedInout),
        "{found:#?}"
    );
}

#[test]
fn malformed_saved_inout_through_index_branch_is_rejected() {
    let report = check_module_report(
        "malformed-saved-inout-index-branch",
        "module m\n\
         resource Book at ^books(id: int)\n    shelf: string\n    index byShelf(shelf, id)\n\n\
         fn touch(inout value: unknown)\n    value = \"x\"\n\
         fn f()\n    touch(inout ^books.byShelf)\n",
    );

    let found = with_code(&report, "check.rejected_surface");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::RejectedSurface(RejectedSurface::SavedInout),
        "{found:#?}"
    );
}

#[test]
fn old_saved_traversal_method_shapers_are_rejected() {
    let report = check_module_report(
        "rejected-saved-traversal-shapers",
        "module m\n\
         resource Book at ^books(id: int)\n    shelf: string\n    index byShelf(shelf, id)\n\n\
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
         resource Book at ^books(id: int)\n    shelf: string\n    window(pos: int): string\n    index take(shelf, id)\n\n\
         fn f(id: Id(^books))\n    \
         ^books(id).window(1) = \"open\"\n    \
         for found in ^books.take(\"fiction\")\n        var typed: Id(^books) = found\n",
    );

    let found = with_code(&report, "check.rejected_surface");
    assert!(found.is_empty(), "{:#?}", report.diagnostics);
    assert_clean(&report);
}
