//! Evaluate pure scalar functions: arithmetic, comparison, logical operators,
//! locals, and conditionals over integer and boolean values.

use std::cell::RefCell;

use marrow_check::{CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, MarrowType};
use marrow_run::{
    Host, RUN_ABSENT, RUN_ASSERT, RUN_CAPABILITY, RUN_DIVIDE_BY_ZERO, RUN_NO_ENCLOSING_LOOP,
    RUN_NO_VALUE, RUN_OVERFLOW, RUN_TYPE, RUN_UNBOUND_NAME, RUN_UNKNOWN_FUNCTION, RUN_UNSUPPORTED,
    RunOutput, Value, evaluate_function, run_entry, run_entry_with_host,
};
use marrow_schema::compile_resource;
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, encode_path};
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
            imports: Vec::new(),
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
        encode_value(&SavedValue::Int(1)),
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
fn detects_integer_overflow() {
    let f = function("fn f(a: int): int\n    return a * a\n");
    let result = evaluate_function(&f, &[Value::Int(i64::MAX)]);
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
    // A decimal value is not yet evaluable in this slice.
    let f = function("fn f(): decimal\n    return 1.5\n");
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
    let f = function("fn f(a: int, b: int): int\n    return a / b\n");
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
    // The source string contains a backslash escape; decoding is a later slice.
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
        encode_value(&SavedValue::Str(title.into())),
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
        encode_value(&SavedValue::Str("A Discworld Novel".into())),
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
            encode_value(&SavedValue::Str("t".into())),
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
        encode_value(&SavedValue::Str(value.into())),
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
    let host = Host::with_clock(1_000_000_000);
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
    let host = Host::with_clock(1_700_000_000_000_000_000); // 2023-11-14T22:13:20Z
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
    let host = Host::with_clock(1_700_000_000_000_000_000);
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
