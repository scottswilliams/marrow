mod support;

use marrow_check::{CheckDiagnostic, DiagnosticPayload, EnumDiagnostic, MarrowType, check_project};

use support::{
    assert_clean, check_module, check_module_report, config, temp_project, with_code, write,
};

fn assert_enum_payload(diagnostic: &CheckDiagnostic, expected: EnumDiagnostic) {
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::Enum(expected),
        "{diagnostic:#?}"
    );
}

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
         resource Order at ^orders(id: int)\n    required state: Status\n",
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
        },
    );
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
        },
    );
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
}

#[test]
fn a_match_over_a_modules_own_same_named_enum_checks_clean() {
    // Two modules each declare an enum `Status`, with different members. Module
    // `b`'s function matches its own `Status` (members `open`/`closed`)
    // exhaustively. Enum identity is module-qualified, so the checker validates
    // the match against `b::Status`, not the first project-wide `Status`
    // (`a::Status`, `active`/`archived`). Resolving by bare name would read
    // `a::Status`'s members and falsely reject `b`'s match as nonexhaustive with
    // unknown arms.
    let root = temp_project("enum-same-name-match", |root| {
        write(
            root,
            "src/a.mw",
            "module a\nenum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             enum Status\n    open\n    closed\n\n\
             fn classify(s: Status): int\n    \
             match s\n        open\n            return 1\n        \
             closed\n            return 2\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn passing_one_enum_where_a_different_enum_is_expected_is_a_check_error() {
    // `classify(s: Status)` is called with a `Color` value. Nominal identity:
    // enum `Color` is not enum `Status`, so the argument is a real mismatch, not
    // silently accepted.
    let found = check_module(
        "enum-arg-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn classify(s: Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n\n\
         fn caller(): int\n    return classify(Color::green)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn passing_a_scalar_where_an_enum_is_expected_is_a_check_error() {
    // A raw scalar into an enum parameter is a mismatch: the parameter is `Status`,
    // the argument is `int`.
    let found = check_module(
        "enum-arg-scalar",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn classify(s: Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n\n\
         fn caller(): int\n    return classify(3)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn returning_a_different_enum_than_declared_is_a_check_error() {
    let found = check_module(
        "enum-return-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn f(): Status\n    return Color::red\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn assigning_a_different_enum_into_an_enum_local_is_a_check_error() {
    let found = check_module(
        "enum-assign-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         fn f()\n    var s: Status = Status::active\n    s = Color::red\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn assignment_between_same_named_enums_reports_qualified_payload() {
    let root = temp_project("enum-same-name-assign-payload", |root| {
        write(root, "src/a.mw", "module a\npub enum Color\n    red\n");
        write(root, "src/b.mw", "module b\npub enum Color\n    blue\n");
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             fn f()\n    var c: a::Color = a::Color::red\n    c = b::Color::blue\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.assignment_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::TypeMismatch {
            expected: MarrowType::Enum {
                module: "a".into(),
                name: "Color".into(),
            },
            found: MarrowType::Enum {
                module: "b".into(),
                name: "Color".into(),
            },
        },
        "{found:#?}"
    );
}

#[test]
fn writing_a_different_enum_into_an_enum_saved_field_is_a_check_error() {
    // The saved field `state: Status` is written a `Color` value: a nominal
    // mismatch at the saved-field write boundary.
    let found = check_module(
        "enum-field-write-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
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
            "module a\n\nuse pkg::kinds\n\nresource Saved at ^saved(id: int)\n    required k: kinds::Color\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn reading_an_enum_saved_field_types_as_that_enum() {
    // A read of `^orders(1).state` (an enum-typed saved field) must type as
    // `Status`: comparing it against the *same* enum is clean. Before the field
    // read was typed it was `Unknown`, so a nominal `==` against any enum reported
    // an operator error — this same-enum comparison was wrongly rejected.
    let report = check_module_report(
        "enum-field-read-eq-same",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         fn f(): bool\n    return ^orders(1).state == Status::active\n",
    );
    assert_clean(&report);

    // And typing as `Status` means a `==` against a *different* enum is rejected.
    let found = check_module(
        "enum-field-read-eq-cross",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         enum Color\n    red\n    green\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         fn f(): bool\n    return ^orders(1).state == Color::red\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_match_over_an_enum_saved_field_enforces_exhaustiveness() {
    // A match over a saved enum field `^orders(1).state` must resolve to `Status`
    // and require every member. Missing `banned` is a check error, not a silently
    // skipped match that faults at runtime.
    let found = check_module(
        "enum-field-read-match",
        "module m\n\
         enum Status\n    active\n    archived\n    banned\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         fn f()\n    \
         match ^orders(1).state\n        active\n            return\n        archived\n            return\n",
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
         resource Book at ^books(id: int)\n    shelf: string\n\n    index byShelf(shelf, id)\n\n\
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
         resource Session at ^session\n    required cursor: int\n    kinds(pos: int): Kind\n\n\
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

#[test]
fn a_nonexhaustive_match_over_a_qualified_enum_scrutinee_is_a_check_error() {
    // `s: b::Status` is a qualified enum annotation. The match over it must resolve
    // to `b::Status` and enforce exhaustiveness; missing `closed` is a check error,
    // not a runtime crash from an unresolved scrutinee that passed open.
    let root = temp_project("enum-qualified-nonexhaustive", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\nuse b\n\
             fn classify(s: b::Status): int\n    \
             match s\n        open\n            return 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.nonexhaustive_match");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_enum_payload(
        found[0],
        EnumDiagnostic::NonexhaustiveMatch {
            enum_name: "Status".into(),
            missing: vec!["closed".into()],
        },
    );
}

#[test]
fn passing_a_third_modules_enum_to_a_qualified_parameter_is_a_check_error() {
    // Module `c` calls `b::classify`, whose parameter is `b::Status`, with
    // `a::Status`. Three modules, three same-or-different enums: only `b::Status`
    // is accepted. Passing `a::Status` is a nominal mismatch.
    let root = temp_project("enum-third-module-arg", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n\n\
             pub fn classify(s: Status): int\n    \
             match s\n        open\n            return 1\n        closed\n            return 2\n",
        );
        write(
            root,
            "src/c.mw",
            "module c\nuse a\nuse b\n\
             fn run(): int\n    return b::classify(a::Status::active)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_bare_foreign_only_enum_annotation_resolves_to_the_real_owner_not_a_phantom() {
    // Module `a` declares `Status`; module `b` does not. A bare `Status` annotation
    // in `b` must resolve to the real owner `a::Status` — the same enum a bare
    // `Status::member` literal resolves to there — not a phantom `b::Status` minted
    // by stamping the referencing module onto a project-wide name (the F3 hole).
    //
    // Proof of correct identity: in `b`, `s == Status::active` (both the
    // annotation and the literal name the real `a::Status`) checks clean, and a
    // `match s` reads `a::Status`'s members exhaustively. A phantom `b::Status`
    // would own no members, so the literal `Status::active` would resolve to
    // `a::Status` while `s` carried `b::Status`, making the `==` a cross-enum
    // operator error — exactly the false rejection a phantom causes.
    let root = temp_project("enum-foreign-real-owner", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             fn same(s: Status): bool\n    return s == Status::active\n\n\
             fn classify(s: Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        !report.has_errors(),
        "a bare foreign-only enum annotation must resolve to the real owner, not a phantom: {:#?}",
        report.diagnostics
    );
}

#[test]
fn passing_a_foreign_enum_to_a_qualified_parameter_is_a_check_error() {
    // `b::dispatch(s: b::Status)` annotates its parameter with the *qualified*
    // `b::Status`. Per-file resolution sees only module `b`'s own enum names, so a
    // qualified `b::Status` slot is left `Unknown` until the whole program is
    // assembled — the argument gate must still fire after the slot is stamped with
    // its true owner. Calling it with `a::Color::green` is a nominal mismatch
    // (`Color` is not `Status`), not a silently dispatched wrong value.
    let root = temp_project("enum-qualified-arg-cross", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Color\n    red\n    green\n    blue\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn dispatch(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             fn run(): int\n    return b::dispatch(a::Color::green)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn passing_a_raw_scalar_to_a_qualified_enum_parameter_is_a_check_error() {
    // The same qualified `b::dispatch(s: b::Status)` slot, called with a raw `int`.
    // A scalar in an enum slot is a concrete mismatch the argument gate must catch
    // once the cross-module parameter carries its real enum identity.
    let root = temp_project("enum-qualified-arg-scalar", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn dispatch(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse b\n\
             fn run(): int\n    return b::dispatch(1)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_wrong_enum_through_a_relay_chain_to_a_qualified_parameter_is_a_check_error() {
    // A three-module relay: `app` calls `mid::relay`, whose parameter is the
    // qualified `b::Status`. Passing `a::Color::green` through the relay is a
    // nominal mismatch the argument gate must catch in `mid`, even though `mid`'s
    // file resolved `b::Status` to `Unknown` before the program was whole.
    let root = temp_project("enum-relay-chain-arg", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Color\n    red\n    green\n    blue\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/leaf.mw",
            "module leaf\nuse b\n\
             pub fn sink(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/mid.mw",
            "module mid\nuse b\nuse leaf\n\
             pub fn relay(s: b::Status): int\n    return leaf::sink(s)\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse mid\n\
             fn run(): int\n    return mid::relay(a::Color::green)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_wrong_enum_to_a_qualified_parameter_in_an_equality_body_is_a_check_error() {
    // `b::isActive(s: b::Status): bool` compares its qualified-enum parameter to
    // `b::Status::active`. Called with `a::Color::red`, the argument is a nominal
    // mismatch the gate must catch — the qualified parameter's identity, recovered
    // once the program is whole, drives the comparison.
    let root = temp_project("enum-qualified-arg-eq", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Color\n    red\n    green\n    blue\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn isActive(s: b::Status): bool\n    return s == b::Status::active\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             fn run(): bool\n    return b::isActive(a::Color::red)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_wrong_enum_to_a_qualified_parameter_inside_a_loop_is_a_check_error() {
    // The same qualified-enum argument mismatch inside a `for` loop body: each
    // iteration's call is checked, so the nominal mismatch is reported once.
    let root = temp_project("enum-qualified-arg-loop", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub enum Color\n    red\n    green\n    blue\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn dispatch(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             fn run()\n    for i in 1..3\n        b::dispatch(a::Color::green)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn passing_the_matching_enum_to_a_qualified_parameter_checks_clean() {
    // The clean counterpart: `b::dispatch(s: b::Status)` called with the matching
    // `b::Status::active`. The argument gate must accept a like-for-like enum across
    // the module boundary, not over-reject once the slot carries its real owner.
    let root = temp_project("enum-qualified-arg-clean", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    active\n    archived\n\n\
             pub fn dispatch(s: b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse b\n\
             fn run(): int\n    return b::dispatch(b::Status::active)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        !report.has_errors(),
        "a matching cross-module enum argument must check clean: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_match_over_a_sequence_enum_element_enforces_its_identity() {
    // A `sequence[Status]` element carries `Status`: iterating it binds the loop
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
         for s in items\n        \
         match s\n            red\n                return\n            green\n                return\n",
        "check.unknown_enum_member",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_enum_payload(
        &found[0],
        EnumDiagnostic::UnknownMember {
            enum_name: "Status".into(),
            member: "red".into(),
        },
    );
    assert_enum_payload(
        &found[1],
        EnumDiagnostic::UnknownMember {
            enum_name: "Status".into(),
            member: "green".into(),
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
}

#[test]
fn a_qualified_enum_var_annotation_accepts_the_same_qualified_member() {
    // A qualified `var t: b::Status` annotation accepts a `b::Status::open` value:
    // the annotation and the qualified member literal name the same enum, so the
    // initializer checks clean. (Proves qualified annotation + qualified member
    // value resolve to the same nominal identity.)
    let root = temp_project("enum-qualified-var-ok", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\nuse b\n\
             fn f()\n    var t: b::Status = b::Status::open\n    return\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn a_match_over_a_qualified_member_typed_local_dispatches_clean() {
    // A `const s: b::Status = b::Status::open` then an exhaustive `match s` over
    // `b::Status` checks clean: the qualified member literal types the local as
    // `b::Status`, so the match resolves and is exhaustive.
    let root = temp_project("enum-qualified-member-match", |root| {
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\nuse b\n\
             fn f(): int\n    const s: b::Status = b::Status::open\n    \
             match s\n        open\n            return 1\n        closed\n            return 2\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}
