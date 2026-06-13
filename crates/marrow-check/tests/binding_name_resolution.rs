//! Lexical name resolution in the project-wide binding index: a use resolves to
//! its binding respecting scope and shadowing, and a function's references span
//! modules through `use` aliases while a bare call stays in its own module.
//! Exercises the same analysis path editor tooling uses.

mod support;
mod support_binding;

use std::path::Path;

use marrow_check::binding::SymbolKind;
use marrow_check::build_binding_index;

use support_binding::{analyze, nth_offset};

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
    let def_start = source.find("k = 7").expect("const name");
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
fn parameter_definitions_expose_their_function_and_parameter_index() {
    let source = "module m\nfn greet(count: int, title: string)\n    print(title)\n";
    let (snapshot, paths) =
        support::analyze_overlay("param-definition-owner", &[("src/m.mw", source)]);
    let index = build_binding_index(&snapshot);
    let file = &paths[0];

    let use_offset = source.rfind("title").expect("parameter use") + 1;
    let def = index
        .definition(file, use_offset)
        .expect("use resolves to the parameter");
    assert_eq!(def.kind, SymbolKind::Param, "{def:?}");

    let param = index
        .parameter_definition(&def)
        .expect("parameter binding has owner metadata");
    let function = snapshot.program.facts.function(param.function);

    assert_eq!(function.name, "greet");
    assert_eq!(param.index, 1);
    assert_eq!(function.params[param.index].id, param.local);
    assert_eq!(function.params[param.index].name, "title");
}

#[test]
fn references_of_a_function_span_modules_through_aliases() {
    // `shelf::books::add` is called from another module two ways: fully qualified
    // and through a `use shelf::books` short-form alias (`books::add`). Both call
    // sites are references of the one function definition.
    let books = "module shelf::books\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        pub fn add(title: string): Id(^books)\n    \
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
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        pub fn add(title: string): Id(^books)\n    \
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
    let call_offset = zzz.rfind("greet()").expect("bare call") + 2;
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
fn a_bare_call_in_a_nested_module_resolves_inside_the_call_token() {
    let source = "module shelf::sample\n\
        resource Book\n    \
        required title: string\n    \
        required visits: int\n\
        store ^books(id: int): Book\n\
        pub fn touch(id: int): string\n    \
        const title: string = ^books(id).title ?? \"\"\n    \
        const visits: int = ^books(id).visits ?? 0\n    \
        transaction\n        \
        ^books(id).visits = visits + 1\n    \
        print(title)\n    \
        return title\n\
        pub fn caller(): string\n    \
        return touch(1)\n";
    let (index, paths) = analyze(
        "binding-nested-bare-call",
        &[("src/shelf/sample.mw", source)],
    );
    let file = &paths[0];

    let call_offset = source.rfind("touch(1)").expect("bare call") + 1;
    let def = index
        .definition(file, call_offset)
        .expect("nested module bare call resolves to a function definition");
    assert_eq!(def.kind, SymbolKind::Function, "{def:?}");
    assert_eq!(def.file, *file, "{def:?}");
}
