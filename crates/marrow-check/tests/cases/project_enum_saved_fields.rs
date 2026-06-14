use crate::support;
use crate::support_enum;
use marrow_check::{EnumDiagnostic, check_project};

use support::{assert_clean, check_module, check_module_report, config, temp_project, write};
use support_enum::assert_enum_payload;

#[test]
fn writing_a_different_enum_into_an_enum_saved_field_is_a_check_error() {
    // The saved field `state: Status` is written a `Color` value: a nominal
    // mismatch at the saved-field write boundary.
    let found = check_module(
        "enum-field-write-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         resource Order\n    required state: Status\n\
         store ^orders(id: int): Order\n\n\
         fn f()\n    ^orders(1).state = Color::red\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_qualified_enum_saved_field_declaration_checks_clean() {
    let root = temp_project("qualified-enum-saved-field", |root| {
        write(
            root,
            "src/pkg/kinds.mw",
            "module pkg::kinds\n\npub enum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\n\nuse pkg::kinds\n\nresource Saved\n    required k: kinds::Color\nstore ^saved(id: int): Saved\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn reading_an_enum_saved_field_types_as_that_enum() {
    // A resolved read of `^orders(1).state` (an enum-typed saved field) must
    // type as `Status`: comparing it against the *same* enum is clean. Before
    // the field read was typed it was `Unknown`, so a nominal `==` against any
    // enum reported an operator error — this same-enum comparison was wrongly
    // rejected.
    let report = check_module_report(
        "enum-field-read-eq-same",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         resource Order\n    required state: Status\n\
         store ^orders(id: int): Order\n\n\
         fn f(): bool\n    return (^orders(1).state ?? Status::archived) == Status::active\n",
    );
    assert_clean(&report);

    // And typing as `Status` means a `==` against a *different* enum is rejected.
    let found = check_module(
        "enum-field-read-eq-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         resource Order\n    required state: Status\n\
         store ^orders(id: int): Order\n\n\
         fn f(): bool\n    return (^orders(1).state ?? Status::archived) == Color::red\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_match_over_an_enum_saved_field_enforces_exhaustiveness() {
    // A match over a resolved saved enum field `^orders(1).state` must resolve
    // to `Status` and require every member. Missing `banned` is a check error,
    // not a silently skipped match that faults at runtime.
    let found = check_module(
        "enum-field-read-match",
        "module m\n\
         enum Status\n    active\n    archived\n    banned\n\n\
         resource Order\n    required state: Status\n\
         store ^orders(id: int): Order\n\n\
         fn f()\n    const state = ^orders(1).state ?? Status::active\n    \
         match state\n        active\n            return\n        archived\n            return\n",
        "check.nonexhaustive_match",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_enum_payload(
        &found[0],
        EnumDiagnostic::NonexhaustiveMatch {
            enum_name: "Status".into(),
            missing: vec!["banned".into()],
        },
    );
}

#[test]
fn non_unique_index_branch_arguments_are_checked() {
    let found = check_module(
        "non-unique-index-args",
        "module m\n\
         resource Book\n    shelf: string\n\
         store ^books(id: int): Book\n\n    index byShelf(shelf, id)\n\n\
         fn f()\n    \
         for id in ^books.byShelf(123)\n        var typed: Id(^books) = id\n    \
         for id in ^books.byShelf(\"fiction\", 1, 2)\n        var typed: Id(^books) = id\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn a_singleton_keyed_enum_leaf_read_types_as_that_enum() {
    let report = check_module_report(
        "enum-singleton-keyed-leaf-read",
        "module m\n\
         enum Kind\n    number\n    plus\n\n\
         resource Session\n    required cursor: int\n    kinds(pos: int): Kind\n\
         store ^session: Session\n\n\
         fn readBack(): int\n    \
         var k: Kind = ^session.kinds(1) ?? Kind::number\n    \
         match k\n        number\n            return 0\n        plus\n            return 1\n",
    );
    assert!(
        !report.has_errors(),
        "a keyed enum leaf under a singleton saved root must read as its enum: {:#?}",
        report.diagnostics
    );
}
