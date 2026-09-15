//! End-to-end `alias` tests: the transparent `alias Name = Type` declaration
//! travels the real production path (capture → compile → encode → verify → VM)
//! through the built binary, via the `alias_types` conformance fixture and
//! inline invalid-source projects asserting typed diagnostics.

use crate::common::{Diagnostics, Project, conformance_dir, marrow_in};

/// The typed diagnostics from a project the compiler must refuse.
fn source_diagnostics(source: &str) -> Diagnostics {
    Project::single(source)
        .try_image()
        .expect_err("expected source diagnostics, got a compiled image")
}

/// The alias conformance fixture passes end to end: every `test` declaration
/// using aliases in parameter, return, constant, optional, and resource-field
/// positions reports `passed` through the production path.
#[test]
fn alias_conformance_fixture_passes_on_the_production_path() {
    let output = marrow_in(
        &conformance_dir("alias_types"),
        &["test", "--format", "jsonl"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "alias fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":5"#), "{summary}");
}

/// A cyclic alias chain is a typed `check.recursion` diagnostic, reported once
/// per alias on the cycle, at check time.
#[test]
fn a_cyclic_alias_chain_is_a_check_recursion_diagnostic() {
    let workspace = Project::single(
        r#"alias A = B

alias B = A

pub fn f(): int {
    return 1
}
"#,
    )
    .materialize("alias-cycle");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "a cycle must fail: {stdout}");
    // One typed record per alias on the cycle, at each declaration's name.
    let recursion_lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.contains(r#""code":"check.recursion""#))
        .collect();
    assert_eq!(recursion_lines.len(), 2, "{stdout}");
    assert!(recursion_lines[0].contains(r#""line":1"#), "{stdout}");
    assert!(recursion_lines[1].contains(r#""line":3"#), "{stdout}");
}

/// A self-referential alias is the one-element cycle.
#[test]
fn a_self_referential_alias_is_a_check_recursion_diagnostic() {
    let workspace = Project::single(
        r#"alias Loop = Loop?

pub fn f(): int {
    return 1
}
"#,
    )
    .materialize("alias-self");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "a cycle must fail: {stdout}");
    assert!(stdout.contains("check.recursion"), "{stdout}");
}

/// Two aliases with one name collide, as do an alias and a resource: names a
/// type annotation resolves against are unique across the project.
#[test]
fn duplicate_alias_names_are_name_conflicts() {
    let workspace = Project::single(
        r#"alias Count = int

alias Count = string

pub fn f(): int {
    return 1
}
"#,
    )
    .materialize("alias-dup");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "a duplicate must fail: {stdout}");
    assert!(stdout.contains("check.name_conflict"), "{stdout}");

    let workspace = Project::single(
        r#"resource Item {
    required count: int
}

alias Item = int

pub fn f(): int {
    return 1
}
"#,
    )
    .materialize("alias-resource-clash");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "an alias/resource clash must fail: {stdout}"
    );
    assert!(stdout.contains("check.name_conflict"), "{stdout}");
}

/// An alias whose expansion names no known type is a typed `check.type`
/// diagnostic at the alias declaration, even when the alias is unused.
#[test]
fn an_alias_to_an_unknown_type_is_a_check_type_diagnostic() {
    let workspace = Project::single(
        r#"alias Broken = Missing

pub fn f(): int {
    return 1
}
"#,
    )
    .materialize("alias-unknown");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "unknown target must fail: {stdout}"
    );
    let diagnostic = stdout
        .lines()
        .find(|line| line.contains(r#""code":"check.type""#))
        .unwrap_or_else(|| panic!("no check.type record: {stdout}"));
    assert!(diagnostic.contains(r#""line":1"#), "{stdout}");
}

/// Alias transparency does not relax the optional-nesting rule: `M?` where `M`
/// expands to `int?` is still a doubled optional and rejects.
#[test]
fn an_alias_cannot_smuggle_a_nested_optional() {
    let workspace = Project::single(
        r#"alias MaybeInt = int?

pub fn f(v: bool): MaybeInt? {
    return absent
}
"#,
    )
    .materialize("alias-nested-opt");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a doubled optional must fail: {stdout}"
    );
    assert!(stdout.contains("check."), "{stdout}");
}

/// A keyword cannot name an alias; the parser reports it at the declaration.
#[test]
fn a_keyword_alias_name_is_a_parse_error() {
    let workspace = Project::single(
        "alias int = string\n\
         \n\
         pub fn f(): int\n\
         \x20   return 1\n",
    )
    .materialize("alias-keyword");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a keyword name must fail: {stdout}"
    );
    assert!(stdout.contains("parse.syntax"), "{stdout}");
}

/// Cycles distinguish their members from dependent aliases. Unsupported
/// applications are refused before their names can enter the alias graph.
#[test]
fn alias_cycle_membership_order_and_spans_follow_the_alias_owner() {
    let diagnostics = source_diagnostics(
        r#"alias Zed = Alpha

alias Alpha = Zed

alias Self = Self?

alias Tail = Alpha

alias Plain = int

alias PlainAlias = Plain

alias Head = Wrapped

alias Wrapped = Head<int>

pub fn f(value: PlainAlias): PlainAlias {
    return value
}
"#,
    );
    let observed: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| {
            let span = diagnostic.span();
            (
                diagnostic.code().as_str(),
                span.start_byte,
                span.end_byte,
                span.line,
                span.column,
            )
        })
        .collect();
    assert_eq!(
        observed,
        vec![
            // Unsupported target shapes are refused before dependency normalization.
            ("check.unsupported", 145, 170, 15, 1),
            ("check.recursion", 25, 30, 3, 7),
            ("check.recursion", 44, 48, 5, 7),
            ("check.recursion", 6, 9, 1, 7),
            ("check.unsupported", 123, 143, 13, 1),
            // `Tail` names `Alpha`, a declaration this project wrote and the compiler
            // refused for the cycle above. The steer reuses that declaring code; calling
            // the name unknown would fabricate an absence for a name declared four lines
            // up.
            ("check.recursion", 58, 76, 7, 1),
        ],
        "observed diagnostics: {:?}",
        diagnostics.all()
    );
}

/// A small accepted alias chain freezes its current-generation canonical image
/// through its domain-separated identity and exact encoded length.
#[test]
fn accepted_alias_image_bytes_remain_frozen() {
    let compiled = Project::single(
        r#"alias Count = int

alias OtherCount = Count

pub fn identity(value: OtherCount): Count {
    return value
}
"#,
    )
    .compiled();
    assert_eq!(
        (compiled.image.bytes.len(), compiled.image.image_id.to_hex()),
        (
            240,
            "b5f02b550278b3ab5f19eb9ca5d31351b8307445b3ae68a0ab78a1af5381583f".to_string(),
        ),
        "accepted alias fixture image identity changed"
    );
}
