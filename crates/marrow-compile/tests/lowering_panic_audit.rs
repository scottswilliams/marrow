//! Lowering-panic audit (E06 M2): lowering reports every source-level problem as a
//! typed diagnostic and never panics.
//!
//! `lower.rs` carries `expect`/`unreachable!`/`panic!` sites that assert invariants the
//! parser, checker, match-arm narrowing, or lowering's own bookkeeping establish before
//! the panicking line (see the module header there for the full classification). This
//! test drives one adversarial source shape per invariant class through the production
//! `compile` path. Each must come back as a typed diagnostic — a `compile` that returned
//! `Err` proves lowering did not abort, and the asserted code proves the checker
//! intercepted the shape before a lowering invariant could be violated. A regression
//! that turned any of these into a panic would abort this test process instead of
//! returning `Err`, making the failure conspicuous.

use marrow_compile::{SourceDiagnostic, compile};
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

/// `compile` must return a diagnostic carrying `code`, not panic and not succeed.
fn rejects_with(source: &str, code: &str) {
    match compile(&project(source)) {
        Ok(_) => panic!("expected `{code}`, but the program compiled:\n{source}"),
        Err(diagnostics) => assert!(
            diagnostics
                .iter()
                .any(|d: &SourceDiagnostic| d.code == code),
            "expected `{code}` for:\n{source}\ngot {diagnostics:#?}",
        ),
    }
}

/// Loop-bookkeeping class: `break`/`continue` reach lowering only inside a loop, where
/// the loop context is present. Outside a loop the checker rejects them first.
#[test]
fn break_and_continue_outside_a_loop_are_diagnostics_not_panics() {
    rejects_with(
        "pub fn f(): int {\n    break\n    return 0\n}\n",
        "check.type",
    );
    rejects_with(
        "pub fn f(): int {\n    continue\n    return 0\n}\n",
        "check.type",
    );
}

/// Checker-classified-type class: a `match` scrutinee lowers only after it resolves to
/// an enum. A scrutinee that is not an enum is rejected before lowering.
#[test]
fn a_match_on_a_non_enum_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(n: int): int {\n    match n {\n        x => return x\n    }\n}\n",
        "check.match_arm",
    );
}

/// Match-arm-narrowing class: a builtin dispatch reaches its op only after the caller
/// matched its name and arity. A mis-arity call is rejected before that point.
#[test]
fn a_mis_arity_builtin_call_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(s: string): int {\n    return length(s, s)\n}\n",
        "check.type",
    );
}

/// Op-classification class: an arithmetic/comparison op lowers only after its operands
/// type-check. An ill-typed operator is rejected before op classification.
#[test]
fn an_ill_typed_operator_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(a: string, b: string): int {\n    return a / b\n}\n",
        "check.type",
    );
}

/// Enum-classification class: a bare enum member lowers only after it resolves to its
/// enum's variants. An unresolved member is rejected before lowering reaches it.
#[test]
fn an_unresolved_enum_member_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(): int {\n    const x = Nope::member\n    return 0\n}\n",
        "check.unsupported",
    );
}

/// List-literal class: the inferred-element path runs only for a non-empty list. An
/// empty `List()` with no element or annotation type is rejected before that path.
#[test]
fn an_empty_inferred_list_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(): int {\n    const xs = List()\n    return 0\n}\n",
        "check.type",
    );
}
