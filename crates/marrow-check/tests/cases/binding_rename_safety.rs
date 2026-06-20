//! Rename-safety classification in the binding index: a saved resource field —
//! whether declared inline or through a split `store` — is `SavedDataBacked`
//! because its name encodes the on-disk path, while a source-only symbol such as a
//! function parameter is `SourceOnly` and safe to rename.
use crate::support;
use crate::support_binding;
use std::path::Path;

use marrow_check::binding::{RenameSafety, SourceEdit, SymbolKind};

use support_binding::analyze;

#[test]
fn a_saved_field_name_is_saved_data_backed_and_unsafe() {
    // `title` is a stored field of the saved `Book` resource; its on-disk path is
    // `^books(id).title`, so renaming the source name orphans saved data.
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn peek(id: int): string\n    \
        return ^books(id).title ?? \"\"\n";
    let (index, paths) = analyze("safety-saved-field", &[("src/m.mw", source)]);
    let file = &paths[0];

    // Cursor on the field declaration `required title: string`.
    let decl_offset = source.find("title: string").expect("field decl") + 1;
    let def = index
        .definition(file, decl_offset)
        .expect("the field declaration is a symbol");
    assert_eq!(def.kind, SymbolKind::Field, "{def:?}");
    assert!(
        matches!(index.rename_safety(&def), RenameSafety::SavedDataBacked),
        "a saved field is data-backed: {:?}",
        index.rename_safety(&def),
    );

    // The saved read `^books(id).title` is a reference to that field.
    let refs = index.references(&def);
    assert!(
        refs.len() >= 2,
        "the field declaration and its saved read are both references: {refs:?}",
    );

    let action = index
        .rename_action(&def, "subtitle")
        .expect("saved field has a rename action");
    assert!(
        action
            .edits
            .iter()
            .all(|edit| edit.replacement == "subtitle"),
        "{action:#?}"
    );
    assert!(
        action.edits.iter().any(|edit| edit.file == *file
            && source[edit.span.start_byte..edit.span.end_byte].contains("title")),
        "source edits include the field declaration or read: {action:#?}"
    );
    assert_eq!(
        action.evolve_rename.as_deref(),
        Some("evolve\n    rename Book.title -> Book.subtitle\n")
    );
    let renamed = apply_edits(source, file, &action.edits);
    assert!(renamed.contains("required subtitle: string"), "{renamed}");
    assert!(renamed.contains("^books(id).subtitle"), "{renamed}");
    assert_checks_clean("safety-saved-field-renamed", &renamed);
}

#[test]
fn a_split_store_field_name_is_saved_data_backed_and_unsafe() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn peek(id: int): string\n    \
        return ^books(id).title\n";
    let (index, paths) = analyze("safety-split-store-field", &[("src/m.mw", source)]);
    let file = &paths[0];

    let decl_offset = source.find("title: string").expect("field decl") + 1;
    let def = index
        .definition(file, decl_offset)
        .expect("the field declaration is a symbol");
    assert_eq!(def.kind, SymbolKind::Field, "{def:?}");
    assert_eq!(
        index.rename_safety(&def),
        RenameSafety::SavedDataBacked,
        "split store fields are backed by the store root",
    );

    let read_offset = source.rfind("title").expect("saved read") + 1;
    let read_def = index
        .definition(file, read_offset)
        .expect("the saved read resolves to the field");
    assert_eq!(read_def.span, def.span, "{read_def:?}");
}

#[test]
fn a_stored_resource_name_has_a_saved_data_rename_action() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn copy(book: Book): Book\n    \
        return Book(title: book.title)\n";
    let (index, paths) = analyze("safety-stored-resource", &[("src/m.mw", source)]);
    let file = &paths[0];

    let decl_offset = source.find("Book").expect("resource decl") + 1;
    let def = index
        .definition(file, decl_offset)
        .expect("the resource declaration is a symbol");
    assert_eq!(def.kind, SymbolKind::Resource, "{def:?}");
    assert_eq!(
        index.rename_safety(&def),
        RenameSafety::SavedDataBacked,
        "a resource attached to a saved root is data-backed",
    );

    let action = index
        .rename_action(&def, "Volume")
        .expect("stored resource has a rename action");
    assert!(
        action
            .edits
            .iter()
            .any(|edit| edit.file == *file && edit.replacement == "Volume"),
        "{action:#?}"
    );
    assert_eq!(
        action.evolve_rename.as_deref(),
        Some("evolve\n    rename Book -> Volume\n")
    );
    let renamed = apply_edits(source, file, &action.edits);
    assert!(renamed.contains("resource Volume"), "{renamed}");
    assert!(renamed.contains(": Volume"), "{renamed}");
    assert!(renamed.contains("return Volume("), "{renamed}");
    assert_checks_clean("safety-stored-resource-renamed", &renamed);
}

#[test]
fn a_saved_layer_name_has_a_token_tight_saved_data_rename_action() {
    let source = "module m\n\
        resource Book\n    \
        versions(version: int)\n        \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn titles(id: Id(^books))\n    \
        for version, book in ^books(id).versions\n        \
        print(book.title)\n";
    let (index, paths) = analyze("safety-saved-layer", &[("src/m.mw", source)]);
    let file = &paths[0];

    let decl_offset = source.find("versions(version").expect("layer decl") + 1;
    let def = index
        .definition(file, decl_offset)
        .expect("the layer declaration is a symbol");
    assert_eq!(def.kind, SymbolKind::Layer, "{def:?}");
    assert_eq!(
        index.rename_safety(&def),
        RenameSafety::SavedDataBacked,
        "a saved layer name is catalog-backed",
    );

    let action = index
        .rename_action(&def, "editions")
        .expect("saved layer has a rename action");
    assert_eq!(
        action.evolve_rename.as_deref(),
        Some("evolve\n    rename Book.versions -> Book.editions\n")
    );
    let renamed = apply_edits(source, file, &action.edits);
    assert!(renamed.contains("editions(version: int)"), "{renamed}");
    assert!(renamed.contains("^books(id).editions"), "{renamed}");
    assert_checks_clean("safety-saved-layer-renamed", &renamed);
}

#[test]
fn a_store_index_name_has_a_token_tight_saved_data_rename_action() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n    \
        index byTitle(title, id)\n\
        fn titled(title: string)\n    \
        for id, book in ^books.byTitle(title)\n        \
        print(book.title)\n";
    let (index, paths) = analyze("safety-store-index", &[("src/m.mw", source)]);
    let file = &paths[0];

    let decl_offset = source.find("byTitle(title").expect("index decl") + 1;
    let def = index
        .definition(file, decl_offset)
        .expect("the index declaration is a symbol");
    assert_eq!(def.kind, SymbolKind::Index, "{def:?}");
    assert_eq!(
        index.rename_safety(&def),
        RenameSafety::SavedDataBacked,
        "a store index name is catalog-backed",
    );

    let action = index
        .rename_action(&def, "byName")
        .expect("store index has a rename action");
    assert_eq!(
        action.evolve_rename.as_deref(),
        Some("evolve\n    rename ^books.byTitle -> ^books.byName\n")
    );
    let renamed = apply_edits(source, file, &action.edits);
    assert!(renamed.contains("index byName(title, id)"), "{renamed}");
    assert!(renamed.contains("^books.byName(title)"), "{renamed}");
    assert_checks_clean("safety-store-index-renamed", &renamed);
}

#[test]
fn an_enum_name_has_a_saved_data_rename_action() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Order\n    \
        required state: Status\n\
        store ^orders(id: int): Order\n\
        fn active(): Status\n    \
        return Status::active\n";
    let (index, paths) = analyze("safety-enum", &[("src/m.mw", source)]);
    let file = &paths[0];

    let decl_offset = source.find("Status").expect("enum decl") + 1;
    let def = index
        .definition(file, decl_offset)
        .expect("the enum declaration is a symbol");
    assert_eq!(def.kind, SymbolKind::Enum, "{def:?}");
    assert_eq!(
        index.rename_safety(&def),
        RenameSafety::SavedDataBacked,
        "an enum name is catalog-backed",
    );

    let action = index
        .rename_action(&def, "State")
        .expect("enum has a rename action");
    assert_eq!(
        action.evolve_rename.as_deref(),
        Some("evolve\n    rename Status -> State\n")
    );
    let renamed = apply_edits(source, file, &action.edits);
    assert!(renamed.contains("enum State"), "{renamed}");
    assert!(renamed.contains("required state: State"), "{renamed}");
    assert!(renamed.contains("return State::active"), "{renamed}");
    assert_checks_clean("safety-enum-renamed", &renamed);
}

#[test]
fn an_enum_member_name_has_a_saved_data_rename_action() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Order\n    \
        required state: Status\n\
        store ^orders(id: int): Order\n\
        fn active(): Status\n    \
        return Status::active\n";
    let (index, paths) = analyze("safety-enum-member", &[("src/m.mw", source)]);
    let file = &paths[0];

    let decl_offset = source.find("active").expect("member decl") + 1;
    let def = index
        .definition(file, decl_offset)
        .expect("the enum member declaration is a symbol");
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");
    assert_eq!(
        index.rename_safety(&def),
        RenameSafety::SavedDataBacked,
        "an enum member name is catalog-backed",
    );

    let action = index
        .rename_action(&def, "enabled")
        .expect("enum member has a rename action");
    assert_eq!(
        action.evolve_rename.as_deref(),
        Some("evolve\n    rename Status::active -> Status::enabled\n")
    );
    let renamed = apply_edits(source, file, &action.edits);
    assert!(renamed.contains("enabled\n"), "{renamed}");
    assert!(renamed.contains("return Status::enabled"), "{renamed}");
    assert_checks_clean("safety-enum-member-renamed", &renamed);
}

#[test]
fn a_source_only_symbol_is_safe_to_rename() {
    // A function parameter is source-only: no saved-data encoding depends on its
    // name, so renaming is safe.
    let source = "module m\nfn greet(title: string)\n    print(title)\n";
    let (index, paths) = analyze("safety-source-only", &[("src/m.mw", source)]);
    let file = &paths[0];

    let use_offset = source.rfind("title").expect("use of title") + 1;
    let def = index
        .definition(file, use_offset)
        .expect("the use resolves to the parameter");
    assert_eq!(def.kind, SymbolKind::Param, "{def:?}");
    assert_eq!(
        index.rename_safety(&def),
        RenameSafety::SourceOnly,
        "a parameter is source-only",
    );
}

#[test]
fn a_source_only_parameter_rename_edits_only_identifier_tokens() {
    let source = "module m\nfn greet(title: string)\n    print(title)\n";
    let renamed = rename_at("rename-param", source, "print(title)", "title", "label");

    assert_eq!(
        renamed,
        "module m\nfn greet(label: string)\n    print(label)\n"
    );
    assert_checks_clean("rename-param-renamed", &renamed);
}

#[test]
fn an_imported_module_alias_has_no_source_only_rename_action() {
    let library = "module shelf::books\n\
        resource Book\n    \
        required title: string\n\
        pub fn title(): string\n    \
        return \"Dune\"\n";
    let app = "module shelf::app\n\
        use shelf::books\n\
        fn run(items: sequence[books::Book]): string\n    \
        return books::title()\n";
    let (index, paths) = analyze(
        "rename-import-alias",
        &[("src/shelf/books.mw", library), ("src/shelf/app.mw", app)],
    );
    let file = &paths[1];

    let call_offset = app.find("books::title").expect("alias use") + 1;
    let def = index
        .definition(file, call_offset)
        .expect("the imported module alias use resolves to its import");
    assert_eq!(def.kind, SymbolKind::ModuleRef, "{def:?}");
    assert_eq!(index.rename_safety(&def), RenameSafety::SourceOnly);
    assert_eq!(&app[def.span.start_byte..def.span.end_byte], "books");

    assert!(
        index.rename_action(&def, "volumes").is_none(),
        "module imports have no alias syntax, so rename_action must not generate module-target edits"
    );

    let type_offset = app.find("sequence[books::Book]").expect("alias type") + "sequence[".len();
    let type_def = index
        .definition(file, type_offset + 1)
        .expect("the imported module alias in a wrapped type resolves to its import");
    assert_eq!(type_def, def, "{type_def:?}");
    assert!(index.rename_action(&type_def, "volumes").is_none());
}

#[test]
fn a_documented_source_only_parameter_rename_edits_only_identifier_tokens() {
    let source = "module m\n\
        pub fn greet(\n    \
        ;; The title to print.\n    \
        title: string,\n\
        ): string\n    \
        return title\n";
    let renamed = rename_at(
        "rename-documented-param",
        source,
        "return title",
        "title",
        "label",
    );

    assert!(renamed.contains("label: string,"), "{renamed}");
    assert!(renamed.contains("return label"), "{renamed}");
    assert_checks_clean("rename-documented-param-renamed", &renamed);
}

#[test]
fn a_source_only_const_rename_edits_only_identifier_tokens() {
    let source = "module m\nfn f(): int\n    const n = 1\n    return n\n";
    let renamed = rename_at("rename-const-local", source, "return n", "n", "total");

    assert_eq!(
        renamed,
        "module m\nfn f(): int\n    const total = 1\n    return total\n"
    );
    assert_checks_clean("rename-const-local-renamed", &renamed);
}

#[test]
fn a_source_only_var_rename_edits_only_identifier_tokens() {
    let source = "module m\nfn f(): int\n    var n = 1\n    n = n + 1\n    return n\n";
    let renamed = rename_at("rename-var-local", source, "return n", "n", "total");

    assert_eq!(
        renamed,
        "module m\nfn f(): int\n    var total = 1\n    total = total + 1\n    return total\n"
    );
    assert_checks_clean("rename-var-local-renamed", &renamed);
}

#[test]
fn a_source_only_single_loop_binding_rename_edits_only_identifier_tokens() {
    let source = "module m\nfn f(): int\n    var total = 0\n    for n in 1..3\n        total = total + n\n    return total\n";
    let renamed = rename_at("rename-loop-local", source, "total + n", "n", "item");

    assert_eq!(
        renamed,
        "module m\nfn f(): int\n    var total = 0\n    for item in 1..3\n        total = total + item\n    return total\n"
    );
    assert_checks_clean("rename-loop-local-renamed", &renamed);
}

#[test]
fn a_source_only_second_loop_binding_rename_edits_only_identifier_tokens() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f()\n    \
        for id, book in ^books\n        \
        print(book.title)\n";
    let renamed = rename_at(
        "rename-second-loop-local",
        source,
        "book.title",
        "book",
        "volume",
    );

    assert!(renamed.contains("for id, volume in ^books"), "{renamed}");
    assert!(renamed.contains("print(volume.title)"), "{renamed}");
    assert_checks_clean("rename-second-loop-local-renamed", &renamed);
}

#[test]
fn a_source_only_if_const_binding_rename_edits_only_identifier_tokens() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f(id: Id(^books)): string\n    \
        if const title = ^books(id).title\n        \
        return title\n    \
        return \"\"\n";
    let renamed = rename_at(
        "rename-if-const-local",
        source,
        "return title",
        "title",
        "foundTitle",
    );

    assert!(
        renamed.contains("if const foundTitle = ^books(id).title"),
        "{renamed}"
    );
    assert!(renamed.contains("return foundTitle"), "{renamed}");
    assert_checks_clean("rename-if-const-local-renamed", &renamed);
}

#[test]
fn a_source_only_catch_binding_rename_edits_only_identifier_tokens() {
    let source = "module m\nfn f(): string\n    try\n        throw Error(code: \"x.y\", message: \"m\")\n    catch err: Error\n        return err.message\n    return \"\"\n";
    let renamed = rename_at("rename-catch-local", source, "err.message", "err", "caught");

    assert!(renamed.contains("catch caught: Error"), "{renamed}");
    assert!(renamed.contains("return caught.message"), "{renamed}");
    assert_checks_clean("rename-catch-local-renamed", &renamed);
}

fn apply_edits(source: &str, file: &Path, edits: &[SourceEdit]) -> String {
    let mut edits: Vec<_> = edits.iter().filter(|edit| edit.file == file).collect();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.span.start_byte));
    let mut edited = source.to_string();
    for edit in edits {
        edited.replace_range(edit.span.start_byte..edit.span.end_byte, &edit.replacement);
    }
    edited
}

fn rename_at(name: &str, source: &str, marker: &str, symbol: &str, new_name: &str) -> String {
    let (index, paths) = analyze(name, &[("src/m.mw", source)]);
    let file = &paths[0];
    let marker_start = source.find(marker).expect("rename cursor marker");
    let symbol_start = marker
        .rfind(symbol)
        .expect("rename cursor marker should contain symbol");
    let offset = marker_start + symbol_start;
    let def = index
        .definition(file, offset)
        .expect("cursor resolves to a symbol");
    assert_eq!(index.rename_safety(&def), RenameSafety::SourceOnly);
    let action = index
        .rename_action(&def, new_name)
        .expect("source-only symbol has a rename action");
    assert!(
        action.evolve_rename.is_none(),
        "source-only rename must not synthesize evolve intent: {action:#?}"
    );
    apply_edits(source, file, &action.edits)
}

fn assert_checks_clean(name: &str, source: &str) {
    let (snapshot, _) = support::analyze_overlay(name, &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "renamed source should check cleanly: {:#?}\n{source}",
        snapshot.report.diagnostics
    );
}
