mod support;

use marrow_check::{
    AppendTargetDiagnostic, DiagnosticPayload, MarrowType, ScalarType, check_project, check_tests,
};
use marrow_project::parse_config;

use support::{
    assert_clean, check_module, check_module_report, check_script, temp_project, with_code, write,
};

/// Check a project whose `src/app.mw` library declares `app_src` and whose
/// `tests/app_test.mw` test script holds `test_src`, returning the test report.
/// Used by tests that assert what `marrow test`/check catches in test files.
fn check_tests_report(name: &str, app_src: &str, test_src: &str) -> marrow_check::CheckReport {
    let root = temp_project(name, |root| {
        write(root, "src/app.mw", app_src);
        write(root, "tests/app_test.mw", test_src);
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let (src_report, src_program) = check_project(&root, &cfg).expect("check src");
    assert!(!src_report.has_errors(), "{:#?}", src_report.diagnostics);
    let (test_report, _modules) = check_tests(&root, &cfg, &src_program).expect("check tests");
    test_report
}

#[test]
fn check_tests_catches_a_std_call_with_the_wrong_argument_type() {
    // `std::text::length` takes a `string`; passing `42` is the same
    // `check.call_argument` mismatch a library file would report — test files run
    // the full type-inference pass, so this is caught at check time, not only at
    // run time.
    let report = check_tests_report(
        "check-tests-std-arg",
        "module app\n",
        "pub fn t()\n    var n = std::text::length(42)\n",
    );
    assert_eq!(
        with_code(&report, "check.call_argument").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn check_tests_catches_a_nextid_misuse_on_a_composite_root() {
    // `^orders` has a composite identity, so it has no default `nextId` policy; a
    // test file calling `nextId(^orders)` gets the `check.next_id_requires_single_int`
    // gate the library files already enforce.
    let report = check_tests_report(
        "check-tests-nextid",
        "module app\n\
         resource Order at ^orders(region: string, id: int)\n    required total: int\n",
        "pub fn t()\n    var id = nextId(^orders)\n",
    );
    assert_eq!(
        with_code(&report, "check.next_id_requires_single_int").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn check_tests_catches_a_type_mismatched_assignment() {
    // A test file's ordinary type errors are reported too: storing an `int` const
    // into a `string` place is a `check.assignment_type` mismatch.
    let report = check_tests_report(
        "check-tests-assign",
        "module app\n",
        "pub fn t()\n    const s: string = 1\n",
    );
    assert_eq!(
        with_code(&report, "check.assignment_type").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn check_tests_leaves_a_clean_test_file_clean() {
    // A well-typed test file that calls a project function and a std helper checks
    // with no diagnostics — the new type pass must not false-positive.
    let report = check_tests_report(
        "check-tests-clean",
        "module app\n\npub fn add(): int\n    return 1\n",
        "pub fn t()\n    std::assert::isTrue(app::add() == 1)\n    var n = std::text::length(\"hi\")\n",
    );
    assert_clean(&report);
}

#[test]
fn check_tests_catches_a_wrong_enum_to_a_qualified_project_parameter() {
    // A test file calls a project function whose parameter is the qualified
    // `app::Status`, passing `app::Color::green`. The test type pass reads the
    // project's already-normalized parameter, so the nominal mismatch is caught the
    // same way it is in a library call — not silently dispatched.
    let report = check_tests_report(
        "check-tests-enum-arg",
        "module app\n\
         pub enum Status\n    active\n    archived\n\n\
         pub enum Color\n    red\n    green\n\n\
         pub fn dispatch(s: app::Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n",
        "pub fn t()\n    var n = app::dispatch(app::Color::green)\n",
    );
    assert_eq!(
        with_code(&report, "check.call_argument").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn finally_return_is_rejected() {
    let found = check_script(
        "fin-return",
        "fn f()\n    try\n        x = 1\n    finally\n        return\n",
        "check.finally_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].span.line, 5, "{:#?}", found[0]);
}

#[test]
fn finally_break_inside_nested_loop_is_allowed() {
    let found = check_script(
        "fin-break-loop",
        "fn f()\n    try\n        x = 1\n    finally\n        while c\n            break\n",
        "check.finally_control_flow",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn finally_unlabeled_break_that_escapes_is_rejected() {
    let found = check_script(
        "fin-break-escape",
        "fn f()\n    try\n        x = 1\n    finally\n        break\n",
        "check.finally_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn finally_labeled_break_to_outer_loop_is_rejected() {
    // The label names a loop outside the finally block, so the break escapes it.
    let found = check_script(
        "fin-break-label",
        "fn f()\n    outer: while a\n        try\n            x = 1\n        finally\n            break outer\n",
        "check.finally_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn finally_labeled_break_to_inner_loop_is_allowed() {
    // The label names a loop nested within the finally block.
    let found = check_script(
        "fin-break-inner-label",
        "fn f()\n    try\n        x = 1\n    finally\n        inner: while c\n            break inner\n",
        "check.finally_control_flow",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn break_outside_any_loop_is_rejected() {
    // A `break` with no enclosing loop only fails late at runtime
    // (RUN_NO_ENCLOSING_LOOP); the checker must reject it statically.
    let found = check_script(
        "break-no-loop",
        "fn f()\n    break\n",
        "check.loop_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].span.line, 2, "{:#?}", found[0]);
}

#[test]
fn continue_outside_any_loop_is_rejected() {
    let found = check_script(
        "continue-no-loop",
        "fn f()\n    continue\n",
        "check.loop_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn labeled_break_naming_no_enclosing_loop_is_rejected() {
    // The label names no enclosing loop, so the break can never resolve.
    let found = check_script(
        "break-bad-label",
        "fn f()\n    while c\n        break outer\n",
        "check.loop_control_flow",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn break_and_continue_inside_a_loop_are_allowed() {
    let found = check_script(
        "break-in-loop",
        "fn f()\n    while c\n        break\n        continue\n",
        "check.loop_control_flow",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn labeled_break_to_an_enclosing_loop_is_allowed() {
    let found = check_script(
        "break-good-label",
        "fn f()\n    outer: while a\n        while b\n            break outer\n",
        "check.loop_control_flow",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn catch_with_non_error_type_is_rejected() {
    let found = check_script(
        "catch-bad-type",
        "fn f()\n    try\n        x = 1\n    catch e: string\n        return\n",
        "check.catch_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn catch_with_error_type_and_bare_catch_are_allowed() {
    let typed = check_script(
        "catch-error-type",
        "fn f()\n    try\n        x = 1\n    catch e: Error\n        return\n",
        "check.catch_type",
    );
    assert!(typed.is_empty(), "{typed:#?}");

    let bare = check_script(
        "catch-bare",
        "fn f()\n    try\n        x = 1\n    catch e\n        return\n",
        "check.catch_type",
    );
    assert!(bare.is_empty(), "{bare:#?}");
}

#[test]
fn throw_requires_an_error_value() {
    let found = check_script(
        "throw-non-error",
        "fn f()\n    throw \"oops\"\n",
        "check.throw_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn throwing_an_error_value_is_allowed() {
    let found = check_script(
        "throw-error",
        "fn f()\n    throw Error(code: \"test.error\", message: \"oops\")\n",
        "check.throw_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn try_requires_a_catch_or_finally_clause() {
    let found = check_script(
        "bare-try",
        "fn f()\n    try\n        write(\"x\")\n",
        "check.try_handler",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn try_with_catch_or_finally_is_allowed() {
    let with_catch = check_script(
        "try-catch",
        "fn f()\n    try\n        write(\"x\")\n    catch e\n        return\n",
        "check.try_handler",
    );
    assert!(with_catch.is_empty(), "{with_catch:#?}");

    let with_finally = check_script(
        "try-finally",
        "fn f()\n    try\n        write(\"x\")\n    finally\n        write(\"done\")\n",
        "check.try_handler",
    );
    assert!(with_finally.is_empty(), "{with_finally:#?}");
}

#[test]
fn call_shaped_assignment_target_is_rejected() {
    // `f(x) = y`: a call on a bare name is not a writable place.
    let found = check_script(
        "assign-call",
        "fn f()\n    f(x) = y\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn literal_assignment_target_is_rejected() {
    let found = check_script(
        "assign-literal",
        "fn f()\n    1 = y\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn saved_path_assignment_targets_are_allowed() {
    let found = check_script(
        "assign-saved",
        "fn f()\n    ^books(id).title = x\n",
        "check.invalid_assign_target",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn bare_keyed_root_field_assignment_paths_are_rejected() {
    let found = check_module(
        "assign-bare-keyed-root-field",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    ^books.title = \"x\"\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn generated_index_branches_are_not_assignment_targets() {
    let found = check_module(
        "assign-generated-index-branches",
        "module m\n\
         resource Book at ^books(id: int)\n    shelf: string\n    index byShelf(shelf, id)\n\n\
         fn f()\n    ^books.byShelf = \"x\"\n    ^books.byShelf(\"fiction\") = \"x\"\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn bare_keyed_root_field_paths_are_rejected_across_expression_contexts() {
    let found = check_module(
        "bare-keyed-root-field-path-contexts",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    const title = ^books.title\n    if exists(^books.title)\n        print(\"hit\")\n    delete ^books.title\n    for title in ^books.title\n        print(title)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 4, "{found:#?}");
}

#[test]
fn generated_index_branches_are_not_delete_targets() {
    let found = check_module(
        "delete-generated-index-branches",
        "module m\n\
         resource Book at ^books(id: int)\n    shelf: string\n    index byShelf(shelf, id)\n\n\
         fn f()\n    delete ^books.byShelf\n    delete ^books.byShelf(\"fiction\")\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn generated_index_branch_member_paths_are_rejected() {
    let found = check_module(
        "generated-index-branch-member-paths",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n    index byShelf(shelf, id)\n\n\
         fn f()\n    const a = ^books.byShelf.title\n    const b = ^books.byShelf(\"fiction\").title\n    if exists(^books.byShelf.title)\n        print(\"hit\")\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 3, "{found:#?}");
}

#[test]
fn generated_index_branch_call_chains_are_rejected() {
    let found = check_module(
        "generated-index-branch-call-chains",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    author: string\n    shelf: string\n    index byShelf(shelf, id)\n    index byAuthorShelf(author, shelf, id)\n\n\
         fn f()\n    const bad = ^books.byShelf(\"fiction\")(1).title\n    if exists(^books.byShelf(\"fiction\")(1).title)\n        print(\"hit\")\n    ^books.byShelf(\"fiction\")(1).title = \"x\"\n    delete ^books.byShelf(\"fiction\")(1).title\n    for id in ^books.byAuthorShelf(\"ann\")(\"fiction\")\n        print($\"{id}\")\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 5, "{found:#?}");
}

#[test]
fn optional_generated_index_branch_syntax_is_rejected() {
    let found = check_module(
        "optional-generated-index-branch",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n    index byShelf(shelf, id)\n\n\
         fn f()\n    const n = count(^books?.byShelf(\"fiction\"))\n    if exists(^books?.byShelf(\"fiction\"))\n        print(\"hit\")\n    const title = ^books?.byShelf(\"fiction\").title\n    ^books?.byShelf(\"fiction\") = \"x\"\n    delete ^books?.byShelf(\"fiction\")\n    for id in ^books?.byShelf(\"fiction\")\n        print($\"{id}\")\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 6, "{found:#?}");
}

#[test]
fn local_field_and_name_assignment_targets_are_allowed() {
    let found = check_script(
        "assign-local",
        "fn f()\n    x = 1\n    book.title = x\n",
        "check.invalid_assign_target",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn nested_local_resource_field_assignment_targets_are_rejected() {
    let found = check_module(
        "assign-nested-local-resource-field",
        "module m\n\
         resource Book\n    title: string\n    meta\n        subtitle: string\n\n\
         fn f()\n    var book: Book\n    book.meta.subtitle = \"x\"\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn nested_local_resource_keyed_layer_field_assignment_targets_are_rejected() {
    let found = check_module(
        "assign-nested-local-resource-keyed-layer-field",
        "module m\n\
         resource Book\n    title: string\n    versions(version: int)\n        title: string\n\n\
         fn f()\n    var book: Book\n    book.versions(1).title = \"x\"\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn nested_read_only_resource_parameter_write_reports_one_assignment_target_error() {
    let found = check_module(
        "assign-nested-readonly-resource-param-field",
        "module m\n\
         resource Book\n    title: string\n    meta\n        subtitle: string\n\n\
         fn f(book: Book)\n    book.meta.subtitle = \"x\"\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn nested_saved_field_assignment_targets_are_allowed() {
    let found = check_module(
        "assign-nested-saved-field",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n    meta\n        subtitle: string\n\n\
         fn f()\n    ^books(1).meta.subtitle = \"x\"\n",
        "check.invalid_assign_target",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn nested_saved_keyed_layer_field_assignment_targets_are_allowed() {
    let found = check_module(
        "assign-nested-saved-keyed-layer-field",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n    versions(version: int)\n        title: string\n\n\
         fn f()\n    ^books(1).versions(1).title = \"x\"\n",
        "check.invalid_assign_target",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn merge_is_rejected_by_the_parser() {
    let report = check_module_report("merge-bad", "module m\nfn f()\n    merge f(x) = y\n");
    assert!(
        with_code(&report, "check.invalid_assign_target").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        with_code(&report, "parse.syntax").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.rejected_surface").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn constant_const_values_are_allowed() {
    // Literals, arithmetic over literals, a reference to another constant, a
    // unary operator, and a standard-library constant are all compile-time
    // constant expressions.
    let found = check_script(
        "const-ok",
        "const A = 1\nconst B = 2 + 3 * 4\nconst C = A\nconst N = -1\nconst P = std::math::PI\n",
        "check.non_constant_const",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn const_value_calling_a_function_is_rejected() {
    // A const cannot call a function or host module.
    let found = check_script(
        "const-call",
        "const X = compute()\n",
        "check.non_constant_const",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn const_value_reading_saved_data_is_rejected() {
    // A const cannot read saved data.
    let found = check_script(
        "const-saved",
        "const X = ^counter\n",
        "check.non_constant_const",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn const_value_with_a_nested_saved_read_is_rejected() {
    // The rule looks through operators: a saved-data read anywhere in the
    // expression makes the whole value non-constant.
    let found = check_script(
        "const-nested-saved",
        "const X = 1 + ^counter\n",
        "check.non_constant_const",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn deleting_the_root_a_loop_traverses_is_rejected() {
    // `keys(^books)` traverses the `^books` identity layer; `delete ^books(id)`
    // removes a key from that same layer, which the checker rejects.
    let found = check_module(
        "loop-delete-root",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for id in keys(^books)\n        delete ^books(id)\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].span.line, 7, "{:#?}", found[0]);
}

#[test]
fn deleting_a_reversed_key_loop_traverses_is_rejected() {
    let found = check_module(
        "loop-reversed-delete-root",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for id in reversed(keys(^books))\n        delete ^books(id)\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn appending_to_the_sequence_a_loop_traverses_is_rejected() {
    // `for tag in ^books(1).tags` traverses the `tags` layer; `append(...tags...)`
    // adds a key to that same layer.
    let found = check_module(
        "loop-append-seq",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for tag in ^books(1).tags\n        append(^books(1).tags, \"x\")\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn appending_to_a_string_keyed_layer_is_rejected() {
    let found = check_module(
        "append-string-keyed",
        "module m\n\
         resource Doc at ^docs(id: int)\n    required title: string\n    scores(who: string): int\n\n\
         fn f()\n    append(^docs(1).scores, 7)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::AppendTarget(AppendTargetDiagnostic::NonIntKeyedLayer {
            key_type: MarrowType::Primitive(ScalarType::Str),
        }),
        "{found:#?}"
    );
}

#[test]
fn writing_a_keyed_leaf_the_loop_traverses_is_rejected() {
    let found = check_module(
        "loop-write-leaf",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for pos in keys(^books(1).tags)\n        ^books(1).tags(pos) = \"x\"\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn reversed_loop_mutating_the_traversed_layer_is_rejected() {
    let found = check_module(
        "loop-reversed-append-seq",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for tag in reversed(^books(1).tags)\n        append(^books(1).tags, \"x\")\n",
        "check.loop_mutates_traversed_layer",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn collecting_keys_first_then_mutating_is_allowed() {
    // The documented safe pattern: snapshot the keys into a local, iterate the
    // local, and mutate the layer. The loop traverses a local value, not the layer.
    let found = check_module(
        "loop-collect-first",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    const ids = keys(^books)\n    for id in ids\n        delete ^books(id)\n",
        "check.loop_mutates_traversed_layer",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn mutating_a_different_record_in_a_layer_loop_is_allowed() {
    // The loop traverses `^books(1).tags`; appending to `^books(2).tags` is a
    // different record's layer, so it is safe.
    let found = check_module(
        "loop-other-record",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\n\
         fn f()\n    for tag in ^books(1).tags\n        append(^books(2).tags, \"x\")\n",
        "check.loop_mutates_traversed_layer",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn writing_a_field_in_a_record_loop_is_allowed() {
    // A two-name root loop traverses records and exposes each identity; writing a
    // scalar field of a record does not change which keys the layer holds.
    let found = check_module(
        "loop-field-write",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n\n\
         fn f()\n    for id, book in ^books\n        ^books(id).title = \"x\"\n",
        "check.loop_mutates_traversed_layer",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn lock_is_rejected_by_the_parser() {
    let report = check_module_report(
        "lock-reserved",
        "module m\n\
         resource Cell at ^cells(id: int)\n    required v: int\n\
         fn f(id: int)\n    lock ^cells(id)\n        ^cells(id).v = 2\n",
    );
    assert_eq!(
        with_code(&report, "parse.syntax").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.rejected_surface").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}
