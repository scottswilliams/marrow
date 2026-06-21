//! Call checking: dispatch in runtime order (special builtins, general builtins,
//! resource constructors, then user functions), each branch's argument rules,
//! and the special-form builtins `nextId`/`next`/`prev`/`append`. Returns
//! the call's declared return type when known.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{ResourceSchema, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::executable::{SavedPlaceResolver, lower_expr_for_file};
use crate::resolve::resolve_store_by_root;
use crate::typerules::{
    as_primitive, expects_conversion, marrow_type_name, mismatch_display, type_compatible,
};
use crate::{
    AppendTargetDiagnostic, CHECK_AMBIGUOUS_CALL, CHECK_CALL_ARGUMENT,
    CHECK_COLLECTION_UNSUPPORTED, CHECK_KEY_REQUIRES_SINGLE_KEY, CHECK_NEIGHBOR_UNSUPPORTED,
    CHECK_NEXT_ID_REQUIRES_SINGLE_INT, CHECK_PRIVATE_FUNCTION, CHECK_UNRESOLVED_CALL,
    CHECK_UNTYPED_VALUE, CheckDiagnostic, CheckedModule, CheckedProgram, ConversionTarget,
    ConversionUnsupportedSourceDiagnostic, Def, DefItem, DiagnosticPayload, MarrowType, Resolution,
    ResolvableKind, TypeNames, builtin_return_type, conversion_return_type,
    identity_type_for_store, is_builtin_call, is_unknown_std_operation, module_of_file, resolve,
    resource_type_name, std_call_params, std_call_return_type,
};

use super::collections::{
    collection_loop_binding_types, has_collection_unsupported, is_concrete_scalar_value,
    is_recognized_collection, is_saved_index_range_path, is_saved_key_range_path,
    is_saved_unique_index_branch_path, saved_path_key_type,
};
use super::diagnostics::{call_diagnostic, key_type_diagnostic};
use super::saved_keys::check_keys_against;

pub(crate) struct CallCheck<'a> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) callee: &'a marrow_syntax::Expression,
    pub(crate) args: &'a [marrow_syntax::Argument],
    pub(crate) arg_types: &'a [MarrowType],
    pub(crate) scope: &'a [HashMap<String, MarrowType>],
    pub(crate) aliases: &'a HashMap<String, Vec<String>>,
    pub(crate) span: SourceSpan,
    pub(crate) file: &'a Path,
    pub(crate) transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
    pub(crate) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

struct CallEnv<'a> {
    program: &'a CheckedProgram,
    scope: &'a [HashMap<String, MarrowType>],
    aliases: &'a HashMap<String, Vec<String>>,
    span: SourceSpan,
    file: &'a Path,
    transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
}

/// Validate a call and return its declared return type when known. Dispatch is
/// kept in runtime order: special builtins, general builtins, constructors, then
/// user functions.
pub(crate) fn check_call(input: CallCheck<'_>) -> MarrowType {
    let CallCheck {
        program,
        callee,
        args,
        arg_types,
        scope,
        aliases,
        span,
        file,
        transform_old,
        diagnostics,
    } = input;
    let mut env = CallEnv {
        program,
        scope,
        aliases,
        span,
        file,
        transform_old,
        diagnostics,
    };
    let marrow_syntax::Expression::Name { segments, .. } = callee else {
        return MarrowType::Unknown;
    };
    let expanded = crate::expand_alias(segments, aliases);
    let segments = expanded.as_slice();

    if let Some(ty) = check_special_single_name_call(&mut env, segments, args, arg_types) {
        return ty;
    }
    if is_builtin_call(segments) {
        return check_builtin_call(&mut env, segments, args, arg_types);
    }
    if is_unknown_std_operation(segments) {
        check_unknown_std_operation(&mut env, segments);
        return MarrowType::Unknown;
    }

    let from_module = module_of_file(env.program, env.file).unwrap_or_default();
    if let Some(ty) =
        check_resource_constructor_call(&mut env, from_module, segments, args, arg_types)
    {
        return ty;
    }
    check_user_function_call(&mut env, from_module, segments, args, arg_types)
}

fn check_special_single_name_call(
    env: &mut CallEnv<'_>,
    segments: &[String],
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> Option<MarrowType> {
    let [name] = segments else {
        return None;
    };
    match name.as_str() {
        "nextId" => Some(check_next_id(
            env.program,
            args,
            arg_types,
            env.span,
            env.file,
            env.diagnostics,
        )),
        "Id" => Some(check_identity_constructor(
            env.program,
            args,
            arg_types,
            env.span,
            env.file,
            env.diagnostics,
        )),
        "reversed" => {
            check_arity(name, 1, args, env.span, env.file, env.diagnostics);
            // A rejected argument has no stream to reverse; typing the result `invalid`
            // (not the argument's own scalar type) keeps a typed consumer from stacking a
            // second diagnostic on the one root-cause error.
            if check_collection_combinator_args(env, name, args, arg_types) {
                return Some(MarrowType::Invalid);
            }
            Some(reversed_type(env, args, arg_types))
        }
        "next" | "prev" => {
            check_arity(name, 1, args, env.span, env.file, env.diagnostics);
            Some(check_neighbor(env, name, args, arg_types))
        }
        "key" => {
            check_arity(name, 1, args, env.span, env.file, env.diagnostics);
            Some(check_key(env, arg_types))
        }
        // `entries` is validated comprehensively by the two-name-loop-head and
        // value-position rules (it is valid nowhere else), so its scalar and
        // wrapped-traversal arguments are reported there; double-checking here would
        // duplicate the diagnostic. `keys`/`values` have no such owner, so they take
        // the shared combinator argument rule.
        "keys" | "values" => {
            check_arity(name, 1, args, env.span, env.file, env.diagnostics);
            let rejected = check_collection_combinator_args(env, name, args, arg_types);
            if !rejected {
                check_index_branch_wrapper_args(env, name, args);
            }
            // A rejected argument has no stream to key or materialize; typing the
            // result `invalid` (not `unknown`) keeps a typed consumer from stacking a
            // second `check.untyped_value` on the one root-cause error. A valid argument
            // stays `unknown` for the saved-shape typing path.
            Some(if rejected {
                MarrowType::Invalid
            } else {
                MarrowType::Unknown
            })
        }
        "entries" => {
            check_arity(name, 1, args, env.span, env.file, env.diagnostics);
            check_index_branch_wrapper_args(env, name, args);
            Some(MarrowType::Unknown)
        }
        "append" => {
            check_arity(name, 2, args, env.span, env.file, env.diagnostics);
            check_append_args(env, args);
            check_append(env, args);
            check_append_error_code_literal(env, args);
            Some(MarrowType::Primitive(ScalarType::Int))
        }
        _ => None,
    }
}

fn check_builtin_call(
    env: &mut CallEnv<'_>,
    segments: &[String],
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> MarrowType {
    let label = segments.join("::");
    if segments == ["Error"] {
        check_error_constructor_args(args, arg_types, env.span, env.file, env.diagnostics);
        return MarrowType::Error;
    }
    if check_builtin_call_args(env, segments, args, arg_types) {
        // `count` of a rejected argument has nothing to count; typing the result
        // `invalid` (not `unknown`) keeps a typed consumer from stacking a second
        // `check.untyped_value` on the one root-cause error.
        return MarrowType::Invalid;
    }
    if segments == ["std", "assert", "equal"] {
        check_assert_equal_args(&label, arg_types, env.span, env.file, env.diagnostics);
        return std_call_return_type(segments).unwrap_or(MarrowType::Unknown);
    }
    if let Some(params) = std_call_params(segments) {
        check_std_call_args(env, segments, args);
        check_std_collection_args(env, &label, args, &params);
        check_args_against(
            &label,
            &params,
            arg_types,
            env.span,
            env.file,
            env.diagnostics,
        );
    }
    std_call_return_type(segments)
        .or_else(|| conversion_return_type(segments))
        .or_else(|| builtin_return_type(segments))
        .unwrap_or(MarrowType::Unknown)
}

fn check_assert_equal_args(
    label: &str,
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if arg_types.len() != 2 {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "`{label}` expects 2 argument(s), but {} were given",
                arg_types.len(),
            ),
        ));
        return;
    }
    match (&arg_types[0], &arg_types[1]) {
        (MarrowType::Primitive(actual), MarrowType::Primitive(expected)) if actual == expected => {}
        (MarrowType::Primitive(_), MarrowType::Primitive(_)) => {
            let (expected, found) = mismatch_display(&arg_types[0], &arg_types[1]);
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("argument to `{label}` expects `{expected}`, but found `{found}`"),
            ));
        }
        (MarrowType::Unknown, _) | (_, MarrowType::Unknown) => {}
        (actual, expected) => {
            let found = if matches!(actual, MarrowType::Primitive(_)) {
                marrow_type_name(expected)
            } else {
                marrow_type_name(actual)
            };
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("argument to `{label}` expects a scalar, but found `{found}`"),
            ));
        }
    }
}

fn check_unknown_std_operation(env: &mut CallEnv<'_>, segments: &[String]) {
    let label = segments.join("::");
    env.diagnostics.push(
        CheckDiagnostic::error(
            CHECK_UNRESOLVED_CALL,
            env.file,
            env.span,
            format!("`{label}` is not a standard-library operation"),
        )
        .with_payload(DiagnosticPayload::UnresolvedCall(label.to_string())),
    );
}

fn check_resource_constructor_call(
    env: &mut CallEnv<'_>,
    from_module: &str,
    segments: &[String],
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> Option<MarrowType> {
    let Resolution::Found(Def {
        module,
        item: DefItem::Resource(resource),
        ..
    }) = resolve(env.program, from_module, segments, ResolvableKind::Resource)
    else {
        return None;
    };
    check_resource_constructor_args(ResourceConstructorCheck {
        program: env.program,
        label: &resource.name,
        module,
        resource,
        args,
        arg_types,
        span: env.span,
        file: env.file,
        diagnostics: env.diagnostics,
    });
    Some(MarrowType::Resource(resource_type_name(
        &module.name,
        &resource.name,
    )))
}

fn check_user_function_call(
    env: &mut CallEnv<'_>,
    from_module: &str,
    segments: &[String],
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> MarrowType {
    let function = match resolve(env.program, from_module, segments, ResolvableKind::Function) {
        Resolution::Found(Def {
            item: DefItem::Function(function),
            ..
        }) => function,
        Resolution::NotVisible(name) => {
            if file_in_program(env.program, env.file) {
                env.diagnostics.push(CheckDiagnostic::error(
                    CHECK_PRIVATE_FUNCTION,
                    env.file,
                    env.span,
                    format!(
                        "function `{name}` is private to its module; mark it `pub` to call it \
                         from another module"
                    ),
                ));
            }
            return MarrowType::Unknown;
        }
        Resolution::Ambiguous(candidates) => {
            if file_in_program(env.program, env.file) {
                let leaf = segments.join("::");
                let options = candidates
                    .iter()
                    .map(|module| format!("`{module}::{leaf}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                env.diagnostics.push(CheckDiagnostic::error(
                    CHECK_AMBIGUOUS_CALL,
                    env.file,
                    env.span,
                    format!("call to `{leaf}` is ambiguous; qualify it as one of {options}"),
                ));
            }
            return MarrowType::Unknown;
        }
        Resolution::Found(_) | Resolution::Unresolved => {
            if file_in_program(env.program, env.file) {
                let name = segments.join("::");
                env.diagnostics.push(
                    CheckDiagnostic::error(
                        CHECK_UNRESOLVED_CALL,
                        env.file,
                        env.span,
                        format!("function `{name}` is not defined"),
                    )
                    .with_payload(DiagnosticPayload::UnresolvedCall(name)),
                );
            }
            return MarrowType::Unknown;
        }
    };

    let callee = segments.join("::");
    if args.len() != function.params.len() {
        env.diagnostics.push(call_diagnostic(
            env.file,
            env.span,
            format!(
                "function `{callee}` expects {} argument(s), but {} were given",
                function.params.len(),
                args.len(),
            ),
        ));
    }
    let mut supplied = vec![false; function.params.len()];
    for (index, (arg, arg_type)) in args.iter().zip(arg_types).enumerate() {
        let param_index = match &arg.name {
            Some(name) => {
                let param_index = function.params.iter().position(|param| &param.name == name);
                if param_index.is_none() {
                    env.diagnostics.push(call_diagnostic(
                        env.file,
                        env.span,
                        format!("function `{callee}` has no parameter `{name}`"),
                    ));
                }
                param_index
            }
            None => function.params.get(index).map(|_| index),
        };
        if let Some(param_index) = param_index {
            let param = &function.params[param_index];
            if supplied[param_index] {
                env.diagnostics.push(
                    CheckDiagnostic::error(
                        CHECK_CALL_ARGUMENT,
                        env.file,
                        env.span,
                        format!(
                            "function `{callee}` parameter `{}` is supplied more than once",
                            param.name
                        ),
                    )
                    .with_payload(DiagnosticPayload::DuplicateNamedArgument(
                        param.name.clone(),
                    )),
                );
                continue;
            }
            supplied[param_index] = true;
            if reject_saved_collection_by_value(
                env.program,
                &callee,
                &arg.value,
                &param.ty,
                env.scope,
                env.file,
                env.diagnostics,
            ) {
                continue;
            }
            check_one_arg(
                &callee,
                &param.ty,
                arg_type,
                env.span,
                env.file,
                env.diagnostics,
            );
        }
    }
    function.return_type.clone().unwrap_or(MarrowType::Unknown)
}

/// Validate a single-name builtin's arguments, returning whether a collection
/// combinator (`count`) rejected its argument so the caller can type the result
/// `invalid`.
fn check_builtin_call_args(
    env: &mut CallEnv<'_>,
    segments: &[String],
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> bool {
    let [name] = segments else { return false };
    if name.as_str() == "exists" {
        check_exists_args(env, args);
        return false;
    }
    if name.as_str() == "count" {
        return check_collection_combinator_args(env, "count", args, arg_types);
    }
    if let Some(target) = ConversionTarget::from_name(name) {
        check_conversion_call_shape(target, args, env.span, env.file, env.diagnostics);
        check_conversion_arg(target, arg_types, env.span, env.file, env.diagnostics);
        if target == ConversionTarget::ErrorCode {
            check_error_code_conversion_literal(args, env.file, env.diagnostics);
        }
    }
    false
}

fn check_std_call_args(
    env: &mut CallEnv<'_>,
    segments: &[String],
    args: &[marrow_syntax::Argument],
) {
    if segments == ["std", "assert", "absent"] {
        check_assert_absent_args(env, args);
    }
}

/// Apply the saved-collection-by-value rejection to a std helper's positional
/// arguments. Std helpers are positional-only, so the parameter at each index types
/// the matching argument; a `sequence[T]` parameter (`text::join`, `csv::row`) is the
/// same by-value collection a user function takes, so it shares the one rejecter.
fn check_std_collection_args(
    env: &mut CallEnv<'_>,
    label: &str,
    args: &[marrow_syntax::Argument],
    params: &[Option<MarrowType>],
) {
    for (arg, parameter) in args.iter().zip(params) {
        if let Some(parameter) = parameter {
            reject_saved_collection_by_value(
                env.program,
                label,
                &arg.value,
                parameter,
                env.scope,
                env.file,
                env.diagnostics,
            );
        }
    }
}

fn check_assert_absent_args(env: &mut CallEnv<'_>, args: &[marrow_syntax::Argument]) {
    let [arg] = args else { return };
    if !assert_absent_arg_is_saved_path(env, &arg.value) {
        env.diagnostics.push(call_diagnostic(
            env.file,
            env.span,
            "`std::assert::absent` expects a saved path".to_string(),
        ));
    }
}

fn assert_absent_arg_is_saved_path(env: &CallEnv<'_>, expr: &marrow_syntax::Expression) -> bool {
    if is_saved_index_range_path(env.program, expr, env.scope, env.file) {
        return true;
    }
    if is_saved_key_range_path(env.program, expr, env.scope, env.file) {
        return false;
    }
    lower_expr_for_file(env.program, env.file, expr, env.scope)
        .is_some_and(|expr| expr.saved_place().is_some())
}

/// Whether a collection combinator's argument is a concrete non-iterable scalar. A
/// recognized saved layer or local collection is excluded first, so a keyed-leaf
/// layer's leaf scalar type is not mistaken for a non-iterable value.
fn combinator_arg_is_scalar(
    env: &CallEnv<'_>,
    arg: &marrow_syntax::Expression,
    arg_type: &MarrowType,
) -> bool {
    !is_recognized_collection(env.program, arg, env.scope, env.aliases, env.file)
        && is_concrete_scalar_value(arg, arg_type)
}

fn check_exists_args(env: &mut CallEnv<'_>, args: &[marrow_syntax::Argument]) {
    let [arg] = args else { return };
    if !exists_target_arg_resolves(env, &arg.value) {
        env.diagnostics.push(call_diagnostic(
            env.file,
            env.span,
            "`exists` expects a saved path".to_string(),
        ));
    }
}

fn exists_target_arg_resolves(env: &CallEnv<'_>, expr: &marrow_syntax::Expression) -> bool {
    if is_saved_index_range_path(env.program, expr, env.scope, env.file) {
        return true;
    }
    if is_saved_key_range_path(env.program, expr, env.scope, env.file) {
        return false;
    }
    let Some(expr) = lower_expr_for_file(env.program, env.file, expr, env.scope) else {
        return false;
    };
    crate::presence::exists_target_in_type_scope(env.program, &expr, env.scope, env.transform_old)
}

/// The call-site context for a named-field constructor check: the constructor
/// `label` used in diagnostics, the supplied `args` and their `arg_types`, the
/// call `span`, the source `file`, and the diagnostic sink.
struct NamedFieldArgs<'a> {
    label: &'a str,
    args: &'a [marrow_syntax::Argument],
    arg_types: &'a [MarrowType],
    span: SourceSpan,
    file: &'a Path,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
}

/// Check named-field constructor arguments against a fixed field list: reject
/// unnamed args, unknown field names, and duplicate-supplied fields, type-check
/// each supplied value against its expected type, and require every required
/// field. `field_name` reads a field's name, `expected_type` yields the type to
/// check a supplied field against (when one applies), and `is_required` decides
/// whether a missing field is an error.
fn check_named_field_args<F>(
    call: NamedFieldArgs<'_>,
    fields: &[F],
    field_name: impl Fn(&F) -> &str,
    expected_type: impl Fn(usize) -> Option<MarrowType>,
    is_required: impl Fn(&F) -> bool,
) {
    let NamedFieldArgs {
        label,
        args,
        arg_types,
        span,
        file,
        diagnostics,
    } = call;
    let mut supplied = vec![false; fields.len()];
    for (arg, arg_type) in args.iter().zip(arg_types) {
        let Some(name) = &arg.name else {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` constructor takes named fields"),
            ));
            continue;
        };
        let Some(index) = fields.iter().position(|field| field_name(field) == name) else {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` has no field `{name}`"),
            ));
            continue;
        };
        if supplied[index] {
            diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_CALL_ARGUMENT,
                    file,
                    span,
                    format!("field `{name}` is supplied more than once"),
                )
                .with_payload(DiagnosticPayload::DuplicateNamedArgument(name.clone())),
            );
            continue;
        }
        supplied[index] = true;
        if let Some(expected) = expected_type(index) {
            check_one_arg(label, &expected, arg_type, span, file, diagnostics);
        }
    }

    for (field, supplied) in fields.iter().zip(supplied) {
        if is_required(field) && !supplied {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` requires `{}`", field_name(field)),
            ));
        }
    }
}

/// Check an `Error(...)` constructor against the named-field contract owned by
/// `marrow_schema::error`; every required field must be supplied. The field set
/// lives in the schema so the checker and runtime validate one definition.
fn check_error_constructor_args(
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let fields = marrow_schema::error::fields();
    check_named_field_args(
        NamedFieldArgs {
            label: "Error",
            args,
            arg_types,
            span,
            file,
            diagnostics,
        },
        fields,
        |field| field.name,
        |index| {
            Some(MarrowType::from_resolved(
                fields[index].ty.clone(),
                TypeNames::default(),
            ))
        },
        |field| field.required,
    );
    check_error_constructor_code_literal(args, file, diagnostics);
}

fn check_error_constructor_code_literal(
    args: &[marrow_syntax::Argument],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for arg in args {
        if arg.name.as_deref() == Some(marrow_schema::error::CODE) {
            check_error_code_literal(&arg.value, "`Error.code`", file, diagnostics);
        }
    }
}

fn check_error_code_conversion_literal(
    args: &[marrow_syntax::Argument],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [arg] = args else { return };
    check_error_code_literal(&arg.value, "`ErrorCode(...)`", file, diagnostics);
}

/// Reject a string literal that does not satisfy the dotted-lowercase error-code
/// grammar, naming the offending place with `label`. The one literal-validation
/// entrypoint shared by the `ErrorCode(...)` constructor, the `Error.code` field,
/// and a literal coerced into an `ErrorCode`-typed place. A non-literal value is
/// left to its run-time coercion.
pub(crate) fn check_error_code_literal(
    expr: &marrow_syntax::Expression,
    label: &str,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let marrow_syntax::Expression::Literal {
        kind: marrow_syntax::LiteralKind::String,
        text,
        span,
    } = expr
    else {
        return;
    };
    let Ok(text) = marrow_syntax::decode_string_literal(text) else {
        return;
    };
    if !marrow_schema::error::is_error_code_text(&text) {
        diagnostics.push(call_diagnostic(
            file,
            *span,
            format!("{label} expects a dotted lowercase error code"),
        ));
    }
}

fn check_conversion_call_shape(
    target: ConversionTarget,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let label = target.spelling();
    check_arity(label, 1, args, span, file, diagnostics);
    if let [arg] = args
        && let Some(name) = &arg.name
    {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!("argument to `{label}` cannot be named `{name}`"),
        ));
    }
}

/// The shared argument rule for the value-materializing combinators (`count`/`keys`/
/// `values`) and `reversed`: a concrete scalar has nothing to traverse, and an
/// argument that yields a saved stream this combinator cannot consume lazily would
/// fault at runtime. `count`/`keys`/`values` reject any combinator already wrapping a
/// saved traversal; `reversed` reverses `keys`/`values`/`entries` streams in place and
/// rejects only a re-`reversed` saved stream. Either is a check error rather than a
/// deferred runtime fault. Returns whether the argument is rejected, so the caller
/// types the result `invalid` and the enclosing `for` does not re-report the same root
/// cause. When the argument is an inner combinator that already reported its own error
/// at the argument span, the outer combinator counts the argument rejected but defers
/// to that single diagnostic instead of pushing a duplicate.
fn check_collection_combinator_args(
    env: &mut CallEnv<'_>,
    name: &str,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> bool {
    let ([arg], [arg_type]) = (args, arg_types) else {
        return false;
    };
    if arg.name.is_some() {
        return false;
    }
    // An inner combinator over a scalar types its result a scalar, so the outer
    // combinator sees one too. Defer to the inner combinator's already-reported error
    // rather than re-flagging the same root cause, while still treating the argument as
    // rejected so the result types `invalid`.
    if has_collection_unsupported(env.diagnostics, env.file, arg.value.span()) {
        return true;
    }
    if combinator_arg_is_scalar(env, &arg.value, arg_type) {
        env.diagnostics.push(CheckDiagnostic::error(
            CHECK_COLLECTION_UNSUPPORTED,
            env.file,
            env.span,
            format!("`{name}` needs a collection, but this value is a scalar"),
        ));
        return true;
    }
    let unconsumable = if name == "reversed" {
        crate::rules::is_reversed_over_saved_traversal(&arg.value)
    } else {
        crate::rules::is_wrapped_saved_traversal(&arg.value)
    };
    if unconsumable {
        env.diagnostics.push(CheckDiagnostic::error(
            CHECK_COLLECTION_UNSUPPORTED,
            env.file,
            env.span,
            format!("`{name}` cannot re-materialize a saved traversal; iterate it directly"),
        ));
        return true;
    }
    false
}

/// Reject a loop-wrapper over a unique index branch. A unique branch is a
/// single-identity lookup, not a stream, so it has no key sequence for `keys` to
/// yield and no record stream for `values`/`entries` to materialize. A non-unique
/// branch streams the store identity, so its record materializes through every
/// wrapper, exactly as the bare two-name form does.
fn check_index_branch_wrapper_args(
    env: &mut CallEnv<'_>,
    name: &str,
    args: &[marrow_syntax::Argument],
) {
    let [arg] = args else { return };
    if arg.name.is_some()
        || !is_saved_unique_index_branch_path(env.program, &arg.value, env.scope, env.file)
    {
        return;
    }
    let message = if name == "keys" {
        "`keys` cannot stream keys from a unique index lookup, which addresses a single identity"
            .to_string()
    } else {
        format!("`{name}` cannot materialize values from a unique index lookup; use `keys`")
    };
    env.diagnostics.push(CheckDiagnostic::error(
        CHECK_COLLECTION_UNSUPPORTED,
        env.file,
        env.span,
        message,
    ));
}

fn check_append_args(env: &mut CallEnv<'_>, args: &[marrow_syntax::Argument]) {
    let [target, _value] = args else { return };
    // A multi-column layer is rejected as composite, the more precise diagnostic, so
    // the group-vs-leaf check only speaks for single-column layers.
    if saved_append_target_is_composite(env.program, &target.value, env.scope, env.file) {
        return;
    }
    if !saved_layer_is_group(env.program, &target.value, env.scope, env.file) {
        return;
    }
    env.diagnostics.push(
        CheckDiagnostic::error(
            CHECK_CALL_ARGUMENT,
            env.file,
            env.span,
            "`append` target must be a keyed leaf layer, but this path names a group layer",
        )
        .with_payload(DiagnosticPayload::AppendTarget(
            AppendTargetDiagnostic::GroupLayer,
        )),
    );
}

fn saved_layer_is_group(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    lower_expr_for_file(program, file, expr, scope)
        .and_then(|expr| expr.saved_place().cloned())
        .and_then(|place| place.layers.last().cloned())
        .is_some_and(|layer| layer.leaf.is_none())
}

/// Whether an `append` target's innermost layer declares more than one key column.
/// Such a composite layer is a chain of single-key sub-layers, so it has no single
/// position for `append` to allocate regardless of how many columns the prefix fills.
fn saved_append_target_is_composite(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    lower_expr_for_file(program, file, expr, scope)
        .and_then(|expr| expr.saved_place().cloned())
        .and_then(|place| place.layers.last().cloned())
        .is_some_and(|layer| layer.key_params.len() > 1)
}

/// Reject a string literal appended to a `sequence[ErrorCode]` (or any keyed leaf
/// declared `ErrorCode`), the same grammar gate the constructor and a field write
/// apply. A dynamic value is validated at the append boundary at run.
fn check_append_error_code_literal(env: &mut CallEnv<'_>, args: &[marrow_syntax::Argument]) {
    let [target, value] = args else { return };
    let appends_error_code = lower_expr_for_file(env.program, env.file, &target.value, env.scope)
        .and_then(|expr| expr.saved_place().cloned())
        .and_then(|place| place.layers.last().cloned())
        .is_some_and(|layer| layer.error_code);
    if appends_error_code {
        check_error_code_literal(
            &value.value,
            "an `ErrorCode` sequence",
            env.file,
            env.diagnostics,
        );
    }
}

fn check_conversion_arg(
    target: ConversionTarget,
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [arg_type] = arg_types else { return };
    if target.accepts(arg_type) {
        return;
    }
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_CALL_ARGUMENT,
            file,
            span,
            format!(
                "`{}` cannot convert `{}`; supported sources are {}",
                target.spelling(),
                marrow_type_name(arg_type),
                target.supported_sources_message()
            ),
        )
        .with_payload(DiagnosticPayload::ConversionUnsupportedSource(
            ConversionUnsupportedSourceDiagnostic {
                target,
                source: arg_type.clone(),
                accepted_sources: target.accepted_source_types(),
            },
        )),
    );
}

fn reversed_type(
    env: &CallEnv<'_>,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> MarrowType {
    if let [arg] = args
        && arg.name.is_none()
        && let Some((element, None)) =
            collection_loop_binding_types(env.program, false, &arg.value, env.scope, env.file)
    {
        return MarrowType::Sequence(Box::new(element));
    }
    if let Some(MarrowType::LocalTree { keys, .. }) = arg_types.first() {
        return MarrowType::Sequence(Box::new(
            keys.first().cloned().unwrap_or(MarrowType::Unknown),
        ));
    }
    arg_types.first().cloned().unwrap_or(MarrowType::Unknown)
}

struct ResourceConstructorCheck<'a> {
    program: &'a CheckedProgram,
    label: &'a str,
    module: &'a CheckedModule,
    resource: &'a ResourceSchema,
    args: &'a [marrow_syntax::Argument],
    arg_types: &'a [MarrowType],
    span: SourceSpan,
    file: &'a Path,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
}

fn check_resource_constructor_args(input: ResourceConstructorCheck<'_>) {
    let ResourceConstructorCheck {
        program,
        label,
        module,
        resource,
        args,
        arg_types,
        span,
        file,
        diagnostics,
    } = input;
    let fields: Vec<&marrow_schema::Node> = resource
        .members
        .iter()
        .filter(|node| node.is_plain_field())
        .collect();
    check_named_field_args(
        NamedFieldArgs {
            label,
            args,
            arg_types,
            span,
            file,
            diagnostics,
        },
        &fields,
        |field| field.name.as_str(),
        |index| {
            fields[index]
                .plain_field_type()
                .map(|ty| constructor_field_type(program, module, ty))
        },
        |field| {
            matches!(
                &field.kind,
                marrow_schema::NodeKind::Slot { required: true, .. }
            )
        },
    );
    check_constructor_error_code_literals(label, &fields, args, file, diagnostics);
}

/// Reject a string literal supplied for an `ErrorCode` field of a resource
/// constructor, the same gate the constructor's own `ErrorCode(...)` applies. A
/// dynamic value is validated at the write boundary.
fn check_constructor_error_code_literals(
    label: &str,
    fields: &[&marrow_schema::Node],
    args: &[marrow_syntax::Argument],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for arg in args {
        let Some(name) = &arg.name else { continue };
        if fields
            .iter()
            .any(|field| field.name == *name && field.is_error_code())
        {
            check_error_code_literal(&arg.value, &format!("`{label}.{name}`"), file, diagnostics);
        }
    }
}

fn constructor_field_type(
    program: &CheckedProgram,
    module: &CheckedModule,
    ty: &Type,
) -> MarrowType {
    crate::enums::resolve_schema_type_for_module(ty, program, module)
}

/// Check one positional/named argument against the type its parameter expects: a
/// known-but-different type is a `check.call_argument`; an `Unknown` argument for a
/// concrete parameter is a `check.untyped_value` (strict typing). Shared by the
/// user-function and std argument loops; `label` names the callee for the message.
pub(crate) fn check_one_arg(
    label: &str,
    parameter: &MarrowType,
    arg_type: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match type_compatible(parameter, arg_type) {
        Some(true) => {}
        Some(false) => {
            // `mismatch_display` qualifies two same-named enums from different
            // modules so the message distinguishes them.
            let (expected, found) = mismatch_display(parameter, arg_type);
            diagnostics.push(
                call_diagnostic(
                    file,
                    span,
                    format!("argument to `{label}` expects `{expected}`, but found `{found}`"),
                )
                .with_payload(DiagnosticPayload::TypeMismatch {
                    expected: parameter.clone(),
                    found: arg_type.clone(),
                }),
            );
        }
        // Strict typing: an untyped argument against a convertible parameter must be
        // converted first.
        None if matches!(arg_type, MarrowType::Unknown) && expects_conversion(parameter) => {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_UNTYPED_VALUE,
                file,
                span,
                format!(
                    "argument to `{label}` has no known type, but `{}` is expected; convert it first",
                    marrow_type_name(parameter),
                ),
            ));
        }
        None => {}
    }
}

/// Whether a slot holds a local collection by value (a local sequence or keyed map).
/// A saved collection has no local materialization, so it can never fill such a slot —
/// a by-value parameter or a declared return type alike; every other shape is
/// irrelevant to that rejection.
pub(crate) fn is_by_value_collection_slot(slot: &MarrowType) -> bool {
    matches!(slot, MarrowType::Sequence(_) | MarrowType::LocalTree { .. })
}

/// Whether `expr` materializes a saved collection by value: either a bare saved
/// collection — a store root, a saved keyed sub-layer, or an index branch, all
/// iterated in place with no single value to copy — or one laundered through a
/// value-materializing traversal combinator (`keys`/`values`/`entries`/`reversed`),
/// which yields the same saved stream the runtime refuses to materialize. The bare
/// case excludes a single saved value (a scalar leaf, a whole record) so a legitimate
/// value copy stays valid; the wrapped case is recognized syntactically, since a
/// combinator over a saved path is a saved stream regardless of scope. The one
/// classifier shared by every by-value boundary — a `const`/`var` binding, a local
/// assignment, a declared return, a by-value parameter, and a std-helper argument — so
/// none lets the un-materializable stream check clean and fault at runtime.
pub(crate) fn materializes_saved_collection_by_value(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    if crate::rules::is_wrapped_saved_traversal(expr) {
        return true;
    }
    lower_expr_for_file(program, file, expr, scope)
        .is_some_and(|checked| SavedPlaceResolver::new(program).is_saved_collection(&checked))
}

/// Reject a saved collection passed to a by-value local-collection parameter. A store
/// root, a saved keyed sub-layer, an index branch, or one laundered through
/// `keys`/`values` is iterated in place, never materialized into a local value, so
/// passing it by value would silently fault at runtime. The single owner of this
/// argument rule, shared by the user-function and std argument loops: it fires only
/// when the parameter is a by-value collection and the argument materializes a saved
/// collection, leaving a local collection or a single saved value (a scalar leaf, a
/// whole record) untouched. Returns whether the argument was rejected so the caller
/// skips the plain compatibility check, whose `Unknown` saved-collection type would
/// otherwise defer.
fn reject_saved_collection_by_value(
    program: &CheckedProgram,
    label: &str,
    arg: &marrow_syntax::Expression,
    parameter: &MarrowType,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> bool {
    if !is_by_value_collection_slot(parameter) {
        return false;
    }
    if !materializes_saved_collection_by_value(program, arg, scope, file) {
        return false;
    }
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_CALL_ARGUMENT,
            file,
            arg.span(),
            format!(
                "argument to `{label}` is a saved collection, which is iterated in place, not \
                 passed by value into `{}`; iterate it or build a local collection",
                marrow_type_name(parameter),
            ),
        )
        .with_payload(DiagnosticPayload::SavedCollectionByValue {
            parameter: parameter.clone(),
        }),
    );
    true
}

/// Check positional `args` against a fixed positional parameter list (the std
/// helper signatures): an arity mismatch is a `check.call_argument`, and each
/// argument with a known-required parameter type is checked by [`check_one_arg`].
/// A `None` parameter slot (e.g. a path argument) is left alone. Std helpers are
/// positional-only — named-argument matching stays user-function-only.
pub(crate) fn check_args_against(
    label: &str,
    params: &[Option<MarrowType>],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if arg_types.len() != params.len() {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "`{label}` expects {} argument(s), but {} were given",
                params.len(),
                arg_types.len(),
            ),
        ));
    }
    for (parameter, arg_type) in params.iter().zip(arg_types) {
        if let Some(parameter) = parameter {
            check_one_arg(label, parameter, arg_type, span, file, diagnostics);
        }
    }
}

/// Type `Id(^root, key...)`, the explicit identity constructor. The first
/// argument names the saved root; the remaining arguments fill the root's declared
/// identity keys using the same nominal keyspace rules as saved lookups.
fn check_identity_constructor(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    for arg in args {
        if arg.name.is_some() {
            diagnostics.push(call_diagnostic(
                file,
                arg.value.span(),
                "`Id` arguments must be positional".to_string(),
            ));
        }
    }
    let Some(root_arg) = args.first() else {
        diagnostics.push(call_diagnostic(
            file,
            span,
            "`Id` expects a saved root followed by its key argument(s)".to_string(),
        ));
        return MarrowType::Unknown;
    };
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = &root_arg.value else {
        diagnostics.push(call_diagnostic(
            file,
            root_arg.value.span(),
            "`Id` expects a saved root as its first argument".to_string(),
        ));
        return MarrowType::Unknown;
    };
    let Some(store) = resolve_store_by_root(program, root) else {
        diagnostics.push(key_type_diagnostic(
            file,
            root_arg.value.span(),
            format!("identity constructor root `^{root}` is not declared"),
        ));
        return MarrowType::Unknown;
    };
    if store.store.identity_keys.is_empty() {
        diagnostics.push(key_type_diagnostic(
            file,
            root_arg.value.span(),
            format!("identity constructor root `^{root}` has no identity keys"),
        ));
        return MarrowType::Unknown;
    }
    check_keys_against(
        &store.store.identity_keys,
        arg_types.get(1..).unwrap_or(&[]),
        span,
        file,
        diagnostics,
    );
    identity_type_for_store(store.store)
}

/// Type `nextId(^root)` and gate it on a single-`int` saved root, which types to
/// `Id(^root)`; any other identity shape reports
/// `check.next_id_requires_single_int`. The argument must be a bare keyed store
/// root — the only shape the runtime can allocate against — so `check.call_argument`
/// rejects a saved path that is not a bare root (an index branch, keyed lookup, or
/// field) on shape, and a concrete non-saved argument (a literal, an identity value,
/// or a scalar) on type. An undeclared root is reported by resolution, and an
/// `unknown`-typed non-saved argument defers to a cross-module result, so neither is
/// double-reported here.
pub(crate) fn check_next_id(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    let [arg] = args else {
        return MarrowType::Unknown;
    };
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = &arg.value else {
        // A saved path that is not a bare `^root` — an index branch, a keyed
        // lookup, a field — is statically the wrong shape regardless of the type
        // it lowers to (an index branch lowers to `unknown`), so reject it on
        // shape. A non-saved argument is rejected only when its type is concrete;
        // an `unknown`-typed non-saved value defers to a cross-module result.
        if crate::rules::is_saved_path(&arg.value) {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_CALL_ARGUMENT,
                file,
                span,
                "`nextId` requires a bare keyed store root (`^store`), \
                 not a saved path into one",
            ));
        } else if let Some(arg_type) = arg_types
            .first()
            .filter(|ty| !matches!(ty, MarrowType::Unknown))
        {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_CALL_ARGUMENT,
                file,
                span,
                format!(
                    "`nextId` requires a keyed store root (`^store`), but this argument is `{}`",
                    marrow_type_name(arg_type)
                ),
            ));
        }
        return MarrowType::Unknown;
    };
    let Some(store) = resolve_store_by_root(program, root) else {
        return MarrowType::Unknown;
    };
    if store.store.single_int_root() {
        return identity_type_for_store(store.store);
    }
    diagnostics.push(CheckDiagnostic::error(
        CHECK_NEXT_ID_REQUIRES_SINGLE_INT,
        file,
        span,
        format!(
            "`nextId` requires a store with one `int` identity key, but `^{root}` \
             ({}) has no default allocation policy; composite and non-integer \
             identities are application-provided",
            store.store.next_id_shape(),
        ),
    ));
    MarrowType::Unknown
}

/// Type `next(<element>)` / `prev(<element>)`. A keyed root or single-key record
/// navigates among record identities (result `Id(^root)`); a keyed or bare child
/// layer navigates among that layer's keys (result the layer's key type). A
/// composite-identity record and an index branch would fault uncatchably at
/// runtime, so each is reported as a compile error. Any other shape is left
/// `Unknown` for the runtime, where a surrounding `??` still types the default.
fn check_neighbor(
    env: &mut CallEnv<'_>,
    which: &str,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> MarrowType {
    let [arg] = args else {
        return MarrowType::Unknown;
    };
    let Some(checked) = lower_expr_for_file(env.program, env.file, &arg.value, env.scope) else {
        if matches!(arg_types.first(), Some(MarrowType::Identity(_))) {
            return neighbor_unsupported(
                which,
                "an identity value (use a saved place)",
                env.span,
                env.file,
                env.diagnostics,
            );
        }
        return MarrowType::Unknown;
    };
    let resolver = SavedPlaceResolver::new(env.program);
    if resolver.is_index_branch(&checked) {
        return neighbor_unsupported(
            which,
            "an index branch",
            env.span,
            env.file,
            env.diagnostics,
        );
    }
    match &checked {
        crate::CheckedExpr::SavedRoot { name, .. } => {
            if composite_identity(env.program, name) {
                return neighbor_unsupported(
                    which,
                    "a composite-identity root (scope a single key level)",
                    env.span,
                    env.file,
                    env.diagnostics,
                );
            }
            resolver.key_type(&checked).unwrap_or(MarrowType::Unknown)
        }
        crate::CheckedExpr::Call { callee, .. }
            if matches!(callee.as_ref(), crate::CheckedExpr::SavedRoot { .. }) =>
        {
            if let crate::CheckedExpr::SavedRoot { name, .. } = callee.as_ref()
                && composite_identity(env.program, name)
            {
                return neighbor_unsupported(
                    which,
                    "a composite-identity record (scope a single key level)",
                    env.span,
                    env.file,
                    env.diagnostics,
                );
            }
            resolver.key_type(callee).unwrap_or(MarrowType::Unknown)
        }
        crate::CheckedExpr::Call { .. } | crate::CheckedExpr::Field { .. } => {
            resolver.key_type(&checked).unwrap_or(MarrowType::Unknown)
        }
        _ if matches!(arg_types.first(), Some(MarrowType::Identity(_))) => neighbor_unsupported(
            which,
            "an identity value (use a saved place)",
            env.span,
            env.file,
            env.diagnostics,
        ),
        _ => MarrowType::Unknown,
    }
}

/// Type `key(id)`, projecting a single-key store identity to its scalar key. A
/// composite identity has no single key to project — it is reconstructed as a
/// whole value, never a tuple of raw components — so it reports
/// `check.key_requires_single_key`. A concrete non-identity argument has no
/// identity to project, so it is rejected with `check.call_argument`; an
/// `unknown`-typed argument defers to a cross-module result, and an unresolved
/// root is reported elsewhere, so neither is double-reported here.
fn check_key(env: &mut CallEnv<'_>, arg_types: &[MarrowType]) -> MarrowType {
    let Some(MarrowType::Identity(root)) = arg_types.first() else {
        if let Some(arg_type) = arg_types
            .first()
            .filter(|ty| !matches!(ty, MarrowType::Unknown))
        {
            env.diagnostics.push(CheckDiagnostic::error(
                CHECK_CALL_ARGUMENT,
                env.file,
                env.span,
                format!(
                    "`key` requires a store identity (`Id(^store)`), but this argument is `{}`",
                    marrow_type_name(arg_type)
                ),
            ));
        }
        return MarrowType::Unknown;
    };
    let Some(store) = resolve_store_by_root(env.program, root) else {
        return MarrowType::Unknown;
    };
    let [single_key] = store.store.identity_keys.as_slice() else {
        env.diagnostics.push(CheckDiagnostic::error(
            CHECK_KEY_REQUIRES_SINGLE_KEY,
            env.file,
            env.span,
            format!(
                "`key` projects a single identity key, but `^{root}` ({}) is reconstructed \
                 as a whole identity value, not a single key",
                store.store.next_id_shape(),
            ),
        ));
        return MarrowType::Unknown;
    };
    single_key
        .ty
        .scalar()
        .map(MarrowType::Primitive)
        .unwrap_or(MarrowType::Unknown)
}

/// Check `append(layer, value)` against the statically declared layer key kind.
/// `append` allocates an integer position, so accepting a string- or bool-keyed
/// layer would create stored keys the schema cannot address.
fn check_append(env: &mut CallEnv<'_>, args: &[marrow_syntax::Argument]) {
    let [target, _] = args else {
        return;
    };
    if target.name.is_some() {
        return;
    }
    // A composite layer is a chain of single-key sub-layers with no single column to
    // allocate a position in, so no shape of it is a valid `append` target — neither
    // the bare outer layer, a partial prefix, nor the full leaf. Reject it before the
    // per-column int check, whose inner-column key type would otherwise admit it.
    if saved_append_target_is_composite(env.program, &target.value, env.scope, env.file) {
        env.diagnostics.push(
            CheckDiagnostic::error(
                CHECK_CALL_ARGUMENT,
                env.file,
                env.span,
                "`append` requires a single int-keyed layer, but this layer keys multiple \
                 columns; allocate a position only in a single-column layer",
            )
            .with_payload(DiagnosticPayload::AppendTarget(
                AppendTargetDiagnostic::CompositeLayer,
            )),
        );
        return;
    }
    let Some(key_type) = saved_path_key_type(env.program, &target.value, env.scope, env.file)
    else {
        return;
    };
    if !matches!(as_primitive(&key_type), Some(ScalarType::Int)) {
        env.diagnostics.push(
            CheckDiagnostic::error(
                CHECK_CALL_ARGUMENT,
                env.file,
                env.span,
                format!(
                    "`append` requires an int-keyed layer, but this layer is keyed by `{}`",
                    marrow_type_name(&key_type)
                ),
            )
            .with_payload(DiagnosticPayload::AppendTarget(
                AppendTargetDiagnostic::NonIntKeyedLayer { key_type },
            )),
        );
    }
}

/// Whether the store at saved root `root` has a composite (multi-key) identity. A
/// non-keyed root or an unknown root is not composite.
pub(crate) fn composite_identity(program: &CheckedProgram, root: &str) -> bool {
    resolve_store_by_root(program, root).is_some_and(|store| store.store.identity_keys.len() > 1)
}

/// Report a `check.neighbor_unsupported` error for a statically-unnavigable
/// `next`/`prev` shape and leave the result `Unknown`.
pub(crate) fn neighbor_unsupported(
    which: &str,
    shape: &str,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    diagnostics.push(CheckDiagnostic::error(
        CHECK_NEIGHBOR_UNSUPPORTED,
        file,
        span,
        format!("`{which}` cannot navigate {shape}"),
    ));
    MarrowType::Unknown
}

/// Report a `check.call_argument` arity diagnostic when a fixed-arity builtin is
/// called with the wrong number of arguments.
pub(crate) fn check_arity(
    name: &str,
    arity: usize,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if args.len() != arity {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "`{name}` expects {arity} argument(s), but {} were given",
                args.len()
            ),
        ));
    }
}

/// Whether `file` contributes a module to the program — a library module or a
/// module-less script. Calls in such a file are resolution-checked; a file
/// excluded by a parse error is not.
pub(crate) fn file_in_program(program: &CheckedProgram, file: &Path) -> bool {
    program
        .modules
        .iter()
        .any(|module| module.source_file == file)
}
