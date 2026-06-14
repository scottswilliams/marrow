use crate::support;
use support::{assert_clean, check_module_report, with_code};

#[test]
fn sparse_local_resource_whole_root_write_reports_required_absent() {
    let report = check_module_report(
        "required-absent-local-root-write",
        "module m\n\
         resource Book\n    required title: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn save(id: int)\n    var b: Book\n    b.shelf = \"fiction\"\n    ^books(id) = b\n",
    );

    let found = with_code(&report, "check.required_absent");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn nested_unkeyed_required_field_reports_required_absent() {
    let report = check_module_report(
        "required-absent-nested-unkeyed-root-write",
        "module m\n\
         resource Book\n    meta\n        required title: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn save(id: int)\n    var b: Book\n    b.shelf = \"fiction\"\n    ^books(id) = b\n",
    );

    let found = with_code(&report, "check.required_absent");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn whole_unkeyed_group_assignment_remains_inconclusive() {
    let report = check_module_report(
        "required-unkeyed-group-assignment-inconclusive",
        "module m\n\
         resource Book\n    required title: string\n    meta\n        required title: string\n\
         store ^books(id: int): Book\n\n\
         fn save(id: int)\n    var b: Book\n    var c: Book\n    c.title = \"filled\"\n    b.title = \"root\"\n    b.meta = c\n    ^books(id) = b\n",
    );

    assert_clean(&report);
}

#[test]
fn whole_unkeyed_group_assignment_keeps_unrelated_missing_fields() {
    let report = check_module_report(
        "required-unkeyed-group-assignment-keeps-unrelated",
        "module m\n\
         resource Book\n    required title: string\n    meta\n        required title: string\n\
         store ^books(id: int): Book\n\n\
         fn save(id: int)\n    var b: Book\n    var c: Book\n    c.title = \"filled\"\n    b.meta = c\n    ^books(id) = b\n",
    );

    let found = with_code(&report, "check.required_absent");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        found[0].message.contains("`title`"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn assigned_required_field_before_whole_root_write_is_clean() {
    let report = check_module_report(
        "required-assigned-local-root-write",
        "module m\n\
         resource Book\n    required title: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn save(id: int)\n    var b: Book\n    b.title = \"Mort\"\n    b.shelf = \"fiction\"\n    ^books(id) = b\n",
    );

    assert_clean(&report);
}

#[test]
fn constructor_and_whole_record_read_do_not_report_required_absent() {
    let report = check_module_report(
        "required-constructor-and-read-root-write",
        "module m\n\
         resource Book\n    required title: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn save_constructed(id: int)\n    var b = Book(title: \"Mort\")\n    ^books(id) = b\n\n\
         fn save_read(id: int, other: int)\n    var fallback = Book(title: \"fallback\")\n    var b: Book = ^books(other) ?? fallback\n    b.shelf = \"fiction\"\n    ^books(id) = b\n",
    );

    assert_clean(&report);
}

#[test]
fn branch_and_loop_assignments_remain_inconclusive() {
    let report = check_module_report(
        "required-branch-loop-inconclusive",
        "module m\n\
         resource Book\n    required title: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn save_after_branch(id: int, ok: bool)\n    var b: Book\n    if ok\n        b.title = \"Mort\"\n    b.shelf = \"fiction\"\n    ^books(id) = b\n\n\
         fn save_after_loop(id: int, ok: bool)\n    var b: Book\n    while ok\n        b.title = \"Mort\"\n    b.shelf = \"fiction\"\n    ^books(id) = b\n",
    );

    assert_clean(&report);
}

#[test]
fn keyed_layer_entry_write_is_out_of_scope() {
    let report = check_module_report(
        "required-keyed-layer-entry-out-of-scope",
        "module m\n\
         resource Book\n    required title: string\n    note: string\n    versions(version: int)\n        required title: string\n        note: string\n\
         store ^books(id: int): Book\n\n\
         fn save_version(id: int, version: int)\n    var b: Book\n    b.note = \"draft\"\n    ^books(id).versions(version) = b\n",
    );

    assert_clean(&report);
}

#[test]
fn keyed_layer_required_field_is_not_a_whole_root_required_absent() {
    let report = check_module_report(
        "required-keyed-layer-whole-root-out-of-scope",
        "module m\n\
         resource Book\n    note: string\n    versions(version: int)\n        required title: string\n        note: string\n\
         store ^books(id: int): Book\n\n\
         fn save(id: int)\n    var b: Book\n    b.note = \"draft\"\n    ^books(id) = b\n",
    );

    let found = with_code(&report, "check.required_absent");
    assert!(found.is_empty(), "{:#?}", report.diagnostics);
}
