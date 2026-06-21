use std::cell::RefCell;
use std::rc::Rc;

use marrow_check::{
    CheckedEntryFunction, CheckedFunctionRef, CheckedLiteralKind, CheckedReadOnlyExpression,
    CheckedRuntimeFunction, CheckedRuntimeProgram, CheckedRuntimeValueType, EntryArgumentShape,
    EntryDescriptor, EntryDescriptorError, EntryIdentity, EntryParameter, StoredValueMeaning,
};
use marrow_store::Decimal;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{
    SavedValue, ScalarType, decode_value, supported_date_days, supported_instant_nanos,
};
use marrow_syntax::{Expression, SourceSpan, parse_expression};
use serde_json::{Map, Value as Json, json};

use crate::activation::{
    Completion, Invocation, bind_module_constants, check_argument_count, executable_body, invoke,
};
use crate::call::function_by_ref;
use crate::env::{Context, Env, TransactionState};
use crate::error::{
    RuntimeError, ambiguous_function, entry_argument, entry_type_error, private_function,
    raise_with_transaction_escape, reraise_fault_with_transaction_escape, unknown_function,
    unsupported,
};
use crate::expr::eval_expr;
use crate::host::{Host, StepHook};
use crate::stdlib::parse_rfc3339_instant_nanos;
use crate::value::{
    RunOutput, RunOutputSink, Sequence, Value, enum_value_from_member, value_scalar_type,
    value_to_key,
};

struct ForwardOutput<'a> {
    sink: &'a mut dyn RunOutputSink,
}

impl RunOutputSink for ForwardOutput<'_> {
    fn write(&mut self, text: &str) {
        self.sink.write(text);
    }
}

#[derive(Debug, Clone)]
pub struct CheckedEntryCall<'p> {
    program: &'p CheckedRuntimeProgram,
    target: CheckedFunctionRef,
    args: Vec<Value>,
    identity: EntryIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInvocation {
    pub identity: EntryIdentity,
    pub arguments: Vec<EntryArgument>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryArgument {
    pub name: String,
    pub value: EntryArgumentValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryArgumentValue {
    Scalar(EntryScalarArgument),
    EnumMember {
        member_catalog_id: CatalogId,
    },
    Identity {
        store_catalog_id: CatalogId,
        keys: Vec<EntryScalarArgument>,
    },
    Sequence(Vec<EntryArgumentValue>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryScalarArgument {
    Int(i64),
    Bool(bool),
    String(String),
    Instant(i128),
    Date(i32),
    Duration(i128),
    Decimal(Decimal),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryArgumentJsonError {
    kind: EntryArgumentJsonErrorKind,
    path: String,
    field: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryArgumentJsonErrorKind {
    ExpectedObject,
    UnknownField,
    ExpectedString,
    EmptyName,
    ExpectedBool,
    ExpectedIntegerString,
    InvalidDecimal,
    InvalidDate,
    InvalidBytes,
    ExpectedArray,
    InvalidCatalogId,
    UnsupportedKind,
    ExpectedScalar,
    DepthLimit,
}

const ENTRY_ARGUMENT_JSON_MAX_DEPTH: usize = 128;

impl EntryArgumentJsonError {
    pub fn kind(&self) -> EntryArgumentJsonErrorKind {
        self.kind
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }

    pub fn message(&self) -> String {
        match self.kind {
            EntryArgumentJsonErrorKind::ExpectedObject => {
                format!("{} must be an object", self.path)
            }
            EntryArgumentJsonErrorKind::UnknownField => format!(
                "{} has unknown field `{}`",
                self.path,
                self.field.as_deref().unwrap_or("")
            ),
            EntryArgumentJsonErrorKind::ExpectedString => format!(
                "{} needs string field `{}`",
                self.path,
                self.field.as_deref().unwrap_or("")
            ),
            EntryArgumentJsonErrorKind::EmptyName => {
                format!("{} needs non-empty string name", self.path)
            }
            EntryArgumentJsonErrorKind::ExpectedBool => format!(
                "{} needs boolean field `{}`",
                self.path,
                self.field.as_deref().unwrap_or("")
            ),
            EntryArgumentJsonErrorKind::ExpectedIntegerString => format!(
                "{} needs integer-string field `{}`",
                self.path,
                self.field.as_deref().unwrap_or("")
            ),
            EntryArgumentJsonErrorKind::InvalidDecimal => format!(
                "{} needs canonical decimal field `{}`",
                self.path,
                self.field.as_deref().unwrap_or("")
            ),
            EntryArgumentJsonErrorKind::InvalidDate => format!(
                "{} needs canonical date field `{}`",
                self.path,
                self.field.as_deref().unwrap_or("")
            ),
            EntryArgumentJsonErrorKind::InvalidBytes => format!(
                "{} needs lowercase hex bytes field `{}`",
                self.path,
                self.field.as_deref().unwrap_or("")
            ),
            EntryArgumentJsonErrorKind::ExpectedArray => format!(
                "{} needs array field `{}`",
                self.path,
                self.field.as_deref().unwrap_or("")
            ),
            EntryArgumentJsonErrorKind::InvalidCatalogId => format!(
                "{} field `{}` is not a catalog id",
                self.path,
                self.field.as_deref().unwrap_or("")
            ),
            EntryArgumentJsonErrorKind::UnsupportedKind => {
                format!("{} has unsupported value kind", self.path)
            }
            EntryArgumentJsonErrorKind::ExpectedScalar => {
                format!("{} must be a scalar value", self.path)
            }
            EntryArgumentJsonErrorKind::DepthLimit => {
                format!("{} exceeds JSON nesting limit", self.path)
            }
        }
    }
}

impl std::fmt::Display for EntryArgumentJsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for EntryArgumentJsonError {}

pub fn entry_arguments_from_json(
    args: &[Json],
) -> Result<Vec<EntryArgument>, EntryArgumentJsonError> {
    args.iter()
        .enumerate()
        .map(|(index, arg)| entry_argument_from_json(index, arg))
        .collect()
}

pub fn entry_argument_json_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "minLength": 1,
                "pattern": "\\S",
                "description": "Entry parameter name."
            },
            "value": entry_argument_value_schema(),
        },
        "required": ["name", "value"],
        "additionalProperties": false,
        "$defs": {
            "scalar": entry_scalar_argument_schema(),
            "value": entry_argument_value_schema(),
        },
    })
}

fn entry_argument_value_schema() -> Json {
    let mut variants = entry_scalar_argument_variants();
    variants.extend([
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "enum_member" },
                "member_catalog_id": { "type": "string", "pattern": "^cat_[0-9a-f]{32}$" }
            },
            "required": ["kind", "member_catalog_id"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "identity" },
                "store_catalog_id": { "type": "string", "pattern": "^cat_[0-9a-f]{32}$" },
                "keys": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/scalar" }
                }
            },
            "required": ["kind", "store_catalog_id", "keys"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "sequence" },
                "value": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/value" }
                }
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        }),
    ]);
    json!({
        "oneOf": variants,
    })
}

fn entry_scalar_argument_schema() -> Json {
    json!({
        "oneOf": entry_scalar_argument_variants(),
    })
}

fn entry_scalar_argument_variants() -> Vec<Json> {
    vec![
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "int" },
                "value": { "type": "string" }
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "bool" },
                "value": { "type": "boolean" }
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "string" },
                "value": { "type": "string" }
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "decimal" },
                "value": { "type": "string" }
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "date" },
                "value": { "type": "string" }
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "instant" },
                "value": { "type": "string" }
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "duration" },
                "value": { "type": "string" }
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": "bytes" },
                "value": {
                    "type": "string",
                    "pattern": "^([0-9a-f]{2})*$"
                }
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        }),
    ]
}

fn entry_argument_from_json(
    index: usize,
    arg: &Json,
) -> Result<EntryArgument, EntryArgumentJsonError> {
    let path = format!("run argument {index}");
    let object = json_object(&path, arg)?;
    reject_unknown_fields(&path, object, &["name", "value"])?;
    let name = json_string_field(&path, object, "name")?;
    if name.trim().is_empty() {
        return Err(json_error(EntryArgumentJsonErrorKind::EmptyName, &path));
    }
    let value = object
        .get("value")
        .ok_or_else(|| json_field_error(EntryArgumentJsonErrorKind::ExpectedObject, &path, "value"))
        .and_then(|value| entry_argument_value_from_json(&format!("{path} value"), value, 0))?;
    Ok(EntryArgument {
        name: name.to_string(),
        value,
    })
}

fn entry_argument_value_from_json(
    path: &str,
    value: &Json,
    depth: usize,
) -> Result<EntryArgumentValue, EntryArgumentJsonError> {
    if depth > ENTRY_ARGUMENT_JSON_MAX_DEPTH {
        return Err(json_error(EntryArgumentJsonErrorKind::DepthLimit, path));
    }
    let object = json_object(path, value)?;
    match json_string_field(path, object, "kind")? {
        "int" => {
            reject_unknown_fields(path, object, &["kind", "value"])?;
            Ok(EntryArgumentValue::Scalar(EntryScalarArgument::Int(
                json_i64_string_field(path, object, "value")?,
            )))
        }
        "bool" => {
            reject_unknown_fields(path, object, &["kind", "value"])?;
            Ok(EntryArgumentValue::Scalar(EntryScalarArgument::Bool(
                json_bool_field(path, object, "value")?,
            )))
        }
        "string" => {
            reject_unknown_fields(path, object, &["kind", "value"])?;
            Ok(EntryArgumentValue::Scalar(EntryScalarArgument::String(
                json_string_field(path, object, "value")?.to_string(),
            )))
        }
        "decimal" => {
            reject_unknown_fields(path, object, &["kind", "value"])?;
            Ok(EntryArgumentValue::Scalar(EntryScalarArgument::Decimal(
                json_decimal_field(path, object, "value")?,
            )))
        }
        "date" => {
            reject_unknown_fields(path, object, &["kind", "value"])?;
            Ok(EntryArgumentValue::Scalar(EntryScalarArgument::Date(
                json_date_string_field(path, object, "value")?,
            )))
        }
        "instant" => {
            reject_unknown_fields(path, object, &["kind", "value"])?;
            Ok(EntryArgumentValue::Scalar(EntryScalarArgument::Instant(
                json_i128_string_field(path, object, "value")?,
            )))
        }
        "duration" => {
            reject_unknown_fields(path, object, &["kind", "value"])?;
            Ok(EntryArgumentValue::Scalar(EntryScalarArgument::Duration(
                json_i128_string_field(path, object, "value")?,
            )))
        }
        "bytes" => {
            reject_unknown_fields(path, object, &["kind", "value"])?;
            Ok(EntryArgumentValue::Scalar(EntryScalarArgument::Bytes(
                json_bytes_hex_field(path, object, "value")?,
            )))
        }
        "enum_member" => {
            reject_unknown_fields(path, object, &["kind", "member_catalog_id"])?;
            Ok(EntryArgumentValue::EnumMember {
                member_catalog_id: json_catalog_id_field(path, object, "member_catalog_id")?,
            })
        }
        "identity" => {
            reject_unknown_fields(path, object, &["kind", "store_catalog_id", "keys"])?;
            Ok(EntryArgumentValue::Identity {
                store_catalog_id: json_catalog_id_field(path, object, "store_catalog_id")?,
                keys: json_scalar_array_field(path, object, "keys", depth + 1)?,
            })
        }
        "sequence" => {
            reject_unknown_fields(path, object, &["kind", "value"])?;
            Ok(EntryArgumentValue::Sequence(json_value_array_field(
                path,
                object,
                "value",
                depth + 1,
            )?))
        }
        _ => Err(json_error(
            EntryArgumentJsonErrorKind::UnsupportedKind,
            path,
        )),
    }
}

fn entry_scalar_argument_from_json(
    path: &str,
    value: &Json,
    depth: usize,
) -> Result<EntryScalarArgument, EntryArgumentJsonError> {
    match entry_argument_value_from_json(path, value, depth)? {
        EntryArgumentValue::Scalar(value) => Ok(value),
        _ => Err(json_error(EntryArgumentJsonErrorKind::ExpectedScalar, path)),
    }
}

fn json_object<'a>(
    path: &str,
    value: &'a Json,
) -> Result<&'a Map<String, Json>, EntryArgumentJsonError> {
    value
        .as_object()
        .ok_or_else(|| json_error(EntryArgumentJsonErrorKind::ExpectedObject, path))
}

fn reject_unknown_fields(
    path: &str,
    object: &Map<String, Json>,
    allowed: &[&str],
) -> Result<(), EntryArgumentJsonError> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(json_field_error(
                EntryArgumentJsonErrorKind::UnknownField,
                path,
                key,
            ));
        }
    }
    Ok(())
}

fn json_string_field<'a>(
    path: &str,
    object: &'a Map<String, Json>,
    key: &str,
) -> Result<&'a str, EntryArgumentJsonError> {
    object
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| json_field_error(EntryArgumentJsonErrorKind::ExpectedString, path, key))
}

fn json_bool_field(
    path: &str,
    object: &Map<String, Json>,
    key: &str,
) -> Result<bool, EntryArgumentJsonError> {
    object
        .get(key)
        .and_then(Json::as_bool)
        .ok_or_else(|| json_field_error(EntryArgumentJsonErrorKind::ExpectedBool, path, key))
}

fn json_i64_string_field(
    path: &str,
    object: &Map<String, Json>,
    key: &str,
) -> Result<i64, EntryArgumentJsonError> {
    match decode_value(
        json_string_field(path, object, key)?.as_bytes(),
        ScalarType::Int,
    ) {
        Some(SavedValue::Int(value)) => Ok(value),
        _ => Err(json_field_error(
            EntryArgumentJsonErrorKind::ExpectedIntegerString,
            path,
            key,
        )),
    }
}

fn json_i128_string_field(
    path: &str,
    object: &Map<String, Json>,
    key: &str,
) -> Result<i128, EntryArgumentJsonError> {
    json_string_field(path, object, key)?
        .parse::<i128>()
        .map_err(|_| json_field_error(EntryArgumentJsonErrorKind::ExpectedIntegerString, path, key))
}

fn json_decimal_field(
    path: &str,
    object: &Map<String, Json>,
    key: &str,
) -> Result<Decimal, EntryArgumentJsonError> {
    Decimal::parse_canonical(json_string_field(path, object, key)?)
        .map_err(|_| json_field_error(EntryArgumentJsonErrorKind::InvalidDecimal, path, key))
}

fn json_date_string_field(
    path: &str,
    object: &Map<String, Json>,
    key: &str,
) -> Result<i32, EntryArgumentJsonError> {
    match decode_value(
        json_string_field(path, object, key)?.as_bytes(),
        ScalarType::Date,
    ) {
        Some(SavedValue::Date(value)) => Ok(value),
        _ => Err(json_field_error(
            EntryArgumentJsonErrorKind::InvalidDate,
            path,
            key,
        )),
    }
}

fn json_bytes_hex_field(
    path: &str,
    object: &Map<String, Json>,
    key: &str,
) -> Result<Vec<u8>, EntryArgumentJsonError> {
    let text = json_string_field(path, object, key)?;
    let Some(bytes) = crate::hex::decode(text) else {
        return Err(json_field_error(
            EntryArgumentJsonErrorKind::InvalidBytes,
            path,
            key,
        ));
    };
    if crate::hex::encode(&bytes) != text {
        return Err(json_field_error(
            EntryArgumentJsonErrorKind::InvalidBytes,
            path,
            key,
        ));
    }
    Ok(bytes)
}

fn json_catalog_id_field(
    path: &str,
    object: &Map<String, Json>,
    key: &str,
) -> Result<CatalogId, EntryArgumentJsonError> {
    CatalogId::new(json_string_field(path, object, key)?.to_string())
        .map_err(|_| json_field_error(EntryArgumentJsonErrorKind::InvalidCatalogId, path, key))
}

fn json_scalar_array_field(
    path: &str,
    object: &Map<String, Json>,
    key: &str,
    depth: usize,
) -> Result<Vec<EntryScalarArgument>, EntryArgumentJsonError> {
    let values = object
        .get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| json_field_error(EntryArgumentJsonErrorKind::ExpectedArray, path, key))?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            entry_scalar_argument_from_json(
                &format!("{path} field `{key}` item {index}"),
                value,
                depth,
            )
        })
        .collect()
}

fn json_value_array_field(
    path: &str,
    object: &Map<String, Json>,
    key: &str,
    depth: usize,
) -> Result<Vec<EntryArgumentValue>, EntryArgumentJsonError> {
    let values = object
        .get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| json_field_error(EntryArgumentJsonErrorKind::ExpectedArray, path, key))?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            entry_argument_value_from_json(
                &format!("{path} field `{key}` item {index}"),
                value,
                depth,
            )
        })
        .collect()
}

fn json_error(kind: EntryArgumentJsonErrorKind, path: &str) -> EntryArgumentJsonError {
    EntryArgumentJsonError {
        kind,
        path: path.to_string(),
        field: None,
    }
}

fn json_field_error(
    kind: EntryArgumentJsonErrorKind,
    path: &str,
    field: &str,
) -> EntryArgumentJsonError {
    EntryArgumentJsonError {
        kind,
        path: path.to_string(),
        field: Some(field.to_string()),
    }
}

impl<'p> CheckedEntryCall<'p> {
    pub fn new(
        program: &'p CheckedRuntimeProgram,
        entry: &str,
        args: Vec<Value>,
    ) -> Result<Self, RuntimeError> {
        let target = entry_target(program, entry)?;
        let (_, function) = function_by_ref(program, target, SourceSpan::default())?;
        let args = canonicalize_entry_args(program, function, args)?;
        let identity = entry_identity(program, entry)?;
        Ok(Self {
            program,
            target,
            args,
            identity,
        })
    }

    pub fn from_text_args(
        program: &'p CheckedRuntimeProgram,
        entry: &str,
        args: &[(&str, &str)],
    ) -> Result<Self, RuntimeError> {
        let target = entry_target(program, entry)?;
        let (_, function) = function_by_ref(program, target, SourceSpan::default())?;
        let args = decode_entry_text_args(program, function, args)?;
        let identity = entry_identity(program, entry)?;
        Ok(Self {
            program,
            target,
            args,
            identity,
        })
    }

    fn from_protocol_args(
        program: &'p CheckedRuntimeProgram,
        identity: &EntryIdentity,
        args: &[EntryArgument],
    ) -> Result<Self, RuntimeError> {
        let target =
            entry_target(program, &identity.canonical_name).map_err(|_| stale_entry_identity())?;
        let descriptor = entry_descriptor(program, &identity.canonical_name)
            .map_err(|_| stale_entry_identity())?;
        admit_entry_identity(identity, &descriptor.identity)?;
        let args = decode_entry_protocol_args(program, &descriptor.parameters, args)?;
        Ok(Self {
            program,
            target,
            args,
            identity: descriptor.identity,
        })
    }

    pub fn from_protocol_invocation(
        program: &'p CheckedRuntimeProgram,
        invocation: &EntryInvocation,
    ) -> Result<Self, RuntimeError> {
        Self::from_protocol_args(program, &invocation.identity, &invocation.arguments)
    }

    pub fn identity(&self) -> &EntryIdentity {
        &self.identity
    }
}

pub fn run_entry(
    store: &TreeStore,
    call: &CheckedEntryCall<'_>,
    output: &mut dyn RunOutputSink,
) -> Result<RunOutput, RuntimeError> {
    run_entry_with_host(store, &Host::new(), call, output)
}

pub fn evaluate_checked_read_only_expression(
    store: &TreeStore,
    program: &CheckedRuntimeProgram,
    expression: &CheckedReadOnlyExpression,
    output: &mut dyn RunOutputSink,
) -> Result<RunOutput, RuntimeError> {
    if expression.source_digest() != program.source_digest() {
        return Err(unsupported(
            "a checked read-only expression from a different checked program",
            SourceSpan::default(),
        ));
    }
    if expression.read_only_context_digest() != program.read_only_context_digest() {
        return Err(unsupported(
            "a checked read-only expression from a different checked program",
            SourceSpan::default(),
        ));
    }
    let module = program
        .modules()
        .get(expression.file_id().0 as usize)
        .ok_or_else(|| {
            unsupported(
                "a checked read-only expression whose source module is missing",
                SourceSpan::default(),
            )
        })?;
    if module.source_file != expression.source_file() {
        return Err(unsupported(
            "a checked read-only expression whose source file no longer matches",
            SourceSpan::default(),
        ));
    }

    let output: Rc<RefCell<dyn RunOutputSink + '_>> =
        Rc::new(RefCell::new(ForwardOutput { sink: output }));
    let host = Host::new();
    let ctx = Context {
        program,
        store,
        host: &host,
        transaction: Rc::new(RefCell::new(TransactionState::default())),
    };
    let mut env = Env::new(ctx, output, Some(module), None, 1);
    env.push_scope();
    let value = (|| {
        bind_module_constants(Some(module), &mut env)?;
        eval_expr(expression.expression(), &mut env)
    })();
    env.pop_scope();
    value
        .map(|value| RunOutput { value: Some(value) })
        .map_err(|error| error.with_origin_from(program, Some(module)))
}

pub fn run_entry_with_host(
    store: &TreeStore,
    host: &Host,
    call: &CheckedEntryCall<'_>,
    output: &mut dyn RunOutputSink,
) -> Result<RunOutput, RuntimeError> {
    run_entry_impl(store, host, call, output, None)
}

pub fn run_entry_with_debugger(
    store: &TreeStore,
    host: &Host,
    hook: &mut dyn StepHook,
    call: &CheckedEntryCall<'_>,
    output: &mut dyn RunOutputSink,
) -> Result<RunOutput, RuntimeError> {
    run_entry_impl(store, host, call, output, Some(hook))
}

fn run_entry_impl<'p>(
    store: &'p TreeStore,
    host: &'p Host,
    call: &'p CheckedEntryCall<'p>,
    output: &'p mut dyn RunOutputSink,
    hook: Option<&'p mut dyn StepHook>,
) -> Result<RunOutput, RuntimeError> {
    let program = call.program;
    let target = call.target;
    let (module, function) = function_by_ref(program, target, SourceSpan::default())?;
    let args = &call.args;
    let output: Rc<RefCell<dyn RunOutputSink + 'p>> = match &host.output {
        Some(output) => Rc::clone(output),
        None => Rc::new(RefCell::new(ForwardOutput { sink: output })),
    };
    let names: Vec<&str> = function
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    let ctx = Context {
        program,
        store,
        host,
        transaction: Rc::new(RefCell::new(TransactionState::default())),
    };
    let value = match invoke(Invocation {
        ctx,
        output: Rc::clone(&output),
        module: Some(module),
        param_names: &names,
        body: executable_body(function)?,
        span: function.span,
        args,
        traversed_layers: &[],
        hook,
        depth: 1,
    })? {
        (Completion::Returned(value), ..) => value,
        (Completion::ReturnedAbsent, ..) => None,
        (
            Completion::Threw {
                error,
                span,
                origin,
                transaction_escape,
            },
            ..,
        ) => {
            return Err(raise_with_transaction_escape(
                error,
                span,
                origin,
                transaction_escape,
            ));
        }
        (
            Completion::Faulted {
                code,
                message,
                span,
                origin,
                transaction_escape,
            },
            ..,
        ) => {
            return Err(reraise_fault_with_transaction_escape(
                code,
                message,
                span,
                origin,
                transaction_escape,
            ));
        }
    };
    Ok(RunOutput { value })
}

fn entry_target(
    program: &CheckedRuntimeProgram,
    entry: &str,
) -> Result<marrow_check::CheckedFunctionRef, RuntimeError> {
    match program.entry_function_ref(entry) {
        CheckedEntryFunction::Found(target) => Ok(target),
        CheckedEntryFunction::Ambiguous => Err(ambiguous_function(entry, SourceSpan::default())),
        CheckedEntryFunction::Private => Err(private_function(entry, SourceSpan::default())),
        CheckedEntryFunction::Missing => Err(unknown_function(entry, SourceSpan::default())),
    }
}

fn entry_identity(
    program: &CheckedRuntimeProgram,
    requested: &str,
) -> Result<EntryIdentity, RuntimeError> {
    entry_descriptor(program, requested).map(|descriptor| descriptor.identity)
}

fn entry_descriptor(
    program: &CheckedRuntimeProgram,
    requested: &str,
) -> Result<EntryDescriptor, RuntimeError> {
    EntryDescriptor::resolve(program, requested).map_err(|error| match error {
        EntryDescriptorError::Ambiguous => ambiguous_function(requested, SourceSpan::default()),
        EntryDescriptorError::Private => private_function(requested, SourceSpan::default()),
        EntryDescriptorError::Missing => unknown_function(requested, SourceSpan::default()),
    })
}

fn admit_entry_identity(
    expected: &EntryIdentity,
    current: &EntryIdentity,
) -> Result<(), RuntimeError> {
    if current.entry_tag == expected.entry_tag
        && current.read_only_context_digest == expected.read_only_context_digest
    {
        return Ok(());
    }
    Err(stale_entry_identity())
}

fn stale_entry_identity() -> RuntimeError {
    entry_argument("entry descriptor identity does not match the checked program")
}

fn canonicalize_entry_args(
    program: &CheckedRuntimeProgram,
    function: &CheckedRuntimeFunction,
    args: Vec<Value>,
) -> Result<Vec<Value>, RuntimeError> {
    let names: Vec<&str> = function
        .entry_params()
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    check_argument_count(&names, &args, SourceSpan::default())?;
    function
        .entry_params()
        .iter()
        .zip(args)
        .map(|(param, value)| canonical_entry_value(program, &param.ty, value, &param.name))
        .collect()
}

fn canonical_entry_value(
    program: &CheckedRuntimeProgram,
    ty: &CheckedRuntimeValueType,
    value: Value,
    name: &str,
) -> Result<Value, RuntimeError> {
    if let Some(value) = canonical_entry_value_impl(program, ty, value) {
        return Ok(value);
    }
    Err(entry_type_error(name, SourceSpan::default()))
}

fn canonical_entry_value_impl(
    program: &CheckedRuntimeProgram,
    expected: &CheckedRuntimeValueType,
    value: Value,
) -> Option<Value> {
    match expected {
        CheckedRuntimeValueType::Primitive(scalar) => {
            (value_scalar_type(&value) == Some(*scalar)).then_some(value)
        }
        CheckedRuntimeValueType::Enum {
            enum_id,
            allowed_members,
            ..
        } => {
            let Some(enum_id) = enum_id else {
                return None;
            };
            let Value::Enum(value) = value else {
                return None;
            };
            if value.enum_id != *enum_id || !allowed_members.contains(&value.member_id) {
                return None;
            }
            enum_value_from_member(program.facts(), value.member_id).map(Value::Enum)
        }
        CheckedRuntimeValueType::Sequence(element) => match value {
            // The entry boundary marshals a sequence as its stored values in position
            // order, skipping holes, so canonicalization densifies to those values.
            Value::Sequence(items) => items
                .into_values()
                .into_iter()
                .map(|item| canonical_entry_value_impl(program, element, item))
                .collect::<Option<Vec<_>>>()
                .map(|values| Value::Sequence(Sequence::dense(values))),
            _ => None,
        },
        CheckedRuntimeValueType::Identity { root, .. } => match value {
            Value::Identity(identity) => {
                let store = program.facts().store_by_root(root)?;
                (identity.root() == root.as_str() && store.identity_keys_match(identity.keys()))
                    .then_some(Value::Identity(identity))
            }
            _ => None,
        },
        CheckedRuntimeValueType::Resource
        | CheckedRuntimeValueType::GroupEntry
        | CheckedRuntimeValueType::LocalTree { .. }
        | CheckedRuntimeValueType::Error
        | CheckedRuntimeValueType::Invalid
        | CheckedRuntimeValueType::Unknown => None,
    }
}

fn decode_entry_text_args(
    program: &CheckedRuntimeProgram,
    function: &CheckedRuntimeFunction,
    supplied: &[(&str, &str)],
) -> Result<Vec<Value>, RuntimeError> {
    let params = function.entry_params();
    let mut slots: Vec<Vec<&str>> = vec![Vec::new(); params.len()];
    for (name, value) in supplied {
        let Some(index) = params.iter().position(|param| param.name == *name) else {
            return Err(entry_argument(format!(
                "entry argument `{name}` does not name a parameter"
            )));
        };
        slots[index].push(*value);
    }
    params
        .iter()
        .zip(slots)
        .map(|(param, values)| decode_entry_param(program, &param.ty, &param.name, values))
        .collect()
}

fn decode_entry_protocol_args(
    program: &CheckedRuntimeProgram,
    params: &[EntryParameter],
    supplied: &[EntryArgument],
) -> Result<Vec<Value>, RuntimeError> {
    let mut slots: Vec<Option<&EntryArgumentValue>> = vec![None; params.len()];
    for argument in supplied {
        let Some(index) = params.iter().position(|param| param.name == argument.name) else {
            return Err(entry_argument(format!(
                "entry argument `{}` does not name a parameter",
                argument.name
            )));
        };
        if slots[index].replace(&argument.value).is_some() {
            return Err(entry_argument(format!(
                "entry argument `{}` was supplied more than once",
                argument.name
            )));
        }
    }
    params
        .iter()
        .zip(slots)
        .map(|(param, value)| {
            let Some(value) = value else {
                return Err(entry_argument(format!(
                    "entry argument `{}` is required",
                    param.name
                )));
            };
            decode_entry_protocol_value(program, &param.shape, &param.name, value)
        })
        .collect()
}

fn decode_entry_protocol_value(
    program: &CheckedRuntimeProgram,
    shape: &EntryArgumentShape,
    name: &str,
    value: &EntryArgumentValue,
) -> Result<Value, RuntimeError> {
    match shape {
        EntryArgumentShape::Scalar(scalar) => {
            let EntryArgumentValue::Scalar(value) = value else {
                return Err(entry_argument(format!(
                    "entry argument `{name}` is not {}",
                    scalar.indefinite()
                )));
            };
            protocol_scalar_value(*scalar, value).ok_or_else(|| {
                entry_argument(format!(
                    "entry argument `{name}` is not {}",
                    scalar.indefinite()
                ))
            })
        }
        EntryArgumentShape::Enum {
            catalog_id,
            members,
            ..
        } => {
            let EntryArgumentValue::EnumMember { member_catalog_id } = value else {
                return Err(entry_argument(format!(
                    "entry argument `{name}` is not an enum member"
                )));
            };
            decode_protocol_enum_arg(program, catalog_id, members, name, member_catalog_id)
        }
        EntryArgumentShape::Identity {
            store_catalog_id: expected_store_catalog_id,
            keys: expected_keys,
            ..
        } => {
            let EntryArgumentValue::Identity {
                store_catalog_id,
                keys,
            } = value
            else {
                return Err(entry_argument(format!(
                    "entry argument `{name}` is not an identity"
                )));
            };
            decode_protocol_identity_arg(
                program,
                name,
                expected_store_catalog_id,
                expected_keys,
                store_catalog_id,
                keys,
            )
        }
        EntryArgumentShape::Sequence(element) => {
            let EntryArgumentValue::Sequence(items) = value else {
                return Err(entry_argument(format!(
                    "entry argument `{name}` is not a sequence"
                )));
            };
            decode_protocol_sequence(program, element, name, items)
        }
        EntryArgumentShape::Unsupported => Err(entry_argument(format!(
            "entry argument `{name}` has a type outside the run entry argument surface"
        ))),
    }
}

fn decode_protocol_sequence(
    program: &CheckedRuntimeProgram,
    element: &EntryArgumentShape,
    name: &str,
    values: &[EntryArgumentValue],
) -> Result<Value, RuntimeError> {
    values
        .iter()
        .map(|value| decode_entry_protocol_value(program, element, name, value))
        .collect::<Result<Vec<_>, _>>()
        .map(|values| Value::Sequence(Sequence::dense(values)))
}

fn decode_protocol_identity_arg(
    program: &CheckedRuntimeProgram,
    name: &str,
    expected_store_catalog_id: &CatalogId,
    expected_keys: &[marrow_check::EntryIdentityKey],
    store_catalog_id: &CatalogId,
    values: &[EntryScalarArgument],
) -> Result<Value, RuntimeError> {
    if store_catalog_id != expected_store_catalog_id {
        return Err(entry_argument(format!(
            "entry argument `{name}` belongs to a different identity store"
        )));
    }
    if values.len() != expected_keys.len() {
        return Err(entry_argument(format!(
            "entry argument `{name}` does not match the identity key shape"
        )));
    }
    let keys: Option<Vec<_>> = values
        .iter()
        .zip(expected_keys)
        .map(|(value, expected)| protocol_scalar_key(expected.scalar, value))
        .collect();
    let Some(keys) = keys else {
        return Err(entry_argument(format!(
            "entry argument `{name}` has an unsupported identity key type"
        )));
    };
    let Some(store) = program
        .facts()
        .stores()
        .iter()
        .find(|store| store.catalog_id.as_deref() == Some(store_catalog_id.as_str()))
    else {
        return Err(entry_argument(format!(
            "entry argument `{name}` belongs to a different identity store"
        )));
    };
    if !store.identity_keys_match(&keys) {
        return Err(entry_argument(format!(
            "entry argument `{name}` does not match the identity key shape"
        )));
    }
    Ok(Value::Identity(crate::value::IdentityValue::for_root(
        store.root.clone(),
        keys,
    )))
}

fn decode_entry_param(
    program: &CheckedRuntimeProgram,
    ty: &CheckedRuntimeValueType,
    name: &str,
    values: Vec<&str>,
) -> Result<Value, RuntimeError> {
    if values.is_empty() {
        return Err(entry_argument(format!(
            "entry argument `{name}` is required"
        )));
    }
    match ty {
        CheckedRuntimeValueType::Sequence(element) => {
            decode_entry_sequence(program, element, name, values)
        }
        _ => {
            if values.len() != 1 {
                return Err(entry_argument(format!(
                    "entry argument `{name}` was supplied more than once"
                )));
            }
            decode_entry_value(program, ty, name, values[0])
        }
    }
}

fn decode_protocol_enum_arg(
    program: &CheckedRuntimeProgram,
    expected_enum_catalog_id: &CatalogId,
    allowed_members: &[marrow_check::EntryEnumMember],
    name: &str,
    member_catalog_id: &CatalogId,
) -> Result<Value, RuntimeError> {
    if !allowed_members
        .iter()
        .any(|member| &member.catalog_id == member_catalog_id)
    {
        return Err(entry_argument(format!(
            "entry argument `{name}` is not an accepted enum member"
        )));
    };
    let matches: Vec<_> = program
        .facts()
        .enum_members()
        .iter()
        .filter(|member| member.catalog_id.as_deref() == Some(member_catalog_id.as_str()))
        .filter(|member| {
            program
                .facts()
                .enum_(member.enum_id)
                .is_some_and(|enum_fact| {
                    enum_fact.catalog_id.as_deref() == Some(expected_enum_catalog_id.as_str())
                })
        })
        .map(|member| member.id)
        .collect();
    let [member_id] = matches.as_slice() else {
        return Err(entry_argument(format!(
            "entry argument `{name}` is not an accepted enum member"
        )));
    };
    enum_value_from_member(program.facts(), *member_id)
        .map(Value::Enum)
        .ok_or_else(|| entry_argument(format!("entry argument `{name}` is not selectable")))
}

fn decode_entry_sequence(
    program: &CheckedRuntimeProgram,
    element: &CheckedRuntimeValueType,
    name: &str,
    values: Vec<&str>,
) -> Result<Value, RuntimeError> {
    if !entry_sequence_element_supported(element) {
        return Err(entry_argument(format!(
            "entry argument `{name}` has an unsupported sequence element type"
        )));
    }
    if values == ["[]"] {
        return Ok(Value::Sequence(Sequence::default()));
    }
    if values.contains(&"[]") {
        return Err(entry_argument(format!(
            "entry argument `{name}` uses [] only as the whole empty sequence"
        )));
    }
    values
        .into_iter()
        .map(|value| decode_entry_value(program, element, name, value))
        .collect::<Result<Vec<_>, _>>()
        .map(|values| Value::Sequence(Sequence::dense(values)))
}

fn entry_sequence_element_supported(ty: &CheckedRuntimeValueType) -> bool {
    matches!(
        ty,
        CheckedRuntimeValueType::Primitive(_) | CheckedRuntimeValueType::Enum { .. }
    )
}

fn protocol_scalar_value(scalar: ScalarType, value: &EntryScalarArgument) -> Option<Value> {
    match (scalar, value) {
        (ScalarType::Int, EntryScalarArgument::Int(value)) => Some(Value::Int(*value)),
        (ScalarType::Bool, EntryScalarArgument::Bool(value)) => Some(Value::Bool(*value)),
        (ScalarType::Str, EntryScalarArgument::String(value)) => Some(Value::Str(value.clone())),
        (ScalarType::Instant, EntryScalarArgument::Instant(value))
            if supported_instant_nanos(*value) =>
        {
            Some(Value::Instant(*value))
        }
        (ScalarType::Date, EntryScalarArgument::Date(value)) if supported_date_days(*value) => {
            Some(Value::Date(*value))
        }
        (ScalarType::Duration, EntryScalarArgument::Duration(value)) => {
            Some(Value::Duration(*value))
        }
        (ScalarType::Decimal, EntryScalarArgument::Decimal(value)) => Some(Value::Decimal(*value)),
        (ScalarType::Bytes, EntryScalarArgument::Bytes(value)) => Some(Value::Bytes(value.clone())),
        _ => None,
    }
}

fn protocol_scalar_key(scalar: ScalarType, value: &EntryScalarArgument) -> Option<SavedKey> {
    match (scalar, value) {
        (ScalarType::Int, EntryScalarArgument::Int(value)) => Some(SavedKey::Int(*value)),
        (ScalarType::Bool, EntryScalarArgument::Bool(value)) => Some(SavedKey::Bool(*value)),
        (ScalarType::Str, EntryScalarArgument::String(value)) => Some(SavedKey::Str(value.clone())),
        (ScalarType::Instant, EntryScalarArgument::Instant(value))
            if supported_instant_nanos(*value) =>
        {
            Some(SavedKey::Instant(*value))
        }
        (ScalarType::Date, EntryScalarArgument::Date(value)) if supported_date_days(*value) => {
            Some(SavedKey::Date(*value))
        }
        (ScalarType::Duration, EntryScalarArgument::Duration(value)) => {
            Some(SavedKey::Duration(*value))
        }
        (ScalarType::Bytes, EntryScalarArgument::Bytes(value)) => {
            Some(SavedKey::Bytes(value.clone()))
        }
        _ => None,
    }
}

fn decode_entry_value(
    program: &CheckedRuntimeProgram,
    ty: &CheckedRuntimeValueType,
    name: &str,
    text: &str,
) -> Result<Value, RuntimeError> {
    match ty {
        CheckedRuntimeValueType::Primitive(scalar) => {
            decode_scalar(*scalar, text).ok_or_else(|| {
                entry_argument(format!(
                    "entry argument `{name}` is not {}",
                    scalar.indefinite()
                ))
            })
        }
        CheckedRuntimeValueType::Enum {
            enum_id,
            allowed_members,
            ..
        } => decode_enum_arg(program, *enum_id, allowed_members, name, text),
        CheckedRuntimeValueType::Identity { root, .. } => {
            decode_identity_arg(program, root, name, text)
        }
        CheckedRuntimeValueType::Sequence(_)
        | CheckedRuntimeValueType::Resource
        | CheckedRuntimeValueType::GroupEntry
        | CheckedRuntimeValueType::LocalTree { .. }
        | CheckedRuntimeValueType::Error
        | CheckedRuntimeValueType::Invalid
        | CheckedRuntimeValueType::Unknown => Err(entry_argument(format!(
            "entry argument `{name}` has a type outside the run entry argument surface"
        ))),
    }
}

fn decode_scalar(scalar: ScalarType, text: &str) -> Option<Value> {
    if scalar == ScalarType::Str {
        return Some(Value::Str(text.to_string()));
    }
    let (expression, diagnostics) = parse_expression(text);
    if !diagnostics.is_empty() {
        return None;
    }
    let value = eval_scalar_arg_expression(scalar, expression.as_ref()?)?;
    (value_scalar_type(&value) == Some(scalar)).then_some(value)
}

fn scalar_key(scalar: ScalarType, text: &str) -> Option<SavedKey> {
    value_to_key(decode_scalar(scalar, text)?, SourceSpan::default())
        .ok()
        .flatten()
}

fn eval_scalar_arg_expression(scalar: ScalarType, expression: &Expression) -> Option<Value> {
    match expression {
        Expression::Literal { kind, text, span } => {
            let value = crate::expr::eval_literal(lower_literal_kind(*kind), text, *span).ok()?;
            (value_scalar_type(&value) == Some(scalar)).then_some(value)
        }
        // A negative numeric literal is a single minus directly prefixing a
        // numeric literal token, as the grammar spells it. The operand must be a
        // bare numeric literal whose span begins immediately after the minus, so
        // a nested sign (`--5`), a gap (`- 5`), or a parenthesized operand
        // (`-(5)`) is not a literal spelling and falls through to rejection.
        Expression::Unary {
            op: marrow_syntax::UnaryOp::Neg,
            operand,
            span,
        } => {
            let Expression::Literal {
                span: operand_span, ..
            } = operand.as_ref()
            else {
                return None;
            };
            if operand_span.start_byte != span.start_byte + 1 {
                return None;
            }
            match eval_scalar_arg_expression(scalar, operand)? {
                Value::Int(n) => n.checked_neg().map(Value::Int),
                Value::Decimal(d) => {
                    Decimal::from_parts(-d.coefficient(), d.scale()).map(Value::Decimal)
                }
                _ => None,
            }
        }
        Expression::Call { callee, args, .. } => {
            let Expression::Name { segments, .. } = callee.as_ref() else {
                return None;
            };
            let [name] = segments.as_slice() else {
                return None;
            };
            let [arg] = args.as_slice() else {
                return None;
            };
            if arg.name.is_some() || *name != scalar.name() {
                return None;
            }
            let Value::Str(text) = eval_scalar_arg_expression(ScalarType::Str, &arg.value)? else {
                return None;
            };
            // An instant from text shares the `instant(...)` constructor's wider
            // standard RFC-3339 input surface; date reads through the canonical
            // store decoder.
            match scalar {
                ScalarType::Instant => parse_rfc3339_instant_nanos(&text).map(Value::Instant),
                ScalarType::Date => match decode_value(text.as_bytes(), scalar)? {
                    SavedValue::Date(days) => Some(Value::Date(days)),
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

fn lower_literal_kind(kind: marrow_syntax::LiteralKind) -> CheckedLiteralKind {
    match kind {
        marrow_syntax::LiteralKind::Integer => CheckedLiteralKind::Integer,
        marrow_syntax::LiteralKind::Decimal => CheckedLiteralKind::Decimal,
        marrow_syntax::LiteralKind::Duration => CheckedLiteralKind::Duration,
        marrow_syntax::LiteralKind::String => CheckedLiteralKind::String,
        marrow_syntax::LiteralKind::Bytes => CheckedLiteralKind::Bytes,
        marrow_syntax::LiteralKind::Bool => CheckedLiteralKind::Bool,
    }
}

fn decode_enum_arg(
    program: &CheckedRuntimeProgram,
    enum_id: Option<marrow_check::EnumId>,
    allowed_members: &[marrow_check::EnumMemberId],
    name: &str,
    text: &str,
) -> Result<Value, RuntimeError> {
    let Some(enum_id) = enum_id else {
        return Err(entry_argument(format!(
            "entry argument `{name}` has an unresolved enum type"
        )));
    };
    let matches: Vec<_> = allowed_members
        .iter()
        .copied()
        .filter(|member_id| {
            program
                .facts()
                .enum_member(*member_id)
                .is_some_and(|member| member.enum_id == enum_id)
                && program
                    .facts()
                    .enum_member_catalog_path(*member_id)
                    .is_some_and(|path| enum_spelling_matches(&path, text))
        })
        .collect();
    let [member_id] = matches.as_slice() else {
        return Err(entry_argument(format!(
            "entry argument `{name}` is not an accepted enum member"
        )));
    };
    enum_value_from_member(program.facts(), *member_id)
        .map(Value::Enum)
        .ok_or_else(|| entry_argument(format!("entry argument `{name}` is not selectable")))
}

fn enum_spelling_matches(path: &str, text: &str) -> bool {
    path == text
        || path
            .rsplit("::")
            .next()
            .is_some_and(|member| member == text)
        || path
            .strip_suffix(text)
            .is_some_and(|prefix| prefix.ends_with("::"))
}

fn decode_identity_arg(
    program: &CheckedRuntimeProgram,
    root: &str,
    name: &str,
    text: &str,
) -> Result<Value, RuntimeError> {
    let Some(store) = program.facts().store_by_root(root) else {
        return Err(entry_argument(format!(
            "entry argument `{name}` references unknown store `^{root}`"
        )));
    };
    let [key] = store.identity_keys.as_slice() else {
        return Err(entry_argument(format!(
            "entry argument `{name}` references composite identity `^{root}`; expose a wrapper entry with scalar key parameters"
        )));
    };
    let Some(StoredValueMeaning::Scalar(scalar)) = key.value_meaning else {
        return Err(entry_argument(format!(
            "entry argument `{name}` identity key is outside the scalar surface"
        )));
    };
    let Some(key) = scalar_key(scalar, text) else {
        return Err(entry_argument(format!(
            "entry argument `{name}` is not a valid key for `^{root}`"
        )));
    };
    Ok(crate::value::identity_value(root, vec![key]))
}
