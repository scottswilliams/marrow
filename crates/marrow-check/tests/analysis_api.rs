//! `type_at`/`scope_at`: position→type and visible-bindings for editor tooling,
//! reconstructing the cursor's lexical scope exactly as the checker does and
//! emitting no diagnostics of their own.

use std::path::PathBuf;

use marrow_check::program::MarrowType;
use marrow_check::{CheckedProgram, ProjectSources, analyze_project, scope_at, type_at};
use marrow_schema::ScalarType;
use marrow_syntax::ParsedSource;

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

/// Analyze a single `src/m.mw` source and return the program plus the parse for
/// that file, so a test can position into the buffer it controls. The source is
/// written to disk so project discovery finds it, then re-supplied as an overlay
/// to exercise the same path editor tooling uses.
fn analyze(name: &str, source: &str) -> (CheckedProgram, ParsedSource, PathBuf) {
    let root = temp_root(name);
    let path = root.join("src/m.mw");
    std::fs::create_dir_all(path.parent().unwrap()).expect("create src dir");
    std::fs::write(&path, source).expect("write source");
    let sources = ProjectSources::new().with(&path, source);
    let snapshot = analyze_project(&root, &config(), &sources).expect("analyze");
    std::fs::remove_dir_all(&root).ok();
    let parsed = snapshot
        .files
        .into_iter()
        .find(|file| file.path == path)
        .expect("the overlaid file is analyzed")
        .parsed;
    (snapshot.program, parsed, path)
}

/// The byte offset of the first occurrence of `needle` in `source`, plus an
/// inner offset, so a test can point at the middle of a token.
fn offset_of(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle present in source")
}

#[test]
fn type_at_a_literal_is_its_scalar_type() {
    let source = "module m\nfn f()\n    const n = 42\n";
    let (program, parsed, path) = analyze("type-at-literal", source);
    let offset = offset_of(source, "42") + 1;

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(ty, Some(MarrowType::Primitive(ScalarType::Int)), "{ty:?}");
}

#[test]
fn type_at_a_parameter_reference_is_the_parameter_type() {
    // `title` is a `string` parameter; a reference to it inside the body must
    // type to `string`, which requires the function's parameter scope.
    let source = "module m\nfn greet(title: string)\n    print(title)\n";
    let (program, parsed, path) = analyze("type-at-param", source);
    // Point at the *use* of `title` in `print(title)`, not the parameter decl.
    let offset = source.rfind("title").expect("use of title") + 1;

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(ty, Some(MarrowType::Primitive(ScalarType::Str)), "{ty:?}");
}

#[test]
fn type_at_a_local_const_reference_is_its_inferred_type() {
    // A `const`/`var` introduced earlier in the block is visible later; the
    // reference must resolve through the reconstructed block scope.
    let source = "module m\nfn f()\n    const k = 7\n    print(k)\n";
    let (program, parsed, path) = analyze("type-at-local", source);
    let offset = source.rfind('k').expect("use of k");

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(ty, Some(MarrowType::Primitive(ScalarType::Int)), "{ty:?}");
}

#[test]
fn type_at_a_saved_field_read_is_the_declared_leaf_type() {
    // `^books(id).title` reads a `string` field of the `Book` resource. Typing
    // it requires both the saved-data machinery and the `id` parameter in scope.
    let source = "module m\n\
        resource Book at ^books(id: int)\n    \
        required title: string\n\
        fn peek(id: int): string\n    \
        return ^books(id).title\n";
    let (program, parsed, path) = analyze("type-at-saved-field", source);
    // Point at the `.title` leaf, so the smallest covering expression is the whole
    // field read `^books(id).title` rather than the inner `^books` root.
    let offset = source.rfind("title").expect("the .title field read") + 1;

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(ty, Some(MarrowType::Primitive(ScalarType::Str)), "{ty:?}");
}

#[test]
fn type_at_an_identity_annotation_binding_is_the_identity_type() {
    // `const id: Id(^books) = ...` binds an identity; a reference to `id` types to
    // `Id(^books)`, the checker's `Identity("books")`.
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f(): Id(^books)\n    \
        const id: Id(^books) = nextId(^books)\n    \
        return id\n";
    let (program, parsed, path) = analyze("type-at-identity", source);
    let offset = source.rfind("id\n").expect("use of id in return") + 1;

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(
        ty,
        Some(MarrowType::Identity("books".to_string())),
        "{ty:?}"
    );
}

#[test]
fn scope_at_lists_params_locals_and_module_constants() {
    // At the `print` line, the visible bindings are the module constant `BASE`,
    // the parameter `n`, and the local `doubled` introduced before the cursor.
    let source = "module m\n\
        const BASE: int = 10\n\
        fn f(n: int)\n    \
        const doubled = n + n\n    \
        print(doubled)\n";
    let (program, parsed, path) = analyze("scope-at", source);
    let offset = source.find("print(doubled)").expect("print line");

    let bindings = scope_at(&program, &path, &parsed, offset);
    let names: Vec<&str> = bindings.iter().map(|(name, _)| name.as_str()).collect();
    assert!(names.contains(&"BASE"), "module const visible: {names:?}");
    assert!(names.contains(&"n"), "parameter visible: {names:?}");
    assert!(names.contains(&"doubled"), "local visible: {names:?}");

    let ty_of = |want: &str| {
        bindings
            .iter()
            .find(|(name, _)| name == want)
            .map(|(_, ty)| ty)
    };
    assert_eq!(ty_of("n"), Some(&MarrowType::Primitive(ScalarType::Int)));
    assert_eq!(
        ty_of("doubled"),
        Some(&MarrowType::Primitive(ScalarType::Int))
    );
    assert_eq!(ty_of("BASE"), Some(&MarrowType::Primitive(ScalarType::Int)));
}

#[test]
fn scope_at_excludes_locals_declared_after_the_cursor() {
    // `later` is declared after the cursor and must not be visible; `early` is.
    let source = "module m\n\
        fn f()\n    \
        const early = 1\n    \
        print(early)\n    \
        const later = 2\n";
    let (program, parsed, path) = analyze("scope-at-order", source);
    let offset = source.find("print(early)").expect("print line");

    let names: Vec<String> = scope_at(&program, &path, &parsed, offset)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(names.iter().any(|n| n == "early"), "{names:?}");
    assert!(!names.iter().any(|n| n == "later"), "{names:?}");
}

#[test]
fn scope_at_includes_a_loop_binding_typed_to_the_element() {
    // A `for` binding is in scope only within the loop body, typed to the
    // sequence element — the same rule the checker applies. Reconstructing the
    // cursor scope must push that frame.
    let source = "module m\n\
        fn f(items: sequence[int])\n    \
        for it in items\n        \
        print(it)\n";
    let (program, parsed, path) = analyze("scope-at-loop", source);
    let offset = source.find("print(it)").expect("loop body");

    let bindings = scope_at(&program, &path, &parsed, offset);
    let it = bindings
        .iter()
        .find(|(name, _)| name == "it")
        .map(|(_, ty)| ty);
    assert_eq!(
        it,
        Some(&MarrowType::Primitive(ScalarType::Int)),
        "{bindings:?}"
    );
}

#[test]
fn scope_at_includes_a_saved_group_loop_binding_typed_to_the_entry() {
    let source = "module m\n\
        resource Book at ^books(id: int)\n    \
        versions(version: int)\n        \
        required title: string\n\
        fn f(id: Id(^books))\n    \
        for n, version in ^books(id).versions\n        \
        print(version.title)\n";
    let (program, parsed, path) = analyze("scope-at-saved-group-loop", source);
    let offset = source.find("print(version.title)").expect("loop body");

    let bindings = scope_at(&program, &path, &parsed, offset);
    let ty_of = |want: &str| {
        bindings
            .iter()
            .find(|(name, _)| name == want)
            .map(|(_, ty)| ty)
    };
    assert_eq!(ty_of("n"), Some(&MarrowType::Primitive(ScalarType::Int)));
    assert_eq!(
        ty_of("version"),
        Some(&MarrowType::GroupEntry {
            resource: "m::Book".to_string(),
            layers: vec!["versions".to_string()],
        }),
        "{bindings:?}"
    );
}

#[test]
fn scope_at_includes_a_catch_binding_typed_error() {
    // A `catch` clause binds an `Error` value for the duration of its block;
    // reconstructing the cursor scope inside the catch must push that frame.
    let source = "module m\n\
        fn f()\n    \
        try\n        \
        print(1)\n    \
        catch e\n        \
        print(e)\n";
    let (program, parsed, path) = analyze("scope-at-catch", source);
    let offset = source.rfind("print(e)").expect("catch body");

    let bindings = scope_at(&program, &path, &parsed, offset);
    let e = bindings
        .iter()
        .find(|(name, _)| name == "e")
        .map(|(_, ty)| ty);
    assert_eq!(e, Some(&MarrowType::Error), "{bindings:?}");
}

#[test]
fn scope_at_includes_use_import_aliases() {
    // A `use std::clock` brings the short name `clock` into scope for call
    // expansion. `scope_at` should surface the import so completion can offer it.
    let source = "module m\n\
        use std::clock\n\
        fn f()\n    \
        const now = std::clock::now()\n    \
        print(now)\n";
    let (program, parsed, path) = analyze("scope-at-import", source);
    let offset = source.find("print(now)").expect("print line");

    let names: Vec<String> = scope_at(&program, &path, &parsed, offset)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(
        names.iter().any(|n| n == "clock"),
        "import alias: {names:?}"
    );
}

#[test]
fn type_at_and_scope_at_emit_no_diagnostics() {
    // The whole point: tooling queries reuse the checker's inference without a
    // diagnostics sink. The queries take an immutable program and parse and return
    // only a type or bindings, so they cannot add to a project's diagnostics; this
    // test pins that the queries return real answers (so "no diagnostics" is not
    // vacuous) and that sweeping the buffer never panics.
    let source = "module m\n\
        fn f(title: string)\n    \
        const greeting = title\n    \
        print(greeting)\n";
    let root = temp_root("no-diagnostics");
    let path = root.join("src/m.mw");
    std::fs::create_dir_all(path.parent().unwrap()).expect("create src dir");
    std::fs::write(&path, source).expect("write source");
    let sources = ProjectSources::new().with(&path, source);

    let snapshot = analyze_project(&root, &config(), &sources).expect("analyze");
    std::fs::remove_dir_all(&root).ok();
    let before = snapshot.report.diagnostics.clone();
    assert!(!snapshot.report.has_errors(), "{before:#?}");
    let program = &snapshot.program;
    let parsed = &snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("the overlaid file is analyzed")
        .parsed;

    // Real answers, so the no-diagnostics guarantee is not vacuously true.
    let title = source.rfind("title").expect("use of title");
    assert_eq!(
        type_at(program, &path, parsed, title),
        Some(MarrowType::Primitive(ScalarType::Str)),
    );
    assert!(!scope_at(program, &path, parsed, title).is_empty());

    // Sweeping every offset is total and never panics, and leaves the analysis
    // report it was derived from untouched — the queries have no diagnostics sink.
    for offset in 0..=source.len() {
        let _ = type_at(program, &path, parsed, offset);
        let _ = scope_at(program, &path, parsed, offset);
    }
    assert_eq!(
        snapshot.report.diagnostics, before,
        "queries left the report unchanged"
    );
}
