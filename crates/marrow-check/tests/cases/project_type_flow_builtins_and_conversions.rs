use crate::support;
use crate::support_conversion;
use marrow_check::{
    AppendTargetDiagnostic, ConversionTarget, DiagnosticPayload, MarrowType, ScalarType,
};

use support::{assert_clean, check_module, check_module_report, with_code};
use support_conversion::conversion_source_payload;

#[test]
fn exists_and_append_builtin_return_types_feed_checks() {
    // `exists` returns `bool` and `append` returns `int`; using them in mismatched
    // operators is caught.
    let found = check_module(
        "builtin-returns",
        "module m\n\
         resource Book\n    title: string\n\
         \x20   tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    var a = exists(^books(1)) + 1\n    var b = append(^books(1).tags, \"t\") and true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn append_to_a_group_layer_is_a_check_error() {
    let found = check_module(
        "append-group-layer",
        "module m\n\
         resource Log\n    items(pos: int)\n        required n: int\n\
         store ^log(name: string): Log\n\n\
         fn add(name: string): int\n    return append(^log(name).items, 1)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::AppendTarget(AppendTargetDiagnostic::GroupLayer),
        "{found:#?}"
    );
}

#[test]
fn append_to_a_keyed_leaf_layer_still_checks_clean() {
    let report = check_module_report(
        "append-leaf-layer",
        "module m\n\
         resource Log\n    items(pos: int): int\n\
         store ^log(name: string): Log\n\n\
         fn add(name: string): int\n    return append(^log(name).items, 1)\n",
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn coalesce_yields_the_default_type() {
    // `path ?? default` types to the path's leaf-or-default type; with a string
    // default it is `string`, so `+ 1` is string-plus-int.
    let found = check_module(
        "coalesce-return",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    var x = (^books(1).title ?? \"none\") + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn coalesce_rejects_a_present_non_path_left_operand() {
    // `??` only defaults an absent read; a literal (or any always-present value)
    // on the left has nothing to default, so it is an operator misuse.
    let found = check_module(
        "coalesce-non-path",
        "module m\nfn f()\n    var x = 1 ?? 2\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn coalesce_rejects_a_mismatched_default_type() {
    // The default must match the path's leaf type: an `int` field defaulted with a
    // string is an operator misuse.
    let found = check_module(
        "coalesce-mismatch",
        "module m\n\
         resource Book\n    pages: int\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    var x = ^books(1).pages ?? \"none\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_std_call_return_type_feeds_operator_checks() {
    // `std::text::length` returns `int`, so `+ true` is int-plus-bool.
    let found = check_module(
        "std-return-op",
        "module m\nfn f()\n    var x = std::text::length(\"hi\") + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_std_call_return_type_feeds_the_return_type_check() {
    // `std::clock::now()` is `instant`, but `f` returns `int`.
    let found = check_module(
        "std-return-mismatch",
        "module m\nfn f(): int\n    return std::clock::now()\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_correctly_typed_std_call_return_is_not_flagged() {
    // `std::text::length` returns `int`, matching `f`'s declared `int` return.
    let found = check_module(
        "std-return-ok",
        "module m\nfn f(): int\n    return std::text::length(\"hi\")\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_sequence_returning_std_call_against_a_scalar_return_is_flagged() {
    // `std::text::split` returns `sequence[string]`; returning it from an `int`
    // function is a real type mismatch — a sequence is not a scalar.
    let found = check_module(
        "std-return-seq",
        "module m\nfn f(): int\n    return std::text::split(\"a,b\", \",\")\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_sequence_returning_std_call_against_a_matching_return_is_not_flagged() {
    // Returning `sequence[string]` from a `sequence[string]` function recurses into
    // the element type and checks clean.
    let found = check_module(
        "std-return-seq-ok",
        "module m\nfn f(): sequence[string]\n    return std::text::split(\"a,b\", \",\")\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_conversion_call_return_type_feeds_operator_checks() {
    // `int(raw)` returns `int`, so `+ true` is int-plus-bool.
    let found = check_module(
        "conv-return-op",
        "module m\nfn f(raw: unknown)\n    var x = int(raw) + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_conversion_into_a_mismatched_annotated_place_is_flagged() {
    // `int(raw)` is `int`, but the place is `string`.
    let found = check_module(
        "conv-assign-bad",
        "module m\nfn f(raw: unknown)\n    const s: string = int(raw)\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_conversion_into_a_matching_annotated_place_is_not_flagged() {
    // `int(raw)` is `int`, matching the declared `int` place — the documented
    // `const n: int = int(raw)` pattern checks clean.
    let found = check_module(
        "conv-assign-ok",
        "module m\nfn f(raw: unknown)\n    const n: int = int(raw)\n",
        "check.assignment_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn bytes_conversion_rejects_a_known_non_string_source() {
    let found = check_module(
        "bytes-conv-int",
        "module m\nfn f(): bytes\n    return bytes(int(9))\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        conversion_source_payload(
            ConversionTarget::Bytes,
            MarrowType::Primitive(ScalarType::Int)
        ),
        "{found:#?}"
    );
}

#[test]
fn bytes_conversion_accepts_string_bytes_and_unknown_sources() {
    let report = check_module_report(
        "bytes-conv-ok",
        "module m\n\
         fn fromString(s: string): bytes\n    return bytes(s)\n\n\
         fn fromBytes(b: bytes): bytes\n    return bytes(b)\n\n\
         fn fromUnknown(raw: unknown): bytes\n    return bytes(raw)\n",
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn conversion_calls_reject_known_unsupported_sources() {
    let found = check_module(
        "conv-known-bad-sources",
        "module m\n\
         enum Color\n    red\n    green\n\n\
         fn dateFromInt(): date\n    return date(1)\n\n\
         fn durationFromInt(): duration\n    return duration(1)\n\n\
         fn boolFromString(): bool\n    return bool(\"true\")\n\n\
         fn decimalFromBool(): decimal\n    return decimal(true)\n\n\
         fn enumToInt(): int\n    return int(Color::green)\n\n\
         fn enumToString(): string\n    return string(Color::green)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 6, "{found:#?}");
    let color = MarrowType::Enum {
        module: "m".into(),
        name: "Color".into(),
    };
    assert_eq!(
        found
            .iter()
            .map(|diagnostic| diagnostic.payload.clone())
            .collect::<Vec<_>>(),
        vec![
            conversion_source_payload(
                ConversionTarget::Date,
                MarrowType::Primitive(ScalarType::Int)
            ),
            conversion_source_payload(
                ConversionTarget::Duration,
                MarrowType::Primitive(ScalarType::Int)
            ),
            conversion_source_payload(
                ConversionTarget::Bool,
                MarrowType::Primitive(ScalarType::Str)
            ),
            conversion_source_payload(
                ConversionTarget::Decimal,
                MarrowType::Primitive(ScalarType::Bool)
            ),
            conversion_source_payload(ConversionTarget::Int, color.clone()),
            conversion_source_payload(ConversionTarget::Str, color),
        ],
        "{found:#?}"
    );
}

#[test]
fn conversion_calls_reject_extra_arguments() {
    let found = check_module(
        "conv-extra-args",
        "module m\n\
         fn f()\n\
         \x20   const asBool = bool(1, 1)\n\
         \x20   const asInt = int(\"1\", \"2\")\n\
         \x20   const asString = string(1, 2)\n\
         \x20   const asCode = ErrorCode(\"app.ok\", \"app.ok\")\n\
         \x20   const asBytes = bytes(\"a\", \"b\")\n\
         \x20   const asDate = date(\"2026-01-01\", \"2026-01-02\")\n\
         \x20   const asInstant = instant(\"2026-01-01T00:00:00Z\", \"2026-01-02T00:00:00Z\")\n\
         \x20   const asDuration = duration(\"PT1S\", \"PT2S\")\n\
         \x20   const asDecimal = decimal(\"1.0\", \"2.0\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 9, "{found:#?}");
}

#[test]
fn conversion_calls_reject_named_arguments() {
    let found = check_module(
        "conv-named-args",
        "module m\n\
         fn f()\n\
         \x20   const asBool = bool(value: 1)\n\
         \x20   const asInt = int(value: \"1\")\n\
         \x20   const asString = string(value: 1)\n\
         \x20   const asCode = ErrorCode(value: \"app.ok\")\n\
         \x20   const asBytes = bytes(value: \"a\")\n\
         \x20   const asDate = date(value: \"2026-01-01\")\n\
         \x20   const asInstant = instant(value: \"2026-01-01T00:00:00Z\")\n\
         \x20   const asDuration = duration(value: \"PT1S\")\n\
         \x20   const asDecimal = decimal(value: \"1.0\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 9, "{found:#?}");
}

#[test]
fn interpolation_rejects_enum_values() {
    let found = check_module(
        "interp-enum",
        "module m\n\
         enum Color\n    red\n    green\n\n\
         fn f(c: Color): string\n    return $\"c={c}\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::InterpolationUnsupportedSource {
            source: MarrowType::Enum {
                module: "m".into(),
                name: "Color".into(),
            },
        }
    );
}

#[test]
fn interpolation_rejects_temporal_values() {
    let found = check_module(
        "interp-temporals",
        "module m\n\
         fn f(): string\n\
         \x20   const d = std::clock::parseDate(\"2026-01-01\")\n\
         \x20   const i = std::clock::parseInstant(\"2026-01-01T00:00:00Z\")\n\
         \x20   const span = 1.hour\n\
         \x20   return $\"{d} {i} {span}\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 3, "{found:#?}");
    for source in [
        MarrowType::Primitive(ScalarType::Date),
        MarrowType::Primitive(ScalarType::Instant),
        MarrowType::Primitive(ScalarType::Duration),
    ] {
        assert!(
            found.iter().any(|diagnostic| diagnostic.payload
                == DiagnosticPayload::InterpolationUnsupportedSource {
                    source: source.clone(),
                }),
            "{source:?}: {found:#?}"
        );
    }
}

#[test]
fn print_allows_runtime_rendering_to_decide_value_support() {
    let report = check_module_report(
        "output-runtime-rendered",
        "module m\n\
         enum Color\n    red\n    green\n\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(c: Color, items: sequence[string], book: Book)\n\
         \x20   const d = std::clock::parseDate(\"2026-01-01\")\n\
         \x20   const i = std::clock::parseInstant(\"2026-01-01T00:00:00Z\")\n\
         \x20   const span = 1.hour\n\
         \x20   const b = b\"hi\"\n\
         \x20   print(d)\n\
         \x20   print(i)\n\
         \x20   print(span)\n\
         \x20   print(b)\n\
         \x20   print(c)\n\
         \x20   print(items)\n\
         \x20   print(book)\n",
    );
    assert_clean(&report);
}

#[test]
fn exists_rejects_neighbor_values() {
    for neighbor in ["next", "prev"] {
        let found = check_module(
            &format!("exists-{neighbor}"),
            &format!(
                "module m\n\
                 resource Book\n    required title: string\n\
                 store ^books(id: int): Book\n\n\
                 fn f(): bool\n    return exists({neighbor}(^books(1)))\n",
            ),
            "check.call_argument",
        );
        assert_eq!(found.len(), 1, "{neighbor}: {found:#?}");
    }
}

#[test]
fn exists_rejects_coalesced_neighbor_values() {
    for neighbor in ["next", "prev"] {
        let found = check_module(
            &format!("exists-coalesced-{neighbor}"),
            &format!(
                "module m\n\
                 resource Book\n    required title: string\n\
                 store ^books(id: int): Book\n\n\
                 fn f(fallback: Id(^books)): bool\n\
                 \x20   return exists({neighbor}(^books(1)) ?? fallback)\n",
            ),
            "check.call_argument",
        );
        assert_eq!(found.len(), 1, "{neighbor}: {found:#?}");
    }
}

#[test]
fn exists_rejects_plain_values() {
    for expression in ["id", "1"] {
        let found = check_module(
            &format!("exists-value-{expression}"),
            &format!(
                "module m\n\
                 resource Book\n    required title: string\n\
                 store ^books(id: int): Book\n\n\
                 fn f(id: int): bool\n    return exists({expression})\n",
            ),
            "check.call_argument",
        );
        assert_eq!(found.len(), 1, "{expression}: {found:#?}");
    }
}

#[test]
fn an_error_code_conversion_into_an_error_code_place_is_not_flagged() {
    let found = check_module(
        "conv-error-code",
        "module m\nfn f(raw: unknown)\n    const code: ErrorCode = ErrorCode(raw)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_invalid_literal_error_code_conversion_is_a_call_argument_error() {
    let found = check_module(
        "conv-error-code-invalid",
        "module m\nfn f(): ErrorCode\n    return ErrorCode(\"Not A Valid Code!!!\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn invalid_error_code_literals_with_bad_conversion_shape_are_diagnosed() {
    let report = check_module_report(
        "conv-error-code-invalid-shape",
        "module m\n\
         fn f()\n\
         \x20   const extra = ErrorCode(\"Not A Valid Code!!!\", \"app.ok\")\n\
         \x20   const named = ErrorCode(code: \"Not A Valid Code!!!\", other: \"app.ok\")\n",
    );
    let found = with_code(&report, "check.call_argument");
    assert!(found.len() >= 2, "{:#?}", report.diagnostics);
}

#[test]
fn type_surface_count_builtin_result_is_an_int() {
    let report = check_module_report(
        "count-result-int",
        "module m\n\
         resource Book\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn countBooks(): int\n    return count(^books)\n\n\
         fn countTags(id: Id(^books)): int\n    return count(^books(id).tags)\n",
    );
    assert_clean(&report);
}

#[test]
fn type_surface_count_of_a_non_path_is_not_an_int() {
    let found = check_module(
        "count-non-path",
        "module m\nfn f(): int\n    return count(1)\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn type_surface_caught_error_fields_have_declared_types() {
    let report = check_module_report(
        "caught-error-fields",
        "module m\n\
         fn f()\n\
         \x20   try\n        throw Error(code: \"x.y\", message: \"boom\")\n\
         \x20   catch err: Error\n\
         \x20       const code: ErrorCode = err.code\n\
         \x20       const message: string = err.message\n",
    );
    assert_clean(&report);
}

#[test]
fn an_unknown_error_field_is_flagged_without_untyped_noise() {
    let report = check_module_report(
        "unknown-error-field",
        "module m\n\
         fn f(): string\n\
         \x20   const err = Error(code: \"x.y\", message: \"boom\")\n\
         \x20   return err.nope\n",
    );
    assert_eq!(
        report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        vec!["check.unknown_field"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_op_in_a_closed_pure_module_is_flagged_at_check() {
    let found = check_module(
        "std-closed-unknown-op",
        "module m\nfn f()\n    std::math::bogus(1)\n",
        "check.unresolved_call",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::UnresolvedCall("std::math::bogus".into()),
        "{found:#?}"
    );
}

#[test]
fn an_unknown_op_in_std_assert_is_flagged_at_check() {
    let found = check_module(
        "std-assert-unknown-op",
        "module m\nfn f()\n    std::assert::bogus(1)\n",
        "check.unresolved_call",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn std_assert_equal_accepts_same_scalar_type_arguments() {
    let report = check_module_report(
        "std-assert-equal-scalars",
        "module m\nfn f()\n    std::assert::equal(1, 1)\n    std::assert::equal(\"a\", \"a\")\n    std::assert::equal(true, false)\n",
    );
    assert_clean(&report);
}

#[test]
fn std_assert_equal_rejects_mismatched_scalar_types() {
    let found = check_module(
        "std-assert-equal-mismatch",
        "module m\nfn f()\n    std::assert::equal(1, \"1\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn std_assert_equal_rejects_non_scalar_arguments() {
    let found = check_module(
        "std-assert-equal-sequence",
        "module m\nfn f(xs: sequence[int])\n    std::assert::equal(xs, xs)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");

    let found = check_module(
        "std-assert-equal-identity",
        "module m\nresource Book\n    title: string\nstore ^books(id: int): Book\n\nfn f(id: Id(^books))\n    std::assert::equal(id, id)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_known_op_in_a_closed_pure_module_checks_clean() {
    let found = check_module(
        "std-closed-known-op",
        "module m\nfn f(): int\n    return std::math::absInt(-1)\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unknown_op_in_a_host_module_is_flagged_at_check() {
    let found = check_module(
        "std-host-unknown-op",
        "module m\nfn f()\n    std::io::frobnicate(\"p\")\n",
        "check.unresolved_call",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::UnresolvedCall("std::io::frobnicate".into()),
        "{found:#?}"
    );
}
