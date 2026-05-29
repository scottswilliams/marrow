//! Evaluate pure scalar functions: arithmetic, comparison, logical operators,
//! locals, and conditionals over integer and boolean values.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use marrow_check::{CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, MarrowType};
use marrow_run::{
    Host, RUN_ABSENT, RUN_ASSERT, RUN_CAPABILITY, RUN_DIVIDE_BY_ZERO, RUN_NO_ENCLOSING_LOOP,
    RUN_NO_VALUE, RUN_OVERFLOW, RUN_STORE, RUN_TRAVERSAL, RUN_TYPE, RUN_UNBOUND_NAME,
    RUN_UNCAUGHT_THROW, RUN_UNKNOWN_FUNCTION, RUN_UNSUPPORTED, RunOutput, SavedPathClass, Value,
    classify_saved_path, evaluate_function, run_entry, run_entry_with_host,
};
use marrow_schema::compile_resource;
use marrow_store::backend::Backend;
use marrow_store::mem::{MemStore, Presence, ScanPage, StoreError};
use marrow_store::path::{ChildSegment, PathSegment, SavedKey, encode_path};
use marrow_store::redb::RedbStore;
use marrow_store::value::{SavedValue, ValueType, decode_value, encode_value};
use marrow_syntax::{Declaration, FunctionDecl, parse_source};

/// Parse `source` and return the single function it declares.
fn function(source: &str) -> FunctionDecl {
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
    parsed
        .file
        .declarations
        .into_iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) => Some(function),
            _ => None,
        })
        .expect("a function declaration")
}

/// Wrap every function in `source` into a one-module checked program named
/// `test`, so `run(&program, "test::name", ...)` resolves calls between
/// them. Parameter types are left `Unknown` — the runtime binds by name.
fn checked_program(source: &str) -> CheckedProgram {
    checked_program_with_imports(source, &[])
}

/// Like [`checked_program`], but with the module's resolved `use` targets
/// populated, so short-form calls (`clock::parseDate(...)`) expand to their full
/// paths at the call site (the checker normally builds this from `use` decls).
fn checked_program_with_imports(source: &str, imports: &[&str]) -> CheckedProgram {
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
    let mut functions = Vec::new();
    let mut resources = Vec::new();
    for declaration in parsed.file.declarations {
        match declaration {
            Declaration::Function(function) => functions.push(CheckedFunction {
                name: function.name.clone(),
                public: function.public,
                params: function
                    .params
                    .iter()
                    .map(|param| CheckedParam {
                        name: param.name.clone(),
                        mode: param.mode,
                        ty: MarrowType::Unknown,
                    })
                    .collect(),
                return_type: None,
                span: function.span,
                touches_saved_data: false,
                body: function.body,
            }),
            Declaration::Resource(resource) => {
                let (schema, errors) = compile_resource(&resource);
                assert!(errors.is_empty(), "{errors:?}");
                resources.push(schema);
            }
            _ => {}
        }
    }
    CheckedProgram {
        modules: vec![CheckedModule {
            name: "test".into(),
            source_file: std::path::PathBuf::new(),
            span: Default::default(),
            imports: imports.iter().map(|name| name.to_string()).collect(),
            constants: Vec::new(),
            functions,
            resources,
        }],
    }
}

/// Run an entry function against an empty store, returning only its value.
fn run(
    program: &CheckedProgram,
    entry: &str,
    args: &[Value],
) -> Result<Option<Value>, marrow_run::RuntimeError> {
    let store = RefCell::new(MemStore::new());
    run_entry(program, &store, entry, args).map(|outcome| outcome.value)
}

/// Run an entry function against an empty store, returning its value and output.
fn run_full(
    program: &CheckedProgram,
    entry: &str,
    args: &[Value],
) -> Result<RunOutput, marrow_run::RuntimeError> {
    let store = RefCell::new(MemStore::new());
    run_entry(program, &store, entry, args)
}

#[test]
fn evaluates_arithmetic_over_parameters() {
    let add = function("fn add(a: int, b: int): int\n    return a + b\n");
    assert_eq!(
        evaluate_function(&add, &[Value::Int(2), Value::Int(40)]),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn respects_arithmetic_precedence() {
    // 2 + 3 * 4 == 14, not 20.
    let f = function("fn f(): int\n    return 2 + 3 * 4\n");
    assert_eq!(evaluate_function(&f, &[]), Ok(Some(Value::Int(14))));
}

#[test]
fn evaluates_decimal_literals_and_arithmetic() {
    // Decimal `+`, `*`, and `-` over decimal operands, rendered to text.
    let program = checked_program(
        "pub fn f(): string\n    return $\"{1.5 + 2.5} {1.5 * 2.0} {5.5 - 0.5}\"\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str("4 3 5".into()))
    );
}

#[test]
fn negates_a_decimal() {
    // Unary `-` on a decimal, and a subtraction that produces a negative decimal.
    let program = checked_program("pub fn f(): string\n    return $\"{-1.5} {0.0 - 2.5}\"\n");
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str("-1.5 -2.5".into()))
    );
}

#[test]
fn division_yields_a_decimal() {
    // `/` always yields a decimal, even for integer operands (1/2 = 0.5).
    let program =
        checked_program("pub fn f(): string\n    return $\"{1 / 2} {7 / 2} {1.0 / 4.0}\"\n");
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str("0.5 3.5 0.25".into()))
    );
}

#[test]
fn decimal_division_rounds_half_even() {
    // 1/3 rounds half-even to 34 significant digits.
    let program = checked_program("pub fn f(): string\n    return $\"{1 / 3}\"\n");
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str(format!("0.{}", "3".repeat(34))))
    );
}

#[test]
fn decimal_division_by_zero_is_a_runtime_error() {
    let program = checked_program("pub fn f(): decimal\n    return 1.0 / 0.0\n");
    assert_eq!(
        run(&program, "test::f", &[]).unwrap_err().code,
        RUN_DIVIDE_BY_ZERO
    );
}

#[test]
fn compares_decimal_values() {
    // Ordering and equality compare by value (1.50 equals 1.5).
    let program = checked_program(
        "pub fn f(): string\n    return $\"{1.5 < 2.0} {1.50 = 1.5} {2.5 > 3.0}\"\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str("true true false".into()))
    );
}

#[test]
fn decimal_round_trips_through_saved_data() {
    // A decimal field saves and loads unchanged.
    let program = checked_program(
        "resource Account at ^accts(id: int)\n\
         \x20   balance: decimal\n\
         \n\
         pub fn seed()\n\
         \x20   ^accts(1).balance = 9.99\n\
         \n\
         pub fn balance(): string\n\
         \x20   return $\"{^accts(1).balance}\"\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed runs");
    assert_eq!(
        run_entry(&program, &store, "test::balance", &[])
            .unwrap()
            .value,
        Some(Value::Str("9.99".into()))
    );
}

#[test]
fn evaluates_bytes_literals_and_equality() {
    let program = checked_program(
        "pub fn same(): bool\n    return b\"abc\" = b\"abc\"\n\n\
         pub fn different(): bool\n    return b\"abc\" = b\"abd\"\n",
    );
    assert_eq!(
        run(&program, "test::same", &[]).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(&program, "test::different", &[]).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn compares_bytes_by_byte_order() {
    let program = checked_program(
        "pub fn f(): bool\n    return b\"a\" < b\"b\"\n\n\
         pub fn g(): bool\n    return b\"ab\" > b\"a\"\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(&program, "test::g", &[]).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn bytes_round_trip_through_saved_data() {
    let program = checked_program(
        "resource Blob at ^blobs(id: int)\n\
         \x20   data: bytes\n\
         \n\
         pub fn seed()\n\
         \x20   ^blobs(1).data = b\"xy\"\n\
         \n\
         pub fn matches(): bool\n\
         \x20   return ^blobs(1).data = b\"xy\"\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed runs");
    assert_eq!(
        run_entry(&program, &store, "test::matches", &[])
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn converts_string_to_bytes_and_measures_length() {
    let program = checked_program(
        "pub fn short(): int\n    return std::bytes::length(bytes(\"hi\"))\n\n\
         pub fn utf8(): int\n    return std::bytes::length(bytes(\"café\"))\n",
    );
    assert_eq!(
        run(&program, "test::short", &[]).unwrap(),
        Some(Value::Int(2))
    );
    // `café` is 4 characters but 5 UTF-8 bytes; std::bytes::length counts bytes.
    assert_eq!(
        run(&program, "test::utf8", &[]).unwrap(),
        Some(Value::Int(5))
    );
}

#[test]
fn bytes_conversion_equals_a_bytes_literal() {
    let program = checked_program("pub fn f(): bool\n    return bytes(\"xy\") = b\"xy\"\n");
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn base64_encodes_with_padding() {
    let program = checked_program(
        "pub fn a(): string\n    return std::bytes::base64Encode(b\"hello\")\n\n\
         pub fn b(): string\n    return std::bytes::base64Encode(b\"a\")\n\n\
         pub fn c(): string\n    return std::bytes::base64Encode(b\"ab\")\n\n\
         pub fn d(): string\n    return std::bytes::base64Encode(b\"abc\")\n",
    );
    assert_eq!(
        run(&program, "test::a", &[]).unwrap(),
        Some(Value::Str("aGVsbG8=".into()))
    );
    assert_eq!(
        run(&program, "test::b", &[]).unwrap(),
        Some(Value::Str("YQ==".into()))
    );
    assert_eq!(
        run(&program, "test::c", &[]).unwrap(),
        Some(Value::Str("YWI=".into()))
    );
    // An exact 3-byte group needs no padding.
    assert_eq!(
        run(&program, "test::d", &[]).unwrap(),
        Some(Value::Str("YWJj".into()))
    );
}

#[test]
fn base64_decodes_and_round_trips() {
    let program = checked_program(
        "pub fn known(): bool\n    return std::bytes::base64Decode(\"aGVsbG8=\") = b\"hello\"\n\n\
         pub fn round(): bool\n    return std::bytes::base64Decode(std::bytes::base64Encode(b\"hi there\")) = b\"hi there\"\n",
    );
    assert_eq!(
        run(&program, "test::known", &[]).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(&program, "test::round", &[]).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn base64_decode_rejects_invalid_text() {
    // Invalid characters, and `=` padding outside the final group.
    let program = checked_program(
        "pub fn bad_chars(): bytes\n    return std::bytes::base64Decode(\"!!!!\")\n\n\
         pub fn early_pad(): bytes\n    return std::bytes::base64Decode(\"AAA=AAAA\")\n",
    );
    assert!(run(&program, "test::bad_chars", &[]).is_err());
    assert!(run(&program, "test::early_pad", &[]).is_err());
}

#[test]
fn splits_a_string_and_iterates_the_sequence() {
    // `std::text::split` yields a sequence the `for` loop iterates in order.
    let program = checked_program(
        "pub fn f(): string\n\
         \x20   var result = \"\"\n\
         \x20   for word in std::text::split(\"a,b,c\", \",\")\n\
         \x20       result = result _ word\n\
         \x20   return result\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str("abc".into()))
    );
}

#[test]
fn iterates_a_sequence_counting_its_elements() {
    let program = checked_program(
        "pub fn count(): int\n\
         \x20   var n = 0\n\
         \x20   for word in std::text::split(\"a,b,c,d\", \",\")\n\
         \x20       n = n + 1\n\
         \x20   return n\n",
    );
    assert_eq!(
        run(&program, "test::count", &[]).unwrap(),
        Some(Value::Int(4))
    );
}

#[test]
fn std_math_decimal_helpers() {
    // absDecimal yields a decimal; floor rounds toward negative infinity to an int.
    let program = checked_program(
        "pub fn a(): string\n    return $\"{std::math::absDecimal(-2.5)}\"\n\n\
         pub fn up(): int\n    return std::math::floor(2.7)\n\n\
         pub fn down(): int\n    return std::math::floor(-2.7)\n",
    );
    assert_eq!(
        run(&program, "test::a", &[]).unwrap(),
        Some(Value::Str("2.5".into()))
    );
    assert_eq!(run(&program, "test::up", &[]).unwrap(), Some(Value::Int(2)));
    assert_eq!(
        run(&program, "test::down", &[]).unwrap(),
        Some(Value::Int(-3))
    );
}

#[test]
fn formats_and_parses_instants() {
    // An instant round-trips through its canonical UTC text.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatInstant(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"))\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str("2026-05-28T12:00:00Z".into()))
    );
}

#[test]
fn parse_instant_rejects_invalid_text() {
    let program = checked_program(
        "pub fn f(): instant\n    return std::clock::parseInstant(\"not a time\")\n",
    );
    assert!(run(&program, "test::f", &[]).is_err());
}

#[test]
fn formats_and_parses_dates() {
    // A date round-trips through its canonical YYYY-MM-DD text (leap day).
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatDate(std::clock::parseDate(\"2024-02-29\"))\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str("2024-02-29".into()))
    );
}

#[test]
fn formats_and_parses_durations() {
    // A duration round-trips through its canonical PT<seconds>S text.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatDuration(std::clock::parseDuration(\"PT90S\"))\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str("PT90S".into()))
    );
}

#[test]
fn clock_add_offsets_an_instant_by_a_duration() {
    // add(instant, duration): one hour after noon UTC is 13:00.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatInstant(std::clock::add(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"), std::clock::parseDuration(\"PT3600S\")))\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap(),
        Some(Value::Str("2026-05-28T13:00:00Z".into()))
    );
}

#[test]
fn clock_today_reads_the_host_clock_capability() {
    // `today()` is the host clock's UTC calendar date.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatDate(std::clock::today())\n",
    );
    let store = RefCell::new(MemStore::new());
    // 2023-11-14T22:13:20Z.
    let host = Host::new().with_clock(1_700_000_000_000_000_000);
    let outcome = run_entry_with_host(&program, &store, &host, "test::f", &[]).expect("today");
    assert_eq!(outcome.value, Some(Value::Str("2023-11-14".into())));
}

#[test]
fn clock_today_without_a_clock_capability_is_a_capability_error() {
    let program = checked_program("fn t(): date\n    return std::clock::today()\n");
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::t", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

#[test]
fn a_date_round_trips_through_saved_data() {
    // A `date` value saves and loads through a managed field write and read.
    let program = checked_program(
        "resource Event at ^events(id: int)\n    on: date\n\nfn record(id: int, text: string)\n    ^events(id).on = std::clock::parseDate(text)\n\nfn dateOf(id: int): string\n    return std::clock::formatDate(^events(id).on)\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::record",
        &[Value::Int(1), Value::Str("2024-02-29".into())],
    )
    .expect("record");
    let outcome = run_entry(&program, &store, "test::dateOf", &[Value::Int(1)]).expect("read");
    assert_eq!(outcome.value, Some(Value::Str("2024-02-29".into())));
}

#[test]
fn temporal_values_order_and_equate() {
    // Dates, instants, and durations compare by their underlying counts, matching
    // the ordered/equatable types the checker already advertises.
    let program = checked_program(
        "fn dateBefore(a: string, b: string): bool\n    return std::clock::parseDate(a) < std::clock::parseDate(b)\nfn dateSame(a: string, b: string): bool\n    return std::clock::parseDate(a) = std::clock::parseDate(b)\nfn instantBefore(a: string, b: string): bool\n    return std::clock::parseInstant(a) < std::clock::parseInstant(b)\nfn durationBefore(a: string, b: string): bool\n    return std::clock::parseDuration(a) < std::clock::parseDuration(b)\n",
    );
    let call = |entry: &str, a: &str, b: &str| {
        run(
            &program,
            entry,
            &[Value::Str(a.into()), Value::Str(b.into())],
        )
    };
    assert_eq!(
        call("test::dateBefore", "2024-01-01", "2024-12-31"),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        call("test::dateBefore", "2024-12-31", "2024-01-01"),
        Ok(Some(Value::Bool(false)))
    );
    assert_eq!(
        call("test::dateSame", "2024-02-29", "2024-02-29"),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        call(
            "test::instantBefore",
            "2026-05-28T12:00:00Z",
            "2026-05-28T13:00:00Z"
        ),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        call("test::durationBefore", "PT60S", "PT3600S"),
        Ok(Some(Value::Bool(true)))
    );
}

/// A short-form `clock::formatDate(clock::parseDate(s))` dispatches at runtime
/// exactly like the fully-qualified `std::clock::...` form, because the call
/// frame carries the module's `use std::clock` alias and `eval_call` expands the
/// leading segment before std dispatch. Uses pure helpers, so no host clock is
/// needed.
#[test]
fn short_form_std_call_runs() {
    let program = checked_program_with_imports(
        "fn roundtrip(s: string): string\n    return clock::formatDate(clock::parseDate(s))\n",
        &["std::clock"],
    );
    assert_eq!(
        run(
            &program,
            "test::roundtrip",
            &[Value::Str("2024-02-29".into())]
        ),
        Ok(Some(Value::Str("2024-02-29".into())))
    );
}

/// Without the matching import, a short-form `clock::parseDate(...)` does not
/// expand and is not a known function — `run.unknown_function`. (The checker
/// catches this earlier with `check.unresolved_call`; this is the runtime's own
/// behavior, kept symmetric.)
#[test]
fn short_form_without_import_is_unknown_at_runtime() {
    let program = checked_program_with_imports(
        "fn stamp(s: string): string\n    return clock::formatDate(clock::parseDate(s))\n",
        &[],
    );
    let result = run(&program, "test::stamp", &[Value::Str("2024-02-29".into())]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNKNOWN_FUNCTION),
        "{result:?}"
    );
}

#[test]
fn scalar_conversions_validate_a_dynamic_value() {
    // A conversion builtin asserts a dynamically-typed value is the target type
    // and returns it (the `unknown` → concrete bridge).
    let program = checked_program(
        "fn asInt(v: int): int\n    return int(v)\nfn asString(v: string): string\n    return string(v)\nfn asBool(v: bool): bool\n    return bool(v)\n",
    );
    assert_eq!(
        run(&program, "test::asInt", &[Value::Int(42)]),
        Ok(Some(Value::Int(42)))
    );
    assert_eq!(
        run(&program, "test::asString", &[Value::Str("hi".into())]),
        Ok(Some(Value::Str("hi".into())))
    );
    assert_eq!(
        run(&program, "test::asBool", &[Value::Bool(true)]),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn a_conversion_rejects_a_value_of_the_wrong_type() {
    // `int(...)` validates; a string value is not an int.
    let program = checked_program("fn f(v: int): int\n    return int(v)\n");
    assert_eq!(
        run(&program, "test::f", &[Value::Str("x".into())])
            .unwrap_err()
            .code,
        RUN_TYPE
    );
}

#[test]
fn temporal_conversions_validate_their_values() {
    // `date`/`instant`/`duration` validate canonical temporal values (here built
    // via the std::clock parsers), returning them unchanged.
    let program = checked_program(
        "fn d(t: string): string\n    return std::clock::formatDate(date(std::clock::parseDate(t)))\nfn span(t: string): string\n    return std::clock::formatDuration(duration(std::clock::parseDuration(t)))\n",
    );
    assert_eq!(
        run(&program, "test::d", &[Value::Str("2024-02-29".into())]),
        Ok(Some(Value::Str("2024-02-29".into())))
    );
    assert_eq!(
        run(&program, "test::span", &[Value::Str("PT90S".into())]),
        Ok(Some(Value::Str("PT90S".into())))
    );
}

#[test]
fn bool_conversion_accepts_canonical_int_and_string_forms() {
    // `types.md` pins `bool(...)` to accept `false`, `true`, `0`, and `1`, from
    // both int and the canonical string forms.
    let program = checked_program(
        "fn b(v: int): bool\n    return bool(v)\nfn bs(v: string): bool\n    return bool(v)\n",
    );
    assert_eq!(
        run(&program, "test::b", &[Value::Int(0)]),
        Ok(Some(Value::Bool(false)))
    );
    assert_eq!(
        run(&program, "test::b", &[Value::Int(1)]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        run(&program, "test::bs", &[Value::Str("true".into())]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        run(&program, "test::bs", &[Value::Str("0".into())]),
        Ok(Some(Value::Bool(false)))
    );
}

#[test]
fn bool_conversion_rejects_a_non_canonical_int() {
    // Only `0` and `1` are canonical; `2` is a type error, not a coercion.
    let program = checked_program("fn b(v: int): bool\n    return bool(v)\n");
    assert_eq!(
        run(&program, "test::b", &[Value::Int(2)]).unwrap_err().code,
        RUN_TYPE
    );
}

#[test]
fn int_conversion_parses_canonical_text() {
    let program = checked_program("fn n(v: string): int\n    return int(v)\n");
    assert_eq!(
        run(&program, "test::n", &[Value::Str("12".into())]),
        Ok(Some(Value::Int(12)))
    );
    assert_eq!(
        run(&program, "test::n", &[Value::Str("-7".into())]),
        Ok(Some(Value::Int(-7)))
    );
}

#[test]
fn decimal_conversion_parses_canonical_text() {
    // `decimal("1.5")` parses to a decimal; rendered back through interpolation it
    // round-trips to its canonical text.
    let program = checked_program("fn d(v: string): string\n    return $\"{decimal(v)}\"\n");
    assert_eq!(
        run(&program, "test::d", &[Value::Str("1.5".into())]),
        Ok(Some(Value::Str("1.5".into())))
    );
}

#[test]
fn a_numeric_conversion_rejects_malformed_text() {
    // Malformed text is a typed numeric error, not a silent zero.
    let program = checked_program("fn n(v: string): int\n    return int(v)\n");
    assert_eq!(
        run(&program, "test::n", &[Value::Str("nope".into())])
            .unwrap_err()
            .code,
        RUN_TYPE
    );
    let program = checked_program("fn d(v: string): decimal\n    return decimal(v)\n");
    assert_eq!(
        run(&program, "test::d", &[Value::Str("1.2.3".into())])
            .unwrap_err()
            .code,
        RUN_TYPE
    );
}

#[test]
fn a_conversion_error_message_is_grammar_independent() {
    // The message must not embed an article, so it reads correctly for
    // vowel-initial type names (not 'requires a int value').
    let program = checked_program("fn n(v: string): int\n    return int(v)\n");
    assert_eq!(
        run(&program, "test::n", &[Value::Str("nope".into())])
            .unwrap_err()
            .message,
        "cannot convert this value to int"
    );
}

#[test]
fn evaluates_conditionals() {
    let max =
        function("fn max(a: int, b: int): int\n    if a > b\n        return a\n    return b\n");
    assert_eq!(
        evaluate_function(&max, &[Value::Int(7), Value::Int(3)]),
        Ok(Some(Value::Int(7)))
    );
    assert_eq!(
        evaluate_function(&max, &[Value::Int(3), Value::Int(7)]),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn std_assert_is_true_passes_and_fails() {
    let program = checked_program("pub fn ok()\n    std::assert::isTrue(1 = 1)\n");
    assert_eq!(run(&program, "test::ok", &[]), Ok(None));

    let program = checked_program("pub fn bad()\n    std::assert::isTrue(1 = 2)\n");
    assert_eq!(
        run(&program, "test::bad", &[]).unwrap_err().code,
        RUN_ASSERT
    );
}

#[test]
fn std_assert_is_false_passes_and_fails() {
    let program = checked_program("pub fn ok()\n    std::assert::isFalse(1 = 2)\n");
    assert_eq!(run(&program, "test::ok", &[]), Ok(None));

    let program = checked_program("pub fn bad()\n    std::assert::isFalse(1 = 1)\n");
    assert_eq!(
        run(&program, "test::bad", &[]).unwrap_err().code,
        RUN_ASSERT
    );
}

#[test]
fn std_assert_fail_raises_with_its_message() {
    let program = checked_program("pub fn bad()\n    std::assert::fail(\"boom\")\n");
    let error = run(&program, "test::bad", &[]).unwrap_err();
    assert_eq!(error.code, RUN_ASSERT);
    assert!(error.message.contains("boom"), "{}", error.message);
}

#[test]
fn std_assert_absent_passes_when_nothing_is_saved() {
    let program = checked_program("pub fn ok()\n    std::assert::absent(^books(1))\n");
    assert_eq!(run(&program, "test::ok", &[]), Ok(None));
}

#[test]
fn std_assert_absent_fails_when_a_value_is_present() {
    let program = checked_program("pub fn bad()\n    std::assert::absent(^books(1))\n");
    let store = RefCell::new(MemStore::new());
    store.borrow_mut().write(
        &encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
        ]),
        encode_value(&SavedValue::Int(1)).expect("in-range value encodes"),
    );
    let error = run_entry(&program, &store, "test::bad", &[]).unwrap_err();
    assert_eq!(error.code, RUN_ASSERT);
}

#[test]
fn std_assert_rejects_misused_arguments() {
    // A non-boolean condition and a non-string message are type errors, distinct
    // from a failed assertion.
    let program = checked_program("pub fn bad()\n    std::assert::isTrue(1)\n");
    assert_eq!(run(&program, "test::bad", &[]).unwrap_err().code, RUN_TYPE);

    let program = checked_program("pub fn bad()\n    std::assert::fail(42)\n");
    assert_eq!(run(&program, "test::bad", &[]).unwrap_err().code, RUN_TYPE);
}

#[test]
fn a_passing_assert_lets_execution_continue() {
    // A passing assertion produces no value and falls through to later statements.
    let program =
        checked_program("pub fn ok(): int\n    std::assert::isTrue(1 = 1)\n    return 7\n");
    assert_eq!(run(&program, "test::ok", &[]), Ok(Some(Value::Int(7))));
}

#[test]
fn a_whole_group_entry_write_creates_the_entry() {
    // `^books(1).versions(2) = b` writes the whole group entry from a resource
    // value; the runtime matches its fields against the group's members by name.
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var b: Book\n\
         \x20\x20\x20\x20b.title = \"v2\"\n\
         \x20\x20\x20\x20^books(1).versions(2) = b\n\
         \n\
         pub fn version_title(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).title\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed runs");
    assert_eq!(
        run_entry(&program, &store, "test::version_title", &[])
            .unwrap()
            .value,
        Some(Value::Str("v2".into()))
    );
}

#[test]
fn a_nested_group_field_round_trips() {
    // `versions(version)` entries hold a nested `comments(pos)` group; writing and
    // reading `^books(1).versions(2).comments(3).text` exercises a saved-tree path
    // deeper than one keyed layer.
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20comments(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20required text: string\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20^books(1).versions(2).comments(3).text = \"deep\"\n\
         \n\
         pub fn comment(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).comments(3).text\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed runs");
    assert_eq!(
        run_entry(&program, &store, "test::comment", &[])
            .unwrap()
            .value,
        Some(Value::Str("deep".into()))
    );
}

#[test]
fn a_whole_group_entry_can_be_read_and_copied() {
    // `^books(1).versions(2) = ^books(1).versions(1)` reads the whole entry as a
    // value (RHS) and writes it to another key (LHS).
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var b: Book\n\
         \x20\x20\x20\x20b.title = \"v1\"\n\
         \x20\x20\x20\x20^books(1).versions(1) = b\n\
         \x20\x20\x20\x20^books(1).versions(2) = ^books(1).versions(1)\n\
         \n\
         pub fn copied_title(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).title\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed runs");
    assert_eq!(
        run_entry(&program, &store, "test::copied_title", &[])
            .unwrap()
            .value,
        Some(Value::Str("v1".into()))
    );
}

#[test]
fn std_text_builtins_operate_on_strings() {
    // `length` counts Unicode scalar values, not bytes ("café" is 4 scalars).
    let program = checked_program("pub fn f(): int\n    return std::text::length(\"café\")\n");
    assert_eq!(run(&program, "test::f", &[]), Ok(Some(Value::Int(4))));

    let program = checked_program("pub fn f(): string\n    return std::text::trim(\"  hi  \")\n");
    assert_eq!(
        run(&program, "test::f", &[]),
        Ok(Some(Value::Str("hi".into())))
    );

    let program =
        checked_program("pub fn f(): bool\n    return std::text::contains(\"hello\", \"ell\")\n");
    assert_eq!(run(&program, "test::f", &[]), Ok(Some(Value::Bool(true))));
}

#[test]
fn std_math_builtins_compute_over_integers() {
    let program = checked_program("pub fn f(): int\n    return std::math::absInt(0 - 7)\n");
    assert_eq!(run(&program, "test::f", &[]), Ok(Some(Value::Int(7))));

    // remainder is truncated (sign of the dividend): -7 rem 3 = -1.
    let program = checked_program("pub fn f(): int\n    return std::math::remainder(0 - 7, 3)\n");
    assert_eq!(run(&program, "test::f", &[]), Ok(Some(Value::Int(-1))));

    // modulo is floored (sign of the divisor): -7 mod 3 = 2.
    let program = checked_program("pub fn f(): int\n    return std::math::modulo(0 - 7, 3)\n");
    assert_eq!(run(&program, "test::f", &[]), Ok(Some(Value::Int(2))));
}

#[test]
fn std_math_modulo_by_zero_is_a_runtime_error() {
    let program = checked_program("pub fn f(): int\n    return std::math::modulo(7, 0)\n");
    assert_eq!(
        run(&program, "test::f", &[]).unwrap_err().code,
        RUN_DIVIDE_BY_ZERO
    );
}

#[test]
fn std_builtins_reject_wrong_argument_types() {
    // A non-string to a text helper and a non-int to a math helper are type errors.
    let program = checked_program("pub fn f(): int\n    return std::text::length(42)\n");
    assert_eq!(run(&program, "test::f", &[]).unwrap_err().code, RUN_TYPE);

    let program = checked_program("pub fn f(): int\n    return std::math::absInt(\"x\")\n");
    assert_eq!(run(&program, "test::f", &[]).unwrap_err().code, RUN_TYPE);
}

#[test]
fn throw_surfaces_as_an_uncaught_error() {
    let program = checked_program(
        "pub fn bad()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
    );
    let error = run(&program, "test::bad", &[]).unwrap_err();
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    assert!(error.message.contains("book.absent"), "{}", error.message);
    assert!(error.message.contains("no book"), "{}", error.message);
}

#[test]
fn error_constructor_requires_code_and_message() {
    let program = checked_program("pub fn bad()\n    throw Error(code: \"x.y\")\n");
    assert_eq!(run(&program, "test::bad", &[]).unwrap_err().code, RUN_TYPE);
}

#[test]
fn throw_is_an_error_value() {
    // `throw` of a non-Error value is a type error, not a thrown error.
    let program = checked_program("pub fn bad()\n    throw 7\n");
    assert_eq!(run(&program, "test::bad", &[]).unwrap_err().code, RUN_TYPE);
}

#[test]
fn catch_binds_the_thrown_error_and_recovers() {
    let program = checked_program(
        "pub fn safe(): string\n    try\n        throw Error(code: \"x.y\", message: \"boom\")\n    catch err: Error\n        return err.message\n",
    );
    assert_eq!(
        run(&program, "test::safe", &[]),
        Ok(Some(Value::Str("boom".into())))
    );
}

#[test]
fn a_try_that_succeeds_skips_catch() {
    let program = checked_program(
        "pub fn ok(): int\n    try\n        return 1\n    catch err: Error\n        return 2\n",
    );
    assert_eq!(run(&program, "test::ok", &[]), Ok(Some(Value::Int(1))));
}

#[test]
fn finally_runs_on_success_and_on_throw() {
    let program = checked_program(
        "pub fn run_it(do_throw: bool)\n    try\n        if do_throw\n            throw Error(code: \"x.y\", message: \"b\")\n    catch err: Error\n        write(\"caught \")\n    finally\n        write(\"cleanup\")\n",
    );
    let out = |b| {
        run_full(&program, "test::run_it", &[Value::Bool(b)])
            .unwrap()
            .output
    };
    assert_eq!(out(false), "cleanup");
    assert_eq!(out(true), "caught cleanup");
}

#[test]
fn a_runtime_fault_in_try_is_not_caught() {
    // `catch` handles thrown Errors, not runtime faults; the fault propagates.
    let program = checked_program(
        "pub fn f(): int\n    try\n        return 1 / 0\n    catch err: Error\n        return 2\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap_err().code,
        RUN_DIVIDE_BY_ZERO
    );
}

#[test]
fn a_throw_from_a_callee_is_caught_by_the_caller() {
    // The spec's `try { loan(...) } catch err` example: an Error thrown inside a
    // called function unwinds through the call and is caught by the caller.
    let program = checked_program(
        "fn boom()\n    throw Error(code: \"x.y\", message: \"deep\")\npub fn safe(): string\n    try\n        boom()\n    catch err: Error\n        return err.message\n    return \"none\"\n",
    );
    assert_eq!(
        run(&program, "test::safe", &[]),
        Ok(Some(Value::Str("deep".into())))
    );
}

#[test]
fn a_throw_propagates_through_intermediate_calls() {
    // a -> b -> c; c throws, a catches. The Error crosses two call boundaries.
    let program = checked_program(
        "fn c()\n    throw Error(code: \"deep.fail\", message: \"from c\")\nfn b()\n    c()\npub fn a(): string\n    try\n        b()\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_eq!(
        run(&program, "test::a", &[]),
        Ok(Some(Value::Str("deep.fail".into())))
    );
}

#[test]
fn a_callee_throw_rolls_back_the_enclosing_transaction() {
    // A transaction writes, then a called function throws. The throw escapes the
    // transaction, so it rolls back and the write never lands.
    let program = checked_program(
        "resource Account at ^accts(id: int)\n    balance: int\n\nfn fail()\n    throw Error(code: \"x\", message: \"boom\")\n\npub fn run_it()\n    transaction\n        ^accts(1).balance = 5\n        fail()\n\npub fn read(): int\n    return get(^accts(1).balance, -1)\n",
    );
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::run_it", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNCAUGHT_THROW),
        "{result:?}"
    );
    let after = run_entry(&program, &store, "test::read", &[])
        .expect("read")
        .value;
    assert_eq!(after, Some(Value::Int(-1)));
}

#[test]
fn a_caught_callee_throw_does_not_leak_into_a_later_fault() {
    // After a caller catches a callee's throw, the pending throw is cleared, so a
    // later genuine fault (divide-by-zero) is NOT mistaken for a catchable throw.
    let program = checked_program(
        "fn callee()\n    throw Error(code: \"e1\", message: \"boom\")\npub fn check(): int\n    try\n        callee()\n    catch err: Error\n        write(\"caught\")\n    try\n        return 1 / 0\n    catch boom: Error\n        return 99\n    return 0\n",
    );
    assert_eq!(
        run(&program, "test::check", &[]).unwrap_err().code,
        RUN_DIVIDE_BY_ZERO
    );
}

#[test]
fn a_throwing_finally_does_not_leak_a_pending_throw() {
    // A `finally` throwing over a call-propagated throw must not leave that throw
    // stashed: after an outer `catch` swallows the finally throw, a later fault
    // still faults rather than being caught with the stale error.
    let program = checked_program(
        "fn callee()\n    throw Error(code: \"e1\", message: \"from call\")\npub fn leak(): int\n    try\n        try\n            callee()\n        finally\n            throw Error(code: \"e2\", message: \"from finally\")\n    catch err: Error\n        write(\"swallowed\")\n    try\n        return 1 / 0\n    catch boom: Error\n        return 99\n    return 0\n",
    );
    assert_eq!(
        run(&program, "test::leak", &[]).unwrap_err().code,
        RUN_DIVIDE_BY_ZERO
    );
}

#[test]
fn a_throw_from_a_call_in_finally_propagates() {
    // A `finally` whose own called function throws: that throw replaces the
    // outcome and is caught by an outer handler.
    let program = checked_program(
        "fn boom()\n    throw Error(code: \"deep\", message: \"x\")\npub fn run_it(): string\n    try\n        try\n            write(\"body\")\n        finally\n            boom()\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_eq!(
        run(&program, "test::run_it", &[]),
        Ok(Some(Value::Str("deep".into())))
    );
}

#[test]
fn a_clean_finally_preserves_a_propagated_call_throw() {
    // A clean `finally` (no throw of its own) over a call-propagated throw must
    // restore the pending throw so an outer `catch` still sees it.
    let program = checked_program(
        "fn boom()\n    throw Error(code: \"deep\", message: \"x\")\npub fn run_it(): string\n    try\n        try\n            boom()\n        finally\n            write(\"cleanup\")\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    let outcome = run_full(&program, "test::run_it", &[]).expect("caught");
    assert_eq!(outcome.value, Some(Value::Str("deep".into())));
    assert_eq!(outcome.output, "cleanup");
}

#[test]
fn an_out_parameter_writes_back_to_a_local() {
    // The spec's parseInt shape: the callee fills an `out` parameter, and the
    // caller's local sees the written value.
    let program = checked_program(
        "fn give(out value: int)\n    value = 42\npub fn main(): int\n    var n: int = 0\n    give(out n)\n    return n\n",
    );
    assert_eq!(run(&program, "test::main", &[]), Ok(Some(Value::Int(42))));
}

#[test]
fn an_uninitialized_scalar_var_starts_at_its_zero() {
    // A typed `var` without an initializer is a writable place that starts at its
    // type's default, so plain declaration-then-use works.
    let program = checked_program("pub fn main(): int\n    var n: int\n    return n\n");
    assert_eq!(run(&program, "test::main", &[]), Ok(Some(Value::Int(0))));
}

#[test]
fn an_out_parameter_writes_back_to_an_uninitialized_var() {
    // The documented `out` pattern declares the place without a value:
    // `var n: int` then `give(out n)`.
    let program = checked_program(
        "fn give(out value: int)\n    value = 42\npub fn main(): int\n    var n: int\n    give(out n)\n    return n\n",
    );
    assert_eq!(run(&program, "test::main", &[]), Ok(Some(Value::Int(42))));
}

#[test]
fn an_out_parameter_ignores_the_caller_value_and_overwrites_it() {
    // `out` does not read the caller's value; whatever the callee assigns wins.
    let program = checked_program(
        "fn give(out value: int)\n    value = 42\npub fn main(): int\n    var n: int = 99\n    give(out n)\n    return n\n",
    );
    assert_eq!(run(&program, "test::main", &[]), Ok(Some(Value::Int(42))));
}

#[test]
fn an_inout_parameter_reads_then_writes_a_local() {
    // `inout` seeds the parameter from the caller's value, then writes back.
    let program = checked_program(
        "fn bump(inout n: int)\n    n = n + 1\npub fn main(): int\n    var n: int = 41\n    bump(inout n)\n    return n\n",
    );
    assert_eq!(run(&program, "test::main", &[]), Ok(Some(Value::Int(42))));
}

#[test]
fn an_inout_parameter_mutates_a_local_resource() {
    // The spec's `normalize(inout book)` shape: mutating a field of a local
    // resource passed `inout` is visible to the caller.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\nfn setTitle(inout book: Book)\n    book.title = \"Small Gods\"\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    setTitle(inout book)\n    return book.title\n",
    );
    assert_eq!(
        run(&program, "test::main", &[]),
        Ok(Some(Value::Str("Small Gods".into())))
    );
}

#[test]
fn an_inout_parameter_writes_back_to_a_local_resource_field() {
    // A field of a local resource, `book.title`, is an assignable place; passing it
    // `inout` reads it to seed the parameter and writes the result back.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\nfn upper(inout s: string)\n    s = \"UPPER\"\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    upper(inout book.title)\n    return book.title\n",
    );
    assert_eq!(
        run(&program, "test::main", &[]),
        Ok(Some(Value::Str("UPPER".into())))
    );
}

#[test]
fn an_out_parameter_writes_back_to_a_local_resource_field() {
    // `out` on a local resource field fills it without reading it first.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\nfn fill(out s: string)\n    s = \"FILLED\"\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    fill(out book.title)\n    return book.title\n",
    );
    assert_eq!(
        run(&program, "test::main", &[]),
        Ok(Some(Value::Str("FILLED".into())))
    );
}

#[test]
fn write_back_is_skipped_when_the_callee_throws() {
    // A callee that mutates an `inout` parameter then throws must not write back:
    // the caller's local keeps its pre-call value.
    let program = checked_program(
        "fn bad(inout n: int)\n    n = 99\n    throw Error(code: \"x\", message: \"boom\")\npub fn main(): int\n    var n: int = 1\n    try\n        bad(inout n)\n    catch err: Error\n        write(\"caught\")\n    return n\n",
    );
    assert_eq!(run(&program, "test::main", &[]), Ok(Some(Value::Int(1))));
}

#[test]
fn an_argument_mode_must_match_the_parameter_mode() {
    // Passing `out` to a plain (by-value) parameter is a type error.
    let program = checked_program(
        "fn plain(n: int): int\n    return n\npub fn main(): int\n    var n: int = 1\n    return plain(out n)\n",
    );
    assert_eq!(run(&program, "test::main", &[]).unwrap_err().code, RUN_TYPE);
}

/// A program exercising the four `std::io` file builtins.
const IO_SAMPLE: &str = "\
fn saveText(path: string, text: string)
    std::io::writeText(path, text)

fn loadText(path: string): string
    return std::io::readText(path)

fn saveBytes(path: string, data: bytes)
    std::io::writeBytes(path, data)

fn loadBytes(path: string): bytes
    return std::io::readBytes(path)

fn loadOrCode(path: string): string
    try
        return std::io::readText(path)
    catch err: Error
        return err.code
";

#[test]
fn io_round_trips_text_through_a_file() {
    let program = checked_program(IO_SAMPLE);
    let store = RefCell::new(MemStore::new());
    let host = Host::new().with_filesystem();
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("note.txt").to_string_lossy().into_owned();
    run_entry_with_host(
        &program,
        &store,
        &host,
        "test::saveText",
        &[Value::Str(path.clone()), Value::Str("hello".into())],
    )
    .expect("write");
    let loaded = run_entry_with_host(
        &program,
        &store,
        &host,
        "test::loadText",
        &[Value::Str(path)],
    )
    .expect("read")
    .value;
    assert_eq!(loaded, Some(Value::Str("hello".into())));
}

#[test]
fn io_round_trips_bytes_through_a_file() {
    let program = checked_program(IO_SAMPLE);
    let store = RefCell::new(MemStore::new());
    let host = Host::new().with_filesystem();
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("blob.bin").to_string_lossy().into_owned();
    let data = Value::Bytes(vec![0, 1, 2, 255, 128]);
    run_entry_with_host(
        &program,
        &store,
        &host,
        "test::saveBytes",
        &[Value::Str(path.clone()), data.clone()],
    )
    .expect("write");
    let loaded = run_entry_with_host(
        &program,
        &store,
        &host,
        "test::loadBytes",
        &[Value::Str(path)],
    )
    .expect("read")
    .value;
    assert_eq!(loaded, Some(data));
}

#[test]
fn io_without_a_filesystem_capability_is_a_capability_error() {
    let program = checked_program(IO_SAMPLE);
    let store = RefCell::new(MemStore::new());
    // Plain `run_entry` provides no host capabilities.
    let result = run_entry(
        &program,
        &store,
        "test::loadText",
        &[Value::Str("x".into())],
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

#[test]
fn an_io_error_raises_a_catchable_error() {
    // Reading a missing file (with the capability present) raises a typed Error
    // the program can `catch`, not a runtime fault.
    let program = checked_program(IO_SAMPLE);
    let store = RefCell::new(MemStore::new());
    let host = Host::new().with_filesystem();
    let dir = tempfile::tempdir().expect("temp dir");
    let missing = dir.path().join("absent.txt").to_string_lossy().into_owned();
    let code = run_entry_with_host(
        &program,
        &store,
        &host,
        "test::loadOrCode",
        &[Value::Str(missing)],
    )
    .expect("caught")
    .value;
    assert_eq!(code, Some(Value::Str("io.read".into())));
}

/// A resource and helpers exercising `out`/`inout` write-back to saved places.
const SAVED_MODE_SAMPLE: &str = "\
resource Account at ^accts(id: int)
    balance: int

resource Book at ^books(id: int)
    title: string

fn addOne(inout n: int)
    n = n + 1

fn give(out n: int)
    n = 7

fn setTitle(inout book: Book)
    book.title = \"renamed\"

fn bad(inout n: int)
    n = 99
    throw Error(code: \"x\", message: \"boom\")

pub fn seedAccount()
    ^accts(1).balance = 41

pub fn bump()
    addOne(inout ^accts(1).balance)

pub fn produce()
    give(out ^accts(1).balance)

pub fn balanceOf(): int
    return ^accts(1).balance

pub fn seedBook()
    ^books(1).title = \"draft\"

pub fn rename()
    setTitle(inout ^books(1))

pub fn titleOf(): string
    return ^books(1).title

pub fn tryBump()
    try
        bad(inout ^accts(1).balance)
    catch err: Error
        write(\"caught\")
";

#[test]
fn inout_writes_back_to_a_saved_field() {
    let program = checked_program(SAVED_MODE_SAMPLE);
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seedAccount", &[]).expect("seed");
    run_entry(&program, &store, "test::bump", &[]).expect("bump");
    let balance = run_entry(&program, &store, "test::balanceOf", &[])
        .expect("read")
        .value;
    assert_eq!(balance, Some(Value::Int(42)));
}

#[test]
fn out_creates_a_saved_field() {
    let program = checked_program(SAVED_MODE_SAMPLE);
    let store = RefCell::new(MemStore::new());
    // `out` never reads the place, so the field need not exist beforehand.
    run_entry(&program, &store, "test::produce", &[]).expect("produce");
    let balance = run_entry(&program, &store, "test::balanceOf", &[])
        .expect("read")
        .value;
    assert_eq!(balance, Some(Value::Int(7)));
}

#[test]
fn inout_writes_back_to_a_whole_saved_resource() {
    // The spec's `normalize(inout ^books(id))` shape.
    let program = checked_program(SAVED_MODE_SAMPLE);
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seedBook", &[]).expect("seed");
    run_entry(&program, &store, "test::rename", &[]).expect("rename");
    let title = run_entry(&program, &store, "test::titleOf", &[])
        .expect("read")
        .value;
    assert_eq!(title, Some(Value::Str("renamed".into())));
}

/// A resource with a `versions(version)` group layer, for `out`/`inout` into a
/// field inside a keyed group entry (a `SavedNestedField` place).
const GROUP_FIELD_MODE_SAMPLE: &str = "\
resource Book at ^books(id: int)
    title: string
    versions(version: int)
        title: string

fn addBang(inout t: string)
    t = t _ \"!\"

fn makeTitle(out t: string)
    t = \"made\"

pub fn seed()
    ^books(1).versions(2).title = \"v\"

pub fn bump()
    addBang(inout ^books(1).versions(2).title)

pub fn produce()
    makeTitle(out ^books(1).versions(3).title)

pub fn versionTitle(): string
    return ^books(1).versions(2).title

pub fn producedTitle(): string
    return ^books(1).versions(3).title
";

#[test]
fn inout_writes_back_to_a_group_entry_field() {
    // `inout ^books(id).versions(v).title` — a field inside a keyed group entry as
    // an inout target: read the current value, mutate, write back.
    let program = checked_program(GROUP_FIELD_MODE_SAMPLE);
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed");
    run_entry(&program, &store, "test::bump", &[]).expect("bump");
    let title = run_entry(&program, &store, "test::versionTitle", &[])
        .expect("read")
        .value;
    assert_eq!(title, Some(Value::Str("v!".into())));
}

#[test]
fn out_creates_a_group_entry_field() {
    // `out` never reads the place, so the group-entry field need not exist first.
    let program = checked_program(GROUP_FIELD_MODE_SAMPLE);
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::produce", &[]).expect("produce");
    let title = run_entry(&program, &store, "test::producedTitle", &[])
        .expect("read")
        .value;
    assert_eq!(title, Some(Value::Str("made".into())));
}

#[test]
fn a_saved_write_back_is_skipped_when_the_callee_throws() {
    let program = checked_program(SAVED_MODE_SAMPLE);
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seedAccount", &[]).expect("seed");
    // The callee mutates the inout saved field then throws; the throw is caught,
    // and the write-back is skipped, so the stored balance is unchanged.
    run_entry(&program, &store, "test::tryBump", &[]).expect("caught");
    let balance = run_entry(&program, &store, "test::balanceOf", &[])
        .expect("read")
        .value;
    assert_eq!(balance, Some(Value::Int(41)));
}

#[test]
fn finally_runs_after_a_fault_and_can_replace_it() {
    // The try body faults (not catchable); finally still runs and its throw
    // replaces the fault, proving finally ran.
    let program = checked_program(
        "pub fn f(): int\n    try\n        return 1 / 0\n    finally\n        throw Error(code: \"cleanup.failed\", message: \"x\")\n",
    );
    let error = run(&program, "test::f", &[]).unwrap_err();
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    assert!(
        error.message.contains("cleanup.failed"),
        "{}",
        error.message
    );
}

#[test]
fn an_uncaught_throw_without_a_catch_propagates_through_finally() {
    let program = checked_program(
        "pub fn f()\n    try\n        throw Error(code: \"x.y\", message: \"boom\")\n    finally\n        write(\"cleanup\")\n",
    );
    assert_eq!(
        run(&program, "test::f", &[]).unwrap_err().code,
        RUN_UNCAUGHT_THROW
    );
}

#[test]
fn a_throw_in_finally_replaces_the_outcome() {
    let program = checked_program(
        "pub fn f(): int\n    try\n        return 1\n    finally\n        throw Error(code: \"from.finally\", message: \"x\")\n",
    );
    let error = run(&program, "test::f", &[]).unwrap_err();
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    assert!(error.message.contains("from.finally"), "{}", error.message);
}

#[test]
fn a_clean_finally_preserves_a_return() {
    // A finally that completes normally lets the try's `return` through.
    let program = checked_program(
        "pub fn f(): int\n    try\n        return 7\n    finally\n        write(\"cleanup\")\n",
    );
    assert_eq!(run(&program, "test::f", &[]), Ok(Some(Value::Int(7))));
}

#[test]
fn a_throw_caught_inside_a_transaction_commits() {
    // The throw is handled within the transaction, so the body completes normally
    // and the catch's write commits.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn safe(id: int)\n    transaction\n        try\n            throw Error(code: \"x.y\", message: \"b\")\n        catch err: Error\n            ^books(id).title = \"recovered\"\n\n\
         pub fn title(id: int): string\n    return ^books(id).title\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::safe", &[Value::Int(1)]).expect("safe runs");
    assert_eq!(
        run_entry(&program, &store, "test::title", &[Value::Int(1)])
            .unwrap()
            .value,
        Some(Value::Str("recovered".into()))
    );
}

#[test]
fn throw_inside_a_transaction_rolls_back() {
    // An escaping throw rolls the transaction back, like any other escape.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn risky(id: int)\n    transaction\n        ^books(id).title = \"staged\"\n        throw Error(code: \"x.y\", message: \"boom\")\n\n\
         pub fn has_book(id: int): bool\n    return exists(^books(id))\n",
    );
    let store = RefCell::new(MemStore::new());
    assert_eq!(
        run_entry(&program, &store, "test::risky", &[Value::Int(1)])
            .unwrap_err()
            .code,
        RUN_UNCAUGHT_THROW
    );
    assert_eq!(
        run_entry(&program, &store, "test::has_book", &[Value::Int(1)])
            .unwrap()
            .value,
        Some(Value::Bool(false))
    );
}

#[test]
fn evaluates_locals_and_reassignment() {
    let f =
        function("fn f(n: int): int\n    var total = n\n    total = total + 1\n    return total\n");
    assert_eq!(
        evaluate_function(&f, &[Value::Int(41)]),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn evaluates_boolean_logic() {
    let f = function("fn f(a: bool, b: bool): bool\n    return a and not b\n");
    assert_eq!(
        evaluate_function(&f, &[Value::Bool(true), Value::Bool(false)]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        evaluate_function(&f, &[Value::Bool(true), Value::Bool(true)]),
        Ok(Some(Value::Bool(false)))
    );
}

#[test]
fn equality_compares_values() {
    // Marrow spells equality `=` (and inequality `!=`); assignment `=` is a
    // statement, so this `=` in expression position is the equality operator.
    let f = function("fn f(a: int, b: int): bool\n    return a = b\n");
    assert_eq!(
        evaluate_function(&f, &[Value::Int(5), Value::Int(5)]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        evaluate_function(&f, &[Value::Int(5), Value::Int(6)]),
        Ok(Some(Value::Bool(false)))
    );
}

#[test]
fn a_function_that_returns_nothing_yields_none() {
    // Falls off the end with no `return`.
    let f = function("fn f(a: int)\n    const x = a + 1\n");
    assert_eq!(evaluate_function(&f, &[Value::Int(1)]), Ok(None));
}

#[test]
fn rejects_division_by_zero() {
    let f = function("fn f(a: int): int\n    return a / 0\n");
    let result = evaluate_function(&f, &[Value::Int(10)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_DIVIDE_BY_ZERO),
        "{result:?}"
    );
}

#[test]
fn integer_remainder_by_zero_reports_one_consistent_message() {
    // The `%` operator and `std::math::remainder`/`modulo` are the same integer
    // remainder, so a zero divisor must report the same divide-by-zero message.
    let f = function("fn f(a: int): int\n    return a % 0\n");
    let result = evaluate_function(&f, &[Value::Int(10)]);
    let Err(error) = result else {
        panic!("expected an error, got {result:?}");
    };
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.message, "integer remainder by zero");

    // std::math::modulo routes through the same integer-remainder path.
    let program = checked_program("pub fn g(): int\n    return std::math::modulo(7, 0)\n");
    assert_eq!(
        run(&program, "test::g", &[]).unwrap_err().message,
        "integer remainder by zero"
    );
}

#[test]
fn detects_integer_overflow() {
    let f = function("fn f(a: int): int\n    return a * a\n");
    let result = evaluate_function(&f, &[Value::Int(i64::MAX)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_OVERFLOW),
        "{result:?}"
    );
}

#[test]
fn detects_an_over_range_integer_literal() {
    // A literal beyond i64::MAX is a runtime overflow, not an arithmetic one.
    let f = function("fn f(): int\n    return 99999999999999999999999999\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_OVERFLOW),
        "{result:?}"
    );
}

#[test]
fn detects_an_over_envelope_decimal_literal() {
    // A decimal literal with more than 34 significant digits is outside the
    // decimal envelope and overflows at runtime.
    let f = function("fn f(): decimal\n    return 9.9999999999999999999999999999999999\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_OVERFLOW),
        "{result:?}"
    );
}

#[test]
fn rejects_an_unbound_name() {
    let f = function("fn f(): int\n    return x\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNBOUND_NAME),
        "{result:?}"
    );
}

#[test]
fn rejects_assignment_to_an_immutable_binding() {
    let f = function("fn f(): int\n    const x = 1\n    x = 2\n    return x\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn a_local_const_binds_a_runtime_computed_value() {
    // `const` is the immutable local binding. Unlike a module constant, its
    // initializer may be any expression — here a call resolved at run time.
    let program = checked_program(
        "fn double(n: int): int\n    return n * 2\nfn f(): int\n    const x = double(5)\n    return x\n",
    );
    assert_eq!(run(&program, "test::f", &[]), Ok(Some(Value::Int(10))));
}

#[test]
fn rejects_an_argument_count_mismatch() {
    let add = function("fn add(a: int, b: int): int\n    return a + b\n");
    let result = evaluate_function(&add, &[Value::Int(1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn reports_an_unsupported_construct() {
    // A range is iterable in a `for` loop but is not a standalone value.
    let f = function("fn f(): int\n    return 1..3\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNSUPPORTED),
        "{result:?}"
    );
}

#[test]
fn an_if_condition_must_be_boolean() {
    let f = function("fn f(a: int): int\n    if a\n        return 1\n    return 0\n");
    let result = evaluate_function(&f, &[Value::Int(5)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn an_inner_scope_shadows_then_restores_an_outer_binding() {
    // `const x = 1` inside the if-block shadows only within that block; after it,
    // the outer `x` (99) is what `return x` sees.
    let f =
        function("fn f(): int\n    const x = 99\n    if true\n        const x = 1\n    return x\n");
    assert_eq!(evaluate_function(&f, &[]), Ok(Some(Value::Int(99))));
}

#[test]
fn an_else_if_chain_selects_the_matching_branch() {
    let grade = function(
        "fn grade(n: int): int\n    if n > 90\n        return 1\n    else if n > 80\n        return 2\n    else\n        return 3\n",
    );
    assert_eq!(
        evaluate_function(&grade, &[Value::Int(95)]),
        Ok(Some(Value::Int(1)))
    );
    assert_eq!(
        evaluate_function(&grade, &[Value::Int(85)]),
        Ok(Some(Value::Int(2)))
    );
    assert_eq!(
        evaluate_function(&grade, &[Value::Int(50)]),
        Ok(Some(Value::Int(3)))
    );
}

#[test]
fn detects_min_over_negative_one_overflow() {
    // `i64::MIN % -1` overflows. (`/` now yields a decimal, so `%` is the only
    // integer-division-family operator that can overflow this way.)
    let f = function("fn f(a: int, b: int): int\n    return a % b\n");
    let result = evaluate_function(&f, &[Value::Int(i64::MIN), Value::Int(-1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_OVERFLOW),
        "{result:?}"
    );
}

#[test]
fn evaluates_a_while_loop() {
    let sum = function(
        "fn sum(n: int): int\n    var total = 0\n    var i = 1\n    while i <= n\n        total = total + i\n        i = i + 1\n    return total\n",
    );
    assert_eq!(
        evaluate_function(&sum, &[Value::Int(5)]),
        Ok(Some(Value::Int(15)))
    );
}

#[test]
fn evaluates_an_inclusive_for_range() {
    let sum = function(
        "fn sum(n: int): int\n    var total = 0\n    for i in 1..=n\n        total = total + i\n    return total\n",
    );
    assert_eq!(
        evaluate_function(&sum, &[Value::Int(5)]),
        Ok(Some(Value::Int(15)))
    );
}

#[test]
fn an_exclusive_for_range_stops_before_the_end() {
    let count = function(
        "fn count(n: int): int\n    var c = 0\n    for i in 0..n\n        c = c + 1\n    return c\n",
    );
    assert_eq!(
        evaluate_function(&count, &[Value::Int(5)]),
        Ok(Some(Value::Int(5)))
    );
}

#[test]
fn break_exits_the_loop() {
    let f = function(
        "fn f(n: int): int\n    var i = 0\n    while true\n        if i > n\n            break\n        i = i + 1\n    return i\n",
    );
    assert_eq!(
        evaluate_function(&f, &[Value::Int(3)]),
        Ok(Some(Value::Int(4)))
    );
}

#[test]
fn continue_skips_to_the_next_iteration() {
    let f = function(
        "fn f(n: int): int\n    var c = 0\n    for i in 1..=n\n        if i = 1\n            continue\n        c = c + 1\n    return c\n",
    );
    // The first iteration is skipped; the rest count.
    assert_eq!(
        evaluate_function(&f, &[Value::Int(3)]),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn a_labeled_break_exits_the_outer_loop() {
    let f = function(
        "fn f(): int\n    var count = 0\n    outer: for i in 1..=3\n        for j in 1..=3\n            if j = 2\n                break outer\n            count = count + 1\n    return count\n",
    );
    // i=1: j=1 counts (1), j=2 breaks the outer loop entirely.
    assert_eq!(evaluate_function(&f, &[]), Ok(Some(Value::Int(1))));
}

#[test]
fn an_unlabeled_break_exits_only_the_inner_loop() {
    let f = function(
        "fn f(): int\n    var count = 0\n    for i in 1..=2\n        for j in 1..=3\n            if j = 2\n                break\n            count = count + 1\n    return count\n",
    );
    // Each outer iteration counts j=1 then breaks the inner loop: 2 total.
    assert_eq!(evaluate_function(&f, &[]), Ok(Some(Value::Int(2))));
}

#[test]
fn break_outside_a_loop_is_an_error() {
    let f = function("fn f()\n    break\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_NO_ENCLOSING_LOOP),
        "{result:?}"
    );
}

#[test]
fn returns_a_string_literal() {
    let f = function("fn f(): string\n    return \"hello\"\n");
    assert_eq!(
        evaluate_function(&f, &[]),
        Ok(Some(Value::Str("hello".into())))
    );
}

#[test]
fn concatenates_strings() {
    // Marrow spells string concatenation `_`.
    let greet = function("fn greet(name: string): string\n    return \"Hello, \" _ name\n");
    assert_eq!(
        evaluate_function(&greet, &[Value::Str("World".into())]),
        Ok(Some(Value::Str("Hello, World".into())))
    );
}

#[test]
fn compares_strings_for_equality_and_order() {
    let eq = function("fn eq(a: string, b: string): bool\n    return a = b\n");
    assert_eq!(
        evaluate_function(&eq, &[Value::Str("x".into()), Value::Str("x".into())]),
        Ok(Some(Value::Bool(true)))
    );
    let lt = function("fn lt(a: string, b: string): bool\n    return a < b\n");
    assert_eq!(
        evaluate_function(
            &lt,
            &[Value::Str("apple".into()), Value::Str("banana".into())]
        ),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn string_escapes_are_not_yet_decoded() {
    // The source string contains a backslash escape, which the runtime does not
    // yet decode.
    let f = function("fn f(): string\n    return \"a\\nb\"\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNSUPPORTED),
        "{result:?}"
    );
}

#[test]
fn concatenation_requires_strings() {
    let f = function("fn f(): string\n    return \"x\" _ 5\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn evaluates_string_interpolation() {
    let f = function("fn f(n: int): string\n    return $\"n is {n}\"\n");
    assert_eq!(
        evaluate_function(&f, &[Value::Int(5)]),
        Ok(Some(Value::Str("n is 5".into())))
    );
}

#[test]
fn interpolation_renders_several_values() {
    let f = function("fn f(name: string, ok: bool): string\n    return $\"{name}={ok}\"\n");
    assert_eq!(
        evaluate_function(&f, &[Value::Str("ready".into()), Value::Bool(true)]),
        Ok(Some(Value::Str("ready=true".into())))
    );
}

#[test]
fn interpolation_unescapes_literal_braces() {
    let f = function("fn f(): string\n    return $\"a {{ b\"\n");
    assert_eq!(
        evaluate_function(&f, &[]),
        Ok(Some(Value::Str("a { b".into())))
    );
}

#[test]
fn run_entry_evaluates_a_function_by_qualified_name() {
    let program = checked_program("fn add(a: int, b: int): int\n    return a + b\n");
    assert_eq!(
        run(&program, "test::add", &[Value::Int(2), Value::Int(3)]),
        Ok(Some(Value::Int(5)))
    );
}

#[test]
fn a_function_can_call_another() {
    let program = checked_program(
        "fn double(n: int): int\n    return n + n\n\nfn quad(n: int): int\n    return double(n) + double(n)\n",
    );
    assert_eq!(
        run(&program, "test::quad", &[Value::Int(3)]),
        Ok(Some(Value::Int(12)))
    );
}

#[test]
fn functions_recurse() {
    let program = checked_program(
        "fn fact(n: int): int\n    if n <= 1\n        return 1\n    return n * fact(n - 1)\n",
    );
    assert_eq!(
        run(&program, "test::fact", &[Value::Int(5)]),
        Ok(Some(Value::Int(120)))
    );
}

#[test]
fn a_void_call_runs_as_a_statement() {
    let program = checked_program(
        "fn note(n: int)\n    const doubled = n + n\n\nfn caller(): int\n    note(3)\n    return 2\n",
    );
    assert_eq!(run(&program, "test::caller", &[]), Ok(Some(Value::Int(2))));
}

#[test]
fn using_a_void_call_as_a_value_is_rejected() {
    let program = checked_program(
        "fn note(n: int)\n    const doubled = n + n\n\nfn caller(): int\n    return note(3)\n",
    );
    let result = run(&program, "test::caller", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_NO_VALUE),
        "{result:?}"
    );
}

#[test]
fn an_unknown_function_is_rejected() {
    let program = checked_program("fn f(): int\n    return 1\n");
    // Unknown entry point...
    assert!(matches!(
        run(&program, "test::missing", &[]),
        Err(ref error) if error.code == RUN_UNKNOWN_FUNCTION
    ));
    // ...and an unknown function called from within a body.
    let calls_missing = checked_program("fn f(): int\n    return g(1)\n");
    let result = run(&calls_missing, "test::f", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNKNOWN_FUNCTION),
        "{result:?}"
    );
}

#[test]
fn values_and_entries_over_an_index_branch_are_unsupported() {
    // builtins.md: on declared index branches use `keys(...)` or direct iteration;
    // `values`/`entries` are for primary roots and ordinary keyed layers. Over an
    // index branch they report `run.unsupported`, never a missing user function.
    let resource = "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\n";
    for builtin in ["values", "entries"] {
        let program = checked_program(&format!(
            "{resource}fn f()\n    {builtin}(^books.byShelf(\"x\"))\n"
        ));
        let result = run(&program, "test::f", &[]);
        assert!(
            matches!(result, Err(ref error) if error.code == RUN_UNSUPPORTED),
            "{builtin}: {result:?}"
        );
    }
}

#[test]
fn print_writes_a_line_to_output() {
    let program = checked_program("fn main()\n    print($\"hello {1}\")\n");
    let outcome = run_full(&program, "test::main", &[]).expect("run");
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "hello 1\n");
}

#[test]
fn write_does_not_add_a_newline() {
    let program = checked_program("fn main()\n    write(\"a\")\n    write(\"b\")\n");
    let outcome = run_full(&program, "test::main", &[]).expect("run");
    assert_eq!(outcome.output, "ab");
}

#[test]
fn output_accumulates_across_calls() {
    let program = checked_program(
        "fn greet(name: string)\n    print($\"hi {name}\")\n\nfn main()\n    greet(\"a\")\n    greet(\"b\")\n",
    );
    let outcome = run_full(&program, "test::main", &[]).expect("run");
    assert_eq!(outcome.output, "hi a\nhi b\n");
}

#[test]
fn print_takes_one_argument() {
    let program = checked_program("fn main()\n    print()\n");
    let result = run_full(&program, "test::main", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

/// A program with a saved `Book` resource and functions that read a title.
const BOOK_READER: &str = "\
resource Book at ^books(id: int)
    required title: string

fn title_of(id: int): string
    return ^books(id).title

fn show(id: int)
    print($\"title: {^books(id).title}\")
";

/// A store holding `^books(id).title = title`.
fn store_with_title(id: i64, title: &str) -> MemStore {
    let mut store = MemStore::new();
    store.write(
        &encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(id)),
            PathSegment::Field("title".into()),
        ]),
        encode_value(&SavedValue::Str(title.into())).expect("in-range value encodes"),
    );
    store
}

#[test]
fn reads_a_scalar_field_from_saved_data() {
    let program = checked_program(BOOK_READER);
    let store = RefCell::new(store_with_title(1, "Mort"));
    let outcome = run_entry(&program, &store, "test::title_of", &[Value::Int(1)]).expect("run");
    assert_eq!(outcome.value, Some(Value::Str("Mort".into())));
}

#[test]
fn reading_an_absent_field_is_an_error() {
    let program = checked_program(BOOK_READER);
    let store = RefCell::new(MemStore::new()); // empty: the title is absent
    let result = run_entry(&program, &store, "test::title_of", &[Value::Int(1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_ABSENT),
        "{result:?}"
    );
}

#[test]
fn a_saved_read_interpolates_and_prints() {
    let program = checked_program(BOOK_READER);
    let store = RefCell::new(store_with_title(7, "Mort"));
    let outcome = run_entry(&program, &store, "test::show", &[Value::Int(7)]).expect("run");
    assert_eq!(outcome.output, "title: Mort\n");
}

/// A program that writes and reads a `Book` title.
const BOOK_WRITER: &str = "\
resource Book at ^books(id: int)
    required title: string

fn set_title(id: int, t: string)
    ^books(id).title = t

fn title_of(id: int): string
    return ^books(id).title
";

#[test]
fn a_field_write_updates_saved_data() {
    let program = checked_program(BOOK_WRITER);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::set_title",
        &[Value::Int(1), Value::Str("Mort".into())],
    )
    .expect("write");
    // Read it back through the runtime against the same store.
    let outcome = run_entry(&program, &store, "test::title_of", &[Value::Int(1)]).expect("read");
    assert_eq!(outcome.value, Some(Value::Str("Mort".into())));
}

#[test]
fn a_mistyped_field_write_is_rejected() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn bad(id: int)\n    ^books(id).title = 5\n",
    );
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::bad", &[Value::Int(1)]);
    // The managed-write layer rejects an int written to a string field.
    assert!(
        matches!(result, Err(ref error) if error.code == "write.type_mismatch"),
        "{result:?}"
    );
}

/// A program that queries saved `Book` data with `exists` and `get`.
const BOOK_QUERY: &str = "\
resource Book at ^books(id: int)
    required title: string
    subtitle: string

fn has_book(id: int): bool
    return exists(^books(id))

fn has_title(id: int): bool
    return exists(^books(id).title)

fn subtitle_or(id: int, fallback: string): string
    return get(^books(id).subtitle, fallback)
";

#[test]
fn exists_reports_record_and_field_presence() {
    let program = checked_program(BOOK_QUERY);
    let store = RefCell::new(store_with_title(1, "Mort"));
    let value = |entry, id| {
        run_entry(&program, &store, entry, &[Value::Int(id)])
            .expect("run")
            .value
    };
    // Record 1 exists (it has the title child); record 2 does not.
    assert_eq!(value("test::has_book", 1), Some(Value::Bool(true)));
    assert_eq!(value("test::has_book", 2), Some(Value::Bool(false)));
    // Its title field is present; its sparse subtitle is not.
    assert_eq!(value("test::has_title", 1), Some(Value::Bool(true)));
}

#[test]
fn get_returns_the_default_for_an_absent_field() {
    let program = checked_program(BOOK_QUERY);
    let store = RefCell::new(store_with_title(1, "Mort")); // subtitle is absent
    let value = run_entry(
        &program,
        &store,
        "test::subtitle_or",
        &[Value::Int(1), Value::Str("(none)".into())],
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(none)".into())));
}

#[test]
fn get_returns_the_value_when_present() {
    let program = checked_program(BOOK_QUERY);
    let store = RefCell::new(store_with_title(1, "Mort"));
    // Populate the sparse subtitle directly.
    store.borrow_mut().write(
        &encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
            PathSegment::Field("subtitle".into()),
        ]),
        encode_value(&SavedValue::Str("A Discworld Novel".into())).expect("in-range value encodes"),
    );
    let value = run_entry(
        &program,
        &store,
        "test::subtitle_or",
        &[Value::Int(1), Value::Str("(none)".into())],
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("A Discworld Novel".into())));
}

#[test]
fn next_id_allocates_past_the_highest_record() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn fresh(): int\n    return nextId(^books)\n",
    );
    let store = RefCell::new(MemStore::new());
    // Empty root: the next id is 1.
    assert_eq!(
        run_entry(&program, &store, "test::fresh", &[])
            .expect("run")
            .value,
        Some(Value::Int(1))
    );
    // Seed records 1 and 4; the next id is one past the highest.
    for id in [1, 4] {
        store.borrow_mut().write(
            &encode_path(&[
                PathSegment::Root("books".into()),
                PathSegment::RecordKey(SavedKey::Int(id)),
                PathSegment::Field("title".into()),
            ]),
            encode_value(&SavedValue::Str("t".into())).expect("in-range value encodes"),
        );
    }
    assert_eq!(
        run_entry(&program, &store, "test::fresh", &[])
            .expect("run")
            .value,
        Some(Value::Int(5))
    );
}

#[test]
fn next_id_skips_ahead_after_restore() {
    // After a restore the store may hold records far above any contiguous run.
    // `nextId` chooses one past the highest existing key, never reusing a gap
    // (builtins.md:185-191).
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn fresh(): int\n    return nextId(^books)\n",
    );
    let store = RefCell::new(MemStore::new());
    store.borrow_mut().write(
        &encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(900)),
            PathSegment::Field("title".into()),
        ]),
        encode_value(&SavedValue::Str("t".into())).expect("in-range value encodes"),
    );
    assert_eq!(
        run_entry(&program, &store, "test::fresh", &[])
            .expect("run")
            .value,
        Some(Value::Int(901))
    );
}

/// `nextId` over a composite-identity root faults with `write.next_id_unsupported`
/// rather than inventing a bogus `Int(1)`: composite identities have no default
/// allocation policy (builtins.md:180-183).
#[test]
fn next_id_over_a_composite_root_faults() {
    let program = checked_program(
        "resource Enrollment at ^enrollments(studentId: int, courseId: int)\n    required grade: string\n\nfn fresh(): int\n    return nextId(^enrollments)\n",
    );
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::fresh", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == "write.next_id_unsupported"),
        "{result:?}"
    );
}

/// `nextId` over a keyless singleton root faults: a singleton has no generated
/// identity to allocate (types.md:262-263).
#[test]
fn next_id_over_a_singleton_root_faults() {
    let program = checked_program(
        "resource Settings at ^settings\n    required theme: string\n\nfn fresh(): int\n    return nextId(^settings)\n",
    );
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::fresh", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == "write.next_id_unsupported"),
        "{result:?}"
    );
}

/// `nextId` over a single non-integer (string) identity key faults: only an
/// `int` identity has the default policy (builtins.md:180-183).
#[test]
fn next_id_over_a_string_keyed_root_faults() {
    let program = checked_program(
        "resource Tag at ^tags(slug: string)\n    required name: string\n\nfn fresh(): int\n    return nextId(^tags)\n",
    );
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::fresh", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == "write.next_id_unsupported"),
        "{result:?}"
    );
}

/// `nextId` of a saved root no resource declares is a `run.unsupported`: there is
/// no schema to decide an allocation policy (mirrors `eval_append`'s unknown-root
/// path).
#[test]
fn next_id_over_an_undeclared_root_is_unsupported() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn fresh(): int\n    return nextId(^bogus)\n",
    );
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::fresh", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNSUPPORTED),
        "{result:?}"
    );
}

#[test]
fn delete_removes_a_record() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn set_title(id: int, t: string)\n    ^books(id).title = t\n\nfn remove(id: int)\n    delete ^books(id)\n\nfn has_book(id: int): bool\n    return exists(^books(id))\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::set_title",
        &[Value::Int(1), Value::Str("Mort".into())],
    )
    .expect("write");
    assert_eq!(
        run_entry(&program, &store, "test::has_book", &[Value::Int(1)])
            .expect("run")
            .value,
        Some(Value::Bool(true))
    );
    run_entry(&program, &store, "test::remove", &[Value::Int(1)]).expect("delete");
    assert_eq!(
        run_entry(&program, &store, "test::has_book", &[Value::Int(1)])
            .expect("run")
            .value,
        Some(Value::Bool(false)),
        "the record is gone after delete"
    );
}

#[test]
fn delete_removes_a_sparse_field_and_leaves_a_sibling() {
    // `delete ^books(id).subtitle` removes that field; a sibling field survives.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    subtitle: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).subtitle = \"A Discworld Novel\"\n\nfn drop_subtitle(id: int)\n    delete ^books(id).subtitle\n\nfn has_subtitle(id: int): bool\n    return exists(^books(id).subtitle)\n\nfn title_of(id: int): string\n    return ^books(id).title\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[Value::Int(1)]).expect("seed");
    assert_eq!(
        run_entry(&program, &store, "test::has_subtitle", &[Value::Int(1)])
            .expect("run")
            .value,
        Some(Value::Bool(true))
    );
    run_entry(&program, &store, "test::drop_subtitle", &[Value::Int(1)]).expect("delete");
    assert_eq!(
        run_entry(&program, &store, "test::has_subtitle", &[Value::Int(1)])
            .expect("run")
            .value,
        Some(Value::Bool(false)),
        "the field is gone after delete"
    );
    assert_eq!(
        run_entry(&program, &store, "test::title_of", &[Value::Int(1)])
            .expect("run")
            .value,
        Some(Value::Str("Mort".into())),
        "the sibling field survives"
    );
}

#[test]
fn deleting_an_indexed_field_removes_its_index_entry() {
    // `delete ^books(id).shelf` where `shelf` feeds `byShelf` tears down the entry,
    // so a later `keys(^books.byShelf(...))` no longer yields the record.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\nfn add(id: int, t: string, s: string)\n    ^books(id).title = t\n    ^books(id).shelf = s\n\nfn drop_shelf(id: int)\n    delete ^books(id).shelf\n\nfn count_on(shelf: string): int\n    var c = 0\n    for id in keys(^books.byShelf(shelf))\n        c = c + 1\n    return c\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::add",
        &[
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("fiction".into()),
        ],
    )
    .expect("add");
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::count_on",
            &[Value::Str("fiction".into())]
        )
        .expect("run")
        .value,
        Some(Value::Int(1))
    );
    run_entry(&program, &store, "test::drop_shelf", &[Value::Int(1)]).expect("delete");
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::count_on",
            &[Value::Str("fiction".into())]
        )
        .expect("run")
        .value,
        Some(Value::Int(0)),
        "the index entry the deleted field fed is gone"
    );
}

#[test]
fn deleting_a_required_field_is_rejected() {
    // A required field can only go away when its entry/resource is deleted.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn drop_title(id: int)\n    delete ^books(id).title\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[Value::Int(1)]).expect("seed");
    let result = run_entry(&program, &store, "test::drop_title", &[Value::Int(1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == "write.required_field"),
        "{result:?}"
    );
}

#[test]
fn deleting_a_layer_entry_leaves_other_entries() {
    // `delete ^books(id).versions(v)` removes one group-entry subtree; siblings
    // survive. Read each entry's `.title` to prove it: the deleted entry's title
    // falls back to the `get` default, the survivor's stays intact.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n    versions(version: int)\n        required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).versions(1).title = \"first\"\n    ^books(id).versions(2).title = \"second\"\n\nfn drop_version(id: int, v: int)\n    delete ^books(id).versions(v)\n\nfn version_title(id: int, v: int): string\n    return get(^books(id).versions(v).title, \"<gone>\")\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[Value::Int(1)]).expect("seed");
    run_entry(
        &program,
        &store,
        "test::drop_version",
        &[Value::Int(1), Value::Int(1)],
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::version_title",
            &[Value::Int(1), Value::Int(1)]
        )
        .expect("run")
        .value,
        Some(Value::Str("<gone>".into())),
        "the deleted version's subtree is gone"
    );
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::version_title",
            &[Value::Int(1), Value::Int(2)]
        )
        .expect("run")
        .value,
        Some(Value::Str("second".into())),
        "the other version survives"
    );
}

#[test]
fn deleting_a_keyed_leaf_entry_leaves_other_entries() {
    // `delete ^books(id).tags(pos)` removes one keyed-leaf entry; siblings survive.
    // `count(^books(id).tags)` counts the remaining entries; reading the deleted
    // one is an absent-element error while the survivor reads back.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).tags(1) = \"fiction\"\n    ^books(id).tags(2) = \"funny\"\n\nfn drop_tag(id: int, pos: int)\n    delete ^books(id).tags(pos)\n\nfn tag_count(id: int): int\n    return count(^books(id).tags)\n\nfn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos)\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[Value::Int(1)]).expect("seed");
    run_entry(
        &program,
        &store,
        "test::drop_tag",
        &[Value::Int(1), Value::Int(1)],
    )
    .expect("delete");
    assert_eq!(
        run_entry(&program, &store, "test::tag_count", &[Value::Int(1)])
            .expect("run")
            .value,
        Some(Value::Int(1)),
        "one tag remains after deleting one of two"
    );
    let deleted = run_entry(
        &program,
        &store,
        "test::tag_at",
        &[Value::Int(1), Value::Int(1)],
    );
    assert!(
        matches!(deleted, Err(ref error) if error.code == RUN_ABSENT),
        "reading the deleted tag is an absent-element error: {deleted:?}"
    );
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::tag_at",
            &[Value::Int(1), Value::Int(2)]
        )
        .expect("run")
        .value,
        Some(Value::Str("funny".into())),
        "the other tag survives"
    );
}

#[test]
fn a_transaction_commits_on_normal_exit() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn save(id: int)\n    transaction\n        ^books(id).title = \"kept\"\n\nfn title_of(id: int): string\n    return ^books(id).title\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::save", &[Value::Int(1)]).expect("commit");
    assert_eq!(
        run_entry(&program, &store, "test::title_of", &[Value::Int(1)])
            .expect("run")
            .value,
        Some(Value::Str("kept".into()))
    );
}

#[test]
fn a_transaction_rolls_back_on_an_escaping_error() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn risky(id: int)\n    transaction\n        ^books(id).title = \"staged\"\n        const x = 1 / 0\n\nfn has_book(id: int): bool\n    return exists(^books(id))\n",
    );
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::risky", &[Value::Int(1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_DIVIDE_BY_ZERO),
        "{result:?}"
    );
    // The write staged before the error was rolled back.
    assert_eq!(
        run_entry(&program, &store, "test::has_book", &[Value::Int(1)])
            .expect("run")
            .value,
        Some(Value::Bool(false)),
        "the staged write rolled back with the transaction"
    );
}

/// A backend that delegates every operation to an inner [`MemStore`] but fails
/// `rollback()` with a store-integrity error. Models a persistent store whose
/// undo could not be applied, so the transaction handler must surface the
/// failure rather than mask it behind the original escape.
struct FailingRollbackStore {
    inner: MemStore,
}

impl FailingRollbackStore {
    fn new() -> Self {
        Self {
            inner: MemStore::new(),
        }
    }
}

impl Backend for FailingRollbackStore {
    fn read(&self, path: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Backend::read(&self.inner, path)
    }
    fn write(&mut self, path: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        Backend::write(&mut self.inner, path, value)
    }
    fn delete(&mut self, path: &[u8]) -> Result<(), StoreError> {
        Backend::delete(&mut self.inner, path)
    }
    fn presence(&self, path: &[u8]) -> Result<Presence, StoreError> {
        Backend::presence(&self.inner, path)
    }
    fn child_keys(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        Backend::child_keys(&self.inner, path)
    }
    fn scan(&self, path: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        Backend::scan(&self.inner, path, limit)
    }
    fn roots(&self) -> Result<Vec<String>, StoreError> {
        Backend::roots(&self.inner)
    }
    fn max_int_record_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError> {
        Backend::max_int_record_key(&self.inner, prefix)
    }
    fn max_int_index_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError> {
        Backend::max_int_index_key(&self.inner, prefix)
    }
    fn begin(&mut self) -> Result<(), StoreError> {
        Backend::begin(&mut self.inner)
    }
    fn commit(&mut self) -> Result<(), StoreError> {
        Backend::commit(&mut self.inner)
    }
    fn rollback(&mut self) -> Result<(), StoreError> {
        Err(StoreError::Corruption {
            message: "rollback could not be applied".into(),
        })
    }
}

#[test]
fn a_failed_rollback_after_an_error_surfaces_a_store_error() {
    // The body errors, so the transaction rolls back — but the rollback itself
    // fails. A failed rollback is a store-integrity failure that supersedes the
    // original cause, so the run surfaces a typed store error, not the divide.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn risky(id: int)\n    transaction\n        ^books(id).title = \"staged\"\n        const x = 1 / 0\n",
    );
    let store = RefCell::new(FailingRollbackStore::new());
    let result = run_entry(&program, &store, "test::risky", &[Value::Int(1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_STORE),
        "a failed rollback must surface as a store error, got {result:?}"
    );
}

#[test]
fn a_failed_rollback_after_a_throw_surfaces_a_store_error() {
    // A throw escapes the transaction, which rolls back — but the rollback
    // fails. The integrity failure must not be masked by a catchable throw, so
    // the run surfaces a typed store error instead of the throw.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn risky(id: int)\n    transaction\n        ^books(id).title = \"staged\"\n        throw Error(code: \"x.y\", message: \"boom\")\n",
    );
    let store = RefCell::new(FailingRollbackStore::new());
    let result = run_entry(&program, &store, "test::risky", &[Value::Int(1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_STORE),
        "a failed rollback after a throw must surface as a store error, got {result:?}"
    );
}

/// A `Book` with a unique `isbn` index plus helpers that seed a record, attempt
/// a conflicting write under `try`/`catch`, and read a field back. Used by the
/// recoverable-write-fault tests.
const UNIQUE_RECOVERY: &str = "\
resource Book at ^books(id: int)
    required title: string
    isbn: string

    index byIsbn(isbn) unique

fn seed(id: int, t: string, isbn: string)
    ^books(id).title = t
    ^books(id).isbn = isbn

fn claimOrCode(id: int, isbn: string): string
    try
        ^books(id).isbn = isbn
    catch err: Error
        return err.code
    return \"written\"

fn claim(id: int, isbn: string)
    ^books(id).isbn = isbn

fn recover(id: int, isbn: string, fallback: string): string
    try
        ^books(id).isbn = isbn
    catch err: Error
        ^books(id).title = fallback
    return ^books(id).title

fn titleOf(id: int): string
    return ^books(id).title

fn isbnOf(id: int): string
    return ^books(id).isbn

fn ownerOf(isbn: string): Book::Id
    return ^books.byIsbn(isbn)
";

#[test]
fn a_unique_conflict_is_catchable_and_binds_the_dotted_code() {
    // The spec's recoverable-write contract: a unique-index conflict surfaces as
    // a catchable Error, so a `try`/`catch` inside the writing function binds it
    // by its `write.unique_conflict` code and the function continues normally.
    let program = checked_program(UNIQUE_RECOVERY);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ],
    )
    .expect("seed");
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ],
    )
    .expect("seed");
    // Book 2 tries to claim book 1's isbn: a unique conflict the catch binds.
    let caught = run_entry(
        &program,
        &store,
        "test::claimOrCode",
        &[Value::Int(2), Value::Str("978-0".into())],
    )
    .expect("caught")
    .value;
    assert_eq!(caught, Some(Value::Str("write.unique_conflict".into())));
}

#[test]
fn a_caught_unique_conflict_lets_following_code_run_and_did_not_write() {
    // After catching the conflict, code keeps running (writes a fallback) and the
    // rejected write left no effect: book 2 still owns its original isbn.
    let program = checked_program(UNIQUE_RECOVERY);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ],
    )
    .expect("seed");
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ],
    )
    .expect("seed");
    let title = run_entry(
        &program,
        &store,
        "test::recover",
        &[
            Value::Int(2),
            Value::Str("978-0".into()),
            Value::Str("fallback".into()),
        ],
    )
    .expect("recovered")
    .value;
    assert_eq!(title, Some(Value::Str("fallback".into())), "catch body ran");
    // The rejected write left no effect: book 2 still has its original isbn and the
    // unique index still maps the conflicting isbn to book 1, not book 2.
    assert_eq!(
        run_entry(&program, &store, "test::isbnOf", &[Value::Int(2)])
            .expect("read")
            .value,
        Some(Value::Str("978-9".into())),
        "book 2's isbn was not overwritten",
    );
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::ownerOf",
            &[Value::Str("978-0".into())]
        )
        .expect("read")
        .value,
        Some(Value::Identity(vec![SavedKey::Int(1)])),
        "the unique index still points at book 1",
    );
}

#[test]
fn an_uncaught_unique_conflict_keeps_its_dotted_code() {
    // Preserve uncaught behavior: a conflict that escapes the entry surfaces with
    // its own `write.unique_conflict` code (not run.uncaught_error), exactly as
    // before it became catchable.
    let program = checked_program(UNIQUE_RECOVERY);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ],
    )
    .expect("seed");
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ],
    )
    .expect("seed");
    let result = run_entry(
        &program,
        &store,
        "test::claim",
        &[Value::Int(2), Value::Str("978-0".into())],
    );
    assert_eq!(result.expect_err("conflict").code, "write.unique_conflict",);
}

#[test]
fn a_unique_conflict_inside_a_transaction_can_be_caught_and_continue() {
    // The spec: a conflict caught inside a transaction has no effect, and the
    // transaction continues and commits its other writes.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\nfn seed(id: int, t: string, isbn: string)\n    ^books(id).title = t\n    ^books(id).isbn = isbn\n\nfn run_it(id: int, isbn: string, t: string)\n    transaction\n        try\n            ^books(id).isbn = isbn\n        catch err: Error\n            ^books(id).title = t\n\nfn titleOf(id: int): string\n    return ^books(id).title\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ],
    )
    .expect("seed");
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ],
    )
    .expect("seed");
    run_entry(
        &program,
        &store,
        "test::run_it",
        &[
            Value::Int(2),
            Value::Str("978-0".into()),
            Value::Str("after".into()),
        ],
    )
    .expect("transaction commits after catching");
    // The transaction's other write (the title) committed.
    assert_eq!(
        run_entry(&program, &store, "test::titleOf", &[Value::Int(2)])
            .expect("read")
            .value,
        Some(Value::Str("after".into())),
    );
}

#[test]
fn a_caught_write_fault_does_not_leak_into_a_later_fault() {
    // After a `try` catches a write fault, the stashed Error is cleared, so a later
    // genuine fault (divide-by-zero) still faults rather than being miscaught.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\nfn seed(id: int, t: string, isbn: string)\n    ^books(id).title = t\n    ^books(id).isbn = isbn\n\nfn run_it(): int\n    try\n        ^books(2).isbn = \"978-0\"\n    catch err: Error\n        write(\"caught\")\n    return 1 / 0\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ],
    )
    .expect("seed");
    run_entry(
        &program,
        &store,
        "test::seed",
        &[
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ],
    )
    .expect("seed");
    assert_eq!(
        run_entry(&program, &store, "test::run_it", &[])
            .unwrap_err()
            .code,
        RUN_DIVIDE_BY_ZERO,
    );
}

#[test]
fn an_absent_element_read_is_catchable() {
    // The documented catchable runtime fault: a direct read of an unpopulated
    // element raises a catchable Error a `try`/`catch` can bind.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\nfn titleOrCode(id: int): string\n    try\n        return ^books(id).title\n    catch err: Error\n        return err.code\n",
    );
    let store = RefCell::new(MemStore::new());
    let caught = run_entry(&program, &store, "test::titleOrCode", &[Value::Int(1)])
        .expect("caught")
        .value;
    assert_eq!(caught, Some(Value::Str("run.absent_element".into())));
}

#[test]
fn reads_inside_a_transaction_see_earlier_writes() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn rww(id: int): string\n    transaction\n        ^books(id).title = \"fresh\"\n        return ^books(id).title\n",
    );
    let store = RefCell::new(MemStore::new());
    let outcome = run_entry(&program, &store, "test::rww", &[Value::Int(1)]).expect("run");
    assert_eq!(outcome.value, Some(Value::Str("fresh".into())));
}

#[test]
fn a_lock_block_runs_its_body_and_releases_on_exit() {
    // `lock` type-checks as a scope guarding its body. Under the single-writer
    // profile it runs the block (writes land, a `return` exits) rather than
    // failing with run.unsupported.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn save(id: int): string\n    lock ^books(id)\n        ^books(id).title = \"kept\"\n        return ^books(id).title\n\nfn title_of(id: int): string\n    return ^books(id).title\n",
    );
    let store = RefCell::new(MemStore::new());
    let outcome =
        run_entry(&program, &store, "test::save", &[Value::Int(1)]).expect("lock body runs");
    assert_eq!(outcome.value, Some(Value::Str("kept".into())));
    // The write inside the lock persisted after the lock released.
    assert_eq!(
        run_entry(&program, &store, "test::title_of", &[Value::Int(1)])
            .expect("run")
            .value,
        Some(Value::Str("kept".into()))
    );
}

#[test]
fn append_writes_at_the_next_position() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n",
    );
    let store = RefCell::new(MemStore::new());
    let appended = |t: &str| {
        run_entry(
            &program,
            &store,
            "test::add_tag",
            &[Value::Int(5), Value::Str(t.into())],
        )
        .expect("run")
        .value
    };
    // Successive appends take positions 1 then 2 (no hole-filling).
    assert_eq!(appended("a"), Some(Value::Int(1)));
    assert_eq!(appended("b"), Some(Value::Int(2)));
    // The values landed at `^books(5).tags(1)` and `tags(2)`.
    let tag = |pos: i64| -> Option<SavedValue> {
        let store = store.borrow();
        let bytes = store.read(&encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(5)),
            PathSegment::ChildLayer("tags".into()),
            PathSegment::IndexKey(SavedKey::Int(pos)),
        ]))?;
        decode_value(bytes, ValueType::Str)
    };
    assert_eq!(tag(1), Some(SavedValue::Str("a".into())));
    assert_eq!(tag(2), Some(SavedValue::Str("b".into())));
}

#[test]
fn appends_then_reads_back_keyed_leaf_entries() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n\nfn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos)\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::add_tag",
        &[Value::Int(5), Value::Str("a".into())],
    )
    .expect("append");
    run_entry(
        &program,
        &store,
        "test::add_tag",
        &[Value::Int(5), Value::Str("b".into())],
    )
    .expect("append");
    let tag = |pos: i64| {
        run_entry(
            &program,
            &store,
            "test::tag_at",
            &[Value::Int(5), Value::Int(pos)],
        )
        .expect("read")
        .value
    };
    assert_eq!(tag(1), Some(Value::Str("a".into())));
    assert_eq!(tag(2), Some(Value::Str("b".into())));
    // Reading an absent position is an absent-element error.
    let missing = run_entry(
        &program,
        &store,
        "test::tag_at",
        &[Value::Int(5), Value::Int(3)],
    );
    assert!(
        matches!(missing, Err(ref error) if error.code == RUN_ABSENT),
        "{missing:?}"
    );
}

#[test]
fn explicit_keyed_leaf_write_then_reads_back() {
    // `^books(id).tags(pos) = value` writes one keyed-leaf entry directly, and a
    // string-keyed leaf `scores(key) = value` writes through the same path.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n    scores(key: string): int\n\nfn set_tag(id: int, pos: int, t: string)\n    ^books(id).tags(pos) = t\n\nfn set_score(id: int, key: string, n: int)\n    ^books(id).scores(key) = n\n\nfn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos)\n\nfn score_at(id: int, key: string): int\n    return ^books(id).scores(key)\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::set_tag",
        &[Value::Int(5), Value::Int(3), Value::Str("fiction".into())],
    )
    .expect("explicit keyed-leaf write");
    run_entry(
        &program,
        &store,
        "test::set_score",
        &[Value::Int(5), Value::Str("alice".into()), Value::Int(7)],
    )
    .expect("string-keyed leaf write");

    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::tag_at",
            &[Value::Int(5), Value::Int(3)],
        )
        .expect("read")
        .value,
        Some(Value::Str("fiction".into()))
    );
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::score_at",
            &[Value::Int(5), Value::Str("alice".into())],
        )
        .expect("read")
        .value,
        Some(Value::Int(7))
    );
}

#[test]
fn explicit_keyed_leaf_write_creates_a_hole_that_append_skips() {
    // An explicit write past the dense range leaves a hole; append chooses one
    // past the highest positive key, not the first gap (builtins.md).
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn set_tag(id: int, pos: int, t: string)\n    ^books(id).tags(pos) = t\n\nfn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n\nfn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos)\n",
    );
    let store = RefCell::new(MemStore::new());
    // Write position 5 directly, leaving 1..=4 as holes.
    run_entry(
        &program,
        &store,
        "test::set_tag",
        &[Value::Int(9), Value::Int(5), Value::Str("hi".into())],
    )
    .expect("explicit write");
    // Append lands at 6 (one past the highest positive key), skipping the holes.
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::add_tag",
            &[Value::Int(9), Value::Str("next".into())],
        )
        .expect("append")
        .value,
        Some(Value::Int(6))
    );
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::tag_at",
            &[Value::Int(9), Value::Int(6)],
        )
        .expect("read")
        .value,
        Some(Value::Str("next".into()))
    );
}

/// A program that indexes books by shelf and traverses the index with `keys`.
const BOOK_SHELF: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

fn count_on(shelf: string): int
    var c = 0
    for id in keys(^books.byShelf(shelf))
        c = c + 1
    return c

fn titles_on(shelf: string)
    for id in keys(^books.byShelf(shelf))
        print(^books(id).title)
";

#[test]
fn iterates_index_keys() {
    let program = checked_program(BOOK_SHELF);
    let store = RefCell::new(MemStore::new());
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &program,
            &store,
            "test::add",
            &[
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ],
        )
        .expect("add");
    };
    add(1, "Mort", "fiction");
    add(2, "Sourcery", "fiction");
    add(3, "Guards", "history");

    let count = |shelf: &str| {
        run_entry(
            &program,
            &store,
            "test::count_on",
            &[Value::Str(shelf.into())],
        )
        .expect("count")
        .value
    };
    assert_eq!(count("fiction"), Some(Value::Int(2)));
    assert_eq!(count("history"), Some(Value::Int(1)));
    assert_eq!(count("romance"), Some(Value::Int(0)));
}

#[test]
fn prints_titles_in_index_key_order() {
    let program = checked_program(BOOK_SHELF);
    let store = RefCell::new(MemStore::new());
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &program,
            &store,
            "test::add",
            &[
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ],
        )
        .expect("add");
    };
    add(2, "Sourcery", "fiction");
    add(1, "Mort", "fiction");

    // The index yields ids in key order (1 then 2), regardless of insert order.
    let outcome = run_entry(
        &program,
        &store,
        "test::titles_on",
        &[Value::Str("fiction".into())],
    )
    .expect("run");
    assert_eq!(outcome.output, "Mort\nSourcery\n");
}

/// A program that reads, copies, and reads back whole `Book` resources.
const BOOK_COPY: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

fn read(id: int): Book
    return ^books(id)

fn copy(from: int, to: int)
    ^books(to) = ^books(from)

fn title_of(id: int): string
    return ^books(id).title

fn shelf_of(id: int): string
    return ^books(id).shelf
";

/// Write `^books(id).field = value` directly into the store.
fn seed_field(store: &RefCell<MemStore>, id: i64, field: &str, value: &str) {
    store.borrow_mut().write(
        &encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(id)),
            PathSegment::Field(field.into()),
        ]),
        encode_value(&SavedValue::Str(value.into())).expect("in-range value encodes"),
    );
}

#[test]
fn reads_a_whole_resource() {
    let program = checked_program(BOOK_COPY);
    let store = RefCell::new(MemStore::new());
    seed_field(&store, 1, "title", "Mort");
    seed_field(&store, 1, "shelf", "fiction");
    let outcome = run_entry(&program, &store, "test::read", &[Value::Int(1)]).expect("read");
    // Present fields, in schema order.
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![
            ("title".into(), Value::Str("Mort".into())),
            ("shelf".into(), Value::Str("fiction".into())),
        ]))
    );
}

#[test]
fn copies_a_whole_resource() {
    let program = checked_program(BOOK_COPY);
    let store = RefCell::new(MemStore::new());
    seed_field(&store, 1, "title", "Mort");
    seed_field(&store, 1, "shelf", "fiction");
    run_entry(
        &program,
        &store,
        "test::copy",
        &[Value::Int(1), Value::Int(2)],
    )
    .expect("copy");
    let read = |entry: &str| {
        run_entry(&program, &store, entry, &[Value::Int(2)])
            .expect("run")
            .value
    };
    assert_eq!(read("test::title_of"), Some(Value::Str("Mort".into())));
    assert_eq!(read("test::shelf_of"), Some(Value::Str("fiction".into())));
}

/// A resource declaring an unkeyed nested group (`name`). A whole-resource read
/// would silently omit the group's fields, and a whole-resource write would
/// delete the group subtree while rewriting only top-level fields — so both
/// must fail fast until group materialization lands.
const PATIENT_WITH_GROUP: &str = "\
resource Patient at ^patients(id: int)
    mrn: string
    name
        first: string
        last: string

fn read(id: int): Patient
    return ^patients(id)

fn copy(from: int, to: int)
    ^patients(to) = ^patients(from)
";

#[test]
fn whole_resource_read_with_unkeyed_group_fails_fast() {
    let program = checked_program(PATIENT_WITH_GROUP);
    let store = RefCell::new(MemStore::new());
    store.borrow_mut().write(
        &encode_path(&[
            PathSegment::Root("patients".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
            PathSegment::Field("mrn".into()),
        ]),
        encode_value(&SavedValue::Str("A1".into())).expect("in-range value encodes"),
    );
    let error = run_entry(&program, &store, "test::read", &[Value::Int(1)]).unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
}

#[test]
fn whole_resource_write_with_unkeyed_group_fails_fast() {
    let program = checked_program(PATIENT_WITH_GROUP);
    let store = RefCell::new(MemStore::new());
    let error = run_entry(
        &program,
        &store,
        "test::copy",
        &[Value::Int(1), Value::Int(2)],
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
}

/// The sample's `add` shape: allocate an id, build a local resource field by
/// field, and save it.
const BOOK_ADD: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

fn add(title: string, shelf: string): int
    const id = nextId(^books)
    var book: Book
    book.title = title
    book.shelf = shelf
    ^books(id) = book
    return id

fn title_of(id: int): string
    return ^books(id).title

fn shelf_of(id: int): string
    return ^books(id).shelf
";

#[test]
fn builds_a_local_resource_and_saves_it() {
    let program = checked_program(BOOK_ADD);
    let store = RefCell::new(MemStore::new());
    let id = run_entry(
        &program,
        &store,
        "test::add",
        &[Value::Str("Mort".into()), Value::Str("fiction".into())],
    )
    .expect("add")
    .value;
    assert_eq!(id, Some(Value::Int(1)));
    let read = |entry: &str| {
        run_entry(&program, &store, entry, &[Value::Int(1)])
            .expect("run")
            .value
    };
    assert_eq!(read("test::title_of"), Some(Value::Str("Mort".into())));
    assert_eq!(read("test::shelf_of"), Some(Value::Str("fiction".into())));
}

#[test]
fn reads_a_local_resource_field() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\nfn echo(t: string): string\n    var book: Book\n    book.title = t\n    return book.title\n",
    );
    let store = RefCell::new(MemStore::new());
    let value = run_entry(&program, &store, "test::echo", &[Value::Str("Mort".into())])
        .expect("run")
        .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
}

#[test]
fn merge_updates_supplied_fields_and_keeps_the_rest() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\nfn move_to(id: int, s: string)\n    var patch: Book\n    patch.shelf = s\n    merge ^books(id) = patch\n\nfn title_of(id: int): string\n    return ^books(id).title\n\nfn shelf_of(id: int): string\n    return ^books(id).shelf\n",
    );
    let store = RefCell::new(MemStore::new());
    seed_field(&store, 1, "title", "Mort");
    seed_field(&store, 1, "shelf", "fiction");
    // Merge a patch that supplies only `shelf`.
    run_entry(
        &program,
        &store,
        "test::move_to",
        &[Value::Int(1), Value::Str("history".into())],
    )
    .expect("merge");
    let read = |entry: &str| {
        run_entry(&program, &store, entry, &[Value::Int(1)])
            .expect("run")
            .value
    };
    assert_eq!(
        read("test::shelf_of"),
        Some(Value::Str("history".into())),
        "shelf updated"
    );
    assert_eq!(
        read("test::title_of"),
        Some(Value::Str("Mort".into())),
        "title kept"
    );
}

#[test]
fn merge_into_a_local_overlays_source_fields_and_keeps_the_rest() {
    // `merge draft = ^books(id)` overlays the saved record's populated fields onto
    // the local `draft`, leaving draft's other fields in place. The seeded record
    // has only `title`, so the merge sets draft.title but keeps draft.shelf.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\nfn draft_title(id: int): string\n    var draft: Book\n    draft.shelf = \"local-shelf\"\n    merge draft = ^books(id)\n    return draft.title\n\nfn draft_shelf(id: int): string\n    var draft: Book\n    draft.shelf = \"local-shelf\"\n    merge draft = ^books(id)\n    return draft.shelf\n",
    );
    let store = RefCell::new(MemStore::new());
    seed_field(&store, 1, "title", "Mort");
    let read = |entry: &str| {
        run_entry(&program, &store, entry, &[Value::Int(1)])
            .expect("run")
            .value
    };
    assert_eq!(
        read("test::draft_title"),
        Some(Value::Str("Mort".into())),
        "the source's populated field overlays the local"
    );
    assert_eq!(
        read("test::draft_shelf"),
        Some(Value::Str("local-shelf".into())),
        "a local field the source does not supply is kept"
    );
}

/// A `Book` with a shelf index AND a `tags` child layer, plus a `copy` that
/// merges one saved record onto another (`merge ^books(to) = ^books(from)`).
const BOOK_TREE_MERGE: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string
    tags(pos: int): string

    index byShelf(shelf, id)

fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

fn add_tag(id: int, tag: string): int
    return append(^books(id).tags, tag)

fn copy(from: int, to: int)
    merge ^books(to) = ^books(from)

fn tag_of(id: int, pos: int): string
    return ^books(id).tags(pos)

fn ids_on(shelf: string)
    for id in keys(^books.byShelf(shelf))
        print($\"{id}\")
";

#[test]
fn a_tree_shaped_merge_copies_a_child_layer_and_moves_the_index() {
    // Source (1) is on the fiction shelf with a tag; target (2) starts on the
    // history shelf with no tags. `merge ^books(2) = ^books(1)` copies the tag
    // onto the target AND moves the target's index entry to the merged shelf.
    let program = checked_program(BOOK_TREE_MERGE);
    let store = RefCell::new(MemStore::new());
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &program,
            &store,
            "test::add",
            &[
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ],
        )
        .expect("add");
    };
    add(1, "Mort", "fiction");
    add(2, "Reaper", "history");
    run_entry(
        &program,
        &store,
        "test::add_tag",
        &[Value::Int(1), Value::Str("favorite".into())],
    )
    .expect("tag source");

    run_entry(
        &program,
        &store,
        "test::copy",
        &[Value::Int(1), Value::Int(2)],
    )
    .expect("merge");

    // The source's child-layer entry is copied onto the target.
    assert_eq!(
        run_entry(
            &program,
            &store,
            "test::tag_of",
            &[Value::Int(2), Value::Int(1)],
        )
        .expect("read copied tag")
        .value,
        Some(Value::Str("favorite".into())),
    );
    // The index reflects the merged shelf: the target is now on fiction (with the
    // source), and nothing is left on history — no stray entry.
    let ids_on = |shelf: &str| {
        run_entry(
            &program,
            &store,
            "test::ids_on",
            &[Value::Str(shelf.into())],
        )
        .expect("read index")
        .output
    };
    assert_eq!(
        ids_on("fiction"),
        "1\n2\n",
        "both records on the merged shelf"
    );
    assert_eq!(
        ids_on("history"),
        "",
        "no stray index entry on the old shelf"
    );
}

/// A program that records the run's clock instant into a saved `instant` field
/// and reads it back, exercising `std::clock::now()` through `const` and a managed
/// write.
const CLOCK_SAMPLE: &str = "\
resource Event at ^events(id: int)
    required changedAt: instant

fn record(id: int)
    const now: instant = std::clock::now()
    ^events(id).changedAt = now

fn changed_at_of(id: int): instant
    return ^events(id).changedAt
";

#[test]
fn clock_now_reads_the_host_clock_capability() {
    let program = checked_program(CLOCK_SAMPLE);
    let store = RefCell::new(MemStore::new());
    // 1970-01-01T00:00:01Z, one second after the epoch.
    let host = Host::new().with_clock(1_000_000_000);
    run_entry_with_host(&program, &store, &host, "test::record", &[Value::Int(1)]).expect("record");
    // The instant round-trips through the managed write and a typed read.
    let outcome =
        run_entry(&program, &store, "test::changed_at_of", &[Value::Int(1)]).expect("read");
    assert_eq!(outcome.value, Some(Value::Instant(1_000_000_000)));
}

#[test]
fn clock_now_without_a_clock_capability_is_a_capability_error() {
    let program = checked_program("fn t(): instant\n    return std::clock::now()\n");
    let store = RefCell::new(MemStore::new());
    // Plain `run_entry` supplies no host capabilities.
    let result = run_entry(&program, &store, "test::t", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

/// A program that reads environment variables through the three `std::env`
/// builtins: presence, lookup with a default, and a required lookup.
const ENV_SAMPLE: &str = "\
fn has(name: string): bool
    return std::env::exists(name)

fn read(name: string, fallback: string): string
    return std::env::get(name, fallback)

fn must(name: string): string
    return std::env::require(name)
";

/// A host whose environment is the test's fixed variables.
fn env_host() -> Host {
    Host::new().with_environment(HashMap::from([
        ("HOME".to_string(), "/home/marrow".to_string()),
        ("EMPTY".to_string(), String::new()),
    ]))
}

#[test]
fn env_reads_variables_from_the_host_capability() {
    let program = checked_program(ENV_SAMPLE);
    let store = RefCell::new(MemStore::new());
    let host = env_host();
    let call = |entry: &str, args: &[Value]| {
        run_entry_with_host(&program, &store, &host, entry, args)
            .expect("env call")
            .value
    };
    // `exists` reports presence, including a present-but-empty variable.
    assert_eq!(
        call("test::has", &[Value::Str("HOME".into())]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call("test::has", &[Value::Str("EMPTY".into())]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call("test::has", &[Value::Str("MISSING".into())]),
        Some(Value::Bool(false))
    );
    // `require` returns a present variable's value.
    assert_eq!(
        call("test::must", &[Value::Str("HOME".into())]),
        Some(Value::Str("/home/marrow".into()))
    );
}

#[test]
fn env_get_falls_back_to_the_default_when_absent() {
    let program = checked_program(ENV_SAMPLE);
    let store = RefCell::new(MemStore::new());
    let host = env_host();
    let call = |name: &str, fallback: &str| {
        run_entry_with_host(
            &program,
            &store,
            &host,
            "test::read",
            &[Value::Str(name.into()), Value::Str(fallback.into())],
        )
        .expect("env get")
        .value
    };
    // A present variable wins over the default; an empty one is still present.
    assert_eq!(
        call("HOME", "fallback"),
        Some(Value::Str("/home/marrow".into()))
    );
    assert_eq!(call("EMPTY", "fallback"), Some(Value::Str(String::new())));
    // An absent variable falls back to the default.
    assert_eq!(
        call("MISSING", "fallback"),
        Some(Value::Str("fallback".into()))
    );
}

#[test]
fn env_require_missing_variable_is_an_absent_error() {
    let program = checked_program(ENV_SAMPLE);
    let store = RefCell::new(MemStore::new());
    let host = env_host();
    let result = run_entry_with_host(
        &program,
        &store,
        &host,
        "test::must",
        &[Value::Str("MISSING".into())],
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_ABSENT),
        "{result:?}"
    );
}

#[test]
fn env_without_an_environment_capability_is_a_capability_error() {
    let program = checked_program(ENV_SAMPLE);
    let store = RefCell::new(MemStore::new());
    // Plain `run_entry` supplies no host capabilities, so the whole module is
    // unavailable — even presence checks.
    let result = run_entry(&program, &store, "test::has", &[Value::Str("HOME".into())]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

/// A program that logs at each level, including an `Error` value.
const LOG_SAMPLE: &str = "\
fn note(m: string)
    std::log::info(m)

fn careful(m: string)
    std::log::warn(m)

fn boom()
    std::log::error(Error(code: \"E_BOOM\", message: \"kaboom\"))
";

#[test]
fn log_writes_each_level_to_the_host_sink() {
    let program = checked_program(LOG_SAMPLE);
    let store = RefCell::new(MemStore::new());
    let sink = Rc::new(RefCell::new(String::new()));
    let host = Host::new().with_log_sink(Rc::clone(&sink));
    run_entry_with_host(
        &program,
        &store,
        &host,
        "test::note",
        &[Value::Str("hello".into())],
    )
    .expect("info");
    run_entry_with_host(
        &program,
        &store,
        &host,
        "test::careful",
        &[Value::Str("watch out".into())],
    )
    .expect("warn");
    run_entry_with_host(&program, &store, &host, "test::boom", &[]).expect("error");
    assert_eq!(
        sink.borrow().as_str(),
        "INFO hello\nWARN watch out\nERROR [E_BOOM] kaboom\n"
    );
}

#[test]
fn log_without_a_log_capability_is_a_capability_error() {
    let program = checked_program(LOG_SAMPLE);
    let store = RefCell::new(MemStore::new());
    // Plain `run_entry` supplies no host capabilities.
    let result = run_entry(&program, &store, "test::note", &[Value::Str("hi".into())]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_CAPABILITY),
        "{result:?}"
    );
}

#[test]
fn log_error_requires_an_error_value() {
    let program = checked_program("fn t()\n    std::log::error(\"not an error\")\n");
    let store = RefCell::new(MemStore::new());
    let sink = Rc::new(RefCell::new(String::new()));
    let host = Host::new().with_log_sink(Rc::clone(&sink));
    let result = run_entry_with_host(&program, &store, &host, "test::t", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

/// The encoded path of a group-entry field `^books(id).layer(key).field`, for
/// asserting group writes directly (the runtime has no group-entry read yet).
fn group_field_path(id: i64, layer: &str, key: SavedKey, field: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::ChildLayer(layer.into()),
        PathSegment::IndexKey(key),
        PathSegment::Field(field.into()),
    ])
}

#[test]
fn a_group_entry_field_write_lands_in_saved_data() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n    notes(noteId: string)\n        text: string\n\nfn add_note(id: int, note: string, t: string)\n    ^books(id).notes(note).text = t\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::add_note",
        &[
            Value::Int(5),
            Value::Str("n1".into()),
            Value::Str("hello".into()),
        ],
    )
    .expect("group-entry write");
    let bytes = store
        .borrow()
        .read(&group_field_path(
            5,
            "notes",
            SavedKey::Str("n1".into()),
            "text",
        ))
        .map(<[u8]>::to_vec);
    assert_eq!(
        bytes
            .as_deref()
            .and_then(|b| decode_value(b, ValueType::Str)),
        Some(SavedValue::Str("hello".into()))
    );
}

#[test]
fn group_entry_field_writes_compose_in_a_transaction() {
    // The sample's `add` shape: a whole-record write plus group-entry history
    // writes, all inside one transaction.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n    versions(version: int)\n        required title: string\n        required shelf: string\n\nfn add(id: int, t: string, s: string)\n    transaction\n        ^books(id).title = t\n        ^books(id).versions(1).title = t\n        ^books(id).versions(1).shelf = s\n\nfn title_of(id: int): string\n    return ^books(id).title\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::add",
        &[
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("fiction".into()),
        ],
    )
    .expect("transactional group writes");
    // The top-level field reads back through the runtime.
    assert_eq!(
        run_entry(&program, &store, "test::title_of", &[Value::Int(1)])
            .expect("read")
            .value,
        Some(Value::Str("Mort".into()))
    );
    // The group-entry members committed alongside it.
    let version_member = |field: &str| {
        store
            .borrow()
            .read(&group_field_path(1, "versions", SavedKey::Int(1), field))
            .map(<[u8]>::to_vec)
            .as_deref()
            .and_then(|b| decode_value(b, ValueType::Str))
    };
    assert_eq!(
        version_member("title"),
        Some(SavedValue::Str("Mort".into()))
    );
    assert_eq!(
        version_member("shelf"),
        Some(SavedValue::Str("fiction".into()))
    );
}

#[test]
fn a_call_binds_named_arguments_by_name() {
    // Named arguments may appear in any order; they bind by name, not position.
    // `sub(b: 10, a: 3)` is `3 - 10`, not `10 - 3`.
    let program = checked_program(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(b: 10, a: 3)\n",
    );
    assert_eq!(run(&program, "test::go", &[]), Ok(Some(Value::Int(-7))));
}

#[test]
fn a_call_mixes_positional_then_named_arguments() {
    let program = checked_program(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(10, b: 3)\n",
    );
    assert_eq!(run(&program, "test::go", &[]), Ok(Some(Value::Int(7))));
}

#[test]
fn a_call_with_an_unknown_parameter_name_is_rejected() {
    let program = checked_program(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(a: 1, c: 2)\n",
    );
    let result = run(&program, "test::go", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn a_call_missing_an_argument_is_rejected() {
    let program = checked_program(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(a: 1)\n",
    );
    let result = run(&program, "test::go", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn a_call_supplying_a_parameter_twice_is_rejected() {
    // Positional `1` fills `a`; the named `a: 2` then collides.
    let program = checked_program(
        "fn sub(a: int, b: int): int\n    return a - b\n\nfn go(): int\n    return sub(1, a: 2)\n",
    );
    let result = run(&program, "test::go", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

// Note: positional-after-named (`sub(b: 1, 2)`) is now rejected by the PARSER
// (parse.syntax), so it cannot reach the runtime via a parsed program; the
// `bind_arguments` guard remains as defensive depth. The parser owns this rule
// and tests it in marrow-syntax.

/// Extract the single `mw` code block from the reference sample document, so the
/// integration test runs the exact source the docs publish.
fn sample_source() -> String {
    let doc = include_str!("../../../docs/language/sample.md");
    doc.split("```mw")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("the sample document has an mw code block")
        .to_string()
}

#[test]
fn the_reference_sample_runs_end_to_end() {
    // The canonical sample (docs/language/sample.md) must run on the in-memory
    // store: add a book in a transaction (whole-resource + history group writes),
    // tag it, and print the fiction shelf via index traversal.
    let program = checked_program(&sample_source());
    let store = RefCell::new(MemStore::new());
    let host = Host::new().with_clock(1_700_000_000_000_000_000); // 2023-11-14T22:13:20Z
    let outcome = run_entry_with_host(&program, &store, &host, "test::main", &[])
        .expect("the sample's main runs end-to-end");
    // `main` returns nothing and prints the one fiction book it added.
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "1: Small Gods\n");
}

#[test]
fn the_reference_sample_runs_on_native_storage() {
    // Step 9's done-criterion: the same sample runs unchanged on the native redb
    // backend, with output identical to the in-memory run.
    let program = checked_program(&sample_source());
    let dir = tempfile::tempdir().expect("create a temp dir");
    let store = RefCell::new(RedbStore::open(&dir.path().join("sample.redb")).expect("open redb"));
    let host = Host::new().with_clock(1_700_000_000_000_000_000);
    let outcome = run_entry_with_host(&program, &store, &host, "test::main", &[])
        .expect("the sample's main runs on native storage");
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "1: Small Gods\n");
}

#[test]
fn a_layer_merge_copies_tags_between_records() {
    // The sample's `copyTags`: build a source layer with `append`, copy it onto
    // another record with `merge`, and read the copies back as keyed-leaf entries.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn add_tag(id: int, tag: string): int\n    return append(^books(id).tags, tag)\n\nfn copy_tags(from: int, to: int)\n    merge ^books(to).tags = ^books(from).tags\n\nfn tag_of(id: int, pos: int): string\n    return ^books(id).tags(pos)\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::add_tag",
        &[Value::Int(1), Value::Str("favorite".into())],
    )
    .expect("tag 1");
    run_entry(
        &program,
        &store,
        "test::add_tag",
        &[Value::Int(1), Value::Str("gift".into())],
    )
    .expect("tag 2");
    run_entry(
        &program,
        &store,
        "test::copy_tags",
        &[Value::Int(1), Value::Int(2)],
    )
    .expect("copy tags");
    let tag_of = |pos: i64| {
        run_entry(
            &program,
            &store,
            "test::tag_of",
            &[Value::Int(2), Value::Int(pos)],
        )
        .expect("read tag")
        .value
    };
    assert_eq!(tag_of(1), Some(Value::Str("favorite".into())));
    assert_eq!(tag_of(2), Some(Value::Str("gift".into())));
}

const BOOK_VERSIONS: &str = "\
resource Book at ^books(id: int)
    required title: string

    versions(version: int)
        required title: string

fn set_version_title(id: int, v: int, t: string)
    ^books(id).versions(v).title = t

fn version_title(id: int, v: int): string
    return ^books(id).versions(v).title
";

#[test]
fn reads_a_field_from_a_group_entry() {
    let program = checked_program(BOOK_VERSIONS);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::set_version_title",
        &[Value::Int(1), Value::Int(2), Value::Str("Mort".into())],
    )
    .expect("write");
    let value = run_entry(
        &program,
        &store,
        "test::version_title",
        &[Value::Int(1), Value::Int(2)],
    )
    .expect("read")
    .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
}

#[test]
fn reading_an_absent_group_field_is_an_error() {
    let program = checked_program(BOOK_VERSIONS);
    let store = RefCell::new(MemStore::new());
    let result = run_entry(
        &program,
        &store,
        "test::version_title",
        &[Value::Int(1), Value::Int(2)],
    );
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_ABSENT),
        "{result:?}"
    );
}

#[test]
fn the_sample_update_functions_run() {
    // Drive the reference sample's mutating API beyond `main`: add a book, add a
    // note (group write guarded by `exists`), and move it between shelves (a
    // field write that also moves its generated index entry).
    let program = checked_program(&sample_source());
    let store = RefCell::new(MemStore::new());
    let when = Value::Instant(1_700_000_000_000_000_000);
    let id = run_entry(
        &program,
        &store,
        "test::add",
        &[
            Value::Str("Small Gods".into()),
            Value::Str("Terry Pratchett".into()),
            Value::Str("fiction".into()),
            when.clone(),
        ],
    )
    .expect("add")
    .value;
    assert_eq!(id, Some(Value::Int(1)));
    // addNote: true for an existing book, false for a missing one.
    let add_note = |book: i64| {
        run_entry(
            &program,
            &store,
            "test::addNote",
            &[
                Value::Int(book),
                Value::Str("n1".into()),
                Value::Str("first".into()),
            ],
        )
        .expect("addNote")
        .value
    };
    assert_eq!(add_note(1), Some(Value::Bool(true)));
    assert_eq!(add_note(2), Some(Value::Bool(false)));
    // moveToShelf updates the shelf and moves its generated index entry.
    run_entry(
        &program,
        &store,
        "test::moveToShelf",
        &[Value::Int(1), Value::Str("history".into()), when],
    )
    .expect("moveToShelf");
    let shelf = |name: &str| {
        run_entry(
            &program,
            &store,
            "test::printShelf",
            &[Value::Str(name.into())],
        )
        .expect("printShelf")
        .output
    };
    assert_eq!(shelf("history"), "1: Small Gods\n", "moved to history");
    assert_eq!(shelf("fiction"), "", "and left fiction");
}

// --- Resource-identity values ---

/// A single-key resource where code constructs an identity with `Book::Id(1)`,
/// passes it to a saved read, and writes through it. The identity carries the
/// lowered key so `^books(id)` reads the same record `^books(1)` does.
const BOOK_IDENTITY: &str = "\
resource Book at ^books(id: int)
    required title: string

fn save(t: string)
    const id = Book::Id(1)
    ^books(id).title = t

fn title(): string
    const id = Book::Id(1)
    return ^books(id).title
";

#[test]
fn constructs_and_uses_a_single_key_identity() {
    let program = checked_program(BOOK_IDENTITY);
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::save", &[Value::Str("Mort".into())]).expect("save");
    let value = run_entry(&program, &store, "test::title", &[])
        .expect("title")
        .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
    // The identity lowered to the same key a plain int does: `^books(1)`.
    let store = store.borrow();
    let bytes = store
        .read(&encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
            PathSegment::Field("title".into()),
        ]))
        .expect("present");
    assert_eq!(
        decode_value(bytes, ValueType::Str),
        Some(SavedValue::Str("Mort".into()))
    );
}

#[test]
fn a_plain_int_identity_still_works() {
    // `^books(Book::Id(1))` and `^books(1)` address the same record: the bare
    // int path (the `nextId` flow) is unchanged by the identity variant.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn save()\n    ^books(Book::Id(1)).title = \"a\"\n\nfn read(): string\n    return ^books(1).title\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::save", &[]).expect("save");
    let value = run_entry(&program, &store, "test::read", &[])
        .expect("read")
        .value;
    assert_eq!(value, Some(Value::Str("a".into())));
}

/// A composite-key resource: `Enrollment::Id(studentId:..., courseId:...)` builds
/// one identity from named keys, in declared order, and `^enrollments(id)` lowers
/// it back into both key segments.
const ENROLLMENT_IDENTITY: &str = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string

fn enroll(s: string, c: string, st: string)
    const id = Enrollment::Id(studentId: s, courseId: c)
    ^enrollments(id).status = st

fn statusOf(s: string, c: string): string
    const id = Enrollment::Id(studentId: s, courseId: c)
    return ^enrollments(id).status
";

#[test]
fn constructs_and_uses_a_composite_identity_round_trips() {
    let program = checked_program(ENROLLMENT_IDENTITY);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::enroll",
        &[
            Value::Str("student-1".into()),
            Value::Str("course-9".into()),
            Value::Str("active".into()),
        ],
    )
    .expect("enroll");
    let value = run_entry(
        &program,
        &store,
        "test::statusOf",
        &[
            Value::Str("student-1".into()),
            Value::Str("course-9".into()),
        ],
    )
    .expect("statusOf")
    .value;
    assert_eq!(value, Some(Value::Str("active".into())));
    // Keys lowered in declared order: studentId then courseId.
    let store = store.borrow();
    let bytes = store
        .read(&encode_path(&[
            PathSegment::Root("enrollments".into()),
            PathSegment::RecordKey(SavedKey::Str("student-1".into())),
            PathSegment::RecordKey(SavedKey::Str("course-9".into())),
            PathSegment::Field("status".into()),
        ]))
        .expect("present");
    assert_eq!(
        decode_value(bytes, ValueType::Str),
        Some(SavedValue::Str("active".into()))
    );
}

#[test]
fn composite_identity_orders_keys_by_declaration_not_arguments() {
    // Named args supplied in reverse order still lower in declared key order.
    let program = checked_program(
        "resource Enrollment at ^enrollments(studentId: string, courseId: string)\n    status: string\n\nfn enroll()\n    const id = Enrollment::Id(courseId: \"c\", studentId: \"s\")\n    ^enrollments(id).status = \"active\"\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::enroll", &[]).expect("enroll");
    let store = store.borrow();
    let bytes = store
        .read(&encode_path(&[
            PathSegment::Root("enrollments".into()),
            PathSegment::RecordKey(SavedKey::Str("s".into())),
            PathSegment::RecordKey(SavedKey::Str("c".into())),
            PathSegment::Field("status".into()),
        ]))
        .expect("present");
    assert_eq!(
        decode_value(bytes, ValueType::Str),
        Some(SavedValue::Str("active".into()))
    );
}

#[test]
fn whole_resource_read_through_an_identity() {
    // `var e: Enrollment = ^enrollments(id)` round-trips through an identity.
    let program = checked_program(
        "resource Enrollment at ^enrollments(studentId: string, courseId: string)\n    status: string\n\nfn statusOf(s: string, c: string): string\n    const id = Enrollment::Id(studentId: s, courseId: c)\n    var e: Enrollment = ^enrollments(id)\n    return e.status\n",
    );
    let store = RefCell::new(MemStore::new());
    store.borrow_mut().write(
        &encode_path(&[
            PathSegment::Root("enrollments".into()),
            PathSegment::RecordKey(SavedKey::Str("s".into())),
            PathSegment::RecordKey(SavedKey::Str("c".into())),
            PathSegment::Field("status".into()),
        ]),
        encode_value(&SavedValue::Str("active".into())).expect("encodes"),
    );
    let value = run_entry(
        &program,
        &store,
        "test::statusOf",
        &[Value::Str("s".into()), Value::Str("c".into())],
    )
    .expect("statusOf")
    .value;
    assert_eq!(value, Some(Value::Str("active".into())));
}

// --- Singleton resources end-to-end ---

/// A singleton resource (`Settings at ^settings`, no identity keys). Field
/// read/write address the root directly, and whole read/write materialize and
/// replace the root as a resource value.
const SETTINGS: &str = "\
resource Settings at ^settings
    theme: string
    required maxLoans: int

fn setTheme(t: string)
    ^settings.theme = t

fn theme(): string
    return ^settings.theme

fn snapshot(): Settings
    return ^settings

fn restore(s: Settings)
    ^settings = s
";

#[test]
fn singleton_field_read_and_write() {
    let program = checked_program(SETTINGS);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::setTheme",
        &[Value::Str("dark".into())],
    )
    .expect("setTheme");
    let value = run_entry(&program, &store, "test::theme", &[])
        .expect("theme")
        .value;
    assert_eq!(value, Some(Value::Str("dark".into())));
    // The field landed at `^settings.theme`, no record key in between.
    let store = store.borrow();
    let bytes = store
        .read(&encode_path(&[
            PathSegment::Root("settings".into()),
            PathSegment::Field("theme".into()),
        ]))
        .expect("present");
    assert_eq!(
        decode_value(bytes, ValueType::Str),
        Some(SavedValue::Str("dark".into()))
    );
}

#[test]
fn singleton_whole_read_and_write_round_trip() {
    let program = checked_program(SETTINGS);
    let store = RefCell::new(MemStore::new());
    // Seed the singleton's fields directly.
    store.borrow_mut().write(
        &encode_path(&[
            PathSegment::Root("settings".into()),
            PathSegment::Field("theme".into()),
        ]),
        encode_value(&SavedValue::Str("light".into())).expect("encodes"),
    );
    store.borrow_mut().write(
        &encode_path(&[
            PathSegment::Root("settings".into()),
            PathSegment::Field("maxLoans".into()),
        ]),
        encode_value(&SavedValue::Int(5)).expect("encodes"),
    );
    // A whole read materializes the singleton's present fields.
    let snapshot = run_entry(&program, &store, "test::snapshot", &[])
        .expect("snapshot")
        .value;
    assert_eq!(
        snapshot,
        Some(Value::Resource(vec![
            ("theme".into(), Value::Str("light".into())),
            ("maxLoans".into(), Value::Int(5)),
        ]))
    );
    // A whole write replaces it; read it back via the field reader.
    run_entry(
        &program,
        &store,
        "test::restore",
        &[Value::Resource(vec![
            ("theme".into(), Value::Str("solar".into())),
            ("maxLoans".into(), Value::Int(9)),
        ])],
    )
    .expect("restore");
    let value = run_entry(&program, &store, "test::theme", &[])
        .expect("theme")
        .value;
    assert_eq!(value, Some(Value::Str("solar".into())));
}

// --- Unkeyed-group field read/write through a saved path ---

/// A resource with an unkeyed nested group (`name { first; last }`). Its fields
/// are addressed `^patients(p).name.first` — a `.field` off a `.field` off the
/// record, with no keyed layer in between.
#[test]
fn a_whole_read_of_a_keyed_root_without_an_identity_is_rejected() {
    // `^books` is a keyed resource root, not a singleton; reading it whole without
    // an identity must error (run.type), not silently read the empty-identity path.
    let program = checked_program(
        "module test\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         pub fn read()\n\
         \x20   var b: Book = ^books\n",
    );
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::read", &[]);
    assert!(
        matches!(result, Err(ref e) if e.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn a_field_read_off_a_keyed_root_without_an_identity_is_rejected() {
    // `^books.title` addresses a keyed root with no identity; it must error rather
    // than read the wrong (identity-less) path.
    let program = checked_program(
        "module test\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         pub fn read(): string\n\
         \x20   return ^books.title\n",
    );
    let store = RefCell::new(MemStore::new());
    let result = run_entry(&program, &store, "test::read", &[]);
    assert!(
        matches!(result, Err(ref e) if e.code == RUN_TYPE),
        "{result:?}"
    );
}

const PATIENT_UNKEYED_GROUP: &str = "\
resource Patient at ^patients(id: int)
    mrn: string
    name
        first: string
        last: string

fn setName(id: int, f: string, l: string)
    ^patients(id).name.first = f
    ^patients(id).name.last = l

fn firstOf(id: int): string
    return ^patients(id).name.first

fn lastOf(id: int): string
    return ^patients(id).name.last
";

#[test]
fn unkeyed_group_field_write_then_read_round_trips() {
    let program = checked_program(PATIENT_UNKEYED_GROUP);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::setName",
        &[
            Value::Int(7),
            Value::Str("Terry".into()),
            Value::Str("Pratchett".into()),
        ],
    )
    .expect("setName");
    let read = |entry: &str| {
        run_entry(&program, &store, entry, &[Value::Int(7)])
            .expect("read")
            .value
    };
    assert_eq!(read("test::firstOf"), Some(Value::Str("Terry".into())));
    assert_eq!(read("test::lastOf"), Some(Value::Str("Pratchett".into())));
    // The field landed under the group layer `^patients(7).name.first`.
    let store = store.borrow();
    let bytes = store
        .read(&encode_path(&[
            PathSegment::Root("patients".into()),
            PathSegment::RecordKey(SavedKey::Int(7)),
            PathSegment::ChildLayer("name".into()),
            PathSegment::Field("first".into()),
        ]))
        .expect("present");
    assert_eq!(
        decode_value(bytes, ValueType::Str),
        Some(SavedValue::Str("Terry".into()))
    );
}

#[test]
fn an_absent_unkeyed_group_field_read_is_absent() {
    let program = checked_program(PATIENT_UNKEYED_GROUP);
    let store = RefCell::new(MemStore::new());
    let error = run_entry(&program, &store, "test::firstOf", &[Value::Int(1)]).unwrap_err();
    assert_eq!(error.code, RUN_ABSENT, "{error:?}");
}

// --- Unique-index identity reads ---

/// A book with a unique index on `isbn`. `register` stores the book, and
/// `titleByIsbn` reads the identity back from the unique-index lookup path and
/// uses it to address the record.
const BOOK_ISBN: &str = "\
resource Book at ^books(id: int)
    required title: string
    isbn: string

    index byIsbn(isbn) unique

fn register(id: int, t: string, isbn: string)
    ^books(id).title = t
    ^books(id).isbn = isbn

fn titleByIsbn(isbn: string): string
    const id: Book::Id = ^books.byIsbn(isbn)
    return ^books(id).title
";

#[test]
fn reads_an_identity_from_a_unique_index() {
    let program = checked_program(BOOK_ISBN);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::register",
        &[
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ],
    )
    .expect("register");
    let value = run_entry(
        &program,
        &store,
        "test::titleByIsbn",
        &[Value::Str("978-0".into())],
    )
    .expect("titleByIsbn")
    .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
}

#[test]
fn an_absent_unique_index_lookup_is_absent() {
    let program = checked_program(BOOK_ISBN);
    let store = RefCell::new(MemStore::new());
    let error = run_entry(
        &program,
        &store,
        "test::titleByIsbn",
        &[Value::Str("missing".into())],
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_ABSENT, "{error:?}");
}

/// A non-unique index in value position has no single identity to yield; the
/// runtime rejects it and points the reader at `keys(...)`.
const BOOK_SHELF_VALUE: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

fn firstOnShelf(shelf: string): Book::Id
    return ^books.byShelf(shelf)
";

#[test]
fn a_non_unique_index_in_value_position_is_rejected() {
    let program = checked_program(BOOK_SHELF_VALUE);
    let store = RefCell::new(MemStore::new());
    let error = run_entry(
        &program,
        &store,
        "test::firstOnShelf",
        &[Value::Str("fiction".into())],
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_UNSUPPORTED, "{error:?}");
    assert!(error.message.contains("keys("), "{error:?}");
}

// --- Composite-identity index traversal ---

/// A composite-identity resource indexed by status. The non-unique index ends
/// with both identity keys, so traversal must descend both levels per entry and
/// reconstruct the full `Enrollment::Id` (not just the first key component).
const ENROLLMENT_STATUS: &str = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string

    index byStatus(status, studentId, courseId)

fn enroll(s: string, c: string, st: string)
    const id = Enrollment::Id(studentId: s, courseId: c)
    ^enrollments(id).status = st

fn activeStatuses()
    for id in keys(^enrollments.byStatus(\"active\"))
        print(^enrollments(id).status)
";

#[test]
fn traverses_a_composite_identity_index() {
    let program = checked_program(ENROLLMENT_STATUS);
    let store = RefCell::new(MemStore::new());
    let enroll = |s: &str, c: &str, st: &str| {
        run_entry(
            &program,
            &store,
            "test::enroll",
            &[
                Value::Str(s.into()),
                Value::Str(c.into()),
                Value::Str(st.into()),
            ],
        )
        .expect("enroll");
    };
    enroll("student-1", "course-9", "active");
    enroll("student-2", "course-9", "active");
    enroll("student-3", "course-9", "dropped");

    // Each reconstructed identity addresses its record: every active enrollment
    // reads back `active`. Two such entries exist, in (studentId, courseId) order.
    let outcome = run_entry(&program, &store, "test::activeStatuses", &[]).expect("run");
    assert_eq!(outcome.output, "active\nactive\n");
}

// --- Unified saved-layer enumeration ---

/// Iterating a primary keyed root yields its record identities. `^books` is a
/// single-`int`-key root, so each identity is a bare `Value::Int` that re-addresses
/// the record.
const BOOK_PRIMARY: &str = "\
resource Book at ^books(id: int)
    required title: string

fn add(id: int, t: string)
    ^books(id).title = t

fn titles()
    for id in ^books
        print(^books(id).title)

fn ids()
    const all = keys(^books)
    for id in all
        print($\"{id}\")
";

#[test]
fn iterates_a_primary_keyed_root() {
    let program = checked_program(BOOK_PRIMARY);
    let store = RefCell::new(MemStore::new());
    let add = |id: i64, title: &str| {
        run_entry(
            &program,
            &store,
            "test::add",
            &[Value::Int(id), Value::Str(title.into())],
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    // Bare-root iteration yields ids in key order, each addressing its record.
    let outcome = run_entry(&program, &store, "test::titles", &[]).expect("run");
    assert_eq!(outcome.output, "Mort\nSourcery\n");
}

#[test]
fn keys_of_a_primary_root_materializes_a_sequence() {
    let program = checked_program(BOOK_PRIMARY);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::add",
        &[Value::Int(1), Value::Str("Mort".into())],
    )
    .expect("add");
    run_entry(
        &program,
        &store,
        "test::add",
        &[Value::Int(2), Value::Str("Sourcery".into())],
    )
    .expect("add");

    // `keys(^books)` is a value: a `Value::Sequence` the loop binds in turn.
    let outcome = run_entry(&program, &store, "test::ids", &[]).expect("run");
    assert_eq!(outcome.output, "1\n2\n");
}

#[test]
fn iterating_a_singleton_root_is_a_type_error() {
    // A keyless singleton has no identities to enumerate; iterating it is a
    // type error, not a silent empty loop.
    let program = checked_program(
        "resource Settings at ^settings\n    theme: string\n\nfn each()\n    for s in ^settings\n        print(\"x\")\n",
    );
    let store = RefCell::new(MemStore::new());
    let error = run_entry(&program, &store, "test::each", &[]).unwrap_err();
    assert_eq!(error.code, RUN_TYPE, "{error:?}");
}

/// Iterating a composite primary root reconstructs the full identity per record,
/// so `^enrollments(id)` re-addresses each one.
const ENROLLMENT_PRIMARY: &str = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string

fn enroll(s: string, c: string, st: string)
    const id = Enrollment::Id(studentId: s, courseId: c)
    ^enrollments(id).status = st

fn statuses()
    for id in ^enrollments
        print(^enrollments(id).status)
";

#[test]
fn iterates_a_composite_primary_root() {
    let program = checked_program(ENROLLMENT_PRIMARY);
    let store = RefCell::new(MemStore::new());
    let enroll = |s: &str, c: &str, st: &str| {
        run_entry(
            &program,
            &store,
            "test::enroll",
            &[
                Value::Str(s.into()),
                Value::Str(c.into()),
                Value::Str(st.into()),
            ],
        )
        .expect("enroll");
    };
    enroll("student-1", "course-9", "active");
    enroll("student-2", "course-1", "dropped");

    // Each reconstructed composite identity re-addresses its record.
    let outcome = run_entry(&program, &store, "test::statuses", &[]).expect("run");
    assert_eq!(outcome.output, "active\ndropped\n");
}

/// Iterating a sequence/keyed child layer yields the layer's keys.
const BOOK_TAGS: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags: sequence[string]

fn seed()
    ^books(1).title = \"Mort\"
    const a: int = append(^books(1).tags, \"fiction\")
    const b: int = append(^books(1).tags, \"funny\")

fn positions()
    for pos in ^books(1).tags
        print($\"{pos}\")

fn keysOf()
    for pos in keys(^books(1).tags)
        print($\"{pos}\")
";

#[test]
fn iterates_a_sequence_child_layer() {
    let program = checked_program(BOOK_TAGS);
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed");

    // Bare iteration over the layer yields its 1-based positions in key order.
    let outcome = run_entry(&program, &store, "test::positions", &[]).expect("run");
    assert_eq!(outcome.output, "1\n2\n");

    // `keys(^books(1).tags)` yields the same positions.
    let outcome = run_entry(&program, &store, "test::keysOf", &[]).expect("run");
    assert_eq!(outcome.output, "1\n2\n");
}

/// A keyed (non-sequence) child tree iterates its declared keys. (Seeded through
/// the store directly to keep the focus on iteration order.)
const PLAYER_SCORES: &str = "\
resource Game at ^games(id: int)
    scores(playerId: string): int

fn players()
    for p in ^games(1).scores
        print(p)
";

#[test]
fn iterates_a_keyed_child_tree() {
    let program = checked_program(PLAYER_SCORES);
    let store = RefCell::new(MemStore::new());
    let score = |player: &str, n: i64| {
        store.borrow_mut().write(
            &encode_path(&[
                PathSegment::Root("games".into()),
                PathSegment::RecordKey(SavedKey::Int(1)),
                PathSegment::ChildLayer("scores".into()),
                PathSegment::IndexKey(SavedKey::Str(player.into())),
            ]),
            encode_value(&SavedValue::Int(n)).expect("in-range value encodes"),
        );
    };
    score("bob", 7);
    score("alice", 10);

    // Keys iterate in sorted key order (alice before bob).
    let outcome = run_entry(&program, &store, "test::players", &[]).expect("run");
    assert_eq!(outcome.output, "alice\nbob\n");
}

#[test]
fn deleting_a_record_while_traversing_the_root_is_a_traversal_fault() {
    // `for id in ^books` traverses the `^books` identity layer; deleting a record
    // inside the loop changes that layer, which is a dynamic traversal fault even
    // when the checker cannot prove it.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\nfn clear()\n    for id in ^books\n        delete ^books(id)\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed");
    let faulted = run_entry(&program, &store, "test::clear", &[]);
    assert!(
        matches!(faulted, Err(ref error) if error.code == RUN_TRAVERSAL),
        "{faulted:?}"
    );
}

#[test]
fn appending_to_the_sequence_being_traversed_is_a_traversal_fault() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn seed()\n    ^books(1).title = \"a\"\n    const p: int = append(^books(1).tags, \"x\")\n\nfn grow()\n    for pos in ^books(1).tags\n        const p: int = append(^books(1).tags, \"y\")\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed");
    let faulted = run_entry(&program, &store, "test::grow", &[]);
    assert!(
        matches!(faulted, Err(ref error) if error.code == RUN_TRAVERSAL),
        "{faulted:?}"
    );
}

#[test]
fn collecting_keys_first_then_deleting_is_allowed() {
    // The documented safe pattern: snapshot the keys into a local, then iterate the
    // local and delete. The loop traverses a local value, so no traversal fault.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\nfn clear()\n    const ids = keys(^books)\n    for id in ids\n        delete ^books(id)\n\nfn remaining(): int\n    return count(^books)\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed");
    run_entry(&program, &store, "test::clear", &[]).expect("clear");
    // Every record was removed.
    assert_eq!(
        run_entry(&program, &store, "test::remaining", &[])
            .expect("count")
            .value,
        Some(Value::Int(0))
    );
}

#[test]
fn mutating_a_different_record_layer_while_traversing_is_allowed() {
    // Traversing `^books(1).tags` and appending to `^books(2).tags` touches a
    // different record's layer, so it is not a traversal fault.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n\nfn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n    const p: int = append(^books(1).tags, \"x\")\n\nfn copy()\n    for pos in ^books(1).tags\n        const p: int = append(^books(2).tags, \"y\")\n\nfn tags2(): int\n    return count(^books(2).tags)\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed");
    run_entry(&program, &store, "test::copy", &[]).expect("copy");
    assert_eq!(
        run_entry(&program, &store, "test::tags2", &[])
            .expect("count")
            .value,
        Some(Value::Int(1))
    );
}

/// `count(path)` over the four presence shapes builtins.md defines: a scalar
/// field, a child-bearing layer, and absent paths.
const BOOK_COUNT: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags: sequence[string]

fn seed()
    ^books(1).title = \"Mort\"
    const a: int = append(^books(1).tags, \"fiction\")
    const b: int = append(^books(1).tags, \"funny\")

fn countTitle(): int
    return count(^books(1).title)

fn countTags(): int
    return count(^books(1).tags)

fn countMissingField(): int
    return count(^books(1).subtitle)

fn countMissingTags(): int
    return count(^books(2).tags)
";

#[test]
fn count_reports_scalar_presence_and_child_counts() {
    let program = checked_program(BOOK_COUNT);
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed");

    let count = |entry: &str| {
        run_entry(&program, &store, entry, &[])
            .expect("count")
            .value
    };
    // A populated scalar field with no children counts as 1.
    assert_eq!(count("test::countTitle"), Some(Value::Int(1)));
    // A layer with two child entries counts its immediate children.
    assert_eq!(count("test::countTags"), Some(Value::Int(2)));
    // An absent field with no children counts as 0.
    assert_eq!(count("test::countMissingField"), Some(Value::Int(0)));
    // An absent layer (the record itself absent) counts as 0.
    assert_eq!(count("test::countMissingTags"), Some(Value::Int(0)));
}

#[test]
fn count_of_a_path_with_both_value_and_children_counts_children() {
    // builtins.md: when a path has BOTH a value and children, `count` returns the
    // number of immediate children, not children-plus-one.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags: sequence[string]\n\nfn n(): int\n    return count(^books(1).tags)\n",
    );
    let store = RefCell::new(MemStore::new());
    // Seed a value at `^books(1).tags` itself and two children below it.
    let tags = |extra: Option<SavedKey>| {
        let mut segments = vec![
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
            PathSegment::ChildLayer("tags".into()),
        ];
        if let Some(key) = extra {
            segments.push(PathSegment::IndexKey(key));
        }
        encode_path(&segments)
    };
    {
        let mut store = store.borrow_mut();
        store.write(
            &tags(None),
            encode_value(&SavedValue::Str("self".into())).expect("encodes"),
        );
        store.write(
            &tags(Some(SavedKey::Int(1))),
            encode_value(&SavedValue::Str("a".into())).expect("encodes"),
        );
        store.write(
            &tags(Some(SavedKey::Int(2))),
            encode_value(&SavedValue::Str("b".into())).expect("encodes"),
        );
    }
    assert_eq!(
        run_entry(&program, &store, "test::n", &[])
            .expect("run")
            .value,
        Some(Value::Int(2)),
    );
}

/// `count` over a declared index branch returns the number of entries under that
/// branch, exactly as `keys(...)` over the same branch would yield. The branch is
/// a non-unique index so several entries share one query key.
const BOOK_COUNT_INDEX: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string
    tags: sequence[string]

    index byShelf(shelf, id)

fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

fn tag(id: int, t: string): int
    return append(^books(id).tags, t)

fn countBranch(shelf: string): int
    return count(^books.byShelf(shelf))

fn keysBranch(shelf: string): int
    var c = 0
    for id in keys(^books.byShelf(shelf))
        c = c + 1
    return c

fn countRoot(): int
    return count(^books)

fn countLayer(id: int): int
    return count(^books(id).tags)

fn countScalar(id: int): int
    return count(^books(id).title)

fn countRecord(id: int): int
    return count(^books(id))
";

#[test]
fn count_over_an_index_branch_matches_branch_entry_count() {
    let program = checked_program(BOOK_COUNT_INDEX);
    let store = RefCell::new(MemStore::new());
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &program,
            &store,
            "test::add",
            &[
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ],
        )
        .expect("add");
    };
    add(1, "Mort", "fiction");
    add(2, "Sourcery", "fiction");
    add(3, "Guards", "history");

    let call = |entry: &str, args: &[Value]| {
        run_entry(&program, &store, entry, args)
            .expect("count")
            .value
    };
    // Two tags on book 1, so its keyed/sequence layer has two entries.
    call("test::tag", &[Value::Int(1), Value::Str("a".into())]);
    call("test::tag", &[Value::Int(1), Value::Str("b".into())]);

    // `count(^books.byShelf(shelf))` returns the entry count under that index
    // branch, matching `keys(...)` over the same branch.
    assert_eq!(
        call("test::countBranch", &[Value::Str("fiction".into())]),
        Some(Value::Int(2))
    );
    assert_eq!(
        call("test::keysBranch", &[Value::Str("fiction".into())]),
        Some(Value::Int(2))
    );
    assert_eq!(
        call("test::countBranch", &[Value::Str("history".into())]),
        Some(Value::Int(1))
    );
    assert_eq!(
        call("test::keysBranch", &[Value::Str("history".into())]),
        Some(Value::Int(1))
    );
    // An empty branch counts as zero, like `keys(...)` of it.
    assert_eq!(
        call("test::countBranch", &[Value::Str("romance".into())]),
        Some(Value::Int(0))
    );
    assert_eq!(
        call("test::keysBranch", &[Value::Str("romance".into())]),
        Some(Value::Int(0))
    );

    // The previously-correct count shapes stay byte-identical: a keyed/sequence
    // layer counts its entries, a scalar counts as 1, and a whole record counts
    // its populated immediate children. These all keep the read/child-keys path.
    assert_eq!(
        call("test::countLayer", &[Value::Int(1)]),
        Some(Value::Int(2))
    );
    assert_eq!(
        call("test::countLayer", &[Value::Int(3)]),
        Some(Value::Int(0))
    );
    assert_eq!(
        call("test::countScalar", &[Value::Int(1)]),
        Some(Value::Int(1))
    );
    assert!(matches!(call("test::countRecord", &[Value::Int(1)]), Some(Value::Int(n)) if n >= 1));
    // A primary root keeps its existing read/child-keys count: it walks the root's
    // immediate children, which includes the declared `byShelf` index node beside
    // the three record keys. This fix does not touch the primary-root path.
    assert_eq!(call("test::countRoot", &[]), Some(Value::Int(4)));
}

/// `values`/`entries` over a primary root materialize whole records; over a
/// keyed/sequence layer they materialize each entry's value. `entries` feeds the
/// two-name `for id, x in entries(...)` binding.
const BOOK_VALUES: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags: sequence[string]

fn add(id: int, t: string)
    ^books(id).title = t

fn tag(id: int, t: string): int
    return append(^books(id).tags, t)

fn titles()
    for book in values(^books)
        print(book.title)

fn idsAndTitles()
    for id, book in entries(^books)
        print($\"{id}: {book.title}\")

fn tagValues(id: int)
    for tag in values(^books(id).tags)
        print(tag)

fn tagEntries(id: int)
    for pos, tag in entries(^books(id).tags)
        print($\"{pos}={tag}\")
";

#[test]
fn values_and_entries_materialize_whole_records_over_a_primary_root() {
    let program = checked_program(BOOK_VALUES);
    let store = RefCell::new(MemStore::new());
    let add = |id: i64, t: &str| {
        run_entry(
            &program,
            &store,
            "test::add",
            &[Value::Int(id), Value::Str(t.into())],
        )
        .expect("add");
    };
    add(2, "Sourcery");
    add(1, "Mort");

    // `values(^books)` yields each whole record, in key order, with field access.
    let titles = run_entry(&program, &store, "test::titles", &[]).expect("run");
    assert_eq!(titles.output, "Mort\nSourcery\n");

    // `entries(^books)` binds the identity and the materialized record together.
    let pairs = run_entry(&program, &store, "test::idsAndTitles", &[]).expect("run");
    assert_eq!(pairs.output, "1: Mort\n2: Sourcery\n");
}

#[test]
fn values_and_entries_materialize_entries_over_a_keyed_layer() {
    let program = checked_program(BOOK_VALUES);
    let store = RefCell::new(MemStore::new());
    run_entry(
        &program,
        &store,
        "test::add",
        &[Value::Int(1), Value::Str("Mort".into())],
    )
    .expect("add");
    run_entry(
        &program,
        &store,
        "test::tag",
        &[Value::Int(1), Value::Str("fiction".into())],
    )
    .expect("tag");
    run_entry(
        &program,
        &store,
        "test::tag",
        &[Value::Int(1), Value::Str("funny".into())],
    )
    .expect("tag");

    // `values(^books(1).tags)` yields each leaf value in key order.
    let values = run_entry(&program, &store, "test::tagValues", &[Value::Int(1)]).expect("run");
    assert_eq!(values.output, "fiction\nfunny\n");

    // `entries(...)` binds each 1-based position to its leaf value.
    let entries = run_entry(&program, &store, "test::tagEntries", &[Value::Int(1)]).expect("run");
    assert_eq!(entries.output, "1=fiction\n2=funny\n");
}

const BOOK_ISBN_SAVE: &str = "\
module test
resource Book at ^books(id: int)
    isbn: string
    index byIsbn(isbn) unique
fn save(i: int, code: string)
    ^books(Book::Id(i)).isbn = code
";

#[test]
fn a_recoverable_write_fault_is_catchable_across_a_call_boundary() {
    // A write fault raised in a CALLED function must be catchable by the caller's
    // try/catch (the transaction-recovery contract), not only within the same frame.
    let program = checked_program(&format!(
        "{BOOK_ISBN_SAVE}\
         pub fn run(): string\n\
         \x20   save(1, \"x\")\n\
         \x20   try\n\
         \x20       save(2, \"x\")\n\
         \x20       return \"uncaught\"\n\
         \x20   catch e: Error\n\
         \x20       return e.code\n"
    ));
    let store = RefCell::new(MemStore::new());
    let value = run_entry(&program, &store, "test::run", &[])
        .expect("run")
        .value;
    assert_eq!(value, Some(Value::Str("write.unique_conflict".into())));
}

#[test]
fn an_uncaught_cross_boundary_write_fault_keeps_its_dotted_code() {
    // Crossing a call boundary must not collapse an uncaught fault to
    // run.uncaught_error: it surfaces with its own dotted code (and exit code).
    let program = checked_program(&format!(
        "{BOOK_ISBN_SAVE}\
         pub fn run()\n\
         \x20   save(1, \"x\")\n\
         \x20   save(2, \"x\")\n"
    ));
    let store = RefCell::new(MemStore::new());
    let error = run_entry(&program, &store, "test::run", &[]).unwrap_err();
    assert_eq!(error.code, "write.unique_conflict", "{error:?}");
}

const PATIENT_SPARSE_GROUP: &str = "\
module test
resource Patient at ^patients(id: string)
    name
        first: string
        last: string
";

#[test]
fn deleting_a_sparse_field_inside_an_unkeyed_group_is_allowed() {
    // Field delete descends unkeyed-group layers. A REQUIRED field inside an
    // unkeyed group cannot be declared today (schema.required_in_unkeyed_group
    // rejects it at compile time), so the nested required-delete guard waits on
    // group materialization, which lifts that rejection.
    let program = checked_program(&format!(
        "{PATIENT_SPARSE_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name.last\n"
    ));
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::drop", &[]).expect("sparse group-field delete is a no-op");
}

// --- Maintenance mode & managed-root protection ---

/// A two-key books program with an index, reused by the maintenance tests below:
/// it can seed records, drop the whole `^books` root, and count remaining records
/// and index entries so a root drop's effect is observable.
const MAINTENANCE_BOOKS: &str = "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\nfn seed()\n    ^books(1).title = \"Mort\"\n    ^books(1).shelf = \"fiction\"\n    ^books(2).title = \"Guards\"\n    ^books(2).shelf = \"fiction\"\n\nfn drop_root()\n    delete ^books\n\nfn record_count(): int\n    var c = 0\n    for id in ^books\n        c = c + 1\n    return c\n\nfn shelf_count(s: string): int\n    var c = 0\n    for id in keys(^books.byShelf(s))\n        c = c + 1\n    return c\n";

#[test]
fn deleting_a_whole_root_without_maintenance_is_rejected() {
    // `delete ^books` on a keyed root is maintenance work; with no maintenance
    // capability the run is rejected with `write.requires_maintenance`.
    let program = checked_program(MAINTENANCE_BOOKS);
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed");
    let result = run_entry(&program, &store, "test::drop_root", &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == "write.requires_maintenance"),
        "{result:?}"
    );
    // The records still exist: the rejected delete did not touch the store.
    assert_eq!(
        run_entry(&program, &store, "test::record_count", &[])
            .expect("count")
            .value,
        Some(Value::Int(2))
    );
}

#[test]
fn deleting_a_whole_root_under_maintenance_drops_records_and_indexes() {
    // With the maintenance capability, `delete ^books` drops the entire managed
    // root subtree: no records and no index entries remain.
    let program = checked_program(MAINTENANCE_BOOKS);
    let store = RefCell::new(MemStore::new());
    let host = Host::new().with_maintenance();
    run_entry_with_host(&program, &store, &host, "test::seed", &[]).expect("seed");
    run_entry_with_host(&program, &store, &host, "test::drop_root", &[]).expect("drop root");
    assert_eq!(
        run_entry_with_host(&program, &store, &host, "test::record_count", &[])
            .expect("count")
            .value,
        Some(Value::Int(0)),
        "no records remain after the root drop"
    );
    assert_eq!(
        run_entry_with_host(
            &program,
            &store,
            &host,
            "test::shelf_count",
            &[Value::Str("fiction".into())]
        )
        .expect("count")
        .value,
        Some(Value::Int(0)),
        "no index entries remain after the root drop"
    );
}

#[test]
fn whole_identity_delete_stays_ungated_under_no_maintenance() {
    // `delete ^books(1)` is ordinary whole-identity work: it must still succeed
    // with no maintenance capability, leaving the sibling record in place.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed()\n    ^books(1).title = \"Mort\"\n    ^books(2).title = \"Guards\"\n\nfn drop_one()\n    delete ^books(1)\n\nfn record_count(): int\n    var c = 0\n    for id in ^books\n        c = c + 1\n    return c\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[]).expect("seed");
    run_entry(&program, &store, "test::drop_one", &[]).expect("ordinary identity delete");
    assert_eq!(
        run_entry(&program, &store, "test::record_count", &[])
            .expect("count")
            .value,
        Some(Value::Int(1)),
        "the sibling record survives an ordinary identity delete"
    );
}

#[test]
fn deleting_a_required_field_under_maintenance_succeeds() {
    // A required-field delete is rejected without maintenance (existing behavior),
    // but a maintenance run lifts the guard and actually removes the field.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn drop_title(id: int)\n    delete ^books(id).title\n\nfn has_title(id: int): bool\n    return exists(^books(id).title)\n",
    );
    let store = RefCell::new(MemStore::new());
    let host = Host::new().with_maintenance();
    run_entry_with_host(&program, &store, &host, "test::seed", &[Value::Int(1)]).expect("seed");
    run_entry_with_host(
        &program,
        &store,
        &host,
        "test::drop_title",
        &[Value::Int(1)],
    )
    .expect("maintenance lifts the required-field guard");
    assert_eq!(
        run_entry_with_host(&program, &store, &host, "test::has_title", &[Value::Int(1)])
            .expect("read")
            .value,
        Some(Value::Bool(false)),
        "the required field is gone after a maintenance delete"
    );
}

#[test]
fn raw_quoted_segment_without_maintenance_is_rejected() {
    // A quoted/raw segment under a managed root is gated: without maintenance it
    // raises `write.raw_requires_maintenance`, distinct from an unknown-field typo.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn raw_write(id: int)\n    ^books(id).\"old-title\" = \"legacy\"\n",
    );
    let store = RefCell::new(MemStore::new());
    run_entry(&program, &store, "test::seed", &[Value::Int(1)]).expect("seed");
    let result = run_entry(&program, &store, "test::raw_write", &[Value::Int(1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == "write.raw_requires_maintenance"),
        "{result:?}"
    );
}

#[test]
fn raw_quoted_segment_under_maintenance_round_trips() {
    // Under maintenance, a quoted/raw segment lowers to a raw backend write and
    // read at the literal segment, bypassing the schema's declared fields.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn seed(id: int)\n    ^books(id).title = \"Mort\"\n\nfn raw_write(id: int, v: string)\n    ^books(id).\"old-title\" = v\n\nfn raw_read(id: int): string\n    return ^books(id).\"old-title\"\n",
    );
    let store = RefCell::new(MemStore::new());
    let host = Host::new().with_maintenance();
    run_entry_with_host(&program, &store, &host, "test::seed", &[Value::Int(1)]).expect("seed");
    run_entry_with_host(
        &program,
        &store,
        &host,
        "test::raw_write",
        &[Value::Int(1), Value::Str("legacy".into())],
    )
    .expect("raw write under maintenance");
    assert_eq!(
        run_entry_with_host(&program, &store, &host, "test::raw_read", &[Value::Int(1)])
            .expect("raw read")
            .value,
        Some(Value::Str("legacy".into())),
        "the raw literal segment round-trips under maintenance"
    );
}

#[test]
fn a_raw_segment_write_of_a_non_string_is_rejected() {
    // Raw segments are an untyped text boundary: they read back as text, so a raw
    // write takes a string. A non-string scalar is rejected (run.type) rather than
    // stored as bytes the raw read could never return — keeping the round-trip
    // symmetric.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn raw_write(id: int, n: int)\n    ^books(id).\"count\" = n\n",
    );
    let store = RefCell::new(MemStore::new());
    let host = Host::new().with_maintenance();
    let error = run_entry_with_host(
        &program,
        &store,
        &host,
        "test::raw_write",
        &[Value::Int(1), Value::Int(5)],
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_TYPE, "{error:?}");
}

#[test]
fn unquoted_undeclared_field_stays_unknown_field_even_under_maintenance() {
    // Maintenance grants RAW (quoted) access only; an unquoted undeclared field is
    // still a typo, so it stays `write.unknown_field` even with maintenance on.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\nfn typo(id: int)\n    ^books(id).nope = \"x\"\n",
    );
    let store = RefCell::new(MemStore::new());
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(&program, &store, &host, "test::typo", &[Value::Int(1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == "write.unknown_field"),
        "{result:?}"
    );
}

#[test]
fn classify_saved_path_distinguishes_fields_layers_indexes_and_orphans() {
    // A resource with a top-level field, a keyed-leaf layer, a nested group
    // field, and an index covers every classification the inspector reports.
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
         \x20\x20\x20\x20shelf: string\n\
         \x20\x20\x20\x20tags(pos: int): string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \n\
         \x20\x20\x20\x20index byShelf(shelf, id)\n",
    );

    let field = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("title".into()),
    ];
    assert_eq!(
        classify_saved_path(&program, &field),
        SavedPathClass::Scalar(ValueType::Str)
    );

    let leaf_layer = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("tags".into()),
        PathSegment::IndexKey(SavedKey::Int(0)),
    ];
    assert_eq!(
        classify_saved_path(&program, &leaf_layer),
        SavedPathClass::Scalar(ValueType::Str)
    );

    let nested = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("versions".into()),
        PathSegment::IndexKey(SavedKey::Int(2)),
        PathSegment::Field("note".into()),
    ];
    assert_eq!(
        classify_saved_path(&program, &nested),
        SavedPathClass::Scalar(ValueType::Str)
    );

    let index_marker = vec![
        PathSegment::Root("books".into()),
        PathSegment::Field("byShelf".into()),
        PathSegment::IndexKey(SavedKey::Str("A".into())),
        PathSegment::IndexKey(SavedKey::Int(1)),
    ];
    assert_eq!(
        classify_saved_path(&program, &index_marker),
        SavedPathClass::IndexMarker
    );

    // Data under an unknown root, or naming a field the schema does not declare,
    // is an orphan.
    let unknown_root = vec![
        PathSegment::Root("ghosts".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("title".into()),
    ];
    assert_eq!(
        classify_saved_path(&program, &unknown_root),
        SavedPathClass::Orphan
    );

    let unknown_field = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("nope".into()),
    ];
    assert_eq!(
        classify_saved_path(&program, &unknown_field),
        SavedPathClass::Orphan
    );
}
