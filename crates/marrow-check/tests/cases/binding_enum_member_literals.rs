//! Enum-member literal resolution in the binding index: a `Status::archived`
//! literal resolves each qualified segment to its own enum or member definition,
//! tolerating trivia, nested member paths, and anchoring intermediate segments to
//! the right category. Exercises the same analysis path editor tooling uses.
use crate::support_binding;
use marrow_check::binding::SymbolKind;

use support_binding::{analyze, assert_def_covers_member};

#[test]
fn an_enum_member_literal_resolves_to_the_member_definition() {
    // `Status::archived` names the `archived` enum member, not an unresolved
    // qualified value path. References stay per-member, so `active` is separate.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn archived(): bool\n    \
        return Status::archived == Status::active\n";
    let (index, paths) = analyze("enum-member-literal", &[("src/m.mw", source)]);
    let file = &paths[0];

    let use_offset = source
        .rfind("Status::archived")
        .expect("archived member use")
        + "Status::".len();
    let def = index
        .definition(file, use_offset)
        .expect("enum member literal resolves");
    assert_def_covers_member(&def, source, "archived\n");

    let member_decl = source.find("archived\n").expect("archived declaration");
    let refs = index.references(&def);
    assert!(
        refs.iter()
            .any(|reference| reference.span.start_byte <= member_decl
                && member_decl <= reference.span.end_byte),
        "member declaration is a reference: {refs:?}",
    );
    assert!(
        refs.iter()
            .any(|reference| reference.span.start_byte <= use_offset
                && use_offset <= reference.span.end_byte),
        "member literal use is a reference: {refs:?}",
    );
    let active_use = source.rfind("Status::active").expect("active member use") + "Status::".len();
    assert!(
        !refs
            .iter()
            .any(|reference| reference.span.start_byte <= active_use
                && active_use <= reference.span.end_byte),
        "`active` use must not be attributed to `archived`: {refs:?}",
    );
}

#[test]
fn an_enum_member_literal_resolves_each_qualified_segment() {
    // `Status::archived` names both the enum prefix and the member leaf. The
    // cursor should resolve to the segment it is actually on.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn archived(): bool\n    \
        return Status::archived\n";
    let (index, paths) = analyze("enum-member-segments", &[("src/m.mw", source)]);
    let file = &paths[0];

    let literal = source
        .rfind("Status::archived")
        .expect("archived member use");
    let enum_def = index
        .definition(file, literal + 1)
        .expect("enum prefix resolves");
    assert_eq!(enum_def.kind, SymbolKind::Enum, "{enum_def:?}");

    let member_def = index
        .definition(file, literal + "Status::".len() + 1)
        .expect("member segment resolves");
    assert_eq!(member_def.kind, SymbolKind::EnumMember, "{member_def:?}");
}

#[test]
fn an_enum_member_literal_with_trivia_resolves_written_segments() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn archived(): bool\n    \
        return Status :: archived\n";
    let (index, paths) = analyze("enum-member-trivia-segments", &[("src/m.mw", source)]);
    let file = &paths[0];

    let literal = source
        .rfind("Status :: archived")
        .expect("archived member use");
    let enum_def = index
        .definition(file, literal + 1)
        .expect("enum prefix resolves");
    assert_eq!(enum_def.kind, SymbolKind::Enum, "{enum_def:?}");

    let member_start = literal + "Status :: ".len();
    let member_def = index
        .definition(file, member_start + "archived".len() - 1)
        .expect("member segment resolves at the end of the token");
    assert_eq!(member_def.kind, SymbolKind::EnumMember, "{member_def:?}");

    let refs = index.references(&member_def);
    assert!(
        refs.iter().any(
            |reference| &source[reference.span.start_byte..reference.span.end_byte] == "archived"
        ),
        "member reference span should cover the written identifier: {refs:?}",
    );
}

#[test]
fn a_nested_enum_member_literal_resolves_each_member_path_segment() {
    let source = "module m\n\
        enum Cat\n    \
        category tiger\n        \
        bengal\n\
        fn favorite(): Cat\n    \
        return Cat::tiger::bengal\n";
    let (index, paths) = analyze("enum-nested-member-segments", &[("src/m.mw", source)]);
    let file = &paths[0];

    let literal = source
        .rfind("Cat::tiger::bengal")
        .expect("nested member use");
    let enum_def = index
        .definition(file, literal + 1)
        .expect("enum prefix resolves");
    assert_eq!(enum_def.kind, SymbolKind::Enum, "{enum_def:?}");

    let category_def = index
        .definition(file, literal + "Cat::".len() + 1)
        .expect("category segment resolves");
    assert_eq!(
        category_def.kind,
        SymbolKind::EnumMember,
        "{category_def:?}"
    );

    let leaf_def = index
        .definition(file, literal + "Cat::tiger::".len() + 1)
        .expect("leaf segment resolves");
    assert_eq!(leaf_def.kind, SymbolKind::EnumMember, "{leaf_def:?}");
    assert_ne!(
        category_def.span, leaf_def.span,
        "category and leaf segments should resolve to their own definitions",
    );
}

#[test]
fn a_nested_enum_member_literal_anchors_intermediate_segments() {
    let source = "module m\n\
        enum Cat\n    \
        category tiger\n        \
        bengal\n    \
        category lion\n        \
        tiger\n\
        fn favorite(): Cat\n    \
        return Cat::tiger::bengal\n";
    let (index, paths) = analyze("enum-nested-member-anchored", &[("src/m.mw", source)]);
    let file = &paths[0];

    let literal = source
        .rfind("Cat::tiger::bengal")
        .expect("nested member use");
    let category_def = index
        .definition(file, literal + "Cat::".len() + 1)
        .expect("top-level category segment resolves");
    assert_eq!(
        category_def.kind,
        SymbolKind::EnumMember,
        "{category_def:?}"
    );

    let top_level_category = source
        .find("tiger\n        bengal")
        .expect("top-level tiger category");
    assert!(
        category_def.span.start_byte <= top_level_category
            && top_level_category <= category_def.span.end_byte,
        "category segment should resolve to the anchored top-level category: {category_def:?}",
    );
}

#[test]
fn an_ambiguous_foreign_enum_owner_member_literal_has_no_binding_definition() {
    let status_a = "module a\npub enum Status\n    active\n";
    let status_b = "module b\npub enum Status\n    active\n";
    let app = "module app\nfn f(): a::Status\n    return Status::active\n";
    let (index, paths) = analyze(
        "enum-member-literal-ambiguous-foreign-owner",
        &[
            ("src/a.mw", status_a),
            ("src/b.mw", status_b),
            ("src/app.mw", app),
        ],
    );
    let app_file = &paths[2];
    let literal = app.find("Status::active").expect("ambiguous literal");

    assert!(
        index.definition(app_file, literal + 1).is_none(),
        "ambiguous enum owner must not bind the enum segment to an arbitrary definition"
    );
    assert!(
        index
            .definition(app_file, literal + "Status::".len() + 1)
            .is_none(),
        "ambiguous enum owner must not bind the member segment to an arbitrary definition"
    );
}

#[test]
fn a_qualified_enum_owner_member_literal_resolves_before_bare_owner_ambiguity() {
    let status_a = "module a\npub enum Status\n    active\n";
    let status_b = "module b\npub enum Status\n    active\n";
    let choice = "module Status\npub enum Choice\n    active\n";
    let app = "module app\nfn f(): Status::Choice\n    return Status::Choice::active\n";
    let (index, paths) = analyze(
        "enum-member-literal-qualified-owner-before-ambiguous-bare",
        &[
            ("src/a.mw", status_a),
            ("src/b.mw", status_b),
            ("src/Status.mw", choice),
            ("src/app.mw", app),
        ],
    );
    let app_file = &paths[3];
    let literal = app
        .find("Status::Choice::active")
        .expect("qualified member literal");

    assert!(
        index.definition(app_file, literal + 1).is_none(),
        "module qualifier must not bind to an arbitrary foreign enum"
    );
    let enum_def = index
        .definition(app_file, literal + "Status::".len() + 1)
        .expect("qualified enum owner resolves");
    assert_eq!(enum_def.kind, SymbolKind::Enum, "{enum_def:?}");
    let member_def = index
        .definition(app_file, literal + "Status::Choice::".len() + 1)
        .expect("qualified enum member resolves");
    assert_eq!(member_def.kind, SymbolKind::EnumMember, "{member_def:?}");
}

#[test]
fn a_qualified_enum_owner_member_literal_resolves_before_same_module_bare_owner() {
    let choice = "module Status\npub enum Choice\n    active\n";
    let app = "module app\n\
        enum Status\n    local\n\
        fn f(): Status::Choice\n    return Status::Choice::active\n";
    let (index, paths) = analyze(
        "enum-member-literal-qualified-owner-before-local-bare",
        &[("src/Status.mw", choice), ("src/app.mw", app)],
    );
    let app_file = &paths[1];
    let literal = app
        .find("Status::Choice::active")
        .expect("qualified member literal");

    assert!(
        index.definition(app_file, literal + 1).is_none(),
        "module qualifier must not bind to the same-module enum"
    );
    let enum_def = index
        .definition(app_file, literal + "Status::".len() + 1)
        .expect("qualified enum owner resolves");
    assert_eq!(enum_def.kind, SymbolKind::Enum, "{enum_def:?}");
    let member_def = index
        .definition(app_file, literal + "Status::Choice::".len() + 1)
        .expect("qualified enum member resolves");
    assert_eq!(member_def.kind, SymbolKind::EnumMember, "{member_def:?}");
}
