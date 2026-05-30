//! The project-wide binding index: identifier uses resolved to their definitions
//! respecting lexical scope (shadowing) and `use` aliases, plus rename-safety
//! classification. Exercises the same analysis path editor tooling uses.

use std::path::{Path, PathBuf};

use marrow_check::binding::{RenameSafety, SymbolKind};
use marrow_check::{ProjectSources, analyze_project, build_binding_index};

fn temp_root(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create project root");
    root
}

fn config() -> marrow_project::ProjectConfig {
    marrow_project::parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config")
}

/// Analyze a set of `(relative-path, source)` files written under `src` and build
/// the binding index over the resulting snapshot. Returns the index and the
/// absolute paths of the written files, in the given order.
fn analyze(
    name: &str,
    files: &[(&str, &str)],
) -> (marrow_check::binding::BindingIndex, Vec<PathBuf>) {
    let root = temp_root(name);
    let mut sources = ProjectSources::new();
    let mut paths = Vec::new();
    for (relative, source) in files {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create dir");
        std::fs::write(&path, source).expect("write source");
        sources.insert(&path, *source);
        paths.push(path);
    }
    let snapshot = analyze_project(&root, &config(), &sources).expect("analyze");
    std::fs::remove_dir_all(&root).ok();
    (build_binding_index(&snapshot), paths)
}

/// The byte offset of the `n`-th (0-based) occurrence of `needle` in `source`,
/// plus one so the cursor lands inside the token rather than at its edge.
fn nth_offset(source: &str, needle: &str, n: usize) -> usize {
    let mut start = 0;
    for _ in 0..n {
        let found = source[start..].find(needle).expect("needle present") + start;
        start = found + needle.len();
    }
    source[start..].find(needle).expect("needle present") + start + 1
}

#[test]
fn definition_of_a_local_use_is_its_binding() {
    // `const k = 7` then `print(k)`: the cursor on the use of `k` resolves to the
    // `const k` binding, classified as a local.
    let source = "module m\nfn f()\n    const k = 7\n    print(k)\n";
    let (index, paths) = analyze("def-local", &[("src/m.mw", source)]);
    let file = &paths[0];

    let use_offset = source.rfind('k').expect("use of k");
    let def = index
        .definition(file, use_offset)
        .expect("the use resolves to a definition");
    assert_eq!(def.kind, SymbolKind::Local, "{def:?}");
    // The definition site is the `const k` statement, which precedes the use.
    let def_start = source.find("const k").expect("const k");
    assert_eq!(def.span.start_byte, def_start, "{def:?}");
    assert!(def.span.start_byte < use_offset, "{def:?}");
}

#[test]
fn references_of_a_param_are_its_uses_shadowing_aware() {
    // `n` is a parameter used twice at top level; an inner block redeclares `n`
    // with `const n`, whose own use must NOT be attributed to the parameter.
    let source = "module m\n\
        fn f(n: int)\n    \
        print(n)\n    \
        if true\n        \
        const n = 0\n        \
        print(n)\n    \
        print(n)\n";
    let (index, paths) = analyze("refs-param-shadow", &[("src/m.mw", source)]);
    let file = &paths[0];

    // Point at the first top-level use of `n` in `print(n)`.
    let first_use = nth_offset(source, "print(n)", 0) + "print(".len() - 1;
    let def = index
        .definition(file, first_use)
        .expect("use resolves to the parameter");
    assert_eq!(def.kind, SymbolKind::Param, "{def:?}");

    let refs = index.references(&def);
    // The parameter's uses are the two top-level `print(n)` reads; the inner
    // `print(n)` reads the shadowing local and is excluded. The definition site is
    // included once.
    let in_func: Vec<usize> = refs.iter().map(|r| r.span.start_byte).collect();
    // Two top-level uses of `n`.
    let top_use_1 = source.find("print(n)").expect("first") + "print(".len();
    let top_use_2 = source.rfind("print(n)").expect("last") + "print(".len();
    assert!(
        in_func.contains(&top_use_1),
        "first top use present: {in_func:?}"
    );
    assert!(
        in_func.contains(&top_use_2),
        "second top use present: {in_func:?}"
    );

    // The inner `print(n)` (the middle one) reads the shadowing local; its span
    // must not appear among the parameter's references.
    let inner_use = {
        let first = source.find("print(n)").expect("first") + "print(".len();
        let last = source.rfind("print(n)").expect("last") + "print(".len();
        // The middle occurrence sits strictly between the first and last.
        let mut mids = source
            .match_indices("print(n)")
            .map(|(i, _)| i + "print(".len())
            .filter(|&i| i != first && i != last);
        mids.next().expect("a middle use exists")
    };
    assert!(
        !in_func.contains(&inner_use),
        "shadowed inner use excluded: refs={in_func:?} inner={inner_use}",
    );
}

#[test]
fn references_of_a_function_span_modules_through_aliases() {
    // `shelf::books::add` is called from another module two ways: fully qualified
    // and through a `use shelf::books` short-form alias (`books::add`). Both call
    // sites are references of the one function definition.
    let books = "module shelf::books\n\
        resource Book at ^books(id: int)\n    \
        required title: string\n\
        pub fn add(title: string): Book::Id\n    \
        return nextId(^books)\n";
    let app = "module app\n\
        use shelf::books\n\
        fn run()\n    \
        const a = shelf::books::add(\"x\")\n    \
        const b = books::add(\"y\")\n";
    let (index, paths) = analyze(
        "refs-function-alias",
        &[("src/shelf/books.mw", books), ("src/app.mw", app)],
    );
    let books_file = &paths[0];
    let app_file = &paths[1];

    // The definition cursor sits on the function name in its own declaration.
    let def_offset = books.find("fn add").expect("fn add") + "fn ".len();
    let def = index
        .definition(books_file, def_offset)
        .expect("function definition at its declaration");
    assert_eq!(def.kind, SymbolKind::Function, "{def:?}");

    let refs = index.references(&def);
    // Every reference: the definition plus the two call sites in `app`.
    let files: Vec<&Path> = refs.iter().map(|r| r.file.as_path()).collect();
    assert!(
        files.contains(&books_file.as_path()),
        "definition file present: {files:?}",
    );
    let app_refs = refs.iter().filter(|r| r.file == *app_file).count();
    assert_eq!(
        app_refs, 2,
        "both the qualified and aliased call sites resolve: {refs:?}",
    );
}

#[test]
fn definition_from_an_aliased_call_site_resolves_to_the_function() {
    // Going the other way: a cursor on the aliased call `books::add(...)` resolves
    // back to the `add` function in `shelf::books`.
    let books = "module shelf::books\n\
        resource Book at ^books(id: int)\n    \
        required title: string\n\
        pub fn add(title: string): Book::Id\n    \
        return nextId(^books)\n";
    let app = "module app\n\
        use shelf::books\n\
        fn run()\n    \
        const b = books::add(\"y\")\n";
    let (index, paths) = analyze(
        "def-from-alias",
        &[("src/shelf/books.mw", books), ("src/app.mw", app)],
    );
    let books_file = &paths[0];
    let app_file = &paths[1];

    let call_offset = app.find("books::add").expect("aliased call") + "books::".len();
    let def = index
        .definition(app_file, call_offset)
        .expect("aliased call resolves to the function definition");
    assert_eq!(def.kind, SymbolKind::Function, "{def:?}");
    assert_eq!(def.file, *books_file, "{def:?}");
}

#[test]
fn a_bare_call_goes_to_its_own_module_not_a_foreign_one() {
    // Both `aaa` and `zzz` declare `fn greet`. A bare `greet()` in `zzz::run`
    // resolves in its own module first, so go-to-def must land on `zzz::greet` —
    // never first-matched to the foreign `aaa::greet`. The binding index now
    // shares the unified resolver, so this matches what the checker and runtime do.
    let aaa = "module aaa\npub fn greet(): int\n    return 1\n";
    let zzz = "module zzz\nfn greet(): int\n    return 2\nfn run(): int\n    return greet()\n";
    let (index, paths) = analyze(
        "binding-bare-own-module",
        &[("src/aaa.mw", aaa), ("src/zzz.mw", zzz)],
    );
    let zzz_file = &paths[1];

    // The cursor sits on the bare call `greet()` inside `zzz::run`.
    let call_offset = zzz.rfind("greet()").expect("bare call");
    let def = index
        .definition(zzz_file, call_offset)
        .expect("bare call resolves to a function definition");
    assert_eq!(def.kind, SymbolKind::Function, "{def:?}");
    assert_eq!(
        def.file, *zzz_file,
        "a bare call goes to its own module's `greet`, not the foreign one: {def:?}",
    );
}

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
        matches!(
            index.rename_safety(&def),
            RenameSafety::SavedDataBacked { .. }
        ),
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
fn a_saved_field_with_a_stable_id_carries_it_for_migration() {
    // A stored field with an `@id(...)` is still saved-data-backed (the on-disk
    // path uses the source name), but the stable id is surfaced so migration
    // tooling can track the rename.
    let source = "module m\n\
        resource Book at ^books(id: int)\n    \
        @id(\"book.title\")\n    \
        required title: string\n\
        fn peek(id: int): string\n    \
        return ^books(id).title\n";
    let (index, paths) = analyze("safety-stable-id", &[("src/m.mw", source)]);
    let file = &paths[0];

    let decl_offset = source.find("title: string").expect("field decl") + 1;
    let def = index
        .definition(file, decl_offset)
        .expect("field declaration");
    match index.rename_safety(&def) {
        RenameSafety::SavedDataBacked { stable_id } => {
            assert_eq!(stable_id.as_deref(), Some("book.title"), "{stable_id:?}");
        }
        other => panic!("a saved field is data-backed, found {other:?}"),
    }
}
