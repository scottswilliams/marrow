//! Present-or-clear saved writes and `?.`/`.` off a materialized optional record.
//!
//! A clearable saved place — a sparse field or a keyed leaf — accepts `absent` and a
//! `T?` value (present-or-clear); a `required` field and a positional sequence element
//! keep their bare `T` and reject an unresolved `T?` (the one rule). A `?.` off a
//! materialized `R?` types the member optional; a plain `.` off one is the one rule.

use crate::support;
use marrow_check::{
    CHECK_UNANNOTATED_ABSENT, CHECK_UNRESOLVED_OPTIONAL, CHECK_UNTYPED_VALUE, CheckReport,
};

use support::{check_module_report, with_code};

const STORE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     \x20   counts(key: string): int\n\
     \x20   xs(pos: int): string\n\
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

#[test]
fn absent_into_a_sparse_field_is_present_or_clear() {
    let report = report("sparse-absent", "fn f()\n    ^books(1).subtitle = absent\n");
    assert_clean(&report);
}

#[test]
fn absent_into_a_required_field_is_the_one_rule() {
    let report = report("required-absent", "fn f()\n    ^books(1).title = absent\n");
    assert_one_rule(&report);
}

#[test]
fn absent_into_a_keyed_leaf_is_present_or_clear() {
    let report = report(
        "keyed-leaf-absent",
        "fn f()\n    ^books(1).counts(\"a\") = absent\n",
    );
    assert_clean(&report);
}

#[test]
fn absent_into_a_positional_sequence_element_is_the_one_rule() {
    let report = report(
        "positional-absent",
        "fn f()\n    ^books(1).xs(2) = absent\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_optional_value_into_a_sparse_field_is_present_or_clear() {
    let report = report(
        "sparse-optional-value",
        "fn put(s: string?)\n    ^books(1).subtitle = s\n",
    );
    assert_clean(&report);
}

#[test]
fn a_present_string_into_a_sparse_field_stays_clean() {
    let report = report(
        "sparse-present",
        "fn f()\n    ^books(1).subtitle = \"hi\"\n",
    );
    assert_clean(&report);
}

#[test]
fn an_optional_chain_off_a_materialized_optional_record_types_optional() {
    let report = report(
        "materialized-optchain",
        "fn f(id: Id(^books)): string?\n    const b = ^books(id)\n    return b?.subtitle\n",
    );
    assert_clean(&report);
    assert!(
        with_code(&report, CHECK_UNTYPED_VALUE).is_empty(),
        "a `?.` off a materialized optional record must type `string?`, not untyped: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_plain_dot_off_a_materialized_optional_record_is_the_one_rule() {
    let report = report(
        "materialized-plain-dot",
        "fn f(id: Id(^books))\n    const b = ^books(id)\n    print(b.subtitle)\n",
    );
    assert_one_rule(&report);
}

#[test]
fn a_plain_dot_off_a_materialized_optional_record_in_interpolation_is_the_one_rule() {
    let report = report(
        "materialized-plain-dot-interp",
        "fn f(id: Id(^books))\n    const b = ^books(id)\n    print($\"v={b.subtitle}\")\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_optional_chain_used_where_a_plain_string_is_required_is_the_one_rule() {
    let report = report(
        "materialized-optchain-plain-slot",
        "fn f(id: Id(^books))\n    const b = ^books(id)\n    const s: string = b?.subtitle\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_optional_chain_off_a_definite_record_param_into_a_return_is_the_one_rule() {
    let report = report(
        "definite-optchain-return",
        "fn f(b: Book): string\n    return b?.subtitle\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_optional_chain_off_a_definite_record_param_into_an_assignment_is_the_one_rule() {
    let report = report(
        "definite-optchain-assign",
        "fn f(b: Book)\n    const s: string = b?.subtitle\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_optional_chain_off_an_if_const_record_into_a_render_slot_is_the_one_rule() {
    let report = report(
        "ifconst-optchain-print",
        "fn f(id: Id(^books))\n    if const b = ^books(id)\n        print(b?.subtitle)\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_optional_chain_off_a_definite_record_param_into_an_optional_slot_stays_clean() {
    let report = report(
        "definite-optchain-optional-slot",
        "fn f(b: Book): string?\n    return b?.subtitle\n",
    );
    assert_clean(&report);
}

#[test]
fn an_unannotated_bare_absent_binding_must_name_its_optional_type() {
    let report = report("absent-unannotated", "fn f()\n    var v = absent\n");
    assert!(
        !with_code(&report, CHECK_UNANNOTATED_ABSENT).is_empty(),
        "a bare `absent` binding carries no element type and must be annotated: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_annotated_absent_binding_is_clean() {
    let report = report("absent-annotated", "fn f()\n    var v: string? = absent\n");
    assert_clean(&report);
}
