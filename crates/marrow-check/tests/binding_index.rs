//! The project-wide binding index: identifier uses resolved to their definitions
//! respecting lexical scope (shadowing) and `use` aliases, plus rename-safety
//! classification. Exercises the same analysis path editor tooling uses.

mod support;

use std::path::{Path, PathBuf};

use marrow_check::binding::{RenameSafety, SymbolKind};
use marrow_check::{
    AnalysisSnapshot, CheckedStmt, ProjectSources, analyze_project, build_binding_index,
};

use support::{config, temp_root};

/// Analyze a set of `(relative-path, source)` files written under `src` and build
/// the binding index over the resulting snapshot. Returns the index and the
/// absolute paths of the written files, in the given order.
fn analyze(
    name: &str,
    files: &[(&str, &str)],
) -> (marrow_check::binding::BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = analyze_snapshot(name, files);
    (build_binding_index(&snapshot), paths)
}

fn analyze_snapshot(name: &str, files: &[(&str, &str)]) -> (AnalysisSnapshot, Vec<PathBuf>) {
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
    (snapshot, paths)
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
        resource Book at ^books(id: int)\n    \
        required title: string\n\
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
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");

    let member_decl = source.find("archived\n").expect("archived declaration");
    assert!(
        def.span.start_byte <= member_decl && member_decl <= def.span.end_byte,
        "definition span covers the enum member declaration: {def:?}",
    );

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
        .find("category tiger")
        .expect("top-level tiger category");
    assert!(
        category_def.span.start_byte <= top_level_category
            && top_level_category <= category_def.span.end_byte,
        "category segment should resolve to the anchored top-level category: {category_def:?}",
    );
}

#[test]
fn a_match_arm_resolves_to_the_enum_member_definition() {
    // Match arms are relative member paths. The scrutinee's enum supplies the
    // `Status` prefix, so a cursor on `active` should still reach `Status::active`.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(s: Status): int\n    \
        match s\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-arm", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n            return 1")
        .expect("active match arm");
    let def = index.definition(file, arm_use).expect("match arm resolves");
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");

    let member_decl = source.find("active\n").expect("active declaration");
    assert!(
        def.span.start_byte <= member_decl && member_decl <= def.span.end_byte,
        "definition span covers the enum member declaration: {def:?}",
    );

    let refs = index.references(&def);
    assert!(
        refs.iter()
            .any(|reference| reference.span.start_byte <= arm_use
                && arm_use <= reference.span.end_byte),
        "match arm use is a reference: {refs:?}",
    );
}

#[test]
fn a_match_arm_resolves_through_an_inferred_enum_local() {
    // The checker infers `s` as `Status` from its enum-member initializer, so the
    // binding index should use that same type when resolving relative match arms.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(): int\n    \
        const s = Status::active\n    \
        match s\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-inferred-local", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n            return 1")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from inferred local resolves");
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");

    let member_decl = source.find("active\n").expect("active declaration");
    assert!(
        def.span.start_byte <= member_decl && member_decl <= def.span.end_byte,
        "definition span covers the enum member declaration: {def:?}",
    );
}

#[test]
fn a_match_arm_resolves_through_a_module_enum_constant() {
    // Module constants are part of the checker prelude for every function body.
    // Match arm navigation should see their enum type too.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        const Default: Status = Status::active\n\
        fn classify(): int\n    \
        match Default\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-module-const", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("archived\n            return 2")
        .expect("archived match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from module constant resolves");
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");

    let member_decl = source.find("archived\n").expect("archived declaration");
    assert!(
        def.span.start_byte <= member_decl && member_decl <= def.span.end_byte,
        "definition span covers the enum member declaration: {def:?}",
    );
}

#[test]
fn a_match_arm_trailing_comment_is_not_a_member_reference() {
    // The reference span should cover the member path, not trivia after it.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(s: Status): int\n    \
        match s\n        \
        active ; chosen case\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-comment-span", &[("src/m.mw", source)]);
    let file = &paths[0];

    let comment = source.find("chosen").expect("arm trailing comment");
    assert!(
        index.definition(file, comment).is_none(),
        "trailing comment text must not resolve as an enum member",
    );

    let after_label = source.find("active ;").expect("active arm") + "active".len();
    assert!(
        index.definition(file, after_label).is_none(),
        "the space after a match arm label must not resolve as an enum member",
    );
}

#[test]
fn a_match_arm_resolves_through_an_enum_returning_call() {
    // Match dispatch uses the scrutinee expression's inferred enum type, not just
    // local names. A call returning `Status` should unlock relative arm refs.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn pick(): Status\n    \
        return Status::active\n\
        fn classify(): int\n    \
        match pick()\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-call-scrutinee", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n            return 1")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from enum-returning call resolves");
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");
}

#[test]
fn an_invalid_enum_member_scrutinee_does_not_create_arm_references() {
    // `Status::missing` names the enum prefix but no member. The checker rejects
    // the scrutinee, and the binding index should avoid false arm references.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(): int\n    \
        match Status::missing\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-invalid-scrutinee", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n            return 1")
        .expect("active match arm");
    assert!(
        index.definition(file, arm_use).is_none(),
        "invalid enum scrutinee should not create arm member refs",
    );
}

#[test]
fn a_match_arm_resolves_through_a_sequence_enum_loop_binding() {
    // Loop bindings use the checker-shared `for` frame, so iterating
    // `sequence[Status]` makes `s` a `Status` value for relative match arms.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(items: sequence[Status]): int\n    \
        for s in items\n        \
        match s\n            \
        active\n                \
        return 1\n            \
        archived\n                \
        return 2\n    \
        return 0\n";
    let (index, paths) = analyze("enum-match-sequence-loop", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n                return 1")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from sequence enum loop binding resolves");
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");

    let member_decl = source.find("active\n").expect("active declaration");
    assert!(
        def.span.start_byte <= member_decl && member_decl <= def.span.end_byte,
        "definition span covers the enum member declaration: {def:?}",
    );
}

#[test]
fn a_match_arm_resolves_through_a_saved_enum_layer_values_loop_binding() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Book at ^books(id: int)\n    \
        states(pos: int): Status\n\
        fn classify(id: Id(^books)): int\n    \
        for s in values(^books(id).states)\n        \
        match s\n            \
        active\n                \
        return 1\n            \
        archived\n                \
        return 2\n    \
        return 0\n";
    let (snapshot, paths) = analyze_snapshot(
        "enum-match-saved-layer-values-loop",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    let index = build_binding_index(&snapshot);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n                return 1")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from saved enum layer values loop binding resolves");
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");

    let member_decl = source.find("active\n").expect("active declaration");
    assert!(
        def.span.start_byte <= member_decl && member_decl <= def.span.end_byte,
        "definition span covers the enum member declaration: {def:?}",
    );
}

#[test]
fn a_match_arm_resolves_through_a_two_name_saved_enum_layer_loop_binding() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Book at ^books(id: int)\n    \
        states(pos: int): Status\n\
        fn classify(id: Id(^books)): int\n    \
        for pos, s in ^books(id).states\n        \
        match s\n            \
        active\n                \
        return pos\n            \
        archived\n                \
        return 0\n    \
        return 0\n";
    let (snapshot, paths) = analyze_snapshot(
        "enum-match-two-name-saved-layer-loop",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    let index = build_binding_index(&snapshot);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n                return pos")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from two-name saved enum layer loop binding resolves");
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");

    let member_decl = source.find("active\n").expect("active declaration");
    assert!(
        def.span.start_byte <= member_decl && member_decl <= def.span.end_byte,
        "definition span covers the enum member declaration: {def:?}",
    );
}

#[test]
fn a_saved_enum_layer_loop_match_records_its_scrutinee_enum() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Book at ^books(id: int)\n    \
        states(pos: int): Status\n\
        fn classify(id: Id(^books)): int\n    \
        for s in values(^books(id).states)\n        \
        match s\n            \
        active\n                \
        return 1\n            \
        archived\n                \
        return 2\n    \
        return 0\n";
    let (snapshot, _) = analyze_snapshot("enum-match-saved-layer-stamp", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    let function = snapshot.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "classify")
        .expect("classify function");
    let runtime_body = function.runtime_body().expect("runtime body");
    let loop_body = runtime_body
        .statements()
        .iter()
        .find_map(|statement| match statement {
            CheckedStmt::For { body, .. } => Some(body),
            _ => None,
        })
        .expect("saved layer loop");
    let enum_ref = loop_body
        .statements()
        .iter()
        .find_map(|statement| match statement {
            CheckedStmt::Match {
                enum_ref: Some(enum_ref),
                ..
            } => Some(*enum_ref),
            _ => None,
        })
        .expect("match in loop body");
    let enum_fact = snapshot
        .program
        .facts
        .enums()
        .iter()
        .find(|fact| fact.id == enum_ref.enum_id)
        .expect("match enum is recorded in checked facts");
    let module = snapshot
        .program
        .facts
        .modules()
        .iter()
        .find(|fact| fact.id == enum_fact.module)
        .expect("enum module is recorded in checked facts");

    assert_eq!(
        (module.name.as_str(), enum_fact.name.as_str()),
        ("m", "Status")
    );
}

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
fn map_sugar_does_not_create_enum_annotation_references() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Order at ^orders(id: int)\n    \
        scores: map[string, Status]\n";
    let (index, paths) = analyze("enum-annotation-map-canary", &[("src/m.mw", source)]);
    let file = &paths[0];

    let status = source.find("map[string, Status]").expect("map annotation") + "map[string, ".len();
    let def = index.definition(file, status + 1);
    assert!(
        !matches!(def, Some(ref def) if def.kind == SymbolKind::Enum),
        "map sugar value type should not be expanded into an enum reference here: {def:?}",
    );
}

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
    let (snapshot, paths) = analyze_snapshot(
        "qualified-resource-constructor",
        &[("src/shelf/state.mw", state), ("src/shelf/app.mw", app)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    let index = build_binding_index(&snapshot);
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
    let (snapshot, paths) = analyze_snapshot(
        "bare-resource-constructor-current-module",
        &[("src/shelf/state.mw", state), ("src/shelf/app.mw", app)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    let index = build_binding_index(&snapshot);
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
    let (snapshot, paths) = analyze_snapshot(
        "alias-id-call-imported-function",
        &[
            ("src/shelf/book.mw", imported),
            ("src/traps.mw", trap),
            ("src/app.mw", app),
        ],
    );
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    let index = build_binding_index(&snapshot);
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
    let (snapshot, paths) = analyze_snapshot(
        "alias-id-call-imported-resource",
        &[
            ("src/shelf/book.mw", imported),
            ("src/traps.mw", trap),
            ("src/app.mw", app),
        ],
    );
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    let index = build_binding_index(&snapshot);
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
    let (snapshot, paths) = analyze_snapshot(
        "alias-id-type-ref-imported-resource",
        &[
            ("src/traps.mw", trap),
            ("src/shelf/book.mw", imported_module),
            ("src/app.mw", app),
        ],
    );
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    let index = build_binding_index(&snapshot);
    let imported_file = &paths[1];
    let app_file = &paths[2];

    let type_ref = app.find("book::Id").expect("aliased resource type ref");
    let def = index
        .definition(app_file, type_ref + "book::".len() + 1)
        .expect("aliased type ref resolves");
    assert_eq!(def.kind, SymbolKind::Resource, "{def:?}");
    assert_eq!(def.file, *imported_file, "{def:?}");
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
