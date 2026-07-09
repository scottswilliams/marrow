//! An optional *annotation* (`Book?`, `Status?`, `Id(^books)?`, `int?`,
//! `sequence[int]?`) keeps its inner type: it resolves to `Optional(Resource)`,
//! `Optional(Enum)`, `Optional(Identity)`, `Optional(Primitive)`, or
//! `Optional(Sequence)`, never `Optional(Unknown)`.
//!
//! The earlier blind spot was that the `?.` suite only fed an un-annotated
//! `const b = ^books(id)` whose type flowed from the saved read, so a resource- or
//! enum-optional *annotation* that dropped its inner type to `Unknown` went
//! unnoticed. These fixtures annotate a parameter and a local as each leaf kind and
//! drive the production pipeline so a lost inner type surfaces as a false-positive,
//! a missing diagnostic, or an `Unknown` deferral escape.

use crate::support;
use marrow_check::{
    CHECK_ASSIGNMENT_TYPE, CHECK_OPERATOR_TYPE, CHECK_UNKNOWN_FIELD, CHECK_UNRESOLVED_OPTIONAL,
    CheckReport, CheckedProgram, DiagnosticPayload, MarrowType,
};

use support::{check_module_program, check_module_report, resource_id, with_code};

const STORE: &str = "module m\n\
     enum Status\n\
     \x20   active\n\
     \x20   archived\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     store ^books(id: int): Book\n\n";

fn report(name: &str, body: &str) -> CheckReport {
    check_module_report(name, &format!("{STORE}{body}"))
}

fn assert_clean(report: &CheckReport) {
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

fn assert_one_rule(report: &CheckReport) {
    assert!(
        !with_code(report, CHECK_UNRESOLVED_OPTIONAL).is_empty(),
        "expected the one rule: {:#?}",
        report.diagnostics
    );
}

fn book_optional(program: &CheckedProgram) -> MarrowType {
    MarrowType::Optional(Box::new(MarrowType::Resource(resource_id(
        program, "m", "Book",
    ))))
}

fn status_optional(program: &CheckedProgram) -> MarrowType {
    MarrowType::Optional(Box::new(MarrowType::Enum(support::enum_id(
        program, "m", "Status",
    ))))
}

/// `b?.subtitle` reads through a maybe-present record to a sparse field, so it types
/// `string?` and flows into a `string?` return without the one rule. If the `Book?`
/// parameter had degraded to `Optional(Unknown)`, the `?.` member would type
/// `Unknown` and this would still pass — so the definite-slot fixture below is the
/// discriminator. This is the spec's named false positive that must stay clean.
#[test]
fn an_optional_record_chain_flows_into_an_optional_return() {
    let report = report(
        "resource-optional-chain-into-optional",
        "fn f(b: Book?): string?\n    return b?.subtitle\n",
    );
    assert_clean(&report);
}

/// The same `string?` chain into a definite `string` return is the one rule: the
/// value is optional, the slot is not.
#[test]
fn an_optional_record_chain_into_a_definite_return_is_the_one_rule() {
    let report = report(
        "resource-optional-chain-into-definite",
        "fn f(b: Book?): string\n    return b?.subtitle\n",
    );
    assert_one_rule(&report);
}

/// `if const r = b` strips one optional layer, binding `r` at `Book`. A required
/// field then reads bare `string` and returns clean. A `Book?` that lost its inner
/// type would bind `r` at `Unknown`, whose `.title` defers — so a green check here
/// only means something paired with the unknown-field escape below.
#[test]
fn a_present_bound_record_reads_a_required_field_clean() {
    let report = report(
        "resource-binding-required-field",
        "fn f(b: Book?): string\n    if const r = b\n        return r.title\n    return \"z\"\n",
    );
    assert_clean(&report);
}

/// The unknown-field escape: `if const r = b` binds `r` at `Book`, so `r.nonexistent`
/// is `check.unknown_field` exactly like a definite `Book`. A `Book?` that degraded
/// to `Optional(Unknown)` would bind `r` at `Unknown` and silently defer the bad
/// field, checking clean — the precise hole this fixture closes.
#[test]
fn a_present_bound_record_reports_unknown_field_like_a_definite_record() {
    let report = report(
        "resource-binding-unknown-field",
        "fn f(b: Book?): string\n    if const r = b\n        return r.nonexistent ?? \"y\"\n    return \"z\"\n",
    );
    assert!(
        !with_code(&report, CHECK_UNKNOWN_FIELD).is_empty(),
        "an optional-record binding must report check.unknown_field like a definite record: {:#?}",
        report.diagnostics
    );
}

/// The same escape reached through a *local* `const cb: Book?` annotation, proving
/// the local annotation — not only the parameter — keeps its inner resource type.
#[test]
fn a_local_optional_record_annotation_reports_unknown_field() {
    let report = report(
        "local-resource-optional-unknown-field",
        "fn f(b: Book?): string\n    const cb: Book? = b\n    if const r = cb\n        return r.nonexistent ?? \"y\"\n    return \"z\"\n",
    );
    assert!(
        !with_code(&report, CHECK_UNKNOWN_FIELD).is_empty(),
        "a local Book? annotation must keep its resource identity: {:#?}",
        report.diagnostics
    );
}

/// `if const v = s` binds `v` at `Status`, so a same-enum comparison is clean. A
/// `Status?` that lost its inner type would bind `v` at `Unknown`, which also checks
/// clean here — the mismatch fixture below is the discriminator.
#[test]
fn a_present_bound_enum_compares_clean_against_its_own_member() {
    let report = report(
        "enum-binding-compare-clean",
        "fn f(s: Status?): bool\n    if const v = s\n        return v == Status::active\n    return false\n",
    );
    assert_clean(&report);
}

/// `if const v = s` binds `v` at `Status`; comparing it to an `int` is a
/// `check.operator_type` cross-type error. A `Status?` that degraded to
/// `Optional(Unknown)` would bind `v` at `Unknown`, whose `==` defers and admits the
/// mismatch — the enum half of the unknown-deferral escape.
#[test]
fn a_present_bound_enum_rejects_a_cross_type_comparison() {
    let report = report(
        "enum-binding-compare-mismatch",
        "fn f(s: Status?): bool\n    if const v = s\n        return v == 1\n    return false\n",
    );
    assert!(
        !with_code(&report, CHECK_OPERATOR_TYPE).is_empty(),
        "an optional-enum binding compared to an int must raise check.operator_type: {:#?}",
        report.diagnostics
    );
}

/// The explicit never-`Unknown` oracle for a resource-optional: assigning a `Book?`
/// into a `Status?` slot recurses through the optional layer to a `Resource` vs
/// `Enum` mismatch whose `found` payload is exactly `Optional(Resource("m::Book"))`.
/// A `Book?` that resolved to `Optional(Unknown)` would make the inner `Status` vs
/// `Unknown` recursion defer, emitting no diagnostic at all.
#[test]
fn an_optional_resource_annotation_never_types_as_unknown() {
    let (found, program) = check_module_program(
        "resource-optional-never-unknown",
        &format!("{STORE}fn f(b: Book?)\n    const x: Status? = b\n"),
        CHECK_ASSIGNMENT_TYPE,
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::TypeMismatch {
            expected: status_optional(&program),
            found: book_optional(&program),
        },
        "{found:#?}"
    );
}

/// The mirror oracle for an enum-optional: assigning a `Status?` into a `Book?` slot
/// yields a mismatch whose `found` payload is exactly `Optional(Enum{m::Status})`,
/// proving the `Status?` annotation kept its nominal enum identity.
#[test]
fn an_optional_enum_annotation_never_types_as_unknown() {
    let (found, program) = check_module_program(
        "enum-optional-never-unknown",
        &format!("{STORE}fn f(s: Status?)\n    const x: Book? = s\n"),
        CHECK_ASSIGNMENT_TYPE,
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::TypeMismatch {
            expected: book_optional(&program),
            found: status_optional(&program),
        },
        "{found:#?}"
    );
}

/// The remaining leaf kinds — identity, scalar, sequence — compose as parameters,
/// returns, and locals without the one rule: each optional flows into a matching
/// optional slot. These already resolved correctly (scalar/sequence/identity never
/// fell into the `Type::Named` `Unknown` fallback), so this pins the family.
#[test]
fn identity_scalar_and_sequence_optionals_compose_as_params_returns_and_locals() {
    let report = report(
        "leaf-optionals-compose",
        "fn ident(i: Id(^books)?): Id(^books)?\n    const ci: Id(^books)? = i\n    return ci\n\
         fn scalar(n: int?): int?\n    const cn: int? = n\n    return cn\n\
         fn seq(xs: sequence[int]?): sequence[int]?\n    const cxs: sequence[int]? = xs\n    return cxs\n",
    );
    assert_clean(&report);
}
