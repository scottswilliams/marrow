//! Lowering reports every source-level problem as a typed diagnostic and never
//! aborts.
//!
//! The crate denies the explicit-abort families in non-test builds
//! (`crates/marrow-compile/src/lib.rs`), so an added or moved abort fails Clippy at its own
//! site and no source-text scan belongs here.
//!
//! What this file owns is behavioral: one adversarial source shape per invariant class is
//! driven through the production `compile` path. `Err` proves lowering did not abort, and
//! the asserted code proves the checker intercepted the shape before a lowering invariant
//! could be violated; a regression into a panic aborts the test process instead.

use marrow_codes::Code;
use marrow_compile::{SourceDiagnostic, compile};
use marrow_project::ProjectInput;

use super::project_capture;

const EXPRS_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/lower/exprs.rs");

/// The generic-enum call guard and constructor share an immutable registry borrow, so the
/// successful template lookup is bound once rather than repeated behind an expectation.
/// Reading the single owning file keeps that conspicuous without a source scanner.
#[test]
fn generic_enum_dispatch_binds_one_template_lookup() {
    let source = std::fs::read_to_string(EXPRS_FILE).expect("lower/exprs.rs is readable");
    let body = source
        .split_once("fn lower_call_core(")
        .expect("lower_call_core remains present")
        .1
        .split_once("/// An unqualified call")
        .expect("next owner boundary remains present")
        .0;
    assert_eq!(
        body.matches("type_template_by_name(&self.bare_type(enum_name.text()))")
            .count(),
        1,
        "the immutable successful lookup must be consumed directly"
    );
}

fn project(source: &str) -> ProjectInput {
    project_capture::project(&[("src/main.mw", source)])
}

/// `compile` must return a diagnostic carrying `code`, not panic and not succeed.
fn rejects_with(source: &str, code: Code) {
    match compile(&project(source)) {
        Ok(_) => panic!("expected `{code:?}`, but the program compiled:\n{source}"),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => assert!(
            diagnostics
                .iter()
                .any(|d: &SourceDiagnostic| d.code() == code),
            "expected `{code:?}` for:\n{source}\ngot {diagnostics:#?}",
        ),
        Err(marrow_compile::CompileFailure::ResourceLimit(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
        Err(marrow_compile::CompileFailure::Invariant(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    }
}

/// Loop bookkeeping: `break`/`continue` reach lowering only inside a loop, where the loop
/// context is present.
#[test]
fn break_and_continue_outside_a_loop_are_diagnostics_not_panics() {
    rejects_with(
        "pub fn f(): int {\n    break\n    return 0\n}\n",
        Code::CheckType,
    );
    rejects_with(
        "pub fn f(): int {\n    continue\n    return 0\n}\n",
        Code::CheckType,
    );
}

/// Checker-classified types: a `match` scrutinee lowers only after it resolves to an enum.
#[test]
fn a_match_on_a_non_enum_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(n: int): int {\n    match n {\n        x => return x\n    }\n}\n",
        Code::CheckMatchArm,
    );
}

/// Match-arm narrowing: a builtin dispatch reaches its op only after the caller matched its
/// name and arity.
#[test]
fn a_mis_arity_builtin_call_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(s: string): int {\n    return length(s, s)\n}\n",
        Code::CheckType,
    );
}

/// Op classification: an arithmetic or comparison op lowers only after its operands
/// type-check.
#[test]
fn an_ill_typed_operator_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(a: string, b: string): int {\n    return a / b\n}\n",
        Code::CheckType,
    );
}

/// Enum classification: a bare enum member lowers only after it resolves to its enum's
/// variants.
#[test]
fn an_unresolved_enum_member_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(): int {\n    const x = Nope::member\n    return 0\n}\n",
        Code::CheckUnsupported,
    );
}

/// List literals: the inferred-element path runs only for a non-empty list.
#[test]
fn an_empty_inferred_list_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(): int {\n    const xs = List()\n    return 0\n}\n",
        Code::CheckType,
    );
}
