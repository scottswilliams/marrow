use crate::support;
use crate::support_enum;
use marrow_check::{DiagnosticPayload, EnumDiagnostic, MarrowType, check_project};

use support::{
    assert_clean, check_module, check_module_report, config, temp_project, with_code, write,
};
use support_enum::assert_enum_payload;

#[test]
fn an_enum_member_reference_checks_clean() {
    // `Status::archived` is a known member of a declared enum; using it as a
    // value must not raise an unresolved-name or unknown-member diagnostic.
    let report = check_module_report(
        "enum-member-ok",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f()\n    const s: Status = Status::archived\n",
    );
    assert_clean(&report);
}

#[test]
fn an_enum_typed_param_and_var_annotation_is_accepted() {
    // An enum name is a valid type annotation on a parameter, a `var`, and a
    // `const`; none should be flagged `check.unknown_type`.
    let report = check_module_report(
        "enum-annotation-ok",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(s: Status): Status\n    var t: Status = Status::active\n    return t\n",
    );
    assert_clean(&report);
}

#[test]
fn an_enum_typed_resource_field_is_accepted() {
    let report = check_module_report(
        "enum-field-ok",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         resource Order\n    required state: Status\n\
         store ^orders(id: int): Order\n",
    );
    assert_clean(&report);
}

#[test]
fn reports_an_unknown_enum_member() {
    let found = check_module(
        "enum-unknown-member",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f()\n    const s: Status = Status::deleted\n",
        "check.unknown_enum_member",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_enum_payload(
        &found[0],
        EnumDiagnostic::UnknownMember {
            enum_name: "Status".into(),
            member: "deleted".into(),
            suggestions: vec![],
        },
    );
}

#[test]
fn a_partial_enum_path_names_the_segment_that_is_not_a_direct_member() {
    // `Animal::cat::tabby` skips the `mammal` parent, so `cat` is not a direct member
    // of `Animal`. The diagnostic names `cat` (not the leaf `tabby`, which does exist)
    // and spans the offending segment, not the enum head.
    let found = check_module(
        "partial-enum-path",
        "module m\n\
         enum Animal\n    \
         category mammal\n        \
         category cat\n            \
         tabby\n            \
         siamese\n    \
         bird\n\n\
         fn f(): Animal\n    return Animal::cat::tabby\n",
        "check.unknown_enum_member",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    // The category segment skips its parents, so the diagnostic guides to the two valid
    // forms: the full path through `cat`'s real parent and the bare leaf.
    assert_enum_payload(
        &found[0],
        EnumDiagnostic::UnknownMember {
            enum_name: "Animal".into(),
            member: "cat".into(),
            suggestions: vec!["Animal::mammal::cat::tabby".into(), "Animal::tabby".into()],
        },
    );
    // `return Animal::cat::tabby` on line 10: `Animal` starts at column 12, so `cat`
    // (after `Animal::`) starts at column 20.
    assert_eq!(found[0].span.line, 10);
    assert_eq!(found[0].span.column, 20);
}

#[test]
fn the_checked_program_carries_enum_schemas() {
    let root = temp_project("enum-program", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nenum Status\n    active\n    archived\n    banned\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
    let status = &program.modules[0].enums[0];
    assert_eq!(status.name, "Status");
    assert_eq!(status.members[2].name, "banned");
}

#[test]
fn enum_equality_against_the_same_enum_is_accepted() {
    let report = check_module_report(
        "enum-eq-ok",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(): bool\n    return Status::active == Status::archived\n",
    );
    assert_clean(&report);
}

#[test]
fn comparing_an_enum_to_a_string_is_an_operator_error() {
    // Nominal `==`: an enum value is comparable only with the same enum, never a
    // raw string. The mismatch is the existing operator-type diagnostic.
    let found = check_module(
        "enum-eq-string",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(): bool\n    return Status::active == \"active\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn comparing_two_different_enums_is_an_operator_error() {
    let found = check_module(
        "enum-eq-cross",
        "module m\n\
         enum Status\n    active\n\nenum Color\n    red\n\n\
         fn f(): bool\n    return Status::active == Color::red\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn enum_operator_errors_do_not_emit_untyped_return_hints() {
    let report = check_module_report(
        "enum-operator-no-untyped-cascade",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn ordered(): bool\n    return Status::active < Status::archived\n\n\
         fn added(): Status\n    return Status::active + Status::archived\n",
    );
    assert_eq!(
        with_code(&report, "check.operator_type").len(),
        2,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_exhaustive_match_over_an_enum_checks_clean() {
    let report = check_module_report(
        "match-ok",
        "module m\n\
         enum Status\n    active\n    archived\n    banned\n\n\
         fn f(s: Status)\n    \
         match s\n        active\n            return\n        \
         archived\n            return\n        banned\n            return\n",
    );
    assert_clean(&report);
}

#[test]
fn a_nonexhaustive_match_is_a_check_error() {
    let found = check_module(
        "match-nonexhaustive",
        "module m\n\
         enum Status\n    active\n    archived\n    banned\n\n\
         fn f(s: Status)\n    \
         match s\n        active\n            return\n        archived\n            return\n",
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
fn a_match_arm_for_an_unknown_member_is_a_check_error() {
    let found = check_module(
        "match-unknown-arm",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(s: Status)\n    \
         match s\n        active\n            return\n        \
         archived\n            return\n        deleted\n            return\n",
        "check.unknown_enum_member",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_enum_payload(
        &found[0],
        EnumDiagnostic::UnknownMember {
            enum_name: "Status".into(),
            member: "deleted".into(),
            suggestions: vec![],
        },
    );
}

#[test]
fn a_partial_match_arm_path_names_the_segment_that_is_not_a_direct_member() {
    // The arm `cat::tabby` skips the `mammal` parent, so `cat` is not a direct member.
    // The diagnostic names `cat` and spans that arm segment, not the leaf `tabby`.
    let found = check_module(
        "match-partial-arm",
        "module m\n\
         enum Animal\n    \
         category mammal\n        \
         category cat\n            \
         tabby\n            \
         siamese\n    \
         bird\n\n\
         fn f(a: Animal): int\n    \
         match a\n        \
         cat::tabby\n            \
         return 1\n        \
         _\n            \
         return 0\n",
        "check.unknown_enum_member",
    );
    let partial: Vec<_> = found
        .iter()
        .filter(|d| {
            d.payload
                == DiagnosticPayload::Enum(EnumDiagnostic::UnknownMember {
                    enum_name: "Animal".into(),
                    member: "cat".into(),
                    suggestions: vec!["Animal::mammal::cat::tabby".into(), "Animal::tabby".into()],
                })
        })
        .collect();
    assert_eq!(partial.len(), 1, "{found:#?}");
    // `cat::tabby` arm: `cat` is the first segment, at column 9 under the arm indent.
    assert_eq!(partial[0].span.column, 9);
}

#[test]
fn a_match_arm_qualified_with_the_scrutinee_enum_is_rejected_clearly() {
    // A match arm is a member path relative to the scrutinee enum, so the bare
    // `active` is correct and `Status::active` re-spells the scrutinee enum as a
    // prefix. The diagnostic must say so directly and never echo the rejected
    // spelling back as the suggestion.
    let found = check_module(
        "match-scrutinee-qualified-arm",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(s: Status)\n    \
         match s\n        Status::active\n            return\n        \
         Status::archived\n            return\n",
        "check.scrutinee_qualified_match_arm",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_enum_payload(
        &found[0],
        EnumDiagnostic::ScrutineeQualifiedMatchArm {
            enum_name: "Status".into(),
            written: "Status::active".into(),
            relative: "active".into(),
        },
    );
    // `Status::active` arm: `Status` is the first segment, at column 9 under the arm indent.
    assert_eq!(found[0].span.column, 9);
}

#[test]
fn a_match_arm_qualified_with_a_foreign_enum_name_stays_an_unknown_member() {
    // Only the scrutinee enum's own name is the relative-arm hint. A different
    // name as the first segment is an ordinary unknown member, not the
    // relative-arm guidance.
    let found = check_module(
        "match-foreign-qualified-arm",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(s: Status)\n    \
         match s\n        Other::active\n            return\n        \
         archived\n            return\n",
        "check.unknown_enum_member",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_arm_through_a_member_sharing_the_enum_name_resolves_normally() {
    // When the enum genuinely has a top-level member with the enum's own name,
    // `Status::active` is a real member step, not the redundant scrutinee prefix.
    // It must resolve through that member and check clean, not trip the
    // relative-arm guidance.
    let report = check_module_report(
        "match-member-named-as-enum",
        "module m\n\
         enum Status\n    category Status\n        active\n        archived\n\n\
         fn f(s: Status)\n    \
         match s\n        Status::active\n            return\n        \
         Status::archived\n            return\n",
    );
    assert_clean(&report);
}

#[test]
fn a_match_over_a_non_enum_scrutinee_is_rejected() {
    let found = check_module(
        "match-non-enum",
        "module m\n\
         fn f(n: int)\n    \
         match n\n        active\n            return\n",
        "check.match_requires_enum",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_duplicate_match_arm_is_a_check_error() {
    let found = check_module(
        "match-duplicate-arm",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f(s: Status)\n    \
         match s\n        active\n            return\n        \
         active\n            return\n        archived\n            return\n",
        "check.duplicate_match_arm",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_enum_payload(
        &found[0],
        EnumDiagnostic::DuplicateMatchArm {
            label: "active".into(),
        },
    );
}

#[test]
fn a_match_over_a_sequence_enum_element_enforces_its_identity() {
    // A `sequence[Status]` element carries `Status`: `values(...)` binds the loop
    // variable to that enum, so a `match` over it is dispatched against `Status`'s
    // members. Arms naming a *different* enum's members (`Color`'s `red`/`green`)
    // are then unknown `Status` members — a check error. Without recursing the
    // element through enum resolution the element binds `Unknown`, the match is
    // left alone as an unresolved scrutinee, and the foreign arms pass open: a
    // silent loss of identity over a sequence of enums.
    let found = check_module(
        "enum-sequence-element-foreign",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn f(items: sequence[Status])\n    \
         for k, s in items\n        \
         match s\n            red\n                return\n            green\n                return\n",
        "check.unknown_enum_member",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_enum_payload(
        &found[0],
        EnumDiagnostic::UnknownMember {
            enum_name: "Status".into(),
            member: "red".into(),
            suggestions: vec![],
        },
    );
    assert_enum_payload(
        &found[1],
        EnumDiagnostic::UnknownMember {
            enum_name: "Status".into(),
            member: "green".into(),
            suggestions: vec![],
        },
    );
}

#[test]
fn a_const_annotated_with_one_enum_and_a_different_enum_value_is_a_check_error() {
    // The const-annotation place is an enum; the initializer is a different enum.
    let found = check_module(
        "enum-const-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn f()\n    const s: Status = Color::red\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::TypeMismatch {
            expected: MarrowType::Enum {
                module: "m".into(),
                name: "Status".into(),
            },
            found: MarrowType::Enum {
                module: "m".into(),
                name: "Color".into(),
            },
        },
        "{found:#?}"
    );
}

#[test]
fn a_module_const_annotated_with_one_enum_and_a_different_enum_value_is_a_check_error() {
    let found = check_module(
        "module-enum-const-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         const s: Status = Color::red\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::TypeMismatch {
            expected: MarrowType::Enum {
                module: "m".into(),
                name: "Status".into(),
            },
            found: MarrowType::Enum {
                module: "m".into(),
                name: "Color".into(),
            },
        },
        "{found:#?}"
    );
}
