use crate::support;
use support::{check_module_report, config, temp_project, with_code, write};

use marrow_check::check_project;

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
