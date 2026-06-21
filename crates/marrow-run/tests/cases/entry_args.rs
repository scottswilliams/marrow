use crate::support;
use std::cell::RefCell;
use std::rc::Rc;

use marrow_run::{
    CheckedEntryCall, EntryArgument, EntryArgumentJsonErrorKind, EntryArgumentShape,
    EntryArgumentValue, EntryDescriptor, EntryInvocation, EntryParameter, EntryScalarArgument,
    Host, RUN_ENTRY_ARGUMENT, Value, entry_argument_json_schema, entry_arguments_from_json,
    run_entry, run_entry_with_host,
};
use marrow_store::Decimal;
use marrow_store::tree::TreeStore;
use marrow_store::value::ScalarType;
use serde_json::json;
use support::{checked_program, checked_program_modules};

fn entry_parameter<'a>(descriptor: &'a EntryDescriptor, name: &str) -> &'a EntryParameter {
    descriptor
        .parameters
        .iter()
        .find(|param| param.name == name)
        .expect("entry parameter")
}

fn protocol_invocation(
    descriptor: &EntryDescriptor,
    arguments: Vec<EntryArgument>,
) -> EntryInvocation {
    EntryInvocation {
        identity: descriptor.identity.clone(),
        arguments,
    }
}

#[test]
fn text_arg_instant_accepts_standard_rfc3339_and_normalizes() {
    // A CLI `--arg t=instant("...")` shares the in-language constructor's wider
    // standard RFC-3339 input surface: a trailing-zero fraction and a numeric
    // offset are accepted and normalized to the canonical UTC value.
    let program = checked_program(
        "pub fn main(t: instant): string\n    return std::clock::formatInstant(t)\n",
    );
    let call = CheckedEntryCall::from_text_args(
        &program,
        "test::main",
        &[("t", "instant(\"2024-01-01T05:00:00+05:00\")")],
    )
    .expect("entry args decode");
    let store = TreeStore::memory();
    let mut output = String::new();

    let result = run_entry(&store, &call, &mut output).expect("run entry");
    assert_eq!(
        result.value,
        Some(Value::Str("2024-01-01T00:00:00Z".into()))
    );
}

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
fn text_args_accept_negative_numeric_spellings() {
    // CLI `--arg` scalars use the same textual spellings runtime literals accept,
    // which include a leading unary minus. A negative int and a negative decimal
    // must decode to their signed values.
    let program =
        checked_program("pub fn main(n: int, d: decimal): string\n    return $\"{n} {d}\"\n");
    let call =
        CheckedEntryCall::from_text_args(&program, "test::main", &[("n", "-5"), ("d", "-1.5")])
            .expect("negative entry args decode");
    let store = TreeStore::memory();
    let mut output = String::new();
    let result = run_entry(&store, &call, &mut output).expect("run entry");
    assert_eq!(result.value, Some(Value::Str("-5 -1.5".into())));
}

#[test]
fn text_args_accept_a_negative_int_identity_key() {
    // An identity key shares the scalar literal grammar, so a negative int key
    // spelling decodes through the same unary-minus path.
    let program = checked_program(
        "resource Reading\n\
         \x20   note: string\n\
         store ^readings(temp: int): Reading\n\
         \n\
         pub fn accept(reading: Id(^readings)): int\n\
         \x20   return 1\n",
    );
    let call = CheckedEntryCall::from_text_args(&program, "test::accept", &[("reading", "-5")])
        .expect("a negative int identity key decodes");
    let store = TreeStore::memory();
    let mut output = String::new();
    let result = run_entry(&store, &call, &mut output).expect("run entry");
    assert_eq!(result.value, Some(Value::Int(1)));
}

#[test]
fn text_args_keep_positive_numeric_spellings() {
    let program = checked_program("pub fn main(n: int): int\n    return n\n");
    let call = CheckedEntryCall::from_text_args(&program, "test::main", &[("n", "5")])
        .expect("positive entry arg decodes");
    let store = TreeStore::memory();
    let mut output = String::new();
    let result = run_entry(&store, &call, &mut output).expect("run entry");
    assert_eq!(result.value, Some(Value::Int(5)));
}

#[test]
fn text_args_reject_non_numeric_spellings() {
    // A genuinely invalid spelling is still rejected; widening to accept a unary
    // minus does not open the decoder to arbitrary text.
    let program = checked_program("pub fn main(n: int): int\n    return n\n");
    let error = CheckedEntryCall::from_text_args(&program, "test::main", &[("n", "abc")])
        .expect_err("a non-numeric int spelling is rejected");
    assert_eq!(error.code(), RUN_ENTRY_ARGUMENT);
}

#[test]
fn text_args_reject_malformed_negative_spellings() {
    // A negative literal is a single sign directly prefixing a numeric literal.
    // Double negation, a space between the sign and the digits, and a mixed
    // sign prefix are not valid literal spellings, so they must be rejected at
    // the entry-argument boundary rather than silently decoding to a value.
    let program = checked_program("pub fn main(n: int): int\n    return n\n");
    for spelling in ["--5", "- 5", "-+5", "-(5)", "-  5"] {
        let error = CheckedEntryCall::from_text_args(&program, "test::main", &[("n", spelling)])
            .map(|_| ())
            .expect_err(&format!(
                "malformed negative spelling `{spelling}` must be rejected"
            ));
        assert_eq!(error.code(), RUN_ENTRY_ARGUMENT, "spelling `{spelling}`");
    }
}

#[test]
fn text_args_reject_malformed_negative_identity_keys() {
    // An identity key shares the scalar literal grammar, so a malformed negative
    // key spelling must be rejected rather than mis-decoded into a different,
    // wrong-but-successful identity.
    let program = checked_program(
        "resource Reading\n\
         \x20   note: string\n\
         store ^readings(temp: int): Reading\n\
         \n\
         pub fn accept(reading: Id(^readings)): int\n\
         \x20   return 1\n",
    );
    for spelling in ["--5", "- 5", "-+5"] {
        let error =
            CheckedEntryCall::from_text_args(&program, "test::accept", &[("reading", spelling)])
                .map(|_| ())
                .expect_err(&format!(
                    "malformed negative key `{spelling}` must be rejected"
                ));
        assert_eq!(error.code(), RUN_ENTRY_ARGUMENT, "key `{spelling}`");
    }
}

#[test]
fn protocol_args_admit_canonical_entry_identity_and_typed_values() {
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\n\
         store ^authors(id: int): Author\n\
         \n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         \n\
         pub fn accept(author: Id(^authors), status: Status, flags: sequence[bool], label: string): string\n\
         \x20   if status == Status::archived and label == \"done\"\n\
         \x20       return \"ok\"\n\
         \x20   return \"no\"\n",
    );
    let descriptor = EntryDescriptor::resolve(&program, "accept").expect("entry descriptor");
    let EntryArgumentShape::Identity {
        store_catalog_id, ..
    } = &entry_parameter(&descriptor, "author").shape
    else {
        panic!("author should be an identity shape");
    };
    let author_store_catalog_id = store_catalog_id.clone();
    let EntryArgumentShape::Enum { members, .. } = &entry_parameter(&descriptor, "status").shape
    else {
        panic!("status should be an enum shape");
    };
    let archived_member_catalog_id = members
        .iter()
        .find(|member| member.render_label == "archived")
        .expect("archived member")
        .catalog_id
        .clone();
    let call = CheckedEntryCall::from_protocol_invocation(
        &program,
        &protocol_invocation(
            &descriptor,
            vec![
                EntryArgument {
                    name: "author".into(),
                    value: EntryArgumentValue::Identity {
                        store_catalog_id: author_store_catalog_id,
                        keys: vec![EntryScalarArgument::Int(7)],
                    },
                },
                EntryArgument {
                    name: "status".into(),
                    value: EntryArgumentValue::EnumMember {
                        member_catalog_id: archived_member_catalog_id,
                    },
                },
                EntryArgument {
                    name: "flags".into(),
                    value: EntryArgumentValue::Sequence(vec![
                        EntryArgumentValue::Scalar(EntryScalarArgument::Bool(true)),
                        EntryArgumentValue::Scalar(EntryScalarArgument::Bool(false)),
                    ]),
                },
                EntryArgument {
                    name: "label".into(),
                    value: EntryArgumentValue::Scalar(EntryScalarArgument::String("done".into())),
                },
            ],
        ),
    )
    .expect("protocol args are admitted");

    assert_eq!(call.identity().canonical_name, "test::accept");
    assert_eq!(call.identity().requested_name, "test::accept");
    assert_eq!(call.identity().source_digest, program.source_digest());
    assert_eq!(
        call.identity().read_only_context_digest,
        program.read_only_context_digest()
    );

    let store = TreeStore::memory();
    let mut output = String::new();
    let result = run_entry(&store, &call, &mut output).expect("run entry");

    assert_eq!(result.value, Some(Value::Str("ok".into())));
}

#[test]
fn json_protocol_args_decode_to_typed_entry_arguments() {
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\n\
         store ^authors(id: int): Author\n\
         \n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         \n\
         pub fn accept(author: Id(^authors), status: Status, flags: sequence[bool], label: string): string\n\
         \x20   if status == Status::archived and label == \"done\"\n\
         \x20       return \"ok\"\n\
         \x20   return \"no\"\n",
    );
    let descriptor = EntryDescriptor::resolve(&program, "accept").expect("entry descriptor");
    let EntryArgumentShape::Identity {
        store_catalog_id, ..
    } = &entry_parameter(&descriptor, "author").shape
    else {
        panic!("author should be an identity shape");
    };
    let EntryArgumentShape::Enum { members, .. } = &entry_parameter(&descriptor, "status").shape
    else {
        panic!("status should be an enum shape");
    };
    let archived = members
        .iter()
        .find(|member| member.render_label == "archived")
        .expect("archived member");
    let args = entry_arguments_from_json(&[
        json!({
            "name": "author",
            "value": {
                "kind": "identity",
                "store_catalog_id": store_catalog_id.as_str(),
                "keys": [{ "kind": "int", "value": "7" }]
            }
        }),
        json!({
            "name": "status",
            "value": {
                "kind": "enum_member",
                "member_catalog_id": archived.catalog_id.as_str()
            }
        }),
        json!({
            "name": "flags",
            "value": {
                "kind": "sequence",
                "value": [
                    { "kind": "bool", "value": true },
                    { "kind": "bool", "value": false }
                ]
            }
        }),
        json!({ "name": "label", "value": { "kind": "string", "value": "done" } }),
    ])
    .expect("json protocol args decode");
    let call = CheckedEntryCall::from_protocol_invocation(
        &program,
        &protocol_invocation(&descriptor, args),
    )
    .expect("json protocol args admitted");

    let store = TreeStore::memory();
    let mut output = String::new();
    let result = run_entry(&store, &call, &mut output).expect("run entry");

    assert_eq!(result.value, Some(Value::Str("ok".into())));
}

#[test]
fn json_protocol_args_use_marrow_scalar_parsers() {
    let args = entry_arguments_from_json(&[json!({
        "name": "amount",
        "value": { "kind": "decimal", "value": "-0.1" }
    })])
    .expect("negative canonical decimal decodes");

    assert_eq!(
        args,
        vec![EntryArgument {
            name: "amount".into(),
            value: EntryArgumentValue::Scalar(EntryScalarArgument::Decimal(
                Decimal::parse_canonical("-0.1").expect("canonical decimal")
            )),
        }]
    );

    let error = entry_arguments_from_json(&[json!({
        "name": "amount",
        "value": {
            "kind": "decimal",
            "value": "99999999999999999999999999999999999"
        }
    })])
    .expect_err("decimal overflow is parser-owned");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::InvalidDecimal);
    assert_eq!(error.path(), "run argument 0 value");
    assert_eq!(error.field(), Some("value"));

    let error = entry_arguments_from_json(&[json!({
        "name": "moment",
        "value": {
            "kind": "instant",
            "value": "170141183460469231731687303715884105728"
        }
    })])
    .expect_err("i128 overflow is parser-owned");
    assert_eq!(
        error.kind(),
        EntryArgumentJsonErrorKind::ExpectedIntegerString
    );
    assert_eq!(error.path(), "run argument 0 value");
    assert_eq!(error.field(), Some("value"));
}

#[test]
fn json_protocol_args_use_exact_scalar_strings() {
    let args = entry_arguments_from_json(&[
        json!({ "name": "n", "value": { "kind": "int", "value": "7" } }),
        json!({ "name": "max", "value": { "kind": "int", "value": i64::MAX.to_string() } }),
        json!({ "name": "day", "value": { "kind": "date", "value": "1969-12-30" } }),
        json!({ "name": "payload", "value": { "kind": "bytes", "value": "01ff" } }),
    ])
    .expect("exact scalar strings decode");

    assert_eq!(
        args,
        vec![
            EntryArgument {
                name: "n".into(),
                value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(7)),
            },
            EntryArgument {
                name: "max".into(),
                value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(i64::MAX)),
            },
            EntryArgument {
                name: "day".into(),
                value: EntryArgumentValue::Scalar(EntryScalarArgument::Date(-2)),
            },
            EntryArgument {
                name: "payload".into(),
                value: EntryArgumentValue::Scalar(EntryScalarArgument::Bytes(vec![1, 255])),
            },
        ]
    );

    let error = entry_arguments_from_json(&[json!({
        "name": "n",
        "value": { "kind": "int", "value": 7 }
    })])
    .expect_err("int values use exact string form");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::ExpectedString);
    assert_eq!(error.field(), Some("value"));

    let error = entry_arguments_from_json(&[json!({
        "name": "n",
        "value": { "kind": "int", "value": "-0" }
    })])
    .expect_err("integer strings are canonical");
    assert_eq!(
        error.kind(),
        EntryArgumentJsonErrorKind::ExpectedIntegerString
    );
    assert_eq!(error.field(), Some("value"));

    let error = entry_arguments_from_json(&[json!({
        "name": "payload",
        "value": { "kind": "bytes", "value": [1, 255] }
    })])
    .expect_err("bytes use lowercase hex string form");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::ExpectedString);
    assert_eq!(error.field(), Some("value"));

    let error = entry_arguments_from_json(&[json!({
        "name": "payload",
        "value": { "kind": "bytes", "value": "01FF" }
    })])
    .expect_err("bytes use canonical lowercase hex");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::InvalidBytes);
    assert_eq!(error.field(), Some("value"));

    let error = entry_arguments_from_json(&[json!({
        "name": "n",
        "value": { "kind": "int", "value": "01" }
    })])
    .expect_err("integer strings are canonical");
    assert_eq!(
        error.kind(),
        EntryArgumentJsonErrorKind::ExpectedIntegerString
    );
    assert_eq!(error.field(), Some("value"));

    let error = entry_arguments_from_json(&[json!({
        "name": "day",
        "value": { "kind": "date", "value": -2 }
    })])
    .expect_err("date values use canonical date string form");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::ExpectedString);
    assert_eq!(error.field(), Some("value"));
}

#[test]
fn malformed_json_protocol_args_return_transport_errors() {
    let error = entry_arguments_from_json(&[json!(1)]).expect_err("argument must be object");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::ExpectedObject);
    assert_eq!(error.path(), "run argument 0");
    assert_eq!(error.field(), None);

    let error = entry_arguments_from_json(&[json!({
        "name": "n",
        "value": { "kind": "decimal", "value": "01.0" }
    })])
    .expect_err("decimal must be canonical");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::InvalidDecimal);
    assert_eq!(error.path(), "run argument 0 value");
    assert_eq!(error.field(), Some("value"));
}

#[test]
fn json_protocol_args_reject_schema_invalid_payloads() {
    let error = entry_arguments_from_json(&[json!({
        "name": "n",
        "extra_arg_field": true,
        "value": { "kind": "int", "value": "7" }
    })])
    .expect_err("argument object is closed");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::UnknownField);
    assert_eq!(error.path(), "run argument 0");
    assert_eq!(error.field(), Some("extra_arg_field"));

    let error = entry_arguments_from_json(&[json!({
        "name": "n",
        "value": { "kind": "int", "value": "7", "extra_value_field": true }
    })])
    .expect_err("value object is closed");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::UnknownField);
    assert_eq!(error.path(), "run argument 0 value");
    assert_eq!(error.field(), Some("extra_value_field"));

    let error = entry_arguments_from_json(&[json!({
        "name": "   ",
        "value": { "kind": "int", "value": "7" }
    })])
    .expect_err("name needs non-whitespace");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::EmptyName);

    let error = entry_arguments_from_json(&[json!({
        "name": "day",
        "value": { "kind": "date", "value": "10000-01-01" }
    })])
    .expect_err("date is canonical and supported");
    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::InvalidDate);
    assert_eq!(error.field(), Some("value"));
}

#[test]
fn json_protocol_args_reject_deep_sequences_before_stack_growth() {
    let mut value = json!({ "kind": "int", "value": "1" });
    for _ in 0..200 {
        value = json!({ "kind": "sequence", "value": [value] });
    }
    let error = entry_arguments_from_json(&[json!({
        "name": "xs",
        "value": value
    })])
    .expect_err("sequence nesting is bounded");

    assert_eq!(error.kind(), EntryArgumentJsonErrorKind::DepthLimit);
}

#[test]
fn entry_argument_json_schema_names_supported_value_kinds() {
    let schema = entry_argument_json_schema();

    assert_eq!(schema["required"], json!(["name", "value"]));
    let value_variants = schema["properties"]["value"]["oneOf"]
        .as_array()
        .expect("value variants");
    let kinds = value_variants
        .iter()
        .filter_map(|variant| variant["properties"]["kind"]["const"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            "int",
            "bool",
            "string",
            "decimal",
            "date",
            "instant",
            "duration",
            "bytes",
            "enum_member",
            "identity",
            "sequence",
        ]
    );
}

#[test]
fn entry_argument_json_schema_leaves_parser_owned_scalar_strings_structural() {
    let schema = entry_argument_json_schema();
    let value_variants = schema["properties"]["value"]["oneOf"]
        .as_array()
        .expect("value variants");
    for kind in ["int", "decimal", "date", "instant", "duration"] {
        let value_schema = value_variants
            .iter()
            .find(|variant| variant["properties"]["kind"]["const"] == json!(kind))
            .expect("scalar variant")["properties"]["value"]
            .clone();
        assert_eq!(value_schema, json!({ "type": "string" }));
    }
}

#[test]
fn entry_argument_json_schema_uses_hex_string_for_bytes() {
    let schema = entry_argument_json_schema();
    let value_variants = schema["properties"]["value"]["oneOf"]
        .as_array()
        .expect("value variants");
    let bytes_value_schema = value_variants
        .iter()
        .find(|variant| variant["properties"]["kind"]["const"] == json!("bytes"))
        .expect("bytes variant")["properties"]["value"]
        .clone();

    assert_eq!(
        bytes_value_schema,
        json!({
            "type": "string",
            "pattern": "^([0-9a-f]{2})*$"
        })
    );
}

#[test]
fn entry_descriptor_exposes_protocol_argument_shapes() {
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\n\
         store ^authors(id: int): Author\n\
         \n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         \n\
         pub fn accept(author: Id(^authors), status: Status, flags: sequence[bool], label: string): string\n\
         \x20   return label\n",
    );

    let descriptor = EntryDescriptor::resolve(&program, "accept").expect("entry descriptor");

    assert_eq!(descriptor.identity.canonical_name, "test::accept");
    let EntryArgumentShape::Identity {
        render_label,
        store_catalog_id,
        keys,
    } = &entry_parameter(&descriptor, "author").shape
    else {
        panic!("author should be an identity shape");
    };
    assert_eq!(render_label, "authors");
    assert!(store_catalog_id.as_str().starts_with("cat_"));
    assert_eq!(
        keys,
        &vec![marrow_run::EntryIdentityKey {
            render_label: "id".into(),
            scalar: ScalarType::Int,
        }]
    );
    let EntryArgumentShape::Enum {
        render_label,
        catalog_id,
        members,
    } = &entry_parameter(&descriptor, "status").shape
    else {
        panic!("status should be an enum shape");
    };
    assert_eq!(render_label, "test::Status");
    assert!(catalog_id.as_str().starts_with("cat_"));
    assert_eq!(
        members
            .iter()
            .map(|member| member.render_label.as_str())
            .collect::<Vec<_>>(),
        vec!["active", "archived"]
    );
    assert!(
        members
            .iter()
            .all(|member| member.catalog_id.as_str().starts_with("cat_"))
    );
    let flags = entry_parameter(&descriptor, "flags");
    assert_eq!(
        flags.shape,
        EntryArgumentShape::Sequence(Box::new(EntryArgumentShape::Scalar(ScalarType::Bool)))
    );
}

#[test]
fn protocol_enum_arguments_round_trip_duplicate_leaf_catalog_ids() {
    let program = checked_program(
        "enum Cat\n\
         \x20   category tiger\n\
         \x20       paw\n\
         \x20   category lion\n\
         \x20       paw\n\
         \n\
         pub fn label(cat: Cat): int\n\
         \x20   match cat\n\
         \x20       tiger::paw\n\
         \x20           return 2\n\
         \x20       lion::paw\n\
         \x20           return 3\n\
         \x20   return 0\n",
    );
    let descriptor = EntryDescriptor::resolve(&program, "label").expect("entry descriptor");
    let EntryArgumentShape::Enum { members, .. } = &entry_parameter(&descriptor, "cat").shape
    else {
        panic!("cat should be an enum shape");
    };
    let [tiger_paw, lion_paw] = members.as_slice() else {
        panic!("expected two selectable paw members");
    };
    assert_eq!(tiger_paw.render_label, "paw");
    assert_eq!(lion_paw.render_label, "paw");
    assert_ne!(tiger_paw.catalog_id, lion_paw.catalog_id);
    let tiger_paw = tiger_paw.catalog_id.clone();
    let lion_paw = lion_paw.catalog_id.clone();

    let store = TreeStore::memory();
    let mut output = String::new();
    let tiger = CheckedEntryCall::from_protocol_invocation(
        &program,
        &protocol_invocation(
            &descriptor,
            vec![EntryArgument {
                name: "cat".into(),
                value: EntryArgumentValue::EnumMember {
                    member_catalog_id: tiger_paw,
                },
            }],
        ),
    )
    .expect("tiger paw arg");
    let tiger_result = run_entry(&store, &tiger, &mut output).expect("run tiger");
    assert_eq!(tiger_result.value, Some(Value::Int(2)));

    let lion = CheckedEntryCall::from_protocol_invocation(
        &program,
        &protocol_invocation(
            &descriptor,
            vec![EntryArgument {
                name: "cat".into(),
                value: EntryArgumentValue::EnumMember {
                    member_catalog_id: lion_paw,
                },
            }],
        ),
    )
    .expect("lion paw arg");
    let lion_result = run_entry(&store, &lion, &mut output).expect("run lion");
    assert_eq!(lion_result.value, Some(Value::Int(3)));
}

#[test]
fn protocol_temporal_arguments_reject_out_of_range_values() {
    let program = checked_program(
        "pub fn date_echo(value: date): date\n\
         \x20   return value\n\
         pub fn instant_echo(value: instant): instant\n\
         \x20   return value\n",
    );

    for (entry, value) in [
        (
            "date_echo",
            EntryArgumentValue::Scalar(EntryScalarArgument::Date(i32::MIN)),
        ),
        (
            "instant_echo",
            EntryArgumentValue::Scalar(EntryScalarArgument::Instant(i128::MAX)),
        ),
    ] {
        let descriptor = EntryDescriptor::resolve(&program, entry).expect("entry descriptor");
        let error = CheckedEntryCall::from_protocol_invocation(
            &program,
            &protocol_invocation(
                &descriptor,
                vec![EntryArgument {
                    name: "value".into(),
                    value,
                }],
            ),
        )
        .expect_err("out-of-range temporal protocol value should reject");

        assert_eq!(error.code(), RUN_ENTRY_ARGUMENT);
    }
}

#[test]
fn entry_tag_changes_with_signature_and_ignores_body_changes() {
    let signature_a = checked_program("pub fn run(n: int): int\n    return n\n");
    let signature_b = checked_program("pub fn run(label: string): string\n    return label\n");
    let body_a = checked_program("pub fn run(n: int): int\n    return n\n");
    let body_b = checked_program("pub fn run(n: int): int\n    return n + 1\n");

    let signature_a = EntryDescriptor::resolve(&signature_a, "run").expect("signature a");
    let signature_b = EntryDescriptor::resolve(&signature_b, "run").expect("signature b");
    let body_a = EntryDescriptor::resolve(&body_a, "run").expect("body a");
    let body_b = EntryDescriptor::resolve(&body_b, "run").expect("body b");

    assert_ne!(
        signature_a.identity.entry_tag,
        signature_b.identity.entry_tag
    );
    assert_eq!(body_a.identity.entry_tag, body_b.identity.entry_tag);
}

#[test]
fn stale_protocol_entry_identity_rejects_signature_changes_before_running() {
    let stale = checked_program("pub fn run(n: int): int\n    return n\n");
    let stale = EntryDescriptor::resolve(&stale, "run").expect("stale descriptor");
    let current = checked_program("pub fn run(label: string): string\n    return label\n");

    let error = CheckedEntryCall::from_protocol_invocation(
        &current,
        &protocol_invocation(
            &stale,
            vec![EntryArgument {
                name: "n".into(),
                value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(1)),
            }],
        ),
    )
    .expect_err("stale descriptor should fail closed");

    assert_eq!(error.code(), RUN_ENTRY_ARGUMENT);
}

#[test]
fn stale_protocol_entry_identity_rejects_removed_entries_as_entry_arguments() {
    let stale = checked_program("pub fn run(n: int): int\n    return n\n");
    let stale = EntryDescriptor::resolve(&stale, "run").expect("stale descriptor");
    let current = checked_program("pub fn renamed(n: int): int\n    return n\n");

    let error = CheckedEntryCall::from_protocol_invocation(
        &current,
        &protocol_invocation(
            &stale,
            vec![EntryArgument {
                name: "n".into(),
                value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(1)),
            }],
        ),
    )
    .expect_err("removed entry descriptor should fail as stale protocol identity");

    assert_eq!(error.code(), RUN_ENTRY_ARGUMENT);
}

#[test]
fn stale_protocol_entry_identity_rejects_private_entries_as_entry_arguments() {
    let stale = checked_program("pub fn run(n: int): int\n    return n\n");
    let stale = EntryDescriptor::resolve(&stale, "run").expect("stale descriptor");
    let current = checked_program("fn run(n: int): int\n    return n\n");

    let error = CheckedEntryCall::from_protocol_invocation(
        &current,
        &protocol_invocation(
            &stale,
            vec![EntryArgument {
                name: "n".into(),
                value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(1)),
            }],
        ),
    )
    .expect_err("private entry descriptor should fail as stale protocol identity");

    assert_eq!(error.code(), RUN_ENTRY_ARGUMENT);
}

#[test]
fn protocol_entry_identity_allows_called_function_body_changes() {
    let stale = checked_program(
        "fn helper(n: int): int\n\
         \x20   return n\n\
         pub fn run(n: int): int\n\
         \x20   return helper(n)\n",
    );
    let stale = EntryDescriptor::resolve(&stale, "run").expect("stale descriptor");
    let current = checked_program(
        "fn helper(n: int): int\n\
         \x20   return n + 1\n\
         pub fn run(n: int): int\n\
         \x20   return helper(n)\n",
    );

    let call = CheckedEntryCall::from_protocol_invocation(
        &current,
        &protocol_invocation(
            &stale,
            vec![EntryArgument {
                name: "n".into(),
                value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(1)),
            }],
        ),
    )
    .expect("body-only helper changes keep the entry ABI");

    let store = TreeStore::memory();
    let mut output = String::new();
    let result = run_entry(&store, &call, &mut output).expect("run current helper body");
    assert_eq!(result.value, Some(Value::Int(2)));
}

#[test]
fn protocol_entry_identity_resolves_by_canonical_descriptor_name() {
    let program = checked_program_modules(&[
        "module a\n\
         pub fn run(n: int): int\n\
         \x20   return n\n",
        "module b\n\
         pub fn run(n: int): int\n\
         \x20   return n + 10\n",
    ]);
    let descriptor = EntryDescriptor::resolve(&program, "b::run").expect("entry descriptor");
    let call = CheckedEntryCall::from_protocol_invocation(
        &program,
        &protocol_invocation(
            &descriptor,
            vec![EntryArgument {
                name: "n".into(),
                value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(5)),
            }],
        ),
    )
    .expect("canonical protocol descriptor");

    let store = TreeStore::memory();
    let mut output = String::new();
    let result = run_entry(&store, &call, &mut output).expect("run entry");

    assert_eq!(result.value, Some(Value::Int(15)));
}

#[test]
fn repeated_text_args_collect_scalar_sequences_in_argv_order() {
    let program = checked_program(
        "pub fn sum(xs: sequence[int]): int\n\
         \x20   var total = 0\n\
         \x20   for x in values(xs)\n\
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

        assert_eq!(
            error.code(),
            "run.entry_argument",
            "{name}={text}: {error:?}"
        );
    }
}

#[test]
fn wrong_typed_arg_names_the_scalar_in_the_surface_convention() {
    // The rejection names the expected scalar by its lowercase language spelling,
    // backticked, with the grammatical indefinite article — never the internal
    // capitalized identifier.
    let program = checked_program("pub fn main(n: int): int\n    return n\n");
    let error = CheckedEntryCall::from_text_args(&program, "test::main", &[("n", "xx")])
        .expect_err("a non-integer int arg should reject");
    assert_eq!(error.code(), RUN_ENTRY_ARGUMENT);
    assert!(
        error.message.contains("an `int`"),
        "message should name the scalar in the surface convention: {}",
        error.message
    );
    assert!(
        !error.message.contains("Int"),
        "message must not leak the capitalized identifier: {}",
        error.message
    );
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

        assert_eq!(error.code(), "run.entry_argument", "{entry} {args:?}");
    }
}

#[test]
fn text_args_decode_scalar_sequence_elements_as_language_literals() {
    let program = checked_program(
        "pub fn check(spans: sequence[duration]): int\n\
         \x20   var total = 0.hours\n\
         \x20   for span in values(spans)\n\
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

    assert_eq!(error.code(), "run.entry_argument");
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
    assert_eq!(error.code(), "run.entry_argument");
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
    assert_eq!(error.code(), "run.entry_argument");
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
fn text_args_reject_composite_identity_params() {
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

    assert_eq!(error.code(), "run.entry_argument");
}
