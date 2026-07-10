use crate::support;
use support::{check_module_report, with_code};

fn codes(report: &marrow_check::CheckReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn assert_codes(name: &str, source: &str, expected: &[&str]) {
    let report = check_module_report(name, source);
    assert_eq!(
        codes(&report),
        expected,
        "{name}: {:#?}",
        report.diagnostics
    );
}

const BOOKS: &str = "module m\n\
     resource Book\n    title: string\n\
     store ^books(id: int): Book\n\n";

const ALLOCATION: &str = "module m\n\
     resource Book\n    title: string\n\
     store ^books(id: int): Book\n\n\
     fn allocate(): Id(^books)\n    \
     ^books(99).title = \"x\"\n    \
     return Id(^books, 99)\n\n";

const DATED_POSTS: &str = "module m\n\
     resource Post\n    published: int\n\
     store ^posts(id: int): Post\n\n    index byDate(published, id)\n\n";

#[test]
fn recursively_poisoned_call_arguments_poison_the_declared_result() {
    let source = "module m\n\
         fn consume(value: int): string\n    return \"ok\"\n\n\
         fn f(): int\n    \
         var values(k: Missing): string\n    \
         return consume(values)\n";
    let report = check_module_report("typed-admission-nested-call-poison", source);

    assert_eq!(
        with_code(&report, "check.unknown_type").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.return_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_invalid_declared_local_tree_key_defers_without_a_key_mismatch() {
    let source = "module m\n\
         fn f(): string\n    \
         var values(k: Missing): string\n    \
         return values(1)\n";
    let report = check_module_report("typed-admission-invalid-local-key", source);

    assert_eq!(
        with_code(&report, "check.unknown_type").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.key_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.return_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn recursively_poisoned_values_precede_dependent_shape_checks() {
    let cases = [
        (
            "return",
            "module m\nfn f(): int\n    var values(k: int): Missing\n    return values\n",
        ),
        (
            "assignment",
            "module m\nfn f()\n    var values(k: int): Missing\n    var result: int\n    result = values\n",
        ),
        (
            "unary",
            "module m\nfn f()\n    var values(k: int): Missing\n    throw -values\n",
        ),
        (
            "condition",
            "module m\nfn f()\n    var values(k: int): Missing\n    if values\n        return\n",
        ),
        (
            "coalesce",
            "module m\nfn f(): int\n    var values(k: int): Missing\n    return values ?? 1\n",
        ),
        (
            "field",
            "module m\nfn f(): string\n    var values(k: int): Missing\n    return values.missing\n",
        ),
        (
            "render",
            "module m\nfn f()\n    var values(k: int): Missing\n    print(values)\n",
        ),
        (
            "range-header",
            "module m\nfn f()\n    var values(k: int): Missing\n    for value in 1..values\n        print(value)\n",
        ),
        (
            "if-const",
            "module m\nfn f()\n    var values(k: int): Missing\n    if const value = values\n        return\n",
        ),
        (
            "match",
            "module m\nfn f()\n    var values(k: int): Missing\n    match values\n        active\n            return\n",
        ),
        (
            "loop-head",
            "module m\nfn f()\n    var values(k: int): Missing\n    for key, value, extra in values\n        return\n",
        ),
    ];

    for (name, source) in cases {
        assert_codes(
            &format!("typed-admission-recursive-poison-{name}"),
            source,
            &["check.unknown_type"],
        );
    }
}

#[test]
fn recursive_poison_precedes_call_and_key_shape_checks() {
    let call_cases = [
        ("next-id-saved-path", "nextId(^books(1 + true))"),
        ("keys-saved-path", "keys(^books(1 + true))"),
        ("append-saved-path", "append(^books(1 + true), 1)"),
        ("identity-named", "Id(^books, id: 1 + true)"),
    ];
    for (name, expression) in call_cases {
        let source = format!("{BOOKS}fn f()\n    {expression}\n");
        assert_codes(
            &format!("typed-admission-poison-before-call-{name}"),
            &source,
            &["check.operator_type"],
        );
    }

    let saved_cases = [
        ("arity", "^books(1 + true, 2)"),
        ("named", "^books(id: 1 + true)"),
    ];
    for (name, access) in saved_cases {
        let source = format!("{BOOKS}fn f(): string\n    return {access}.title\n");
        assert_codes(
            &format!("typed-admission-poison-before-saved-key-{name}"),
            &source,
            &["check.operator_type"],
        );
    }

    assert_codes(
        "typed-admission-poison-before-local-key-arity",
        "module m\nfn f(values: sequence[string]): string\n    return values(1 + true, 2)\n",
        &["check.operator_type"],
    );
}

#[test]
fn strict_boundaries_reject_recovery_with_one_owning_diagnostic() {
    let cases = [
        (
            "throw",
            "module m\nfn f(xs: sequence[int])\n    throw keys(xs)\n".to_string(),
            "check.throw_type",
        ),
        (
            "scalar-assertion",
            "module m\nfn f(xs: sequence[int])\n    std::assert::equal(keys(xs), 1)\n".to_string(),
            "check.call_argument",
        ),
        (
            "identity-root",
            format!("{BOOKS}fn f(xs: sequence[int])\n    Id(keys(xs), 1)\n"),
            "check.call_argument",
        ),
        (
            "next-id",
            format!("{ALLOCATION}fn f()\n    nextId(next(allocate()))\n"),
            "check.call_argument",
        ),
        (
            "key",
            format!("{ALLOCATION}fn f()\n    key(next(allocate()))\n"),
            "check.call_argument",
        ),
    ];

    for (name, source, expected) in cases {
        assert_codes(
            &format!("typed-admission-strict-recovery-{name}"),
            &source,
            &[expected],
        );
    }
}

#[test]
fn strict_boundaries_preserve_poison_and_reject_other_non_values() {
    let cases = [
        (
            "throw-poison",
            "module m\nfn f()\n    throw 1 + true\n",
            "check.operator_type",
        ),
        (
            "throw-dynamic",
            "module m\nfn f(v: unknown)\n    throw v\n",
            "check.throw_type",
        ),
        (
            "throw-no-value",
            "module m\nfn f()\n    throw print(\"x\")\n",
            "check.throw_type",
        ),
        (
            "throw-shape",
            "module m\nfn f()\n    throw 1\n",
            "check.throw_type",
        ),
        (
            "assert-poison",
            "module m\nfn f()\n    std::assert::equal(1 + true, 1)\n",
            "check.operator_type",
        ),
        (
            "assert-dynamic",
            "module m\nfn f(v: unknown)\n    std::assert::equal(v, 1)\n",
            "check.call_argument",
        ),
        (
            "assert-no-value",
            "module m\nfn f()\n    std::assert::equal(print(\"x\"), 1)\n",
            "check.call_argument",
        ),
        (
            "assert-shape",
            "module m\nfn f(xs: sequence[int])\n    std::assert::equal(xs, 1)\n",
            "check.call_argument",
        ),
    ];
    for (name, source, expected) in cases {
        assert_codes(name, source, &[expected]);
    }

    let call_cases = [
        ("Id", "Id({value}, 1)"),
        ("nextId", "nextId({value})"),
        ("key", "key({value})"),
    ];
    for (boundary, expression) in call_cases {
        for (state, value, expected) in [
            ("poison", "1 + true", "check.operator_type"),
            ("dynamic", "v", "check.call_argument"),
            ("no-value", "print(\"x\")", "check.call_argument"),
            ("shape", "1", "check.call_argument"),
        ] {
            let source = format!(
                "{BOOKS}fn f(v: unknown)\n    {}\n",
                expression.replace("{value}", value),
            );
            assert_codes(
                &format!("typed-admission-{boundary}-{state}"),
                &source,
                &[expected],
            );
        }
    }
}

#[test]
fn saved_key_admission_preserves_result_provenance() {
    let cases = [
        ("poison", "1 + true", "check.operator_type"),
        ("recovery", "keys(xs)", "check.key_type"),
        ("dynamic", "dynamic", "check.key_type"),
        ("no-value", "print(\"x\")", "check.key_type"),
        ("optional", "optional", "check.unresolved_optional"),
        ("mismatch", "\"wrong\"", "check.key_type"),
    ];
    for (name, key, expected) in cases {
        let source = format!(
            "{BOOKS}fn f(xs: sequence[int], dynamic: unknown, optional: int?): string\n    return ^books({key}).title\n"
        );
        assert_codes(
            &format!("typed-admission-saved-key-{name}"),
            &source,
            &[expected],
        );
    }

    assert_codes(
        "typed-admission-saved-key-arity",
        &format!("{BOOKS}fn f(): string\n    return ^books(1, 2).title\n"),
        &["check.key_type"],
    );
    assert_codes(
        "typed-admission-saved-key-named",
        &format!("{BOOKS}fn f(): string\n    return ^books(id: 1).title\n"),
        &["check.call_argument"],
    );
}

#[test]
fn local_sequence_and_tree_key_admission_preserves_result_provenance() {
    let cases = [
        ("poison", "1 + true", "check.operator_type"),
        ("recovery", "keys(xs)", "check.key_type"),
        ("no-value", "print(\"x\")", "check.key_type"),
        ("optional", "optional", "check.unresolved_optional"),
        ("mismatch", "\"wrong\"", "check.key_type"),
    ];
    for collection in ["sequence[string]", "tree(k: int): string"] {
        let declaration = if collection.starts_with("tree") {
            "values(k: int): string"
        } else {
            "values: sequence[string]"
        };
        for (name, key, expected) in cases {
            let source = format!(
                "module m\nfn f({declaration}, xs: sequence[int], dynamic: unknown, optional: int?): string\n    return values({key})\n"
            );
            assert_codes(
                &format!("typed-admission-local-{collection}-{name}"),
                &source,
                &[expected],
            );
        }
        let source = format!("module m\nfn f({declaration}): string\n    return values(1, 2)\n");
        assert_codes(
            &format!("typed-admission-local-{collection}-arity"),
            &source,
            &["check.key_type"],
        );

        let source = format!(
            "module m\nfn f({declaration}, dynamic: unknown): string\n    return values(dynamic) ?? \"\"\n"
        );
        assert_codes(
            &format!("typed-admission-local-{collection}-explicit-dynamic"),
            &source,
            &[],
        );
    }
}

#[test]
fn saved_range_endpoints_preserve_state_in_both_orders() {
    let cases = [
        ("poison-left", "1 + true", "1", "check.operator_type"),
        ("poison-right", "1", "1 + true", "check.operator_type"),
        ("recovery-left", "keys(xs)", "1", "check.key_type"),
        ("recovery-right", "1", "keys(xs)", "check.key_type"),
        ("dynamic-left", "dynamic", "1", "check.key_type"),
        ("dynamic-right", "1", "dynamic", "check.key_type"),
        ("no-value-left", "print(\"x\")", "1", "check.key_type"),
        ("no-value-right", "1", "print(\"x\")", "check.key_type"),
        ("mismatch-left", "\"x\"", "1", "check.key_type"),
        ("mismatch-right", "1", "\"x\"", "check.key_type"),
    ];
    for (name, start, end, expected) in cases {
        let source = format!(
            "{DATED_POSTS}fn f(xs: sequence[int], dynamic: unknown): string\n    return count(^posts.byDate({start}..{end}))\n"
        );
        assert_codes(
            &format!("typed-admission-saved-range-{name}"),
            &source,
            &[expected],
        );
    }
}

#[test]
fn saved_range_by_preserves_state_and_remains_forbidden() {
    let cases = [
        ("poison", "1 + true", "check.operator_type"),
        ("recovery", "keys(xs)", "check.key_type"),
        ("dynamic", "dynamic", "check.key_type"),
        ("no-value", "print(\"x\")", "check.key_type"),
        ("concrete", "1", "check.key_type"),
    ];
    for (name, step, expected) in cases {
        let source = format!(
            "{DATED_POSTS}fn f(xs: sequence[int], dynamic: unknown): string\n    return count(^posts.byDate(1..2 by {step}))\n"
        );
        assert_codes(
            &format!("typed-admission-saved-range-by-{name}"),
            &source,
            &[expected],
        );
    }
}

#[test]
fn collection_and_append_boundaries_reject_no_value() {
    let cases = [
        (
            "count-subject",
            "module m\nfn f()\n    count(print(\"x\"))\n",
            "check.collection_unsupported",
        ),
        (
            "append-target",
            "module m\nfn f()\n    append(print(\"x\"), 1)\n",
            "check.call_argument",
        ),
        (
            "append-value",
            "module m\nfn f(xs: sequence[int])\n    append(xs, print(\"x\"))\n",
            "check.call_argument",
        ),
    ];
    for (name, source, expected) in cases {
        assert_codes(name, source, &[expected]);
    }
    assert_codes(
        "append-open-dynamic-element",
        "module m\nfn f(xs: sequence[unknown])\n    append(xs, 1)\n",
        &[],
    );
}

#[test]
fn rejected_predicates_poison_their_result() {
    assert_codes(
        "typed-admission-equality-result",
        "module m\nfn f(xs: sequence[int]): int\n    return xs == xs\n",
        &["check.operator_type"],
    );
    assert_codes(
        "typed-admission-is-result",
        "module m\nenum Status\n    active\n\nfn f(value: Status): int\n    return value is Status::missing\n",
        &["check.is_type"],
    );
    assert_codes(
        "typed-admission-is-poison",
        "module m\nenum Status\n    active\n\nfn f(): int\n    return 1 + true is Status::active\n",
        &["check.operator_type"],
    );
}
