use crate::support;
use support::{check_module_report, config, temp_project, with_code, write};

use marrow_check::{CheckReport, check_project};

fn assert_diagnostic_codes(report: &CheckReport, expected: &[&str]) {
    let actual: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(actual, expected, "{:#?}", report.diagnostics);
}

/// A single source fault must surface as one diagnostic, not a recovery cascade of
/// several. A type-checked expression whose own type is wrong is reported once and
/// the checker recovers cleanly: the body around it raises nothing further.
///
/// `return "x"` from an `int`-returning function is exactly one fault — the returned
/// value's type does not match the declared return — so the whole report holds one
/// diagnostic, coded `check.return_type`. A cascade would add a second, unrelated
/// code from re-checking the same already-faulted expression.
#[test]
fn a_single_return_type_mismatch_reports_exactly_one_diagnostic() {
    let report = check_module_report(
        "cascade-return-type",
        "module m\nfn f(): int\n    return \"x\"\n",
    );

    assert_eq!(
        report.diagnostics.len(),
        1,
        "a lone type mismatch must not cascade: {:#?}",
        report.diagnostics
    );
    assert_eq!(report.diagnostics[0].code, "check.return_type");
}

/// A single unresolved name reports one diagnostic, not an unresolved-name fault plus
/// a follow-on "untyped value" complaint. A bare reference to an undefined name as an
/// expression statement is one fault: the name does not resolve. The report holds one
/// diagnostic, coded `check.unresolved_name`. (A name used where a *typed* obligation
/// also applies — a typed return, a call argument — is a distinct, multi-fault site;
/// this pins the lone-name case to one.)
#[test]
fn a_single_unresolved_name_reports_exactly_one_diagnostic() {
    let report = check_module_report("cascade-unresolved-name", "module m\nfn f()\n    missing\n");

    assert_eq!(
        report.diagnostics.len(),
        1,
        "a lone unresolved name must not cascade: {:#?}",
        report.diagnostics
    );
    assert_eq!(report.diagnostics[0].code, "check.unresolved_name");
}

/// A rejected binary operator is one fault, not two. `a + b` over `int` and `decimal`
/// is the lone mistake: it reports one `check.operator_type` at the operator and types
/// its result poisoned, so the surrounding `const c: decimal = …` does not stack a
/// second `check.untyped_value` over the same expression.
#[test]
fn a_rejected_operator_does_not_cascade_an_untyped_value() {
    let report = check_module_report(
        "cascade-operator-untyped",
        "module m\n\
         fn f()\n\
         \x20   const a: int = 3\n\
         \x20   const b: decimal = 2.5\n\
         \x20   const c: decimal = a + b\n",
    );

    let operator = with_code(&report, "check.operator_type");
    assert_eq!(operator.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(operator[0].span.line, 5);
    assert_eq!(operator[0].span.column, 24);
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "a rejected operator poisons its result, so no untyped-value cascades: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_invalid_throw_value_does_not_cascade_a_throw_type_error() {
    let report = check_module_report(
        "cascade-invalid-throw",
        "module m\nfn f()\n    throw 1 + true\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.operator_type"],
        "the rejected operator is the root cause; its invalid result must defer throw checking: {:#?}",
        report.diagnostics
    );
}

#[test]
fn dynamic_and_no_value_operands_are_rejected_at_value_boundaries() {
    let cases = [
        (
            "dynamic-throw-value",
            "module m\nfn f(value: unknown)\n    throw value\n",
            "check.throw_type",
        ),
        (
            "no-value-throw-value",
            "module m\nfn f()\n    throw print(\"x\")\n",
            "check.throw_type",
        ),
        (
            "dynamic-assert-equal-value",
            "module m\nfn f(value: unknown)\n    std::assert::equal(value, 1)\n",
            "check.call_argument",
        ),
        (
            "no-value-assert-equal-value",
            "module m\nfn f()\n    std::assert::equal(print(\"x\"), 1)\n",
            "check.call_argument",
        ),
        (
            "dynamic-next-id-value",
            "module m\nfn f(value: unknown)\n    nextId(value)\n",
            "check.call_argument",
        ),
        (
            "no-value-next-id-value",
            "module m\nfn f()\n    nextId(print(\"x\"))\n",
            "check.call_argument",
        ),
        (
            "dynamic-key-value",
            "module m\nfn f(value: unknown)\n    key(value)\n",
            "check.call_argument",
        ),
        (
            "no-value-key-value",
            "module m\nfn f()\n    key(print(\"x\"))\n",
            "check.call_argument",
        ),
    ];

    for (name, source, expected) in cases {
        let report = check_module_report(name, source);
        assert_diagnostic_codes(&report, &[expected]);
    }
}

#[test]
fn an_invalid_assert_operand_does_not_cascade_a_call_argument_error() {
    let report = check_module_report(
        "cascade-invalid-assert",
        "module m\nfn f()\n    std::assert::equal(1 + true, 1)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.operator_type"],
        "the rejected operator is the root cause; its invalid result must defer assert operand checking: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_invalid_assert_absent_operand_does_not_cascade_a_call_argument_error() {
    let report = check_module_report(
        "cascade-invalid-assert-absent",
        "module m\nfn f()\n    std::assert::isAbsent(1 + true)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.operator_type"],
        "the rejected operator is the root cause; its invalid result must defer absence-assertion checking: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_invalid_exists_operand_does_not_cascade_a_call_argument_error() {
    let report = check_module_report(
        "cascade-invalid-exists",
        "module m\nfn f()\n    exists(1 + true)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.operator_type"],
        "the rejected operator is the root cause; its invalid result must defer existence checking: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_assert_absent_operand_defers_to_its_type_diagnostic() {
    let report = check_module_report(
        "cascade-unknown-assert-absent",
        "module m\nfn f(value: Missing)\n    std::assert::isAbsent(value)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.unknown_type"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_exists_operand_defers_to_its_type_diagnostic() {
    let report = check_module_report(
        "cascade-unknown-exists",
        "module m\nfn f(value: Missing)\n    exists(value)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.unknown_type"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn clean_structural_unknowns_are_rejected_by_presence_call_boundaries() {
    const PREFIX: &str = "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn allocate(): Id(^books)\n\
         \x20   ^books(99).title = \"x\"\n\
         \x20   return Id(^books, 99)\n\n";
    let cases = [
        (
            "structural-unknown-exists",
            "fn f(): bool\n    return exists(next(allocate()))\n",
        ),
        (
            "structural-unknown-assert-absent",
            "fn f()\n    std::assert::isAbsent(next(allocate()))\n",
        ),
    ];

    for (name, body) in cases {
        let report = check_module_report(name, &format!("{PREFIX}{body}"));
        assert_diagnostic_codes(&report, &["check.call_argument"]);
    }
}

#[test]
fn an_invalid_identity_root_does_not_cascade_a_constructor_error() {
    let report = check_module_report(
        "cascade-invalid-identity-root",
        "module m\nfn f()\n    Id(1 + true, 1)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.operator_type"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_identity_root_defers_to_its_type_diagnostic() {
    let report = check_module_report(
        "cascade-unknown-identity-root",
        "module m\nfn f(root: Missing)\n    Id(root, 1)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.unknown_type"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn diagnosed_identity_constructor_shapes_poison_the_result() {
    let cases = [
        (
            "cascade-identity-no-root",
            "module m\nfn f(): int\n    return Id()\n",
            vec!["check.call_argument"],
        ),
        (
            "cascade-identity-non-root",
            "module m\nfn f(): int\n    return Id(1)\n",
            vec!["check.call_argument"],
        ),
        (
            "cascade-identity-named",
            "module m\n\
             resource Book\n    title: string\n\
             store ^books(id: int): Book\n\n\
             fn f(): string\n    return Id(root: ^books, key: 1)\n",
            vec!["check.call_argument", "check.call_argument"],
        ),
    ];

    for (name, source, expected) in cases {
        let report = check_module_report(name, source);
        let codes: Vec<&str> = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect();
        assert_eq!(codes, expected, "{name}: {:#?}", report.diagnostics);
    }
}

#[test]
fn an_invalid_neighbor_argument_poison_does_not_cascade() {
    let report = check_module_report(
        "cascade-invalid-neighbor",
        "module m\nfn f()\n    const n: int = next(1 + true)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.operator_type"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_invalid_next_id_argument_does_not_cascade_a_call_argument_error() {
    let report = check_module_report(
        "cascade-invalid-next-id",
        "module m\nfn f()\n    nextId(1 + true)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.operator_type"],
        "the rejected operator is the root cause; its invalid result must defer nextId shape checking: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_invalid_key_argument_does_not_cascade_a_call_argument_error() {
    let report = check_module_report(
        "cascade-invalid-key",
        "module m\nfn f()\n    key(1 + true)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.operator_type"],
        "the rejected operator is the root cause; its invalid result must defer key shape checking: {:#?}",
        report.diagnostics
    );
}

#[test]
fn invalid_or_diagnosed_calls_poison_their_result_under_typed_consumers() {
    let cases = [
        (
            "cascade-invalid-user-call-result",
            "module m\n\
             fn make(value: int): string\n    return \"\"\n\
             fn f(): int\n    return make(1 + true)\n",
            vec!["check.operator_type"],
        ),
        (
            "cascade-invalid-append-result",
            "module m\n\
             fn f(values: sequence[int]): string\n    return append(values, 1 + true)\n",
            vec!["check.operator_type"],
        ),
        (
            "cascade-invalid-conversion-result",
            "module m\nfn f(): string\n    return int(1 + true)\n",
            vec!["check.operator_type"],
        ),
        (
            "cascade-invalid-std-result",
            "module m\nfn f(): string\n    return std::text::length(1 + true)\n",
            vec!["check.operator_type"],
        ),
        (
            "cascade-invalid-resource-result",
            "module m\n\
             resource Entry\n    value: string\n\n\
             fn f(): int\n    return Entry(value: 1 + true)\n",
            vec!["check.operator_type"],
        ),
        (
            "cascade-builtin-arity-result",
            "module m\nfn f(): string\n    return int()\n",
            vec!["check.call_argument"],
        ),
        (
            "cascade-unresolved-call-result",
            "module m\nfn f(): int\n    return missing()\n",
            vec!["check.unresolved_call"],
        ),
        (
            "cascade-invalid-next-id-result",
            "module m\n\
             resource Book\n    title: string\n\
             store ^books(id: int): Book\n\n\
             fn f(): Id(^books)\n    return nextId(1 + true)\n",
            vec!["check.operator_type"],
        ),
        (
            "cascade-invalid-key-result",
            "module m\nfn f(): int\n    return key(1 + true)\n",
            vec!["check.operator_type"],
        ),
        (
            "cascade-diagnosed-next-id-result",
            "module m\nfn f(): string\n    return nextId(1)\n",
            vec!["check.call_argument"],
        ),
        (
            "cascade-diagnosed-key-result",
            "module m\nfn f(): string\n    return key(1)\n",
            vec!["check.call_argument"],
        ),
    ];

    for (name, source, expected) in cases {
        let report = check_module_report(name, source);
        assert_diagnostic_codes(&report, &expected);
    }
}

#[test]
fn rejected_saved_keys_poison_the_read_under_typed_consumers() {
    const STORE: &str = "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n";
    let cases = [
        (
            "cascade-saved-dynamic-key",
            "fn f(value: unknown): string\n    return ^books(value).title\n",
        ),
        (
            "cascade-saved-no-value-key",
            "fn f(): string\n    return ^books(print(\"x\")).title\n",
        ),
        (
            "cascade-saved-concrete-key",
            "fn f(): string\n    return ^books(\"wrong\").title\n",
        ),
        (
            "cascade-saved-key-arity",
            "fn f(): string\n    return ^books(1, 2).title\n",
        ),
        (
            "cascade-saved-structural-unknown-key",
            "fn f(): string\n    return ^books(^books).title\n",
        ),
        (
            "cascade-identity-structural-unknown-key",
            "fn f(): Id(^books)\n    return Id(^books, ^books)\n",
        ),
    ];

    for (name, body) in cases {
        let report = check_module_report(name, &format!("{STORE}{body}"));
        assert_diagnostic_codes(&report, &["check.key_type"]);
    }
}

#[test]
fn rejected_local_keys_poison_the_read_under_typed_consumers() {
    let cases = [
        (
            "cascade-sequence-dynamic-key",
            "module m\nfn f(values: sequence[string], key: unknown): string\n    return values(key)\n",
        ),
        (
            "cascade-sequence-no-value-key",
            "module m\nfn f(values: sequence[string]): string\n    return values(print(\"x\"))\n",
        ),
        (
            "cascade-sequence-concrete-key",
            "module m\nfn f(values: sequence[string]): string\n    return values(\"wrong\")\n",
        ),
        (
            "cascade-sequence-key-arity",
            "module m\nfn f(values: sequence[string]): string\n    return values(1, 2)\n",
        ),
        (
            "cascade-tree-dynamic-key",
            "module m\nfn f(values(key: int): string, key: unknown): string\n    return values(key)\n",
        ),
        (
            "cascade-tree-no-value-key",
            "module m\nfn f(values(key: int): string): string\n    return values(print(\"x\"))\n",
        ),
        (
            "cascade-tree-concrete-key",
            "module m\nfn f(values(key: int): string): string\n    return values(\"wrong\")\n",
        ),
        (
            "cascade-tree-key-arity",
            "module m\nfn f(values(key: int): string): string\n    return values(1, 2)\n",
        ),
    ];

    for (name, source) in cases {
        let report = check_module_report(name, source);
        assert_diagnostic_codes(&report, &["check.key_type"]);
    }
}

#[test]
fn an_unresolved_if_const_subject_does_not_cascade_a_condition_type_error() {
    let report = check_module_report(
        "cascade-unresolved-if-const",
        "module m\nfn f()\n    if const value = missing\n        print(value)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.unresolved_name"],
        "the unresolved subject is the root cause; its unknown result must defer if-const bindability checking: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_explicit_dynamic_if_const_subject_remains_rejected() {
    let report = check_module_report(
        "if-const-explicit-dynamic",
        "module m\nfn f(value: unknown)\n    if const present = value\n        print(present)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.condition_type"],
        "an explicit dynamic boundary has no statically maybe-present value to bind: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_diagnosed_saved_assignment_target_does_not_cascade() {
    let report = check_module_report(
        "cascade-invalid-saved-assignment-target",
        "module m\n\
         resource Book\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    ^books.shelf = \"fiction\"\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.key_type"],
        "the diagnosed target must remain the sole assignment fault: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_diagnosed_count_argument_does_not_cascade_a_return_type_error() {
    let report = check_module_report(
        "cascade-invalid-count-argument",
        "module m\n\
         resource Book\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): string\n    return count(^books.byShelf(\"fiction\"))\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.collection_unsupported"],
        "the rejected count argument must poison the dependent result: {:#?}",
        report.diagnostics
    );
}

#[test]
fn an_invalid_saved_key_poison_does_not_cascade() {
    let report = check_module_report(
        "cascade-invalid-saved-key",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): string\n    return ^books(missing).title\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.unresolved_name"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_saved_key_defers_to_its_type_diagnostic() {
    let report = check_module_report(
        "cascade-unknown-saved-key",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(key: Missing): string\n    return ^books(key).title\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.unknown_type"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_invalid_local_sequence_key_poison_does_not_cascade() {
    let report = check_module_report(
        "cascade-invalid-local-sequence-key",
        "module m\n\
         enum E\n    present\n\n\
         fn f(xs: sequence[string]): string\n    return xs(E::missing)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.unknown_enum_member"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_local_tree_key_defers_to_its_type_diagnostic() {
    let report = check_module_report(
        "cascade-unknown-local-tree-key",
        "module m\nfn f(values(k: int): string, key: Missing): string\n    return values(key)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.unknown_type"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_local_tree_key_type_does_not_collapse_to_a_mismatch() {
    let report = check_module_report(
        "cascade-unknown-local-tree-compatibility",
        "module m\n\
         fn take(values(k: int): string)\n    return\n\n\
         fn f()\n    var values(k: Missing): string\n    take(values)\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["schema.nonscalar_key", "check.unknown_type"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_optional_field_with_invalid_declared_type_does_not_cascade() {
    let report = check_module_report(
        "cascade-optional-field-invalid-type",
        "module m\n\
         resource Book\n    value: Missing\n\n\
         fn f(book: Book): int\n    return book?.value\n",
    );

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.unknown_type"],
        "{:#?}",
        report.diagnostics
    );
}

/// An unresolved name used as an assignment target is one fault. `x = 5` for an
/// undeclared `x` reports exactly one `check.unresolved_name`; the recovery never
/// re-reports it.
#[test]
fn an_unresolved_assignment_target_reports_exactly_one_diagnostic() {
    let report = check_module_report("cascade-unresolved-target", "module m\nfn f()\n    x = 5\n");

    let unresolved = with_code(&report, "check.unresolved_name");
    assert_eq!(unresolved.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(unresolved[0].span.line, 3);
    assert_eq!(unresolved[0].span.column, 5);
}

/// One undeclared name is one root cause however many times it is used. `x = 5`
/// followed by `print(x)` references the same undeclared `x` twice, but the report
/// holds a single `check.unresolved_name` — keyed at the first use — not one per
/// reference. The dedup is by name within the function, so the read after the failed
/// assignment does not re-report the same missing binding.
#[test]
fn a_repeated_undeclared_name_reports_one_diagnostic() {
    let report = check_module_report(
        "cascade-repeated-name",
        "module m\nfn f()\n    x = 5\n    print(x)\n",
    );

    let unresolved = with_code(&report, "check.unresolved_name");
    assert_eq!(unresolved.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(unresolved[0].span.line, 3);
    assert_eq!(unresolved[0].span.column, 5);
}

/// Distinct undeclared names are distinct root causes: each reports once. `print(a)`
/// and `print(b)` for two undeclared names hold two `check.unresolved_name`
/// diagnostics, so the name dedup never collapses different names into one.
#[test]
fn distinct_undeclared_names_each_report_once() {
    let report = check_module_report(
        "cascade-distinct-names",
        "module m\nfn f()\n    print(a)\n    print(b)\n",
    );

    let unresolved = with_code(&report, "check.unresolved_name");
    assert_eq!(unresolved.len(), 2, "{:#?}", report.diagnostics);
}

/// A parse error in a module must not cascade into a spurious unknown-type complaint in the same
/// module. A dangling-indentation fault leaves the parser unable to finish the module, so the
/// `Id(^books)` annotation on a function in that file resolves against an incomplete store and
/// reads as an unknown type. The parse error is the real cause: the report keeps it and drops the
/// follow-on `check.unknown_type` from the broken file, while a real type error in a clean sibling
/// module survives untouched.
#[test]
fn a_parse_error_suppresses_the_spurious_unknown_type_in_its_own_module() {
    let root = temp_project("cascade-parse-unknown-type", |root| {
        write(
            root,
            "src/broken.mw",
            "module broken\n\
             store ^books(id: int): Book\n\
             resource Book\n    required title: string\n\
             fn add(): Id(^books)\n    return nextId(^books)\n\
             fn main()\n        add()\n    add()\n",
        );
        write(
            root,
            "src/clean.mw",
            "module clean\nfn f(): int\n    return \"x\"\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        !with_code(&report, "parse.syntax").is_empty(),
        "the parse error is the real cause and must be reported: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.unknown_type").is_empty(),
        "the unknown-type cascade in the broken module must be suppressed: {:#?}",
        report.diagnostics
    );
    let sibling = with_code(&report, "check.return_type");
    assert_eq!(
        sibling.len(),
        1,
        "a real type error in a clean sibling module must survive: {:#?}",
        report.diagnostics
    );
}

fn config_with_default_entry(entry: &str) -> marrow_project::ProjectConfig {
    marrow_project::parse_config(&format!(
        r#"{{ "sourceRoots": ["src"], "store": {{ "backend": "memory" }}, "run": {{ "defaultEntry": "{entry}" }} }}"#
    ))
    .expect("config")
}

/// A parse error in a module suppresses the spurious schema and default-entry cascades that
/// depend on its unparsed region. A stray `#` line cannot parse, so the parser stops before
/// finishing the module: the `state: Status` field reads as a non-enum field even though the
/// `Status` enum is declared, and `main` never enters the program so the default entry reads as
/// missing. The parse error is the real cause: the report keeps it and drops both follow-on
/// `schema.non_enum_named_field` and `check.default_entry`.
#[test]
fn a_parse_error_suppresses_spurious_schema_and_default_entry_in_its_own_module() {
    let root = temp_project("cascade-parse-schema-entry", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             enum Status\n    draft\n    shipped\n\
             resource Order\n    state: Status\n\
             store ^orders(id: int): Order\n\
             # stray comment that cannot parse\n\
             pub fn main()\n    print(\"go\")\n",
        );
    });
    let (report, _) = check_project(&root, &config_with_default_entry("app::main")).expect("check");

    assert!(
        !with_code(&report, "parse.syntax").is_empty(),
        "the parse error is the real cause and must be reported: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "schema.non_enum_named_field").is_empty(),
        "the field's enum is declared; the non-enum cascade must be suppressed: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.default_entry").is_empty(),
        "`main` is defined; the default-entry cascade must be suppressed: {:#?}",
        report.diagnostics
    );
}

/// The parse-cascade suppression must not hide real schema or default-entry errors in
/// cleanly-parsed source. A module that parses cleanly but truly names a non-enum type in a
/// field and configures a default entry that names no public function reports both
/// `schema.non_enum_named_field` and `check.default_entry`.
#[test]
fn a_clean_module_still_reports_real_schema_and_default_entry_errors() {
    let root = temp_project("clean-schema-entry", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Order\n    state: NotAnEnum\n\
             store ^orders(id: int): Order\n\
             pub fn other()\n    print(\"go\")\n",
        );
    });
    let (report, _) = check_project(&root, &config_with_default_entry("app::main")).expect("check");

    assert!(
        with_code(&report, "parse.syntax").is_empty(),
        "the module parses cleanly: {:#?}",
        report.diagnostics
    );
    assert_eq!(
        with_code(&report, "schema.non_enum_named_field").len(),
        1,
        "a genuine non-enum field in clean source must still report: {:#?}",
        report.diagnostics
    );
    assert_eq!(
        with_code(&report, "check.default_entry").len(),
        1,
        "a genuine missing default entry in clean source must still report: {:#?}",
        report.diagnostics
    );
}

/// Two independent faults in two separate functions stay independent: one diagnostic
/// each, neither leaking a recovery artifact into the other's report. This guards the
/// isolation boundary — a fault is local to the expression that caused it, so a clean
/// sibling function never inherits a phantom diagnostic.
#[test]
fn two_independent_faults_report_one_diagnostic_each() {
    let root = temp_project("cascade-two-faults", |root| {
        write(
            root,
            "src/m.mw",
            "module m\n\
             fn bad(): int\n    return \"x\"\n\
             fn alsoBad()\n    missing\n\
             fn clean(): int\n    return 1\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec!["check.return_type", "check.unresolved_name"],
        "each fault reports once and the clean function reports nothing: {:#?}",
        report.diagnostics
    );
}
