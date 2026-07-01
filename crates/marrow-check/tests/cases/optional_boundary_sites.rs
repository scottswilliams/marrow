//! The one rule at the collection/place builtins and the argument boundaries.
//!
//! A maybe-present collection (`sequence[T]?`) cannot be iterated, counted, or appended
//! to until it is resolved, and a `T?` value passed as a user-function argument or a
//! saved key argument is the one rule routed through the shared `unresolved_optional`
//! helper so the message names the four resolution forms, not a generic argument or
//! key-type mismatch.

use crate::support;
use marrow_check::{
    CHECK_CALL_ARGUMENT, CHECK_COLLECTION_UNSUPPORTED, CHECK_KEY_TYPE, CHECK_OPERATOR_TYPE,
    CHECK_UNRESOLVED_OPTIONAL, CHECK_UNTYPED_VALUE, CheckReport,
};

use support::{check_module_report, with_code};

const STORE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     \x20   counts(key: string): int\n\
     \x20   scores(pos: int): int\n\
     store ^books(id: int): Book\n\n";

fn report(name: &str, body: &str) -> CheckReport {
    check_module_report(name, &format!("{STORE}{body}"))
}

fn assert_one_rule(report: &CheckReport) {
    assert!(
        !with_code(report, CHECK_UNRESOLVED_OPTIONAL).is_empty(),
        "expected the one rule: {:#?}",
        report.diagnostics
    );
}

fn assert_clean(report: &CheckReport) {
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn iterating_a_maybe_present_sequence_is_the_one_rule() {
    let report = report(
        "for-optional-sequence",
        "fn f(xs: sequence[int]?)\n    for x in xs\n        print(x)\n",
    );
    assert_one_rule(&report);
}

#[test]
fn counting_a_maybe_present_sequence_is_the_one_rule() {
    let report = report(
        "count-optional-sequence",
        "fn f(xs: sequence[int]?): int\n    return count(xs)\n",
    );
    assert_one_rule(&report);
}

#[test]
fn appending_to_a_maybe_present_sequence_is_the_one_rule() {
    let report = report(
        "append-optional-sequence",
        "fn f(xs: sequence[int]?)\n    append(xs, 1)\n",
    );
    assert_one_rule(&report);
}

#[test]
fn iterating_a_definite_sequence_stays_clean() {
    let report = report(
        "for-definite-sequence",
        "fn f(xs: sequence[int])\n    for x in xs\n        print(x)\n",
    );
    assert_clean(&report);
}

#[test]
fn a_resolved_sequence_iterates_clean() {
    let report = report(
        "for-resolved-sequence",
        "fn f(xs: sequence[int]?)\n    if const ys = xs\n        for x in ys\n            print(x)\n",
    );
    assert_clean(&report);
}

#[test]
fn an_optional_user_function_argument_is_the_one_rule_not_a_generic_mismatch() {
    let report = report(
        "optional-fn-arg",
        "fn need(s: string)\n    print(s)\nfn caller(v: string?)\n    need(v)\n",
    );
    assert_one_rule(&report);
    assert!(
        with_code(&report, CHECK_CALL_ARGUMENT).is_empty(),
        "an optional argument must route through the one rule, not `check.call_argument`: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_optional_saved_key_argument_is_the_one_rule_not_a_key_type_mismatch() {
    let report = report(
        "optional-key-arg",
        "fn f(v: string?)\n    const c: int? = ^books(1).counts(v)\n",
    );
    assert_one_rule(&report);
    assert!(
        with_code(&report, CHECK_KEY_TYPE).is_empty(),
        "an optional key argument must route through the one rule, not `check.key_type`: {:#?}",
        report.diagnostics
    );
}

/// A top-level `append` to a purely local sequence is not a saved write, so it must
/// not expire a still-present saved narrowing.
#[test]
fn a_local_append_keeps_a_saved_narrowing() {
    let report = report(
        "local-append-keeps-narrowing",
        "fn f(ks: sequence[int])\n    \
         if not exists(^books(1).subtitle)\n        return\n    \
         append(ks, 1)\n    \
         const s: string = ^books(1).subtitle\n",
    );
    assert_clean(&report);
}

/// An `append` whose target is a saved layer still expires saved narrowings: the
/// local-target exemption must not reach a saved collection.
#[test]
fn a_saved_append_still_expires_a_saved_narrowing() {
    let report = check_module_report(
        "saved-append-expires-narrowing",
        "module m\n\
         resource Book\n\
         \x20   subtitle: string\n\
         \x20   tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    \
         if not exists(^books(1).subtitle)\n        return\n    \
         append(^books(1).tags, \"x\")\n    \
         const s: string = ^books(1).subtitle\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_optional_operator_operand_routes_through_the_one_rule_not_a_generic_operator_error() {
    for body in [
        "fn f(v: int?)\n    v + 1\n",
        "fn f(v: int?)\n    v == 1\n",
        "fn f(v: int?, w: int?)\n    v == w\n",
        "fn f(v: int?)\n    not v\n",
        "fn f(v: int?)\n    -v\n",
        "fn f(v: int?)\n    v < 1\n",
    ] {
        let report = report("optional-operator-operand", body);
        assert_one_rule(&report);
        assert!(
            with_code(&report, CHECK_OPERATOR_TYPE).is_empty(),
            "an optional operand must route through the one rule, not `check.operator_type`: {body:?}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn an_optional_conversion_source_routes_through_the_one_rule_not_a_generic_mismatch() {
    for body in [
        "fn f(v: int?)\n    string(v)\n",
        "fn f(v: int?)\n    int(v)\n",
    ] {
        let report = report("optional-conversion-source", body);
        assert_one_rule(&report);
        assert!(
            with_code(&report, CHECK_CALL_ARGUMENT).is_empty(),
            "an optional conversion source must route through the one rule, not `check.call_argument`: {body:?}\n{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn a_coalesce_chain_types_to_the_present_type() {
    let report = report(
        "coalesce-chain",
        "fn f(a: int?, b: int?, c: int): int\n    return a ?? b ?? c\n",
    );
    assert_clean(&report);
}

#[test]
fn a_parenthesized_coalesce_chain_still_types() {
    let report = report(
        "coalesce-chain-parens",
        "fn f(a: int?, b: int?, c: int): int\n    return (a ?? b) ?? c\n",
    );
    assert_clean(&report);
}

#[test]
fn an_absent_append_value_into_a_local_sequence_is_the_one_rule() {
    let report = report(
        "append-local-absent-value",
        "fn f(xs: sequence[int])\n    append(xs, absent)\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_optional_append_value_into_a_local_sequence_is_the_one_rule() {
    let report = report(
        "append-local-optional-value",
        "fn f(xs: sequence[int], v: int?)\n    append(xs, v)\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_absent_append_value_into_a_saved_keyed_leaf_is_the_one_rule() {
    let report = report(
        "append-saved-absent-value",
        "fn f()\n    append(^books(1).scores, absent)\n",
    );
    assert_one_rule(&report);
}

#[test]
fn an_optional_append_value_into_a_saved_keyed_leaf_is_the_one_rule() {
    let report = report(
        "append-saved-optional-value",
        "fn f(v: int?)\n    append(^books(1).scores, v)\n",
    );
    assert_one_rule(&report);
}

#[test]
fn a_present_append_value_stays_clean() {
    for body in [
        "fn f(xs: sequence[int])\n    append(xs, 5)\n",
        "fn f()\n    append(^books(1).scores, 5)\n",
    ] {
        let report = report("append-present-value", body);
        assert_clean(&report);
    }
}

/// Exactly one one-rule diagnostic and no key-type mismatch: an optional key argument
/// reports the one rule once at the key position, and the read it addresses is poisoned
/// so a present value slot does not stack a second one-rule on the same mistake.
fn assert_exactly_one_rule(report: &CheckReport) {
    assert_eq!(
        with_code(report, CHECK_UNRESOLVED_OPTIONAL).len(),
        1,
        "expected exactly one one-rule diagnostic: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(report, CHECK_KEY_TYPE).is_empty(),
        "an optional key must not also report a key-type mismatch: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_optional_store_identity_key_reports_the_one_rule_once() {
    let report = report(
        "optional-store-key-single",
        "fn f(mid: int?)\n    print(^books(mid).title)\n",
    );
    assert_exactly_one_rule(&report);
}

#[test]
fn an_optional_store_key_under_a_positional_read_reports_the_one_rule_once() {
    // The optional identity key sits under a deeper positional read; the poisoned base
    // read propagates so the outer value slot does not double the key one-rule.
    let report = report(
        "optional-store-key-deep-single",
        "fn f(mid: int?, pos: int)\n    print(^books(mid).scores(pos))\n",
    );
    assert_exactly_one_rule(&report);
}

#[test]
fn an_optional_keyed_leaf_key_reports_the_one_rule_once() {
    let report = report(
        "optional-keyed-leaf-key-single",
        "fn f(mkey: string?)\n    print(^books(1).counts(mkey))\n",
    );
    assert_exactly_one_rule(&report);
}

#[test]
fn an_optional_positional_index_reports_the_one_rule_once_not_a_key_type() {
    let report = report(
        "optional-positional-index-single",
        "fn f(mpos: int?)\n    print(^books(1).scores(mpos))\n",
    );
    assert_exactly_one_rule(&report);
}

#[test]
fn an_optional_local_sequence_index_reports_the_one_rule_once_not_a_key_type() {
    let report = report(
        "optional-local-seq-index-single",
        "fn f(xs: sequence[string], mpos: int?): string\n    return xs(mpos)\n",
    );
    assert_exactly_one_rule(&report);
}

#[test]
fn an_optional_local_keyed_tree_key_reports_the_one_rule_once_not_a_key_type() {
    let report = report(
        "optional-local-keyed-key-single",
        "fn f(mkey: string?): int\n    var counts(day: string): int\n    return counts(mkey)\n",
    );
    assert_exactly_one_rule(&report);
}

#[test]
fn an_optional_index_branch_key_reports_the_one_rule_once() {
    let report = check_module_report(
        "optional-index-key-single",
        "module m\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         \x20   index byTitle(title, id)\n\
         fn f(mt: string?)\n    for id in ^books.byTitle(mt)\n        print(\"x\")\n",
    );
    assert_exactly_one_rule(&report);
}

#[test]
fn exists_accepts_a_local_optional_binding() {
    let report = report(
        "exists-local-optional",
        "fn f(x: string?)\n    if exists(x)\n        print(x)\n",
    );
    assert_clean(&report);
}

#[test]
fn exists_on_a_non_optional_value_reports_it_is_always_present() {
    let report = report(
        "exists-non-optional",
        "fn f(x: string)\n    if exists(x)\n        print(x)\n",
    );
    let found = with_code(&report, CHECK_CALL_ARGUMENT);
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        found[0].message.contains("always present"),
        "exists on a present value names it always present: {:#?}",
        found[0]
    );
}

#[test]
fn a_coalesce_on_a_non_optional_left_reports_only_the_always_present_error() {
    let report = report(
        "coalesce-non-optional-left",
        "fn f(): string\n    const x: string = \"hi\"\n    return x ?? \"d\"\n",
    );
    assert_eq!(
        with_code(&report, CHECK_OPERATOR_TYPE).len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, CHECK_UNTYPED_VALUE).is_empty(),
        "an always-present `??` left recovers to its type, no untyped cascade: {:#?}",
        report.diagnostics
    );
}

#[test]
fn iterating_a_scalar_reports_only_the_unsupported_error_not_a_binding_cascade() {
    let report = report(
        "for-scalar-no-binding-cascade",
        "fn f(): int\n    const x: int = 3\n    var total: int = 0\n    \
         for i in x\n        total = total + i\n    return total\n",
    );
    assert_eq!(
        with_code(&report, CHECK_COLLECTION_UNSUPPORTED).len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, CHECK_UNTYPED_VALUE).is_empty(),
        "a rejected scalar iterable binds its element to the scalar type, no cascade: {:#?}",
        report.diagnostics
    );
}
