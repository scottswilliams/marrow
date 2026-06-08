//! Resource-constructor and alias resolution in the binding index: a qualified
//! `state::Book(..)` call uses the named module's resource, a bare constructor
//! prefers the current module, and an aliased `book::Id` call, constructor, or
//! type ref resolves to the imported function or resource named `Id` rather than a
//! same-named local trap. Resolved against a cleanly-checked program.

mod support;
mod support_binding;

use marrow_check::binding::SymbolKind;

use support_binding::checked_index;

#[test]
fn qualified_resource_constructor_uses_qualified_module_resource() {
    let state = "module shelf::state\n\
        resource Book at ^state_books(id: int)\n    \
        required title: string\n";
    let app = "module shelf::app\n\
        use shelf::state\n\
        resource Book at ^app_books(code: string)\n    \
        required subtitle: string\n\
        fn make()\n    \
        const b = state::Book(title: \"x\")\n";
    let (index, paths) = checked_index(
        "qualified-resource-constructor",
        &[("src/shelf/state.mw", state), ("src/shelf/app.mw", app)],
    );
    let state_file = &paths[0];
    let app_file = &paths[1];

    let book_leaf = app.find("state::Book").expect("qualified constructor") + "state::".len();
    let def = index
        .definition(app_file, book_leaf + 1)
        .expect("qualified resource constructor resolves");
    assert_eq!(def.kind, SymbolKind::Resource, "{def:?}");
    assert_eq!(def.file, *state_file, "{def:?}");
}

#[test]
fn bare_resource_constructor_prefers_current_module_resource() {
    let state = "module shelf::state\n\
        resource Book at ^state_books(id: int)\n    \
        required title: string\n";
    let app = "module shelf::app\n\
        use shelf::state\n\
        resource Book at ^app_books(code: string)\n    \
        required subtitle: string\n\
        fn make()\n    \
        const b = Book(subtitle: \"b\")\n";
    let (index, paths) = checked_index(
        "bare-resource-constructor-current-module",
        &[("src/shelf/state.mw", state), ("src/shelf/app.mw", app)],
    );
    let app_file = &paths[1];

    let book = app.find("Book(subtitle").expect("bare constructor");
    let def = index
        .definition(app_file, book + 1)
        .expect("bare resource constructor resolves");
    assert_eq!(def.kind, SymbolKind::Resource, "{def:?}");
    assert_eq!(def.file, *app_file, "{def:?}");
}

#[test]
fn alias_qualified_id_call_resolves_to_imported_function_named_id() {
    let imported = "module shelf::book\n\
        pub fn Id(): int\n    \
        return 1\n";
    let trap = "module traps\n\
        resource book at ^local_books(id: int)\n    \
        required title: string\n";
    let app = "module app\n\
        use shelf::book\n\
        fn run(): int\n    \
        return book::Id()\n";
    let (index, paths) = checked_index(
        "alias-id-call-imported-function",
        &[
            ("src/shelf/book.mw", imported),
            ("src/traps.mw", trap),
            ("src/app.mw", app),
        ],
    );
    let imported_file = &paths[0];
    let app_file = &paths[2];

    let call = app.find("book::Id").expect("aliased function call");
    let def = index
        .definition(app_file, call + "book::".len() + 1)
        .expect("aliased call resolves");
    assert_eq!(def.kind, SymbolKind::Function, "{def:?}");
    assert_eq!(def.file, *imported_file, "{def:?}");
}

#[test]
fn alias_qualified_id_call_resolves_to_imported_resource_named_id() {
    let imported = "module shelf::book\n\
        resource Id at ^imported_ids(id: int)\n    \
        required title: string\n";
    let trap = "module traps\n\
        resource book at ^local_books(id: int)\n    \
        required title: string\n";
    let app = "module app\n\
        use shelf::book\n\
        fn make()\n    \
        const value = book::Id(title: \"x\")\n";
    let (index, paths) = checked_index(
        "alias-id-call-imported-resource",
        &[
            ("src/shelf/book.mw", imported),
            ("src/traps.mw", trap),
            ("src/app.mw", app),
        ],
    );
    let imported_file = &paths[0];
    let app_file = &paths[2];

    let call = app.find("book::Id").expect("aliased resource constructor");
    let def = index
        .definition(app_file, call + "book::".len() + 1)
        .expect("aliased constructor resolves");
    assert_eq!(def.kind, SymbolKind::Resource, "{def:?}");
    assert_eq!(def.file, *imported_file, "{def:?}");
}

#[test]
fn alias_qualified_id_type_ref_expands_alias_to_imported_resource() {
    let trap = "module traps\n\
        resource book at ^trap_books(id: int)\n    \
        required title: string\n";
    let imported_module = "module shelf::book\n\
        resource Id at ^imported_ids(id: int)\n    \
        required title: string\n";
    let app = "module app\n\
        use shelf::book\n\
        fn load(value: book::Id)\n    \
        return\n";
    let (index, paths) = checked_index(
        "alias-id-type-ref-imported-resource",
        &[
            ("src/traps.mw", trap),
            ("src/shelf/book.mw", imported_module),
            ("src/app.mw", app),
        ],
    );
    let imported_file = &paths[1];
    let app_file = &paths[2];

    let type_ref = app.find("book::Id").expect("aliased resource type ref");
    let def = index
        .definition(app_file, type_ref + "book::".len() + 1)
        .expect("aliased type ref resolves");
    assert_eq!(def.kind, SymbolKind::Resource, "{def:?}");
    assert_eq!(def.file, *imported_file, "{def:?}");
}
