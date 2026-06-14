use crate::support;
use std::cell::RefCell;
use std::rc::Rc;

use marrow_run::{CheckedEntryCall, Host, Value, run_entry, run_entry_with_host};
use marrow_store::tree::TreeStore;
use support::checked_program;

#[test]
fn text_args_decode_scalars_and_keep_string_remainder_raw() {
    let program = checked_program(
        "pub fn main(n: int, ok: bool, label: string): int\n\
         \x20   print(label)\n\
         \x20   if ok\n\
         \x20       return n\n\
         \x20   return 0\n",
    );
    let call = CheckedEntryCall::from_text_args(
        &program,
        "test::main",
        &[("n", "7"), ("ok", "true"), ("label", "a=b")],
    )
    .expect("entry args decode");
    let store = TreeStore::memory();
    let mut output = String::new();

    let result = run_entry(&store, &call, &mut output).expect("run entry");

    assert_eq!(output, "a=b\n");
    assert_eq!(result.value, Some(Value::Int(7)));
}

#[test]
fn repeated_text_args_collect_scalar_sequences_in_argv_order() {
    let program = checked_program(
        "pub fn sum(xs: sequence[int]): int\n\
         \x20   var total = 0\n\
         \x20   for x in xs\n\
         \x20       total = total + x\n\
         \x20   return total\n",
    );
    let call = CheckedEntryCall::from_text_args(
        &program,
        "test::sum",
        &[("xs", "4"), ("xs", "5"), ("xs", "6")],
    )
    .expect("entry args decode");
    let store = TreeStore::memory();
    let mut output = String::new();

    let result = run_entry(&store, &call, &mut output).expect("run entry");

    assert_eq!(result.value, Some(Value::Int(15)));

    let empty = CheckedEntryCall::from_text_args(&program, "test::sum", &[("xs", "[]")])
        .expect("empty sequence decodes");
    let result = run_entry(&store, &empty, &mut output).expect("run entry");
    assert_eq!(result.value, Some(Value::Int(0)));
}

#[test]
fn text_args_decode_language_scalar_literals_for_full_supported_surface() {
    let program = checked_program(
        "pub fn check(payload: bytes, day: date, moment: instant, span: duration, amount: decimal, ok: bool, n: int, label: string): int\n\
         \x20   if payload == b\"mw\" and day == date(\"2026-01-02\") and moment == instant(\"2026-01-02T03:04:05Z\") and span == 2.hours and amount == 1.0 and ok and label == \"a=b\"\n\
         \x20       return n\n\
         \x20   return 0\n",
    );
    let call = CheckedEntryCall::from_text_args(
        &program,
        "test::check",
        &[
            ("payload", "b\"mw\""),
            ("day", "date(\"2026-01-02\")"),
            ("moment", "instant(\"2026-01-02T03:04:05Z\")"),
            ("span", "2.hours"),
            ("amount", "1.0"),
            ("ok", "true"),
            ("n", "7"),
            ("label", "a=b"),
        ],
    )
    .expect("language scalar literals decode");
    let store = TreeStore::memory();
    let mut output = String::new();

    let result = run_entry(&store, &call, &mut output).expect("run entry");

    assert_eq!(result.value, Some(Value::Int(7)));
}

#[test]
fn text_args_reject_storage_encodings_and_hostile_scalar_literals() {
    let program = checked_program(
        "pub fn check(payload: bytes, span: duration, amount: decimal, n: int, ok: bool, day: date, moment: instant): int\n\
         \x20   return 1\n",
    );
    for (name, text) in [
        ("payload", "bXc="),
        ("span", "PT7200S"),
        ("amount", "1.0.0"),
        ("amount", "99999999999999999999999999999999999.0"),
        ("n", "7.0"),
        ("n", "9223372036854775808"),
        ("ok", "True"),
        ("ok", "1"),
        ("day", "2026-01-02"),
        ("day", "date(\"2026-99-99\")"),
        ("moment", "2026-01-02T03:04:05Z"),
        ("moment", "instant(\"not-an-instant\")"),
    ] {
        let payload = if name == "payload" { text } else { "b\"mw\"" };
        let span = if name == "span" { text } else { "2.hours" };
        let amount = if name == "amount" { text } else { "1.0" };
        let n = if name == "n" { text } else { "7" };
        let ok = if name == "ok" { text } else { "true" };
        let day = if name == "day" {
            text
        } else {
            "date(\"2026-01-02\")"
        };
        let moment = if name == "moment" {
            text
        } else {
            "instant(\"2026-01-02T03:04:05Z\")"
        };
        let error = CheckedEntryCall::from_text_args(
            &program,
            "test::check",
            &[
                ("payload", payload),
                ("span", span),
                ("amount", amount),
                ("n", n),
                ("ok", ok),
                ("day", day),
                ("moment", moment),
            ],
        )
        .expect_err("hostile scalar arg should fail");

        assert_eq!(error.code, "run.entry_argument", "{name}={text}: {error:?}");
    }
}

#[test]
fn text_args_reject_scalar_conversion_calls() {
    let program = checked_program(
        "resource Blob\n\
         \x20   label: string\n\
         store ^blobs(hash: bytes): Blob\n\
         \n\
         pub fn scalars(payload: bytes, span: duration, n: int): int\n\
         \x20   return 1\n\
         \n\
         pub fn collect(xs: sequence[int]): int\n\
         \x20   var total = 0\n\
         \x20   for x in xs\n\
         \x20       total = total + x\n\
         \x20   return total\n\
         \n\
         pub fn identity(blob: Id(^blobs)): int\n\
         \x20   return 1\n",
    );

    for (entry, args) in [
        ("test::scalars", vec![("payload", "bytes(\"mw\")")]),
        ("test::scalars", vec![("span", "duration(\"PT7200S\")")]),
        ("test::scalars", vec![("n", "int(\"7\")")]),
        ("test::collect", vec![("xs", "int(\"7\")")]),
        ("test::identity", vec![("blob", "bytes(\"mw\")")]),
    ] {
        let error = CheckedEntryCall::from_text_args(&program, entry, &args)
            .expect_err("scalar conversion calls are outside the entry argv grammar");

        assert_eq!(error.code, "run.entry_argument", "{entry} {args:?}");
    }
}

#[test]
fn text_args_decode_scalar_sequence_elements_as_language_literals() {
    let program = checked_program(
        "pub fn check(spans: sequence[duration]): int\n\
         \x20   var total = 0.hours\n\
         \x20   for span in spans\n\
         \x20       total = total + span\n\
         \x20   if total == 3.hours\n\
         \x20       return 1\n\
         \x20   return 0\n",
    );
    let call = CheckedEntryCall::from_text_args(
        &program,
        "test::check",
        &[("spans", "1.hours"), ("spans", "2.hours")],
    )
    .expect("sequence elements use literal grammar");
    let store = TreeStore::memory();
    let mut output = String::new();

    let result = run_entry(&store, &call, &mut output).expect("run entry");

    assert_eq!(result.value, Some(Value::Int(1)));
}

#[test]
fn text_args_reject_sequence_elements_outside_scalar_or_enum_surface() {
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\n\
         store ^authors(id: int): Author\n\
         \n\
         pub fn unsupported(ids: sequence[Id(^authors)]): int\n\
         \x20   return 0\n",
    );

    let error = CheckedEntryCall::from_text_args(&program, "test::unsupported", &[("ids", "7")])
        .expect_err("identity sequence args are outside the argv surface");

    assert_eq!(error.code, "run.entry_argument");
}

#[test]
fn text_args_decode_enum_members_by_spelling_not_ordinals() {
    let program = checked_program(
        "enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         \n\
         pub fn label(status: Status): string\n\
         \x20   if status == Status::archived\n\
         \x20       return \"archived\"\n\
         \x20   return \"active\"\n",
    );
    let call = CheckedEntryCall::from_text_args(&program, "test::label", &[("status", "archived")])
        .expect("enum arg decodes by member spelling");
    let store = TreeStore::memory();
    let mut output = String::new();

    let result = run_entry(&store, &call, &mut output).expect("run entry");

    assert_eq!(result.value, Some(Value::Str("archived".to_string())));
    assert!(
        CheckedEntryCall::from_text_args(&program, "test::label", &[("status", "1")]).is_err(),
        "source-order ordinals must not decode as enum args"
    );
}

#[test]
fn text_args_decode_single_key_id_and_reject_resource_params() {
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\n\
         store ^authors(id: int): Author\n\
         resource Book\n\
         \x20   title: string\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn authorId(author: Id(^authors)): int\n\
         \x20   return 42\n\
         \n\
         pub fn unsupported(book: Book): int\n\
         \x20   return 0\n",
    );

    let call = CheckedEntryCall::from_text_args(&program, "test::authorId", &[("author", "42")])
        .expect("single-key identity decodes");
    let store = TreeStore::memory();
    let mut output = String::new();
    let result = run_entry(&store, &call, &mut output).expect("run entry");
    assert_eq!(result.value, Some(Value::Int(42)));

    let error =
        CheckedEntryCall::from_text_args(&program, "test::unsupported", &[("book", "anything")])
            .expect_err("resource params are outside the entry argv surface");
    assert_eq!(error.code, "run.entry_argument");
}

#[test]
fn text_args_decode_identity_keys_with_language_scalar_literals() {
    let program = checked_program(
        "resource Blob\n\
         \x20   label: string\n\
         store ^blobs(hash: bytes): Blob\n\
         \n\
         pub fn accept(blob: Id(^blobs)): int\n\
         \x20   return 1\n",
    );

    let call = CheckedEntryCall::from_text_args(&program, "test::accept", &[("blob", "b\"mw\"")])
        .expect("bytes identity keys use bytes literal grammar");
    let store = TreeStore::memory();
    let mut output = String::new();
    let result = run_entry(&store, &call, &mut output).expect("run entry");
    assert_eq!(result.value, Some(Value::Int(1)));

    let error = CheckedEntryCall::from_text_args(&program, "test::accept", &[("blob", "bXc=")])
        .expect_err("base64 identity keys are outside the arg grammar");
    assert_eq!(error.code, "run.entry_argument");
}

#[test]
fn typed_entry_call_accepts_checked_identity_values() {
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\n\
         store ^authors(id: int): Author\n\
         \n\
         pub fn make(): Id(^authors)\n\
         \x20   return Id(^authors, 7)\n\
         \n\
         pub fn accept(author: Id(^authors)): int\n\
         \x20   return 1\n",
    );
    let make = CheckedEntryCall::new(&program, "test::make", Vec::new()).expect("make call");
    let store = TreeStore::memory();
    let mut output = String::new();
    let identity = run_entry(&store, &make, &mut output)
        .expect("make identity")
        .value
        .expect("identity return");

    let accept = CheckedEntryCall::new(&program, "test::accept", vec![identity])
        .expect("typed identity arg canonicalizes");
    let result = run_entry(&store, &accept, &mut output).expect("accept identity");

    assert_eq!(result.value, Some(Value::Int(1)));
}

#[test]
fn host_output_sink_receives_print_output() {
    let program = checked_program("pub fn main()\n    print(\"from host\")\n");
    let call = CheckedEntryCall::new(&program, "test::main", Vec::new()).expect("entry call");
    let store = TreeStore::memory();
    let host_output = Rc::new(RefCell::new(String::new()));
    let host = Host::new().with_output_sink(Rc::clone(&host_output));
    let mut fallback_output = String::new();

    run_entry_with_host(&store, &host, &call, &mut fallback_output).expect("run entry");

    assert_eq!(host_output.borrow().as_str(), "from host\n");
    assert_eq!(fallback_output, "");
}

#[test]
fn composite_identity_params_name_the_wrapper_entry_pattern() {
    let program = checked_program(
        "resource Enrollment\n\
         \x20   status: string\n\
         store ^enrollments(student: string, course: string): Enrollment\n\
         \n\
         pub fn mark(id: Id(^enrollments)): string\n\
         \x20   return \"unused\"\n",
    );

    let error = CheckedEntryCall::from_text_args(&program, "test::mark", &[("id", "student-1")])
        .expect_err("composite identity params are excluded");

    assert_eq!(error.code, "run.entry_argument");
    assert!(
        error.message.contains("wrapper entry"),
        "message should name the wrapper-entry pattern: {}",
        error.message
    );
}
