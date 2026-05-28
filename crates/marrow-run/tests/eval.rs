//! Evaluate pure scalar functions: arithmetic, comparison, logical operators,
//! locals, and conditionals over integer and boolean values.

use std::cell::RefCell;

use marrow_check::{CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, MarrowType};
use marrow_run::{
    RUN_ABSENT, RUN_DIVIDE_BY_ZERO, RUN_NO_ENCLOSING_LOOP, RUN_NO_VALUE, RUN_OVERFLOW, RUN_TYPE,
    RUN_UNBOUND_NAME, RUN_UNKNOWN_FUNCTION, RUN_UNSUPPORTED, RunOutput, Value, evaluate_function,
    run_entry,
};
use marrow_schema::compile_resource;
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, encode_path};
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
    let f = function("fn f(a: int)\n    let x = a + 1\n");
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
    let f = function("fn f(): int\n    let x = 1\n    x = 2\n    return x\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
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
    // `let x = 1` inside the if-block shadows only within that block; after it,
    // the outer `x` (99) is what `return x` sees.
    let f = function("fn f(): int\n    let x = 99\n    if true\n        let x = 1\n    return x\n");
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
        "fn note(n: int)\n    let doubled = n + n\n\nfn caller(): int\n    note(3)\n    return 2\n",
    );
    assert_eq!(run(&program, "test::caller", &[]), Ok(Some(Value::Int(2))));
}

#[test]
fn using_a_void_call_as_a_value_is_rejected() {
    let program = checked_program(
        "fn note(n: int)\n    let doubled = n + n\n\nfn caller(): int\n    return note(3)\n",
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
        "resource Book at ^books(id: int)\n    required title: string\n\nfn risky(id: int)\n    transaction\n        ^books(id).title = \"staged\"\n        let x = 1 / 0\n\nfn has_book(id: int): bool\n    return exists(^books(id))\n",
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
