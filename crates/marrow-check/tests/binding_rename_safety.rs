//! Rename-safety classification in the binding index: a saved resource field —
//! whether declared inline or through a split `store` — is `SavedDataBacked`
//! because its name encodes the on-disk path, while a source-only symbol such as a
//! function parameter is `SourceOnly` and safe to rename.

mod support;
mod support_binding;

use marrow_check::binding::{RenameSafety, SymbolKind};

use support_binding::analyze;

#[test]
fn a_saved_field_name_is_saved_data_backed_and_unsafe() {
    // `title` is a stored field of the saved `Book` resource; its on-disk path is
    // `^books(id).title`, so renaming the source name orphans saved data.
    let source = "module m\n\
        resource Book at ^books(id: int)\n    \
        required title: string\n\
        fn peek(id: int): string\n    \
        return ^books(id).title\n";
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
