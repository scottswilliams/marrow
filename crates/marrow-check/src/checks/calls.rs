//! Call checking: dispatch in runtime order (special builtins, general builtins,
//! resource constructors, then user functions), each branch's argument rules,
//! and the special-form builtins `nextId`/`next`/`prev`/`append`. Returns
//! the call's declared return type when known.

use std::collections::HashMap;
use std::path::Path;

use marrow_codes::Code;
use marrow_schema::{ResourceSchema, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::executable::{SavedPlaceResolver, lower_expr_for_file};
use crate::model::decls::DeclIds;
use crate::resolve::resolve_store_by_root;
use crate::typerules::{
    as_primitive, expects_conversion, is_optional_value, marrow_type_name, type_compatible,
    unresolved_optional, unresolved_optional_diagnostic,
};
use crate::{
    AppendTargetDiagnostic, CHECK_COLLECTION_UNSUPPORTED, CHECK_KEY_REQUIRES_SINGLE_KEY,
    CHECK_NEIGHBOR_UNSUPPORTED, CHECK_NEXT_ID_REQUIRES_SINGLE_INT, CHECK_UNTYPED_VALUE,
    CallArgumentFault, CallArgumentSlot, CheckDiagnostic, CheckedModule, CheckedProgram,
    ConversionTarget, ConversionUnsupportedSourceDiagnostic, Def, DefItem, DiagnosticAnchor,
    DiagnosticPayload, MarrowType, Resolution, ResolvableKind, UnresolvedCallKind,
    builtin_return_type, conversion_return_type, identity_type_for_store, is_builtin_call,
    is_unknown_std_operation, module_of_file, resolve, std_call_params, std_call_return_type,
};

use super::diagnostics::key_type_diagnostic;
use super::loop_head::is_recognized_collection;
use super::saved_keys::{check_identity_sequence_position, check_keys_against};
use super::saved_paths::{
    is_concrete_scalar_value, is_saved_index_range_path, is_saved_key_range_path,
    saved_path_key_type, saved_path_value_type,
};

/// A `check.call_argument` diagnostic carrying its typed fault, located at a call
/// or argument span. The one construction path for the code's many argument-fault
/// shapes; its prose lives in the diagnostic renderer.
pub(super) fn call_argument(
    names: &DeclIds<'_>,
    file: &Path,
    span: SourceSpan,
    fault: CallArgumentFault,
) -> CheckDiagnostic {
    CheckDiagnostic::new(
        Code::CheckCallArgument,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::CallArgument(fault),
        names,
    )
}

pub(crate) struct CallCheck<'a> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) callee: &'a marrow_syntax::Expression,
    pub(crate) args: &'a [marrow_syntax::Argument],
    pub(crate) arg_types: &'a [MarrowType],
    pub(crate) scope: &'a [HashMap<String, MarrowType>],
    pub(crate) const_ints: &'a [HashMap<String, Option<i64>>],
    pub(crate) aliases: &'a HashMap<String, Vec<String>>,
    pub(crate) span: SourceSpan,
    pub(crate) file: &'a Path,
    pub(crate) read_scope: crate::presence::ReadScope<'a>,
    pub(crate) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

struct CallEnv<'a> {
    program: &'a CheckedProgram,
    scope: &'a [HashMap<String, MarrowType>],
    const_ints: &'a [HashMap<String, Option<i64>>],
    aliases: &'a HashMap<String, Vec<String>>,
    span: SourceSpan,
    file: &'a Path,
    read_scope: crate::presence::ReadScope<'a>,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
}

/// The recovery view and owning file an argument-check diagnostic renders through.
/// They travel together to every emitter in the argument loops while each argument's
/// span varies, so bundling them keeps the per-argument helpers to a single context
/// rather than a pair of parallel parameters.
#[derive(Clone, Copy)]
pub(crate) struct ArgEmit<'a> {
    names: &'a DeclIds<'a>,
    file: &'a Path,
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
        const_ints,
        aliases,
        span,
        file,
        read_scope,
        diagnostics,
    } = input;
    let mut env = CallEnv {
        program,
        scope,
        const_ints,
        aliases,
        span,
        file,
        read_scope,
        diagnostics,
    };
    let marrow_syntax::Expression::Name { segments, .. } = callee else {
        return MarrowType::Unknown;
    };
    let expanded = crate::expand_alias(segments, aliases);
    let segments = expanded.as_slice();

    // One arity owner for every builtin. A fixed-arity builtin called with the wrong
    // number of arguments is a `check.call_argument` here, before any branch below
    // resolves its arguments, so no builtin faults its arity only at runtime.
    if let [name] = segments
        && let Some(builtin) = crate::executable::CheckedBuiltinCall::from_name(name)
    {
        check_arity(
            &env.program.decl_ids(),
            name,
            builtin.arity(),
            args,
            env.span,
            env.file,
            env.diagnostics,
        );
    }

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
            env.const_ints,
            env.span,
            env.file,
            env.diagnostics,
        )),
        "next" | "prev" => Some(check_neighbor(env, name, args, arg_types)),
        "key" => Some(check_key(env, arg_types)),
        // `keys`/`values` build a local sequence in value position: they materialize a
        // local collection's keys or values. Saved data is iterated in place with
        // `for ... in`, never materialized, so a saved-path argument is rejected here.
        "keys" | "values" => {
            let rejected = if let [arg] = args
                && arg.name.is_none()
                && crate::rules::is_saved_path(&arg.value)
            {
                env.diagnostics.push(CheckDiagnostic::error(
                    CHECK_COLLECTION_UNSUPPORTED,
                    env.file,
                    arg.value.span(),
                    format!(
                        "`{name}` materializes a local collection; iterate saved data in place with `for ... in`",
                    ),
                ));
                true
            } else {
                check_collection_combinator_args(env, name, args, arg_types)
            };
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
        "append" => {
            check_append_args(env, args, arg_types);
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
        check_error_constructor_args(
            &env.program.decl_ids(),
            args,
            arg_types,
            env.span,
            env.file,
            env.diagnostics,
        );
        return MarrowType::Error;
    }
    if check_builtin_call_args(env, segments, args, arg_types) {
        // `count` of a rejected argument has nothing to count; typing the result
        // `invalid` (not `unknown`) keeps a typed consumer from stacking a second
        // `check.untyped_value` on the one root-cause error.
        return MarrowType::Invalid;
    }
    if segments == ["std", "assert", "equal"] {
        check_assert_equal_args(
            &env.program.decl_ids(),
            &label,
            arg_types,
            env.span,
            env.file,
            env.diagnostics,
        );
        return std_call_return_type(segments).unwrap_or(MarrowType::Unknown);
    }
    if let Some(params) = std_call_params(segments) {
        check_std_call_args(env, segments, args, arg_types);
        check_std_collection_args(env, &label, args, &params);
        let names = env.program.decl_ids();
        check_args_against(
            ArgEmit {
                names: &names,
                file: env.file,
            },
            &label,
            &params,
            args,
            arg_types,
            env.span,
            env.diagnostics,
        );
    }
    std_call_return_type(segments)
        .or_else(|| conversion_return_type(segments))
        .or_else(|| builtin_return_type(segments))
        .unwrap_or(MarrowType::Unknown)
}

fn check_assert_equal_args(
    names: &DeclIds<'_>,
    label: &str,
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if arg_types.len() != 2 {
        diagnostics.push(call_argument(
            names,
            file,
            span,
            CallArgumentFault::Arity {
                label: label.to_string(),
                expected: 2,
                given: arg_types.len(),
            },
        ));
        return;
    }
    match (&arg_types[0], &arg_types[1]) {
        (MarrowType::Primitive(actual), MarrowType::Primitive(expected)) if actual == expected => {}
        (MarrowType::Primitive(_), MarrowType::Primitive(_)) => {
            diagnostics.push(call_argument(
                names,
                file,
                span,
                CallArgumentFault::AssertEqualMismatch {
                    label: label.to_string(),
                    first: arg_types[0].clone(),
                    second: arg_types[1].clone(),
                },
            ));
        }
        (MarrowType::Unknown, _) | (_, MarrowType::Unknown) => {}
        (actual, expected) => {
            let found = if matches!(actual, MarrowType::Primitive(_)) {
                expected
            } else {
                actual
            };
            diagnostics.push(call_argument(
                names,
                file,
                span,
                CallArgumentFault::AssertEqualNonScalar {
                    label: label.to_string(),
                    found: found.clone(),
                },
            ));
        }
    }
}

fn check_unknown_std_operation(env: &mut CallEnv<'_>, segments: &[String]) {
    let label = segments.join("::");
    env.diagnostics.push(CheckDiagnostic::new(
        Code::CheckUnresolvedCall,
        DiagnosticAnchor::at(env.file, env.span),
        DiagnosticPayload::UnresolvedCall {
            name: label,
            kind: UnresolvedCallKind::StdOperation,
        },
        &env.program.decl_ids(),
    ));
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
    env.program
        .resource_leaf_id(&module.name, &resource.name)
        .map(MarrowType::Resource)
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
                env.diagnostics.push(CheckDiagnostic::new(
                    Code::CheckPrivateFunction,
                    DiagnosticAnchor::at(env.file, env.span),
                    DiagnosticPayload::PrivateFunction(name),
                    &env.program.decl_ids(),
                ));
            }
            return MarrowType::Unknown;
        }
        Resolution::Ambiguous(candidates) => {
            if file_in_program(env.program, env.file) {
                env.diagnostics.push(CheckDiagnostic::new(
                    Code::CheckAmbiguousCall,
                    DiagnosticAnchor::at(env.file, env.span),
                    DiagnosticPayload::AmbiguousCall {
                        leaf: segments.join("::"),
                        candidates,
                    },
                    &env.program.decl_ids(),
                ));
            }
            return MarrowType::Unknown;
        }
        Resolution::Found(_) | Resolution::Unresolved => {
            if file_in_program(env.program, env.file) {
                env.diagnostics.push(CheckDiagnostic::new(
                    Code::CheckUnresolvedCall,
                    DiagnosticAnchor::at(env.file, env.span),
                    DiagnosticPayload::UnresolvedCall {
                        name: segments.join("::"),
                        kind: UnresolvedCallKind::Function,
                    },
                    &env.program.decl_ids(),
                ));
            }
            return MarrowType::Unknown;
        }
    };

    let callee = segments.join("::");
    let mut supplied = vec![false; function.params.len()];
    // A malformed named argument (unknown or duplicated) also perturbs the count.
    // Report that per-argument fault alone; the arity mismatch is its consequence,
    // not a separate error, matching the constructor named-field checker.
    let mut named_argument_fault = false;
    for (index, (arg, arg_type)) in args.iter().zip(arg_types).enumerate() {
        let param_index = match &arg.name {
            Some(name) => {
                let param_index = function.params.iter().position(|param| &param.name == name);
                if param_index.is_none() {
                    named_argument_fault = true;
                    env.diagnostics.push(call_argument(
                        &env.program.decl_ids(),
                        env.file,
                        env.span,
                        CallArgumentFault::UnknownParameter {
                            callee: callee.clone(),
                            parameter: name.clone(),
                        },
                    ));
                }
                param_index
            }
            None => function.params.get(index).map(|_| index),
        };
        if let Some(param_index) = param_index {
            let param = &function.params[param_index];
            if supplied[param_index] {
                named_argument_fault = true;
                env.diagnostics.push(call_argument(
                    &env.program.decl_ids(),
                    env.file,
                    env.span,
                    CallArgumentFault::DuplicateParameter {
                        callee: callee.clone(),
                        name: param.name.clone(),
                    },
                ));
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
            let names = env.program.decl_ids();
            check_one_arg(
                ArgEmit {
                    names: &names,
                    file: env.file,
                },
                &callee,
                &ArgParam::Named(&param.name),
                &param.ty,
                arg_type,
                arg.value.span(),
                env.diagnostics,
            );
        }
    }
    if args.len() != function.params.len() && !named_argument_fault {
        env.diagnostics.push(call_argument(
            &env.program.decl_ids(),
            env.file,
            env.span,
            CallArgumentFault::FunctionArity {
                callee: callee.clone(),
                expected: function.params.len(),
                given: args.len(),
            },
        ));
    }
    // A maybe-present (`T?`) return types the call as its present arm `T`; the
    // call's maybe-presence is resolved at the read site, like a saved read.
    function
        .return_type
        .clone()
        .map(MarrowType::without_optional)
        .unwrap_or(MarrowType::Unknown)
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
        check_exists_args(env, args, arg_types);
        return false;
    }
    if name.as_str() == "count" {
        return check_collection_combinator_args(env, "count", args, arg_types);
    }
    if let Some(target) = ConversionTarget::from_name(name) {
        let names = env.program.decl_ids();
        check_conversion_call_shape(&names, target, args, env.span, env.file, env.diagnostics);
        check_conversion_arg(
            &names,
            target,
            arg_types,
            env.span,
            env.file,
            env.diagnostics,
        );
        if target == ConversionTarget::ErrorCode {
            check_error_code_conversion_literal(&names, args, env.file, env.diagnostics);
        }
    }
    false
}

fn check_std_call_args(
    env: &mut CallEnv<'_>,
    segments: &[String],
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) {
    if segments == ["std", "assert", "isAbsent"] {
        check_assert_absent_args(env, args, arg_types);
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

fn check_assert_absent_args(
    env: &mut CallEnv<'_>,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) {
    let [arg] = args else { return };
    if assert_absent_arg_is_saved_path(env, &arg.value) {
        return;
    }
    // `isAbsent` tests any optional value for absence, mirroring `exists`: an optional
    // local or parameter, a positional/keyed read, or a stdlib `T?` result are exactly
    // what it asserts. Resolving them first would destroy the absence being tested.
    if arg_types.first().is_some_and(is_optional_value) {
        return;
    }
    env.diagnostics.push(call_argument(
        &env.program.decl_ids(),
        env.file,
        env.span,
        CallArgumentFault::AssertAbsentRequiresOptional,
    ));
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

fn check_exists_args(
    env: &mut CallEnv<'_>,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) {
    let [arg] = args else { return };
    if crate::presence::guard_subject_key_effect(env.program, &arg.value, env.scope, env.file) {
        env.diagnostics.push(call_argument(
            &env.program.decl_ids(),
            env.file,
            env.span,
            CallArgumentFault::ExistsEffectInKey,
        ));
        return;
    }
    if exists_target_arg_resolves(env, &arg.value) {
        return;
    }
    // A concrete, always-present value has no absence to test — mirror the `??`
    // always-present rejection rather than the saved-path shape error, since `exists`
    // now accepts local optionals too. A non-value argument is not a testable place.
    if arg_types.first().is_some_and(is_always_present_value) {
        env.diagnostics.push(call_argument(
            &env.program.decl_ids(),
            env.file,
            env.span,
            CallArgumentFault::ExistsAlwaysPresent,
        ));
        return;
    }
    env.diagnostics.push(call_argument(
        &env.program.decl_ids(),
        env.file,
        env.span,
        CallArgumentFault::ExistsRequiresSavedPath,
    ));
}

/// Whether a value type is a concrete, definitely-present value: not an optional, the
/// empty `absent`, or a deferred `unknown`/`invalid`. Such a value has nothing for a
/// presence guard to resolve.
fn is_always_present_value(ty: &MarrowType) -> bool {
    !matches!(
        ty,
        MarrowType::Optional(_) | MarrowType::Absent | MarrowType::Unknown | MarrowType::Invalid
    )
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
    crate::presence::exists_target_in_type_scope(
        env.program,
        &expr,
        env.scope,
        env.read_scope.transform_old,
    )
}

/// The call-site context for a named-field constructor check: the constructor
/// `label` used in diagnostics, the supplied `args` and their `arg_types`, the
/// call `span`, the source `file`, and the diagnostic sink.
struct NamedFieldArgs<'a> {
    names: &'a DeclIds<'a>,
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
        names,
        label,
        args,
        arg_types,
        span,
        file,
        diagnostics,
    } = call;
    let mut supplied = vec![false; fields.len()];
    let mut reported_positional = false;
    for (arg, arg_type) in args.iter().zip(arg_types) {
        let Some(name) = &arg.name else {
            // Positional args are one mistake per call: report the named-field requirement once,
            // located on the first offending arg, rather than repeating it for each.
            if !reported_positional {
                reported_positional = true;
                diagnostics.push(call_argument(
                    names,
                    file,
                    arg.value.span(),
                    CallArgumentFault::ConstructorNeedsNamedFields {
                        label: label.to_string(),
                    },
                ));
            }
            continue;
        };
        let Some(index) = fields.iter().position(|field| field_name(field) == name) else {
            diagnostics.push(call_argument(
                names,
                file,
                span,
                CallArgumentFault::UnknownField {
                    label: label.to_string(),
                    field: name.clone(),
                },
            ));
            continue;
        };
        if supplied[index] {
            diagnostics.push(call_argument(
                names,
                file,
                span,
                CallArgumentFault::DuplicateField { name: name.clone() },
            ));
            continue;
        }
        supplied[index] = true;
        if let Some(expected) = expected_type(index) {
            check_one_arg(
                ArgEmit { names, file },
                label,
                &ArgParam::Named(name),
                &expected,
                arg_type,
                arg.value.span(),
                diagnostics,
            );
        }
    }

    for (field, supplied) in fields.iter().zip(supplied) {
        if is_required(field) && !supplied {
            diagnostics.push(call_argument(
                names,
                file,
                span,
                CallArgumentFault::RequiredField {
                    label: label.to_string(),
                    field: field_name(field).to_string(),
                },
            ));
        }
    }
}

/// Check an `Error(...)` constructor against the named-field contract owned by
/// `marrow_schema::error`; every required field must be supplied. The field set
/// lives in the schema so the checker and runtime validate one definition.
fn check_error_constructor_args(
    names: &DeclIds<'_>,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let fields = marrow_schema::error::fields();
    check_named_field_args(
        NamedFieldArgs {
            names,
            label: "Error",
            args,
            arg_types,
            span,
            file,
            diagnostics,
        },
        fields,
        |field| field.name,
        |index| Some(MarrowType::from_resolved(fields[index].ty.clone())),
        |field| field.required,
    );
    check_error_constructor_code_literal(names, args, file, diagnostics);
}

fn check_error_constructor_code_literal(
    names: &DeclIds<'_>,
    args: &[marrow_syntax::Argument],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for arg in args {
        if arg.name.as_deref() == Some(marrow_schema::error::CODE) {
            check_error_code_literal(names, &arg.value, "`Error.code`", file, diagnostics);
        }
    }
}

fn check_error_code_conversion_literal(
    names: &DeclIds<'_>,
    args: &[marrow_syntax::Argument],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [arg] = args else { return };
    check_error_code_literal(names, &arg.value, "`ErrorCode(...)`", file, diagnostics);
}

/// Reject a string literal that does not satisfy the dotted-lowercase error-code
/// grammar, naming the offending place with `label`. The one literal-validation
/// entrypoint shared by the `ErrorCode(...)` constructor, the `Error.code` field,
/// and a literal coerced into an `ErrorCode`-typed place. A non-literal value is
/// left to its run-time coercion.
pub(crate) fn check_error_code_literal(
    names: &DeclIds<'_>,
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
        diagnostics.push(call_argument(
            names,
            file,
            *span,
            CallArgumentFault::ErrorCodeLiteral {
                label: label.to_string(),
            },
        ));
    }
}

fn check_conversion_call_shape(
    names: &DeclIds<'_>,
    target: ConversionTarget,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let label = target.spelling();
    if let [arg] = args
        && let Some(name) = &arg.name
    {
        diagnostics.push(call_argument(
            names,
            file,
            span,
            CallArgumentFault::ConversionArgumentNamed {
                label: label.to_string(),
                name: name.clone(),
            },
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
/// cause.
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
    // A maybe-present collection (`sequence[T]?`) must be resolved before it is counted
    // or streamed; the one rule owns it before the scalar/saved gates so the message
    // names the four resolution forms.
    if is_optional_value(arg_type) {
        env.diagnostics
            .push(unresolved_optional_diagnostic(env.file, arg.value.span()));
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
    // A ranged index branch counts its matching entries, but a bare ranged store root
    // or keyed layer names a traversal span with no single cardinality to read; reject
    // it here with the accurate rule so the range-value catch-all does not claim the
    // range is `for`-only.
    if name == "count"
        && is_saved_key_range_path(env.program, &arg.value, env.scope, env.file)
        && !is_saved_index_range_path(env.program, &arg.value, env.scope, env.file)
    {
        env.diagnostics.push(CheckDiagnostic::error(
            CHECK_COLLECTION_UNSUPPORTED,
            env.file,
            env.span,
            "`count` over a range is supported only on a non-unique index branch; \
             store-root and keyed-layer ranges are traversed, not counted"
                .to_string(),
        ));
        return true;
    }
    false
}

fn check_append_args(
    env: &mut CallEnv<'_>,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) {
    let ([target, value], [target_type, value_type]) = (args, arg_types) else {
        return;
    };
    // A maybe-present collection (`sequence[T]?`) target must be resolved before it is
    // appended to; the one rule owns it before the layer-shape gates so the message
    // names the four resolution forms.
    if is_optional_value(target_type) {
        env.diagnostics.push(unresolved_optional_diagnostic(
            env.file,
            target.value.span(),
        ));
        return;
    }
    check_append_value(env, &target.value, target_type, value, value_type);
    // A multi-column layer is rejected as composite, the more precise diagnostic, so
    // the group-vs-leaf check only speaks for single-column layers.
    if saved_append_target_is_composite(env.program, &target.value, env.scope, env.file) {
        return;
    }
    if !saved_layer_is_group(env.program, &target.value, env.scope, env.file) {
        return;
    }
    env.diagnostics.push(call_argument(
        &env.program.decl_ids(),
        env.file,
        env.span,
        CallArgumentFault::AppendTarget(AppendTargetDiagnostic::GroupLayer),
    ));
}

/// Type-check the value `append` writes into one element slot. A maybe-present
/// value is the one rule (an element slot is never `T?`), and a concrete element
/// type rejects an incompatible value, so an `absent` or `T?` value is caught at
/// check time rather than only at the runtime append boundary. The check runs only
/// for a genuine leaf element; a group or composite target reports its own
/// shape error and yields no element type to compare against.
fn check_append_value(
    env: &mut CallEnv<'_>,
    target: &marrow_syntax::Expression,
    target_type: &MarrowType,
    value: &marrow_syntax::Argument,
    value_type: &MarrowType,
) {
    let element = append_element_type(env, target, target_type);
    if !is_appendable_element(&element) {
        return;
    }
    let span = value.value.span();
    if let Some(diagnostic) = unresolved_optional(&element, value_type, span, env.file) {
        env.diagnostics.push(diagnostic);
        return;
    }
    if matches!(type_compatible(&element, value_type), Some(false)) {
        env.diagnostics.push(call_argument(
            &env.program.decl_ids(),
            env.file,
            span,
            CallArgumentFault::AppendValue {
                expected: element,
                found: value_type.clone(),
            },
        ));
    }
}

/// The element type an `append` target holds: a local sequence's element, or the
/// leaf type of a saved keyed-leaf layer. A group, composite, or otherwise invalid
/// target yields no leaf type, so the value check defers to the target-shape error.
fn append_element_type(
    env: &CallEnv<'_>,
    target: &marrow_syntax::Expression,
    target_type: &MarrowType,
) -> MarrowType {
    match target_type {
        MarrowType::Sequence(element) => element.as_ref().clone(),
        _ => saved_path_value_type(env.program, target, env.scope, env.file),
    }
}

/// Whether a type is a leaf an `append` element slot can hold — a scalar, enum,
/// identity, or error code. A collection, record, group entry, or unresolved type
/// is not a comparable element, so the value check leaves it to the target gates.
fn is_appendable_element(ty: &MarrowType) -> bool {
    matches!(
        ty,
        MarrowType::Primitive(_)
            | MarrowType::Enum { .. }
            | MarrowType::Identity(_)
            | MarrowType::Error
    )
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
            &env.program.decl_ids(),
            &value.value,
            "an `ErrorCode` sequence",
            env.file,
            env.diagnostics,
        );
    }
}

fn check_conversion_arg(
    names: &DeclIds<'_>,
    target: ConversionTarget,
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [arg_type] = arg_types else { return };
    // A maybe-present source must be resolved before a conversion consumes it; the
    // one rule owns it before the unsupported-source mismatch so the message names
    // the four resolution forms.
    if is_optional_value(arg_type) {
        diagnostics.push(unresolved_optional_diagnostic(file, span));
        return;
    }
    if target.accepts(arg_type) {
        return;
    }
    diagnostics.push(call_argument(
        names,
        file,
        span,
        CallArgumentFault::ConversionUnsupportedSource(ConversionUnsupportedSourceDiagnostic {
            target,
            source: arg_type.clone(),
            accepted_sources: target.accepted_source_types(),
        }),
    ));
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
    let names = program.decl_ids();
    check_named_field_args(
        NamedFieldArgs {
            names: &names,
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
    check_constructor_error_code_literals(&names, label, &fields, args, file, diagnostics);
}

/// Reject a string literal supplied for an `ErrorCode` field of a resource
/// constructor, the same gate the constructor's own `ErrorCode(...)` applies. A
/// dynamic value is validated at the write boundary.
fn check_constructor_error_code_literals(
    names: &DeclIds<'_>,
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
            check_error_code_literal(
                names,
                &arg.value,
                &format!("`{label}.{name}`"),
                file,
                diagnostics,
            );
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

/// Identifies the argument an [`check_one_arg`] message is about: a named
/// parameter when one is known (user functions, constructor fields) or a 1-based
/// position otherwise (positional std helpers). Rendered as `parameter \`name\``
/// or `argument N` so two failures on one line are distinguishable.
pub(crate) enum ArgParam<'a> {
    Named(&'a str),
    Position(usize),
}

impl ArgParam<'_> {
    /// The owned slot fact carried by a `check.call_argument` type mismatch.
    fn to_slot(&self) -> CallArgumentSlot {
        match self {
            ArgParam::Named(name) => CallArgumentSlot::Named((*name).to_string()),
            ArgParam::Position(index) => CallArgumentSlot::Position(*index),
        }
    }

    /// The rendered slot phrase (`parameter \`name\`` / `argument N`), shared with the
    /// diagnostic renderer through [`CallArgumentSlot::describe`].
    fn describe(&self) -> String {
        self.to_slot().describe()
    }
}

/// Check one positional/named argument against the type its parameter expects: a
/// known-but-different type is a `check.call_argument`; an `Unknown` argument for a
/// concrete parameter is a `check.untyped_value` (strict typing). Shared by the
/// user-function and std argument loops; `label` names the callee, `param` names
/// the failing parameter or position, and `span` locates the argument expression so
/// the diagnostic points at the offending argument rather than the call token.
pub(crate) fn check_one_arg(
    emit: ArgEmit<'_>,
    label: &str,
    param: &ArgParam<'_>,
    parameter: &MarrowType,
    arg_type: &MarrowType,
    span: SourceSpan,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    // A maybe-present (`T?`) argument into a definite parameter is the one rule; route it
    // through the shared helper before the generic mismatch so the message names the four
    // resolution forms.
    if let Some(diagnostic) = unresolved_optional(parameter, arg_type, span, emit.file) {
        diagnostics.push(diagnostic);
        return;
    }
    match type_compatible(parameter, arg_type) {
        Some(true) => {}
        Some(false) => {
            diagnostics.push(call_argument(
                emit.names,
                emit.file,
                span,
                CallArgumentFault::ArgumentType {
                    label: label.to_string(),
                    slot: param.to_slot(),
                    expected: parameter.clone(),
                    found: arg_type.clone(),
                },
            ));
        }
        // Strict typing: an untyped argument against a convertible parameter must be
        // converted first.
        None if matches!(arg_type, MarrowType::Unknown) && expects_conversion(parameter) => {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_UNTYPED_VALUE,
                emit.file,
                span,
                format!(
                    "{} to `{label}` has no known type, but `{}` is expected; convert it first",
                    param.describe(),
                    marrow_type_name(emit.names, parameter),
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
    diagnostics.push(call_argument(
        &program.decl_ids(),
        file,
        arg.span(),
        CallArgumentFault::SavedCollectionByValue {
            label: label.to_string(),
            parameter: parameter.clone(),
        },
    ));
    true
}

/// Check positional `args` against a fixed positional parameter list (the std
/// helper signatures): an arity mismatch is a `check.call_argument`, and each
/// argument with a known-required parameter type is checked by [`check_one_arg`].
/// A `None` parameter slot (e.g. a path argument) is left alone. Std helpers are
/// positional-only — named-argument matching stays user-function-only.
pub(crate) fn check_args_against(
    emit: ArgEmit<'_>,
    label: &str,
    params: &[Option<MarrowType>],
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if arg_types.len() != params.len() {
        diagnostics.push(call_argument(
            emit.names,
            emit.file,
            span,
            CallArgumentFault::Arity {
                label: label.to_string(),
                expected: params.len(),
                given: arg_types.len(),
            },
        ));
    }
    for (index, (parameter, arg_type)) in params.iter().zip(arg_types).enumerate() {
        if let Some(parameter) = parameter {
            // An argument-count mismatch is reported above; the per-argument span
            // falls back to the call token when this slot has no argument.
            let arg_span = args.get(index).map_or(span, |arg| arg.value.span());
            check_one_arg(
                emit,
                label,
                &ArgParam::Position(index),
                parameter,
                arg_type,
                arg_span,
                diagnostics,
            );
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
    const_ints: &[HashMap<String, Option<i64>>],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    let names = program.decl_ids();
    for arg in args {
        if arg.name.is_some() {
            diagnostics.push(call_argument(
                &names,
                file,
                arg.value.span(),
                CallArgumentFault::IdArgumentsPositional,
            ));
        }
    }
    let Some(root_arg) = args.first() else {
        diagnostics.push(call_argument(
            &names,
            file,
            span,
            CallArgumentFault::IdExpectsRoot,
        ));
        return MarrowType::Unknown;
    };
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = &root_arg.value else {
        diagnostics.push(call_argument(
            &names,
            file,
            root_arg.value.span(),
            CallArgumentFault::IdExpectsRootFirst,
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
    let key_args = args.get(1..).unwrap_or(&[]);
    check_keys_against(
        &names,
        &store.store.identity_keys,
        arg_types.get(1..).unwrap_or(&[]),
        span,
        file,
        diagnostics,
    );
    check_identity_sequence_position(store.store, key_args, const_ints, span, file, diagnostics);
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
            diagnostics.push(call_argument(
                &program.decl_ids(),
                file,
                span,
                CallArgumentFault::NextIdRequiresBareRoot,
            ));
        } else if let Some(arg_type) = arg_types
            .first()
            .filter(|ty| !matches!(ty, MarrowType::Unknown))
        {
            diagnostics.push(call_argument(
                &program.decl_ids(),
                file,
                span,
                CallArgumentFault::NextIdRequiresRoot {
                    found: arg_type.clone(),
                },
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
        if let Some(MarrowType::Identity(root)) = arg_types.first() {
            return neighbor_unsupported_bare_identity(env, which, root);
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
                    &format!(
                        "a composite-identity root (iterate it whole with \
                         `for id in ^{name}` or `reversed(^{name})`)"
                    ),
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
        crate::CheckedExpr::Call { .. } | crate::CheckedExpr::Field { .. } => resolver
            .neighbor_key_type(&checked)
            .unwrap_or(MarrowType::Unknown),
        _ => match arg_types.first() {
            Some(MarrowType::Identity(root)) => {
                neighbor_unsupported_bare_identity(env, which, root)
            }
            _ => MarrowType::Unknown,
        },
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
            env.diagnostics.push(call_argument(
                &env.program.decl_ids(),
                env.file,
                env.span,
                CallArgumentFault::KeyRequiresIdentity {
                    found: (*arg_type).clone(),
                },
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
        env.diagnostics.push(call_argument(
            &env.program.decl_ids(),
            env.file,
            env.span,
            CallArgumentFault::AppendTarget(AppendTargetDiagnostic::CompositeLayer),
        ));
        return;
    }
    let Some(key_type) = saved_path_key_type(env.program, &target.value, env.scope, env.file)
    else {
        return;
    };
    if !matches!(as_primitive(&key_type), Some(ScalarType::Int)) {
        env.diagnostics.push(call_argument(
            &env.program.decl_ids(),
            env.file,
            env.span,
            CallArgumentFault::AppendTarget(AppendTargetDiagnostic::NonIntKeyedLayer { key_type }),
        ));
    }
}

/// Whether the store at saved root `root` has a composite (multi-key) identity. A
/// non-keyed root or an unknown root is not composite.
pub(crate) fn composite_identity(program: &CheckedProgram, root: &str) -> bool {
    resolve_store_by_root(program, root).is_some_and(|store| store.store.identity_keys.len() > 1)
}

/// Report a `check.neighbor_unsupported` error for a statically-unnavigable
/// `next`/`prev` shape and poison the result. The poison `Invalid` type (not the
/// untyped `Unknown`) keeps the rejected read from cascading a second
/// `untyped_value` or `unresolved_optional` on the same mistake.
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
    MarrowType::Invalid
}

/// Report `check.neighbor_unsupported` for a bare identity *value* (a value typed
/// `Id(^root)` with no saved-place expression to navigate from), branching the
/// remedy on identity arity. A single-key sequence is navigable from a saved
/// place, so its remedy names that construct; a composite identity has no single
/// key level to seek, so the saved-place remedy would dead-end into a second
/// neighbor error — the only working construct is whole-store iteration.
fn neighbor_unsupported_bare_identity(
    env: &mut CallEnv<'_>,
    which: &str,
    root: &str,
) -> MarrowType {
    let shape = if composite_identity(env.program, root) {
        format!(
            "a composite-identity value (iterate the store whole with \
             `for id in ^{root}` or `reversed(^{root})`)"
        )
    } else {
        "an identity value (use a saved place)".to_string()
    };
    neighbor_unsupported(which, &shape, env.span, env.file, env.diagnostics)
}

/// Report a `check.call_argument` arity diagnostic when a fixed-arity builtin is
/// called with the wrong number of arguments.
pub(crate) fn check_arity(
    names: &DeclIds<'_>,
    name: &str,
    arity: usize,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if args.len() != arity {
        diagnostics.push(call_argument(
            names,
            file,
            span,
            CallArgumentFault::Arity {
                label: name.to_string(),
                expected: arity,
                given: args.len(),
            },
        ));
    }
}

/// Whether `file` contributes a module to the program — a library module or a
/// module-less script. Calls in such a file are resolution-checked; a file
/// excluded by a parse error is not.
pub(crate) fn file_in_program(program: &CheckedProgram, file: &Path) -> bool {
    program.module_index_by_file(file).is_some()
}
