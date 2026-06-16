use crate::support;
use marrow_check::{CHECK_COMMIT_AMPLIFICATION, CheckReport};
use marrow_syntax::Severity;

use support::{assert_clean, check_module_report, with_code};

fn assert_commit_amplification_warnings(report: &CheckReport, expected: usize) {
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(report, CHECK_COMMIT_AMPLIFICATION);
    assert_eq!(found.len(), expected, "{:#?}", report.diagnostics);
    for diagnostic in found {
        assert_eq!(diagnostic.severity, Severity::Warning);
    }
}

#[test]
fn bare_saved_write_in_loop_warns_without_making_report_error() {
    let report = check_module_report(
        "commit-amplification-bare-write",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn rename(id: int, title: string, again: bool)\n    while again\n        ^books(id).title = title\n",
    );

    assert_commit_amplification_warnings(&report, 1);
}

#[test]
fn transaction_inside_loop_suppresses_saved_write_warning() {
    let report = check_module_report(
        "commit-amplification-inner-transaction",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn rename(id: int, title: string, again: bool)\n    while again\n        transaction\n            ^books(id).title = title\n",
    );

    assert_clean(&report);
}

#[test]
fn transaction_around_loop_suppresses_saved_write_warning() {
    let report = check_module_report(
        "commit-amplification-outer-transaction",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn rename(id: int, title: string, again: bool)\n    transaction\n        while again\n            ^books(id).title = title\n",
    );

    assert_clean(&report);
}

#[test]
fn saved_write_shapes_in_loop_warn() {
    let cases = [
        (
            "assignment",
            "module m\n\
             resource Book\n    required title: string\n\
             store ^books(id: int): Book\n\n\
             fn rename(id: int, title: string, again: bool)\n    while again\n        ^books(id).title = title\n",
        ),
        (
            "delete",
            "module m\n\
             resource Book\n    required title: string\n    subtitle: string\n\
             store ^books(id: int): Book\n\n\
             fn clear(id: int, again: bool)\n    while again\n        delete ^books(id).subtitle\n",
        ),
        (
            "append",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn tag(id: int, tag: string, again: bool)\n    while again\n        append(^books(id).tags, tag)\n",
        ),
    ];

    for (name, source) in cases {
        let report = check_module_report(name, source);
        assert_commit_amplification_warnings(&report, 1);
    }
}

#[test]
fn append_in_value_position_inside_loop_warns() {
    let cases = [
        (
            "return",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             pub fn main(again: bool): int\n    while again\n        return append(^books(1).tags, \"x\")\n    return 0\n",
        ),
        (
            "const",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn tag(again: bool)\n    while again\n        const pos = append(^books(1).tags, \"x\")\n        print(pos)\n",
        ),
        (
            "nested-binary",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn tag(again: bool): int\n    while again\n        return append(^books(1).tags, \"x\") + 1\n    return 0\n",
        ),
    ];

    for (name, source) in cases {
        let report = check_module_report(name, source);
        assert_commit_amplification_warnings(&report, 1);
    }
}

#[test]
fn append_in_top_level_while_condition_warns() {
    let report = check_module_report(
        "commit-amplification-while-condition",
        "module m\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn tag()\n    while append(^books(1).tags, \"x\") > 0\n        print(\"tagged\")\n",
    );

    assert_commit_amplification_warnings(&report, 1);
}

#[test]
fn transaction_around_while_condition_suppresses_append_warning() {
    let report = check_module_report(
        "commit-amplification-while-condition-transaction",
        "module m\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn tag()\n    transaction\n        while append(^books(1).tags, \"x\") > 0\n            print(\"tagged\")\n",
    );

    assert_clean(&report);
}

#[test]
fn nested_while_condition_warns_once_per_append() {
    let report = check_module_report(
        "commit-amplification-nested-while-condition",
        "module m\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn tag(again: bool)\n    while again\n        while append(^books(1).tags, \"x\") > 0\n            print(\"tagged\")\n",
    );

    assert_commit_amplification_warnings(&report, 1);
}

#[test]
fn append_inside_nested_evaluated_expressions_in_loop_warns_once_per_call() {
    let cases = [
        (
            "var",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn tag(again: bool)\n    while again\n        var pos = append(^books(1).tags, \"x\")\n        print(pos)\n",
        ),
        (
            "assignment-rhs",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn tag(again: bool)\n    var pos = 0\n    while again\n        pos = append(^books(1).tags, \"x\")\n",
        ),
        (
            "call-arg",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn tag(again: bool)\n    while again\n        print(append(^books(1).tags, \"x\"))\n",
        ),
        (
            "interpolation",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn tag(again: bool)\n    while again\n        print($\"{append(^books(1).tags, \"x\")}\")\n",
        ),
        (
            "condition",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn tag(again: bool)\n    while again\n        if append(^books(1).tags, \"x\") > 0\n            print(\"tagged\")\n",
        ),
        (
            "else-if-condition",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn tag(again: bool)\n    while again\n        if false\n            print(\"first\")\n        else if append(^books(1).tags, \"x\") > 0\n            print(\"tagged\")\n",
        ),
    ];

    for (name, source) in cases {
        let report = check_module_report(name, source);
        assert_commit_amplification_warnings(&report, 1);
    }
}

#[test]
fn append_inside_assignment_target_expressions_in_loop_warns() {
    let cases = [
        (
            "local-collection-target",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             pub fn main(again: bool)\n    var labels(pos: int): string\n    while again\n        labels(append(^books(1).tags, \"x\")) = \"seen\"\n",
            1,
        ),
        (
            "saved-target-key",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn main(again: bool)\n    while again\n        ^books(1).tags(append(^books(1).tags, \"x\")) = \"seen\"\n",
            2,
        ),
    ];

    for (name, source, expected) in cases {
        let report = check_module_report(name, source);
        assert_commit_amplification_warnings(&report, expected);
    }
}

#[test]
fn append_inside_delete_path_expression_in_loop_warns_for_append_and_delete() {
    let report = check_module_report(
        "commit-amplification-delete-path-append",
        "module m\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn main(again: bool)\n    while again\n        delete ^books(1).tags(append(^books(1).tags, \"x\"))\n",
    );

    assert_commit_amplification_warnings(&report, 2);
}

#[test]
fn transaction_suppresses_append_in_value_position_inside_loop() {
    let report = check_module_report(
        "commit-amplification-append-value-transaction",
        "module m\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn tag(again: bool)\n    while again\n        transaction\n            const pos = append(^books(1).tags, \"x\")\n            print(pos)\n",
    );

    assert_clean(&report);
}

#[test]
fn nested_control_flow_in_loop_warns_on_saved_write() {
    let report = check_module_report(
        "commit-amplification-nested-if",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn rename(id: int, title: string, again: bool)\n    while again\n        if again\n            ^books(id).title = title\n",
    );

    assert_commit_amplification_warnings(&report, 1);
}

#[test]
fn local_assignment_host_call_and_cross_function_call_do_not_warn() {
    let cases = [
        (
            "local-assignment",
            "module m\n\
             fn tally(again: bool)\n    var n = 0\n    while again\n        n = n + 1\n",
        ),
        (
            "host-call",
            "module m\n\
             fn log(title: string, again: bool)\n    while again\n        print(title)\n",
        ),
        (
            "cross-function-call",
            "module m\n\
             resource Book\n    required title: string\n\
             store ^books(id: int): Book\n\n\
             fn rename(id: int, title: string)\n    ^books(id).title = title\n\n\
             fn caller(id: int, title: string, again: bool)\n    while again\n        rename(id, title)\n",
        ),
    ];

    for (name, source) in cases {
        let report = check_module_report(name, source);
        assert_clean(&report);
    }
}
