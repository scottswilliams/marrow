//! Enum type-annotation references in the binding index: an enum named in a
//! signature, a `sequence[..]` inner type, a resource field, or a qualified path
//! resolves to the enum definition, with the reference span covering only the
//! written enum name.
use crate::support_binding;
use marrow_check::binding::SymbolKind;

use support_binding::analyze;

#[test]
fn enum_type_annotations_in_function_signature_reference_the_enum() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn set(status: Status): Status\n    \
        return status\n";
    let (index, paths) = analyze(
        "enum-annotation-function-signature",
        &[("src/m.mw", source)],
    );
    let file = &paths[0];

    let param_type = source.find("status: Status").expect("param type") + "status: ".len();
    let def = index
        .definition(file, param_type + 1)
        .expect("parameter annotation resolves to the enum");
    assert_eq!(def.kind, SymbolKind::Enum, "{def:?}");

    let return_type = source.find("): Status").expect("return type") + "): ".len();
    let return_def = index
        .definition(file, return_type + 1)
        .expect("return annotation resolves to the enum");
    assert_eq!(return_def, def, "{return_def:?}");

    let refs = index.references(&def);
    assert_eq!(
        refs.iter()
            .filter(|reference| reference.kind == SymbolKind::Enum)
            .count(),
        3,
        "declaration plus both signature annotations are enum references: {refs:?}",
    );
    assert!(
        refs.iter().any(|reference| reference.file == *file
            && &source[reference.span.start_byte..reference.span.end_byte] == "Status"),
        "annotation reference spans should cover the written type name: {refs:?}",
    );
}

#[test]
fn enum_type_annotation_inside_sequence_references_the_inner_enum() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn set(items: sequence[Status])\n    \
        return\n";
    let (index, paths) = analyze("enum-annotation-sequence", &[("src/m.mw", source)]);
    let file = &paths[0];

    let status = source
        .find("sequence[Status]")
        .expect("sequence annotation")
        + "sequence[".len();
    let def = index
        .definition(file, status + 1)
        .expect("inner sequence enum type resolves");
    assert_eq!(def.kind, SymbolKind::Enum, "{def:?}");

    let refs = index.references(&def);
    assert!(
        refs.iter().any(
            |reference| &source[reference.span.start_byte..reference.span.end_byte] == "Status"
        ),
        "the reference span should cover only the inner enum name: {refs:?}",
    );
    assert!(
        !refs.iter().any(
            |reference| &source[reference.span.start_byte..reference.span.end_byte]
                == "sequence[Status]"
        ),
        "the sequence wrapper should not be recorded as the enum reference: {refs:?}",
    );
}

#[test]
fn enum_type_annotation_on_resource_field_references_the_enum() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Order\n    \
        required state: Status\n\
        store ^orders(id: int): Order\n";
    let (index, paths) = analyze("enum-annotation-resource-field", &[("src/m.mw", source)]);
    let file = &paths[0];

    let status = source.find("state: Status").expect("field annotation") + "state: ".len();
    let def = index
        .definition(file, status + 1)
        .expect("resource field annotation resolves");
    assert_eq!(def.kind, SymbolKind::Enum, "{def:?}");

    let refs = index.references(&def);
    assert!(
        refs.iter().any(
            |reference| &source[reference.span.start_byte..reference.span.end_byte] == "Status"
        ),
        "resource field annotation should be an enum reference: {refs:?}",
    );
}

#[test]
fn qualified_enum_type_annotation_references_the_leaf_enum() {
    let status_module = "module a::b::c\npub enum Status\n    active\n    archived\n";
    let app = "module app\n\
        use a::b::c\n\
        fn set(s: c::Status): a::b::c::Status\n    \
        return s\n";
    let (index, paths) = analyze(
        "enum-annotation-qualified",
        &[("src/a/b/c.mw", status_module), ("src/app.mw", app)],
    );
    let status_file = &paths[0];
    let app_file = &paths[1];

    let aliased_leaf = app.find("c::Status").expect("aliased annotation") + "c::".len();
    let def = index
        .definition(app_file, aliased_leaf + 1)
        .expect("aliased enum annotation resolves");
    assert_eq!(def.kind, SymbolKind::Enum, "{def:?}");
    assert_eq!(def.file, *status_file, "{def:?}");

    let full_leaf = app
        .find("a::b::c::Status")
        .expect("fully-qualified annotation")
        + "a::b::c::".len();
    let full_def = index
        .definition(app_file, full_leaf + 1)
        .expect("fully-qualified enum annotation resolves");
    assert_eq!(full_def, def, "{full_def:?}");

    let refs = index.references(&def);
    assert!(
        refs.iter().any(|reference| reference.file == *app_file
            && &app[reference.span.start_byte..reference.span.end_byte] == "Status"),
        "qualified annotation reference span should cover only the leaf Status: {refs:?}",
    );
    assert!(
        !refs.iter().any(|reference| reference.file == *app_file
            && &app[reference.span.start_byte..reference.span.end_byte] == "c::Status"),
        "the qualifier should not be part of the enum reference: {refs:?}",
    );
}

#[test]
fn ambiguous_bare_foreign_enum_annotation_has_no_binding_definition() {
    let status_a = "module a\npub enum Status\n    active\n";
    let status_b = "module b\npub enum Status\n    active\n";
    let app = "module app\nfn set(s: Status)\n    return\n";
    let (index, paths) = analyze(
        "enum-annotation-ambiguous-bare-foreign",
        &[
            ("src/a.mw", status_a),
            ("src/b.mw", status_b),
            ("src/app.mw", app),
        ],
    );
    let app_file = &paths[2];
    let status = app.find("s: Status").expect("ambiguous annotation") + "s: ".len();

    assert!(
        index.definition(app_file, status + 1).is_none(),
        "ambiguous bare enum annotation must not bind to an arbitrary enum"
    );
}

#[test]
fn resource_type_annotation_does_not_bind_to_foreign_enum() {
    let foreign = "module a\npub enum Order\n    active\n";
    let app = "module app\n\
        resource Order\n    \
        title: string\n\
        fn set(o: Order)\n    \
        return\n";
    let (index, paths) = analyze(
        "enum-annotation-resource-shadows-foreign-enum",
        &[("src/a.mw", foreign), ("src/app.mw", app)],
    );
    let app_file = &paths[1];
    let order = app.find("o: Order").expect("resource annotation") + "o: ".len();

    let def = index
        .definition(app_file, order + 1)
        .expect("resource annotation should resolve");
    assert_eq!(def.kind, SymbolKind::Resource, "{def:?}");
    assert_eq!(def.file, *app_file, "{def:?}");
}
