//! Enum type-annotation references in the binding index: an enum named in a
//! signature, a `sequence[..]` inner type, a resource field, or a qualified path
//! resolves to the enum definition, with the reference span covering only the
//! written enum name. Map-value sugar is a saved keyed-leaf member, not an enum
//! annotation, so it produces a saved field symbol and no enum reference.

mod support;
mod support_binding;

use marrow_check::binding::{RenameSafety, SymbolKind};

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
        resource Order at ^orders(id: int)\n    \
        required state: Status\n";
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
fn map_sugar_produces_a_saved_field_distinct_from_an_enum_annotation() {
    // A map-sugar member and a sibling enum-typed field share the enum name `Status`,
    // but only the direct annotation is an enum reference. The map member is a saved
    // `Field` symbol whose value type is keyed-leaf sugar, not a type annotation, so it
    // contributes no enum reference; the direct `state: Status` field does.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Order at ^orders(id: int)\n    \
        required state: Status\n    \
        scores: map[string, Status]\n";
    let (index, paths) = analyze("enum-annotation-map-vs-field", &[("src/m.mw", source)]);
    let file = &paths[0];

    // The map member resolves to a saved-data-backed `Field` symbol: that is the fact
    // map sugar produces.
    let scores_decl = source.find("scores:").expect("map field decl") + 1;
    let scores = index
        .definition(file, scores_decl)
        .expect("the map member is a saved field symbol");
    assert_eq!(scores.kind, SymbolKind::Field, "{scores:?}");
    assert_eq!(
        index.rename_safety(&scores),
        RenameSafety::SavedDataBacked,
        "{scores:?}",
    );

    // The direct `state: Status` annotation resolves to the enum.
    let state_annotation =
        source.find("state: Status").expect("field annotation") + "state: ".len();
    let enum_def = index
        .definition(file, state_annotation + 1)
        .expect("the direct field annotation resolves to the enum");
    assert_eq!(enum_def.kind, SymbolKind::Enum, "{enum_def:?}");

    // The enum is referenced exactly twice: its declaration and the one direct
    // annotation. The map value position spells `Status` but adds no third reference,
    // so the enum's reference set observes that map sugar is not an annotation.
    let refs = index.references(&enum_def);
    let enum_refs = refs
        .iter()
        .filter(|reference| reference.kind == SymbolKind::Enum)
        .count();
    assert_eq!(
        enum_refs, 2,
        "declaration plus the one direct annotation, not the map value: {refs:?}",
    );
    let annotation_refs = refs
        .iter()
        .filter(|reference| {
            reference.file == *file
                && &source[reference.span.start_byte..reference.span.end_byte] == "Status"
        })
        .count();
    assert_eq!(
        annotation_refs, 1,
        "only the direct annotation spans the written enum name: {refs:?}",
    );

    // Cursor inside the map value `Status` does not resolve to the enum.
    let map_value =
        source.find("map[string, Status]").expect("map annotation") + "map[string, ".len();
    let map_def = index.definition(file, map_value + 1);
    assert!(
        !matches!(map_def, Some(ref def) if def.kind == SymbolKind::Enum),
        "map sugar value type is not an enum reference: {map_def:?}",
    );
}
