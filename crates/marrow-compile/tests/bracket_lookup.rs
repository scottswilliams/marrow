//! Local bracket lookup and assignment (`xs[i]`, `m[k]`, `m[k] = v`) through the
//! production `compile` path: the presence-typed optional read, the 1-based list
//! positions with their literal dead-index teaching diagnostic, the map create-or-
//! replace write, the refused list keyed write, and the deletion of the `get`/`insert`
//! builtins. Diagnostics are asserted by typed code and, for the teaching diagnostics
//! this lane mints, by the governing teaching sentence.

use marrow_compile::{Compiled, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

fn project(source: &str) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn compile_ok(source: &str) -> Compiled {
    compile(&project(source)).unwrap_or_else(|diagnostics| {
        panic!("expected a clean compile, got {diagnostics:#?}");
    })
}

fn compile_err(source: &str) -> Vec<SourceDiagnostic> {
    match compile(&project(source)) {
        Ok(_) => panic!("expected a diagnostic, but the program compiled"),
        Err(diagnostics) => diagnostics,
    }
}

fn first_of(diagnostics: &[SourceDiagnostic], code: &str) -> SourceDiagnostic {
    diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == code)
        .unwrap_or_else(|| panic!("no `{code}` diagnostic in {diagnostics:#?}"))
        .clone()
}

fn wrap(body: &str) -> String {
    format!("module main\n\n{body}\n")
}

/// A local list bracket read and a map bracket read both type as the presence-typed
/// optional, consumed by `??`, `if const`, let-else, and an `else` clause.
#[test]
fn a_bracket_read_yields_the_optional_consumed_by_the_presence_family() {
    compile_ok(&wrap(
        r#"pub fn coalesce(xs: List<int>): int {
    return xs[1] ?? 0
}

pub fn guarded(xs: List<int>): int {
    if const first = xs[1] {
        return first
    }
    return 0
}

pub fn let_else(m: Map<string, int>): int {
    const hit = m["k"] else { return 0 }
    return hit
}

pub fn else_clause(m: Map<string, int>): int {
    const hit = m["k"] else return 0
    return hit
}"#,
    ));
}

/// The literal dead indexes `xs[0]` and `xs[-1]` are refused with a `check.type`
/// teaching diagnostic: list positions count from 1.
#[test]
fn a_literal_dead_list_index_is_a_teaching_check_type() {
    for index in ["0", "-1"] {
        let diagnostics = compile_err(&wrap(&format!(
            "pub fn f(xs: List<int>): int {{\n    return xs[{index}] ?? 0\n}}"
        )));
        let diagnostic = first_of(&diagnostics, "check.type");
        // Fact-first (voice standard, rule 1): the source spelling of the dead index
        // leads, the governing law follows, the canonical first position closes.
        assert!(
            diagnostic
                .message
                .starts_with(&format!("`xs[{index}]` names no list position")),
            "dead index {index}: {}",
            diagnostic.message
        );
        assert!(
            diagnostic.message.contains("List positions count from 1")
                && diagnostic.message.contains("`xs[1]`"),
            "dead index {index}: {}",
            diagnostic.message
        );
    }
}

/// A positive literal past the end is not statically dead — the length is a runtime
/// fact — so `xs[100]` compiles and reads absent rather than being refused.
#[test]
fn a_positive_literal_index_is_not_a_dead_index() {
    compile_ok(&wrap(
        "pub fn f(xs: List<int>): int {\n    return xs[100] ?? 0\n}",
    ));
}

/// A `Map<int, V>` key of `0` is an ordinary key, exempt from the list dead-zero rule.
#[test]
fn a_map_int_key_zero_is_admitted() {
    compile_ok(&wrap(
        "pub fn f(m: Map<int, int>): int {\n    return m[0] ?? -1\n}",
    ));
}

/// `m[k] = v` on a `var` map binding is create-or-replace; a `const` binding gets the
/// ordinary assignment-to-const rejection.
#[test]
fn a_map_bracket_write_needs_a_var_binding() {
    compile_ok(&wrap(
        "pub fn f(): int {\n    var m: Map<string, int> = Map()\n    m[\"k\"] = 1\n    m[\"k\"] = 9\n    return m[\"k\"] ?? 0\n}",
    ));
    let diagnostics = compile_err(&wrap(
        "pub fn f(): int {\n    const m: Map<string, int> = Map()\n    m[\"k\"] = 1\n    return 0\n}",
    ));
    let diagnostic = first_of(&diagnostics, "check.type");
    assert!(
        diagnostic.message.contains("`const`")
            && diagnostic.message.contains("cannot be reassigned"),
        "{}",
        diagnostic.message
    );
}

/// A list has no keyed write: `xs[i] = v` is a `check.type` teaching diagnostic naming
/// `append` for growth and `Map<int, T>` for keyed replacement at a position.
#[test]
fn a_list_keyed_write_is_a_teaching_check_type() {
    let diagnostics = compile_err(&wrap(
        "pub fn f(): int {\n    var xs: List<int> = List()\n    xs = append(xs, 1)\n    xs[1] = 9\n    return 0\n}",
    ));
    let diagnostic = first_of(&diagnostics, "check.type");
    // Fact-first: the list is named in source spelling, the law follows, and the fix
    // names the user's own right-hand side (`9`) with the canonical spellings.
    assert!(
        diagnostic.message.starts_with("`xs` is a list"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic.message.contains("append(xs, 9)")
            && diagnostic.message.contains("Map<int, int>"),
        "{}",
        diagnostic.message
    );
}

/// The `get` and `insert` builtins are deleted: a bare call to either is an unresolved
/// name (`check.type`, not in scope), and a user-defined function of the same name
/// still shadows cleanly.
#[test]
fn get_and_insert_are_deleted_but_shadowable() {
    for name in ["get", "insert"] {
        let diagnostics = compile_err(&wrap(&format!(
            "pub fn f(m: Map<string, int>): int {{\n    return {name}(m, \"k\") ?? 0\n}}"
        )));
        let diagnostic = first_of(&diagnostics, "check.type");
        assert!(
            diagnostic.message.contains("not in scope"),
            "{name}: {}",
            diagnostic.message
        );
    }
    compile_ok(&wrap(
        "fn insert(a: int, b: int): int {\n    return a + b\n}\n\npub fn f(): int {\n    return insert(2, 3)\n}",
    ));
    compile_ok(&wrap(
        "fn get(a: int): int {\n    return a\n}\n\npub fn f(): int {\n    return get(7)\n}",
    ));
}
