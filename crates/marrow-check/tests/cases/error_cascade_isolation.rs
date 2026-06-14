use crate::support;
use support::{check_module_report, config, temp_project, write};

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
