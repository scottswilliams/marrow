use crate::support;
use crate::support_enum;
use marrow_check::{
    CheckedCallTarget, CheckedExpr, CheckedRuntimeValueType, CheckedStmt, DiagnosticPayload,
    EnumDiagnostic, MarrowType, ScalarType, check_project,
};

use support::{
    analyze_overlay, assert_clean, check_module, config, temp_project, with_code, write,
};
use support_enum::assert_enum_payload;

fn enum_type(module: &str, name: &str) -> MarrowType {
    MarrowType::Enum {
        module: module.into(),
        name: name.into(),
    }
}

/// Assert that the lone diagnostic carries a `TypeMismatch` naming the concrete
/// `expected`/`found` operands, pinning *which* nominal identities mismatched
/// rather than merely that some type error of this code fired.
fn assert_only_mismatch<D: std::fmt::Debug>(
    found: &[D],
    payload_of: impl Fn(&D) -> &DiagnosticPayload,
    expected: MarrowType,
    actual: MarrowType,
) {
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        *payload_of(&found[0]),
        DiagnosticPayload::TypeMismatch {
            expected,
            found: actual,
        },
        "{found:#?}"
    );
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("m", "Status"),
        enum_type("m", "Color"),
    );
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("m", "Status"),
        MarrowType::Primitive(ScalarType::Int),
    );
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("m", "Status"),
        enum_type("m", "Color"),
    );
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("m", "Status"),
        enum_type("m", "Color"),
    );
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("a", "Color"),
        enum_type("b", "Color"),
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("b", "Status"),
        enum_type("a", "Status"),
    );
}

#[test]
fn a_bare_foreign_only_enum_annotation_resolves_to_the_real_owner_not_a_phantom() {
    // Module `a` declares `Status`; module `b` does not. A bare `Status` annotation
    // in `b` must resolve to the real owner `a::Status` — the same enum a bare
    // `Status::member` literal resolves to there — not a phantom `b::Status` minted
    // by stamping the referencing module onto a project-wide name.
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("b", "Status"),
        enum_type("a", "Color"),
    );
}

#[test]
fn resource_constructor_field_with_a_foreign_enum_enforces_nominal_identity() {
    let root = temp_project("enum-resource-constructor-foreign-field", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/other.mw",
            "module other\npub enum Shade\n    dark\n    light\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse kinds\nuse other\n\
             resource Paint\n    required color: kinds::Color\n\n\
             fn run()\n    var p = Paint(color: other::Shade::dark)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.call_argument");
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("kinds", "Color"),
        enum_type("other", "Shade"),
    );
}

#[test]
fn resource_constructor_metadata_preserves_a_foreign_enum_field_identity() {
    let root = temp_project("enum-resource-constructor-field-metadata", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse kinds\n\
             resource Paint\n    required color: kinds::Color\n\n\
             fn run()\n    var p = Paint(color: kinds::Color::red)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);

    let run = program
        .modules
        .iter()
        .find(|module| module.name == "app")
        .and_then(|module| {
            module
                .functions
                .iter()
                .find(|function| function.name == "run")
        })
        .expect("run function");
    let body = run.runtime_body().expect("runtime body");
    let constructor = body
        .statements()
        .iter()
        .find_map(|statement| match statement {
            CheckedStmt::Var {
                value:
                    Some(CheckedExpr::Call {
                        target: CheckedCallTarget::ResourceConstructor(constructor),
                        ..
                    }),
                ..
            } => Some(constructor),
            _ => None,
        })
        .expect("resource constructor");
    let color = constructor
        .fields
        .iter()
        .find(|field| field.name == "color")
        .expect("color field");
    assert!(matches!(
        &color.ty,
        CheckedRuntimeValueType::Enum { module, name, .. }
            if module == "kinds" && name == "Color"
    ));
}

#[test]
fn saved_field_with_a_bare_unique_foreign_enum_preserves_nominal_identity() {
    let root = temp_project("enum-saved-field-bare-foreign", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse kinds\n\
             resource Paint\n    color: Color\n\
             store ^paints(id: int): Paint\n\n\
             fn read(id: Id(^paints))\n    \
             const n: int = (^paints(id).color ?? kinds::Color::red)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.assignment_type");
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        MarrowType::Primitive(ScalarType::Int),
        enum_type("kinds", "Color"),
    );
}

#[test]
fn saved_field_with_a_bare_unique_foreign_enum_accepts_matching_write() {
    let root = temp_project("enum-saved-field-bare-foreign-write", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Status\n    active\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse kinds\n\
             resource Post\n    status: Status\n\
             store ^posts(id: int): Post\n\n\
             fn write()\n    ^posts(1).status = kinds::Status::active\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn saved_field_with_an_ambiguous_bare_foreign_enum_reports_ambiguous_type() {
    let root = temp_project("enum-saved-field-ambiguous-foreign", |root| {
        write(root, "src/a.mw", "module a\npub enum Status\n    active\n");
        write(root, "src/b.mw", "module b\npub enum Status\n    active\n");
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             resource Post\n    status: Status\n\
             store ^posts(id: int): Post\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        with_code(&report, "schema.non_enum_named_field").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    let found = with_code(&report, "check.unknown_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::AmbiguousType {
            ty: marrow_schema::Type::Named("Status".into()),
            name: "Status".into(),
        },
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn keyed_leaf_with_a_bare_unique_foreign_enum_accepts_matching_write() {
    let root = temp_project("enum-keyed-leaf-bare-foreign-write", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Status\n    active\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse kinds\n\
             resource Post\n    statuses(key: string): Status\n\
             store ^posts(id: int): Post\n\n\
             fn write()\n    ^posts(1).statuses(\"x\") = kinds::Status::active\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn keyed_leaf_with_a_bare_unique_foreign_enum_rejects_resource_value() {
    let root = temp_project("enum-keyed-leaf-bare-foreign-resource-write", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Status\n    active\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse kinds\n\
             resource Post\n    statuses(key: string): Status\n\
             store ^posts(id: int): Post\n\n\
             fn write()\n    ^posts(1).statuses(\"x\") = Post()\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.assignment_type");
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("kinds", "Status"),
        MarrowType::Resource("app::Post".into()),
    );
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("b", "Status"),
        MarrowType::Primitive(ScalarType::Int),
    );
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("b", "Status"),
        enum_type("a", "Color"),
    );
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("b", "Status"),
        enum_type("a", "Color"),
    );
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
    assert_only_mismatch(
        &found,
        |d| &d.payload,
        enum_type("b", "Status"),
        enum_type("a", "Color"),
    );
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

/// The lone `check.unknown_type` diagnostic an `analyze_project` run reports for
/// `source`, asserting it carries the `Id(^config)` identity payload. Routes
/// through the editor-facing analysis API — the same path `marrow check` and the
/// LSP take — so a keyless-singleton identity in a signature, field, or key
/// rebuilds facts without panicking the prefix-preserving fact pass.
fn singleton_identity_unknown_type(name: &str, source: &str) -> DiagnosticPayload {
    let (snapshot, _) = analyze_overlay(name, &[("src/m.mw", source)]);
    let found = with_code(&snapshot.report, "check.unknown_type");
    assert_eq!(found.len(), 1, "{:#?}", snapshot.report.diagnostics);
    found[0].payload.clone()
}

#[test]
fn a_keyless_singleton_identity_parameter_is_a_check_error() {
    // `store ^config: Config` is a singleton: it has no generated identity type,
    // so `Id(^config)` is uninhabitable. The parameter site must reject it with a
    // located diagnostic rather than resolving a phantom identity no value fills —
    // and must not collapse the parameter's checked type so far that the rebuilt
    // signature facts fall below their preserved prefix and abort the analysis.
    let payload = singleton_identity_unknown_type(
        "singleton-identity-param",
        "module m\n\
         resource Config\n    required name: string\n\n\
         store ^config: Config\n\n\
         fn apply(c: Id(^config))\n    return\n",
    );
    assert_eq!(
        payload,
        DiagnosticPayload::UnknownType(marrow_schema::Type::Identity("config".into())),
    );
}

#[test]
fn a_keyless_singleton_identity_return_type_is_a_check_error() {
    let payload = singleton_identity_unknown_type(
        "singleton-identity-return",
        "module m\n\
         resource Config\n    required name: string\n\n\
         store ^config: Config\n\n\
         fn make(): Id(^config)\n    return\n",
    );
    assert_eq!(
        payload,
        DiagnosticPayload::UnknownType(marrow_schema::Type::Identity("config".into())),
    );
}

#[test]
fn a_keyless_singleton_identity_saved_field_is_a_check_error() {
    let payload = singleton_identity_unknown_type(
        "singleton-identity-field",
        "module m\n\
         resource Config\n    required name: string\n\n\
         resource Holder\n    ref: Id(^config)\n\n\
         store ^config: Config\n\
         store ^holders(id: int): Holder\n",
    );
    assert_eq!(
        payload,
        DiagnosticPayload::UnknownType(marrow_schema::Type::Identity("config".into())),
    );
}

#[test]
fn a_keyless_singleton_identity_store_key_is_a_check_error() {
    // A keyed store's key annotation is a declaration site too: `^entries(k:
    // Id(^config))` names the uninhabitable identity as the key type. A key must be
    // an orderable scalar, so an identity type — keyless singleton or not — is
    // rejected upstream as a non-scalar key, with a located diagnostic and no
    // panic in the prefix-preserving fact rebuild.
    let (snapshot, _) = analyze_overlay(
        "singleton-identity-key",
        &[(
            "src/m.mw",
            "module m\n\
             resource Config\n    required name: string\n\n\
             resource Entry\n    required note: string\n\n\
             store ^config: Config\n\
             store ^entries(k: Id(^config)): Entry\n",
        )],
    );
    let found = with_code(&snapshot.report, "schema.nonscalar_key");
    assert_eq!(found.len(), 1, "{:#?}", snapshot.report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::Schema(marrow_schema::SchemaErrorKind::NonScalarKey {
            target: marrow_schema::SchemaKeyTarget::IdentityKey { name: "k".into() },
            ty: marrow_schema::Type::Identity("config".into()),
        }),
        "{:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn a_keyed_store_identity_parameter_checks_clean() {
    // The clean counterpart: a keyed store *does* define `Id(^books)`, so the same
    // parameter annotation is well formed and must not be flagged.
    let (snapshot, _) = analyze_overlay(
        "keyed-identity-param",
        &[(
            "src/m.mw",
            "module m\n\
             resource Book\n    required title: string\n\n\
             store ^books(id: int): Book\n\n\
             fn apply(b: Id(^books))\n    return\n",
        )],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
}

#[test]
fn two_unknown_signature_types_carry_distinct_token_spans() {
    // Both `Widget` and `Gadget` are unknown. Each diagnostic must point at its own
    // type token, not collapse to the `fn` keyword at column 1 — otherwise two
    // errors on one signature line are indistinguishable.
    let found = check_module(
        "signature-unknown-spans",
        "module m\nfn f(x: Widget): Gadget\n    return\n",
        "check.unknown_type",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
    let mut columns: Vec<u32> = found.iter().map(|d| d.span.column).collect();
    columns.sort_unstable();
    assert_eq!(columns, vec![9, 18], "{found:#?}");
    assert!(found.iter().all(|d| d.span.line == 2), "{found:#?}");
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
