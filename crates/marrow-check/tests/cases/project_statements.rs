use crate::support;
use marrow_check::{DiagnosticPayload, check_project};
use marrow_project::parse_config;

use support::{
    assert_clean, check_module, check_module_report, check_script, config, temp_project, with_code,
    write,
};

/// A `memory`-backend project config, for the durable-store-required suite. A durable
/// surface under it cannot establish committed identity, so the checker rejects it.
fn memory_config() -> marrow_project::ProjectConfig {
    parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#).expect("config")
}

#[test]
fn reports_unknown_types_in_signatures_and_consts() {
    let found = check_module(
        "unknown-type",
        "module m\nconst X: Nope = 1\nfn f(a: Booook): Alsobad\n    return 1\n",
        "check.unknown_type",
    );
    assert_eq!(found.len(), 3, "{found:#?}");
    for name in ["Booook", "Alsobad", "Nope"] {
        assert!(
            found.iter().any(|d| d.payload
                == DiagnosticPayload::UnknownType(marrow_schema::Type::Named(name.into()))),
            "{name}: {found:#?}"
        );
    }
}

#[test]
fn reports_unknown_types_for_parser_migrated_signature_spellings() {
    let found = check_module(
        "unknown-type-migrated-signature-spellings",
        "module m\nfn f(rows: FutureBox[string, int]): FutureBox[string, int]\n    return 1\n",
        "check.unknown_type",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(
        found.iter().all(|diagnostic| diagnostic.payload
            == DiagnosticPayload::UnknownType(marrow_schema::Type::Named(
                "FutureBox[string,int]".into()
            ))
            && diagnostic.span.line == 2),
        "{found:#?}"
    );
}

#[test]
fn reports_unknown_type_for_parser_migrated_keyed_var_key_annotation() {
    let found = check_script(
        "unknown-type-migrated-keyed-var-key",
        "fn f()\n    var counts(name: 1): int\n",
        "check.unknown_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::UnknownType(marrow_schema::Type::Named("1".into())),
        "{found:#?}"
    );
    assert_eq!(found[0].span.line, 2, "{found:#?}");
}

#[test]
fn rejects_a_local_keyed_var_with_a_nonscalar_key_type() {
    // A local keyed tree obeys the same key-type allowlist as a saved keyed layer:
    // an identity, an enum, or a resource key projects to no orderable scalar, so it
    // is rejected at check rather than faulting at the first key write.
    for key_type in ["Id(^books)", "Color", "Book"] {
        let src = format!(
            "module m\n\
             enum Color\n    red\n    green\n\
             resource Book\n    required title: string\n\
             store ^books(id: int): Book\n\n\
             fn f()\n    var t(k: {key_type}): bool\n"
        );
        let found = check_module(
            "local-keyed-var-nonscalar-key",
            &src,
            "schema.nonscalar_key",
        );
        assert_eq!(found.len(), 1, "{key_type}: {found:#?}");
    }
}

#[test]
fn rejects_a_keyed_parameter_with_a_nonscalar_key_type() {
    // A keyed function parameter is a local keyed collection too, so the same key-type
    // rule applies to its declared key columns.
    let found = check_module(
        "keyed-param-nonscalar-key",
        "module m\n\
         enum Color\n    red\n    green\n\n\
         fn f(seen(k: Color): bool)\n    print(\"h\")\n",
        "schema.nonscalar_key",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn accepts_a_local_keyed_var_with_a_scalar_key_type() {
    let found = check_script(
        "local-keyed-var-scalar-key",
        "fn f()\n    var scores(player: string): int\n    var seen(k: int): bool\n",
        "schema.nonscalar_key",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn rejects_an_uninitialized_enum_var() {
    // An enum has no default member and no incremental construction, so an
    // uninitialized enum `var` is caught at check rather than faulting at first use.
    let found = check_module(
        "uninitialized-enum-var",
        "module m\n\
         enum Status\n    active\n    archived\n\n\
         fn f()\n    var s: Status\n    s = Status::active\n",
        "check.uninitialized_var",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_an_uninitialized_identity_var() {
    let found = check_module(
        "uninitialized-identity-var",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    var x: Id(^books)\n    x = nextId(^books)\n",
        "check.uninitialized_var",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn accepts_an_initialized_enum_var_and_uninitialized_buildable_vars() {
    // An enum var with an initializer, a resource var built field by field, a keyed
    // tree, and a scalar that defaults are all legitimate uninitialized or
    // initialized declarations.
    let found = check_module(
        "initialized-enum-and-buildable-vars",
        "module m\n\
         enum Status\n    active\n    archived\n\
         resource Book\n    required title: string\n\n\
         fn f()\n    \
         var s: Status = Status::active\n    \
         var b: Book\n    var n: int\n    var t(k: int): bool\n",
        "check.uninitialized_var",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn unknown_annotation_diagnostics_do_not_cascade_to_untyped_values() {
    let report = check_module_report(
        "unknown-annotation-cascade",
        "module m\n\
         fn from_bad_annotation(param: MissingParam)\n\
         \x20   var fromParam: int = param\n\
         \x20   var local: MissingLocal = 1\n\
         \x20   var fromLocal: int = local\n\n\
         fn make_bad(): MissingReturn\n\
         \x20   return 1\n\n\
         fn use_return()\n\
         \x20   var fromReturn: int = make_bad()\n",
    );

    assert_eq!(
        with_code(&report, "check.unknown_type").len(),
        3,
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
fn known_types_are_not_flagged_as_unknown() {
    let root = temp_project("known-types", |root| {
        // Primitive, sequence, identity, the module's own resource, `unknown`, and
        // a qualified cross-module reference are all accepted.
        write(
            root,
            "src/m.mw",
            "module m\nresource Book\n    required title: string\nstore ^books(id: int): Book\n\nfn f(a: int, b: sequence[string], c: Id(^books), d: Book, e: unknown, g: shelf::Thing): bool\n    return true\n",
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\nresource Thing\n    name: string\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        with_code(&report, "check.unknown_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn reports_a_bare_return_in_a_value_returning_function() {
    // The bare `return` (inside the `if`) leaves a value-returning function without a
    // value on that path.
    let found = check_module(
        "bare-return",
        "module m\nfn f(c: bool): int\n    if c\n        return\n    return 1\n",
        "check.return_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn reports_a_value_return_in_a_void_function() {
    let found = check_module(
        "void-return",
        "module m\nfn g()\n    return 1\n",
        "check.return_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn matching_returns_are_not_flagged() {
    let found = check_module(
        "ok-return",
        "module m\nfn ok(c: bool): int\n    if c\n        return 1\n    return 2\n\nfn void_fn(c: bool)\n    if c\n        return\n",
        "check.return_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn reports_a_value_function_that_may_not_return() {
    // `f` falls through the `if` (no else) without returning; `g` ends in an
    // assignment.
    let found = check_module(
        "missing-return",
        "module m\nfn f(c: bool): int\n    if c\n        return 1\n\nfn g(): int\n    var x = 1\n",
        "check.missing_return",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn functions_that_return_on_all_paths_are_not_flagged() {
    // Exhaustive if/else; ends in return; void body; ends in a loop.
    let found = check_module(
        "returns-all-paths",
        "module m\n\
         fn a(c: bool): int\n    if c\n        return 1\n    else\n        return 2\n\n\
         fn b(): int\n    return 7\n\n\
         fn c()\n    var x = 1\n\n\
         fn e(c: bool): int\n    while c\n        return 1\n",
        "check.missing_return",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_value_function_ending_in_a_trailing_call_is_flagged() {
    // A trailing expression is discarded, never returned, so a declared-return
    // function whose body ends in a call — void or value-producing — does not
    // return on that path. `helper()` ignores its result; `g(): int` does too.
    let found = check_module(
        "trailing-call-missing-return",
        "module m\n\
         fn helper()\n    var x = 1\n\n\
         fn g(): int\n    return 1\n\n\
         fn ends_in_void_call(): int\n    helper()\n\n\
         fn ends_in_print(): int\n    print(\"x\")\n\n\
         fn ends_in_value_call(): int\n    g()\n",
        "check.missing_return",
    );
    assert_eq!(found.len(), 3, "{found:#?}");
}

/// Each operator enforces its operand types: a single `check.operator_type` fires when
/// a `var x = ...` body mixes types the operator does not accept. The rows cover the
/// arithmetic, string addition, logical, comparison, and unary operators in turn.
#[test]
fn rejects_an_operator_on_wrongly_typed_operands() {
    let cases: &[(&str, &str)] = &[
        // `+` needs matching numeric operands; `1 + true` adds an int and a bool.
        ("op-arith", "fn f()\n    var x = 1 + true\n"),
        // String addition requires two strings; `"x" + 1` mixes string and int.
        ("op-string-add", "fn f()\n    var x = \"x\" + 1\n"),
        // `and` needs bool operands; `true and 1` mixes in an int.
        ("op-logical", "fn f()\n    var x = true and 1\n"),
        // Ordering compares same-typed values; `1 < "a"` mixes int and string.
        ("op-compare", "fn f()\n    var x = 1 < \"a\"\n"),
        // `not` needs a bool operand; `not 1` negates an int.
        ("op-unary", "fn f()\n    var x = not 1\n"),
    ];
    for (name, source) in cases {
        let found = check_script(name, source, "check.operator_type");
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
    }
}

#[test]
fn bytes_interpolation_renders_as_hex() {
    // A `bytes` value renders directly in interpolation as `0x`-prefixed hex, so it
    // is an accepted render source rather than a check error.
    let report = check_module_report(
        "interp-bytes",
        "module m\nfn f(): string\n    const b: bytes = b\"hi\"\n    return $\"<{b}>\"\n",
    );
    assert_clean(&report);
}

#[test]
fn infers_parameter_types_for_operator_checks() {
    // `b` is declared `bool`, so `b + 1` adds a bool to an int.
    let found = check_script(
        "op-param",
        "fn f(b: bool): int\n    return b + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_durable_surface_under_a_memory_backend_is_a_check_error() {
    // A store, an enum, and a resource each need committed catalog identity, which a
    // `memory` backend cannot establish — the runtime faults `run.durable_store_required`,
    // and the checker rejects them earlier because the backend is statically known.
    let cases: &[(&str, &str)] = &[
        (
            "durable-store-memory",
            "module m\nresource Book\n    required title: string\n\nstore ^books(id: int): Book\n",
        ),
        (
            "durable-enum-memory",
            "module m\nenum Color\n    Red\n    Green\n",
        ),
        (
            "durable-resource-memory",
            "module m\nresource Book\n    required title: string\n",
        ),
    ];
    for (name, src) in cases {
        let root = temp_project(name, |root| write(root, "src/m.mw", src));
        let (report, _program) = check_project(&root, &memory_config()).expect("check");
        assert_eq!(
            with_code(&report, "check.durable_store_required").len(),
            1,
            "{name}: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn a_pure_scalar_program_under_a_memory_backend_is_clean() {
    // A program that declares no durable surface proposes no catalog identity, so it
    // runs under a `memory` backend and the checker leaves it clean.
    let root = temp_project("pure-scalar-memory", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nfn add(a: int, b: int): int\n    return a + b\n",
        );
    });
    let (report, _program) = check_project(&root, &memory_config()).expect("check");
    assert!(
        with_code(&report, "check.durable_store_required").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn well_typed_operators_are_not_flagged() {
    // Every operator here has correctly typed operands.
    let found = check_script(
        "op-ok",
        "fn ok(a: int, b: int, s: string, t: string, p: bool, q: bool): bool\n\
         \x20   const sum = a + b\n\
         \x20   const quot = a / b\n\
         \x20   const cat = s + t\n\
         \x20   const cmp = a < b\n\
         \x20   const ne = a != b\n\
         \x20   const both = p and q\n\
         \x20   const neg = -a\n\
         \x20   const inv = not p\n\
         \x20   return both\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn operators_on_unknown_operands_are_not_flagged() {
    // `mystery()` calls an unresolved function, so its result type is unknown; the
    // checker only flags an operator when both operand types are known to be
    // incompatible. (A bare name would itself be a `check.unresolved_name` error,
    // so a call is used here to isolate the operator behavior.)
    let found = check_script(
        "op-unknown",
        "fn f()\n    var x = mystery() + 1\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_bare_undefined_name_is_flagged() {
    // Strict typing: `mystery` is not a parameter, local, loop binding, catch
    // binding, or module constant, so it is genuinely undefined.
    let found = check_script(
        "name-undefined",
        "fn f()\n    var x = mystery\n",
        "check.unresolved_name",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_defined_name_is_not_flagged() {
    // A parameter is in scope, so referencing it is not an unresolved name.
    let found = check_script(
        "name-defined",
        "fn f(a: int)\n    var x = a\n",
        "check.unresolved_name",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unresolved_call_is_not_flagged_as_a_name() {
    // A bare name in callee position names a function, not a value. An unresolved
    // function call is a separate concern, so it is not a `check.unresolved_name`.
    let found = check_script(
        "name-callee",
        "fn f()\n    var x = mystery()\n",
        "check.unresolved_name",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_assignment_to_an_undeclared_name_is_flagged() {
    // Assigning to a name that was never declared targets an unresolved name. The
    // runtime faults the same way (`run.unbound_name`), so the checker catches it
    // earlier rather than weaker than its own runtime.
    let found = check_script(
        "name-assign-undeclared",
        "fn f()\n    x = 1\n",
        "check.unresolved_name",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_mixing_int_and_decimal_arithmetic() {
    // Numeric operands must match exactly; there is no implicit int-to-decimal
    // promotion, so `1.0 + 1` is an error.
    let found = check_script(
        "op-promote",
        "fn f()\n    var x = 1.0 + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_nested_operator_error_is_reported_once() {
    // `1 + true` is the error; the outer `+ 2` sees an unknown left operand (the
    // flagged subexpression) and does not fire a second diagnostic.
    let found = check_script(
        "op-nested",
        "fn f()\n    var x = 1 + true + 2\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_non_bool_if_condition() {
    // `if 1` tests an int where a bool is required.
    let found = check_script(
        "cond-if",
        "fn f()\n    if 1\n        return\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_non_bool_while_condition() {
    // `while "go"` tests a string where a bool is required.
    let found = check_script(
        "cond-while",
        "fn f()\n    while \"go\"\n        break\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_non_bool_else_if_condition() {
    // The `else if 2` clause tests an int condition.
    let found = check_script(
        "cond-elseif",
        "fn f(c: bool)\n    if c\n        return\n    else if 2\n        return\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn bool_conditions_are_not_flagged() {
    // A bool binding and a comparison both yield bool conditions.
    let found = check_script(
        "cond-ok",
        "fn f(a: int, b: int, c: bool)\n    if a < b\n        return\n    while c\n        break\n",
        "check.condition_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unresolved_condition_is_flagged() {
    // Strict typing: `mystery` is unbound (unknown type), so the condition cannot
    // be shown to be `bool` — a `check.untyped_value` error (not a
    // `check.condition_type` non-bool mismatch).
    let found = check_script(
        "cond-unknown",
        "fn f()\n    if mystery\n        return\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    let non_bool = check_script(
        "cond-unknown",
        "fn f()\n    if mystery\n        return\n",
        "check.condition_type",
    );
    assert!(non_bool.is_empty(), "{non_bool:#?}");
}

#[test]
fn an_exists_condition_is_not_flagged() {
    // `exists(...)` resolves to `bool`, so a presence-check condition is clean.
    let found = check_module(
        "cond-exists",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    if exists(^books(1))\n        return\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn rejects_if_const_over_a_non_saved_read() {
    // `if const` is a saved-read binding guard, not a general binding statement.
    let found = check_script(
        "if-const-scalar",
        "fn f(): int\n    if const n = 1\n        return n\n    return 0\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn accepts_if_const_over_a_singleton_saved_root() {
    let found = check_module(
        "if-const-singleton",
        "module m\n\
         resource Settings\n    title: string\n\
         store ^settings: Settings\n\n\
         fn f(): string\n    if const settings = ^settings\n        return settings.title\n    return \"\"\n",
        "check.condition_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn rejects_if_const_over_a_keyed_root_without_identity() {
    let found = check_module(
        "if-const-keyed-root",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    if const books = ^books\n        return\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn accepts_if_const_over_a_fully_addressed_record() {
    let found = check_module(
        "if-const-record",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books)): string\n    if const book = ^books(id)\n        return book.title ?? \"\"\n    return \"\"\n",
        "check.condition_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn accepts_if_const_over_a_composite_identity_splice() {
    let found = check_module(
        "if-const-composite-identity",
        "module m\n\
         resource Enrollment\n    status: string\n\
         store ^enrollments(studentId: string, courseId: string): Enrollment\n\n\
         fn f(id: Id(^enrollments)): string\n    if const status = ^enrollments(id).status\n        return status\n    return \"\"\n",
        "check.condition_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn accepts_if_const_over_a_constructed_composite_identity() {
    let found = check_module(
        "if-const-constructed-composite-identity",
        "module m\n\
         resource Enrollment\n    status: string\n\
         store ^enrollments(studentId: string, courseId: string): Enrollment\n\n\
         fn f(): string\n    if const status = ^enrollments(Id(^enrollments, \"s1\", \"c1\")).status\n        return status\n    return \"\"\n",
        "check.condition_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn rejects_if_const_over_an_address_only_saved_layer() {
    // A bare keyed layer (`^books(id).tags`, no key column filled) names an iterable
    // sub-layer, not a bindable value. The partial-key type pass owns that mistake with
    // a precise `check.layer_not_value`; the generic "requires a saved value read"
    // condition check suppresses its cascade once that is recorded on the span, so the
    // subject is rejected with exactly one diagnostic.
    let report = check_module_report(
        "if-const-layer",
        "module m\n\
         resource Book\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books))\n    if const tags = ^books(id).tags\n        return\n",
    );
    let found = with_code(&report, "check.layer_not_value");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        with_code(&report, "check.condition_type").is_empty(),
        "the precise partial-key error owns the rejection: {:#?}",
        report.diagnostics
    );
}

#[test]
fn accepts_if_const_over_a_fully_addressed_layer_entry() {
    let found = check_module(
        "if-const-layer-entry",
        "module m\n\
         resource Book\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books)): string\n    if const tag = ^books(id).tags(1)\n        return tag\n    return \"\"\n",
        "check.condition_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn accepts_if_const_over_a_neighbor_read() {
    // A `next`/`prev` neighbor result is maybe-present and resolves at the read
    // site like any maybe-present value, so it binds under `if const`. The bound
    // record identity types like any saved identity and reads a field.
    let report = check_module_report(
        "if-const-neighbor-read",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books)): string\n    if const n = next(^books(id))\n        return ^books(n).title ?? \"\"\n    return \"\"\n",
    );
    assert_clean(&report);
}

#[test]
fn accepts_if_const_over_a_keyed_layer_neighbor_read() {
    // Over a keyed child layer `next`/`prev` types to the layer's key, so the
    // `if const` binding is usable as that key — addressing the sibling entry —
    // and as a plain value of the key's scalar type. The store-root neighbor
    // already binds a usable identity; a keyed-layer neighbor must bind a usable
    // key the same way, without a `??` default to rescue the type.
    let report = check_module_report(
        "if-const-keyed-layer-neighbor-read",
        "module m\n\
         resource Book\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books)): string\n\
         \x20   if const n = next(^books(id).tags(1))\n\
         \x20       return ^books(id).tags(n) ?? \"\"\n\
         \x20   return \"\"\n",
    );
    assert_clean(&report);
}

#[test]
fn accepts_if_const_over_a_composite_keyed_layer_neighbor_read() {
    // A composite layer is a chain of single-key sub-layers; a fully-keyed leaf
    // steps the final column, so the neighbor binds that column's key type and
    // is usable as the trailing key argument.
    let report = check_module_report(
        "if-const-composite-layer-neighbor-read",
        "module m\n\
         resource Grid\n    cells(row: int, col: int): string\n\
         store ^grids(id: int): Grid\n\n\
         fn f(id: Id(^grids)): string\n\
         \x20   if const c = next(^grids(id).cells(0, 2))\n\
         \x20       return ^grids(id).cells(0, c) ?? \"\"\n\
         \x20   return \"\"\n",
    );
    assert_clean(&report);
}

#[test]
fn rejects_if_const_over_a_non_unique_index_branch() {
    let found = check_module(
        "if-const-non-unique-index",
        "module m\n\
         resource Book\n    shelf: string\n\
         store ^books(id: int): Book\n    index byShelf(shelf)\n\n\
         fn f()\n    if const found = ^books.byShelf(\"fiction\")\n        return\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn accepts_if_const_over_a_complete_unique_index_lookup() {
    let found = check_module(
        "if-const-unique-index",
        "module m\n\
         resource Book\n    isbn: string\n\
         store ^books(id: int): Book\n    index byIsbn(isbn) unique\n\n\
         fn f(): Id(^books)\n    if const id = ^books.byIsbn(\"isbn-1\")\n        return id\n    return Id(^books, 1)\n",
        "check.condition_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn rejects_if_const_over_an_incomplete_unique_index_lookup() {
    let found = check_module(
        "if-const-incomplete-unique-index",
        "module m\n\
         resource Book\n    isbn: string\n    edition: int\n\
         store ^books(id: int): Book\n    index byIsbn(isbn, edition) unique\n\n\
         fn f()\n    if const id = ^books.byIsbn(\"isbn-1\")\n        return\n",
        "check.condition_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_call_with_the_wrong_argument_count() {
    // `add` takes two parameters; `add(1)` and `add(1, 2, 3)` are both arity errors.
    let found = check_module(
        "call-arity",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(1)\n    var y = add(1, 2, 3)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn rejects_a_named_argument_that_is_not_a_parameter() {
    // `add` has no parameter `c`.
    let found = check_module(
        "call-named",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(a: 1, c: 2)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_duplicate_named_arguments() {
    // The second `a:` cannot stand in for the missing `c:` parameter.
    let found = check_module(
        "call-duplicate-named",
        "module m\n\
         fn add(a: int, b: int, c: int): int\n    return a + b + c\n\n\
         fn caller()\n    var x = add(a: 1, a: 2, b: 3)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::DuplicateNamedArgument("a".into())
    );
}

#[test]
fn correct_calls_are_not_flagged() {
    // Positional and named calls that match the signature are accepted.
    let found = check_module(
        "call-ok",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(1, 2)\n    var y = add(a: 5, b: 6)\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn calls_keep_their_declared_return_types() {
    let report = check_module_report(
        "call-return-types",
        "module m\n\
         fn parse(value: int): bool\n    return value == 0\n\
         fn take(remaining: int, unit: int): string\n    const next: int = remaining - unit\n    return \"ok\"\n\n\
         fn caller(): string\n    var n: int = 0\n    if parse(n)\n        const piece: string = take(n, 1)\n        return piece\n    return \"no\"\n",
    );
    assert_clean(&report);
}

#[test]
fn read_only_parameters_are_not_assignment_targets() {
    let found = check_module(
        "readonly-param",
        "module m\n\
         fn bump(value: int): int\n    value = value + 1\n    return value\n",
        "check.invalid_assign_target",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_bare_keyed_store_root_is_not_an_assignment_target() {
    // A keyed store root addressed with no identity key names the whole collection,
    // not a writable record. Every right-hand side is rejected at check; the runtime
    // would otherwise fault on the unaddressed root. The diagnostic is span-anchored
    // on the target and carries no payload, matching the other invalid-target cases.
    let cases: &[(&str, &str)] = &[
        ("bare-root-scalar", "    ^books = 5\n"),
        (
            "bare-root-local-seq",
            "    var xs: sequence[int]\n    append(xs, 1)\n    ^books = xs\n",
        ),
        ("bare-root-keys", "    ^books = keys(^others)\n"),
    ];
    for (name, body) in cases {
        let src = format!(
            "module m\n\
             resource Book\n    required title: string\n\
             store ^books(id: int): Book\n\
             store ^others(id: int): Book\n\n\
             fn f()\n{body}"
        );
        let report = check_module_report(name, &src);
        let found = with_code(&report, "check.invalid_assign_target");
        assert_eq!(found.len(), 1, "{name}: {:#?}", report.diagnostics);
        assert_eq!(found[0].payload, DiagnosticPayload::None, "{name}");
        // The diagnostic anchors on the bare-root target, on the line that writes it.
        let target_line = src
            .lines()
            .position(|line| line.trim_start().starts_with("^books ="))
            .expect("target line") as u32
            + 1;
        assert_eq!(found[0].span.line, target_line, "{name}: {:#?}", found[0]);
    }
}

#[test]
fn a_keyed_record_write_and_keyless_root_write_remain_valid_targets() {
    // The legitimate saved writes are untouched: a fully keyed record write addresses
    // one entry, and a keyless singleton root addresses its sole record directly.
    let report = check_module_report(
        "valid-saved-write-targets",
        "module m\n\
         resource Book\n    required title: string\n\
         resource Settings\n    required maxLoans: int\n\
         store ^books(id: int): Book\n\
         store ^settings: Settings\n\n\
         fn writeKeyed(b: Book)\n    ^books(1) = b\n\n\
         fn writeKeyless(s: Settings)\n    ^settings = s\n",
    );
    let found = with_code(&report, "check.invalid_assign_target");
    assert!(found.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn read_only_parameter_checks_respect_local_shadowing() {
    let report = check_module_report(
        "readonly-param-shadow",
        "module m\n\
         fn set_to(value: int): int\n    return 1\n\
         fn caller(value: int): int\n    if true\n        var value: int = 0\n        value = value + 1\n        value = set_to(value)\n        return value\n    return value\n",
    );
    assert_clean(&report);
}

#[test]
fn redeclaring_a_local_in_the_same_block_is_a_check_error() {
    // A second `const`/`var` of the same name in one block is a redeclaration, even
    // when the type changes. A redeclaration carries the first declaration's line in
    // its payload.
    let cases: &[(&str, &str)] = &[
        (
            "redeclare-const",
            "fn f()\n    const x = 1\n    const x = 2\n",
        ),
        ("redeclare-var", "fn f()\n    var x = 1\n    var x = 2\n"),
        (
            "redeclare-type-change",
            "fn f()\n    const x = 1\n    const x = \"two\"\n",
        ),
    ];
    for (name, source) in cases {
        let found = check_script(name, source, "check.duplicate_declaration");
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
        assert!(
            matches!(
                &found[0].payload,
                DiagnosticPayload::DuplicateDeclaration { name, .. } if name == "x"
            ),
            "{name}: {found:#?}"
        );
    }
}

#[test]
fn shadowing_a_local_in_an_inner_block_is_allowed() {
    // A `const` in an inner block shadows an outer one of the same name; distinct
    // blocks are distinct scopes, so this is not a redeclaration.
    let found = check_script(
        "shadow-inner-block",
        "fn f()\n    const x = 1\n    if true\n        const x = 2\n        print(x)\n",
        "check.duplicate_declaration",
    );
    assert!(found.is_empty(), "{found:#?}");
}
