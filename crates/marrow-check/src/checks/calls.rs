//! Call checking: dispatch in runtime order (special builtins, general builtins,
//! resource constructors, then user functions), each branch's argument and mode
//! rules, and the special-form builtins `nextId`/`next`/`prev`/`append`. Returns
//! the call's declared return type when known.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{ResourceSchema, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::{Severity, SourceSpan};

use crate::infer::{layer_key_type, record_identity_type, saved_group_chain};
use crate::resolve::resolve_store_by_root;
use crate::typerules::{
    as_primitive, expects_conversion, marrow_type_name, mismatch_display, type_compatible,
};
use crate::{
    AppendTargetDiagnostic, CHECK_AMBIGUOUS_CALL, CHECK_CALL_ARGUMENT,
    CHECK_COLLECTION_UNSUPPORTED, CHECK_NEIGHBOR_UNSUPPORTED, CHECK_NEXT_ID_REQUIRES_SINGLE_INT,
    CHECK_PRIVATE_FUNCTION, CHECK_UNRESOLVED_CALL, CHECK_UNTYPED_VALUE, CheckDiagnostic,
    CheckedProgram, ConversionTarget, ConversionUnsupportedSourceDiagnostic, Def, DefItem,
    DiagnosticPayload, MarrowType, Resolution, ResolvableKind, TypeNames, builtin_return_type,
    conversion_return_type, identity_type_for_store, is_builtin_call, module_of_file, resolve,
    resolve_resource_schema_type, resource_type_name, std_call_params, std_call_return_type,
};

use super::collections::{
    collection_loop_binding_types, is_saved_index_branch_path, saved_path_key_type,
};
use super::diagnostics::call_diagnostic;

pub(crate) struct CallCheck<'a> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) callee: &'a marrow_syntax::Expression,
    pub(crate) args: &'a [marrow_syntax::Argument],
    pub(crate) arg_types: &'a [MarrowType],
    pub(crate) aliases: &'a HashMap<String, Vec<String>>,
    pub(crate) span: SourceSpan,
    pub(crate) file: &'a Path,
    pub(crate) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

struct CallEnv<'a> {
    program: &'a CheckedProgram,
    span: SourceSpan,
    file: &'a Path,
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
        aliases,
        span,
        file,
        diagnostics,
    } = input;
    let mut env = CallEnv {
        program,
        span,
        file,
        diagnostics,
    };
    let marrow_syntax::Expression::Name { segments, .. } = callee else {
        check_plain_call_modes("call", args, env.span, env.file, env.diagnostics);
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
    if let [name] = segments
        && name == "nextId"
    {
        check_plain_call_modes(name, args, env.span, env.file, env.diagnostics);
        return Some(check_next_id(
            env.program,
            args,
            env.span,
            env.file,
            env.diagnostics,
        ));
    }
    if let [name] = segments {
        match name.as_str() {
            "reversed" => {
                check_plain_call_modes(name, args, env.span, env.file, env.diagnostics);
                check_arity(name, 1, args, env.span, env.file, env.diagnostics);
                return Some(reversed_type(env.program, args, arg_types));
            }
            "next" | "prev" => {
                check_plain_call_modes(name, args, env.span, env.file, env.diagnostics);
                check_arity(name, 1, args, env.span, env.file, env.diagnostics);
                return Some(check_neighbor(
                    env.program,
                    name,
                    args,
                    arg_types,
                    env.span,
                    env.file,
                    env.diagnostics,
                ));
            }
            "values" | "entries" => {
                check_plain_call_modes(name, args, env.span, env.file, env.diagnostics);
                check_arity(name, 1, args, env.span, env.file, env.diagnostics);
                check_value_materialization_args(
                    env.program,
                    name,
                    args,
                    env.span,
                    env.file,
                    env.diagnostics,
                );
                return Some(MarrowType::Unknown);
            }
            "append" => {
                check_plain_call_modes(name, args, env.span, env.file, env.diagnostics);
                check_arity(name, 2, args, env.span, env.file, env.diagnostics);
                check_append_args(env.program, args, env.span, env.file, env.diagnostics);
                check_append(env.program, args, env.span, env.file, env.diagnostics);
                return Some(MarrowType::Primitive(ScalarType::Int));
            }
            _ => {}
        }
    }
    None
}

fn check_builtin_call(
    env: &mut CallEnv<'_>,
    segments: &[String],
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> MarrowType {
    let label = segments.join("::");
    check_plain_call_modes(&label, args, env.span, env.file, env.diagnostics);
    if segments == ["Error"] {
        check_error_constructor_args(args, arg_types, env.span, env.file, env.diagnostics);
        return MarrowType::Error;
    }
    check_builtin_call_args(segments, arg_types, env.span, env.file, env.diagnostics);
    if let Some(params) = std_call_params(segments) {
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
    check_plain_call_modes(
        &segments.join("::"),
        args,
        env.span,
        env.file,
        env.diagnostics,
    );
    let enum_names: Vec<String> = module
        .enums
        .iter()
        .map(|enum_| enum_.name.clone())
        .collect();
    check_resource_constructor_args(ResourceConstructorCheck {
        program: env.program,
        label: &resource.name,
        module_name: &module.name,
        resource,
        enum_names: &enum_names,
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
                env.diagnostics.push(CheckDiagnostic {
                    code: CHECK_PRIVATE_FUNCTION,
                    severity: Severity::Error,
                    file: env.file.to_path_buf(),
                    message: format!(
                        "function `{name}` is private to its module; mark it `pub` to call it \
                         from another module"
                    ),
                    span: env.span,
                    payload: DiagnosticPayload::None,
                });
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
                env.diagnostics.push(CheckDiagnostic {
                    code: CHECK_AMBIGUOUS_CALL,
                    severity: Severity::Error,
                    file: env.file.to_path_buf(),
                    message: format!(
                        "call to `{leaf}` is ambiguous; qualify it as one of {options}"
                    ),
                    span: env.span,
                    payload: DiagnosticPayload::None,
                });
            }
            return MarrowType::Unknown;
        }
        Resolution::Found(_) | Resolution::Unresolved => {
            if file_in_program(env.program, env.file) {
                let name = segments.join("::");
                env.diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNRESOLVED_CALL,
                    severity: Severity::Error,
                    file: env.file.to_path_buf(),
                    message: format!("function `{name}` is not defined"),
                    span: env.span,
                    payload: DiagnosticPayload::UnresolvedCall(name),
                });
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
                env.diagnostics.push(CheckDiagnostic {
                    code: CHECK_CALL_ARGUMENT,
                    severity: Severity::Error,
                    file: env.file.to_path_buf(),
                    message: format!(
                        "function `{callee}` parameter `{}` is supplied more than once",
                        param.name
                    ),
                    span: env.span,
                    payload: DiagnosticPayload::DuplicateNamedArgument(param.name.clone()),
                });
                continue;
            }
            supplied[param_index] = true;
            check_call_mode(
                &callee,
                arg,
                param.mode,
                env.span,
                env.file,
                env.diagnostics,
            );
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

fn check_builtin_call_args(
    segments: &[String],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [name] = segments else { return };
    if let Some(target) = ConversionTarget::from_name(name) {
        check_conversion_arg(target, arg_types, span, file, diagnostics);
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
    let mut supplied = vec![false; fields.len()];
    for (arg, arg_type) in args.iter().zip(arg_types) {
        let Some(name) = &arg.name else {
            diagnostics.push(call_diagnostic(
                file,
                span,
                "`Error` constructor takes named fields".to_string(),
            ));
            continue;
        };
        let Some(index) = fields.iter().position(|field| field.name == name) else {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`Error` has no field `{name}`"),
            ));
            continue;
        };
        if supplied[index] {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_CALL_ARGUMENT,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!("field `{name}` is supplied more than once"),
                span,
                payload: DiagnosticPayload::DuplicateNamedArgument(name.clone()),
            });
            continue;
        }
        supplied[index] = true;
        let expected = MarrowType::from_resolved(fields[index].ty.clone(), TypeNames::default());
        check_one_arg("Error", &expected, arg_type, span, file, diagnostics);
    }

    for (field, supplied) in fields.iter().zip(supplied) {
        if field.required && !supplied {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`Error` requires `{}`", field.name),
            ));
        }
    }
}

fn check_value_materialization_args(
    program: &CheckedProgram,
    name: &str,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [arg] = args else { return };
    if arg.mode.is_some() || arg.name.is_some() || !is_saved_index_branch_path(program, &arg.value)
    {
        return;
    }
    diagnostics.push(CheckDiagnostic {
        code: CHECK_COLLECTION_UNSUPPORTED,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: format!("`{name}` cannot materialize values from an index branch; use `keys`"),
        span,
        payload: DiagnosticPayload::None,
    });
}

fn check_append_args(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [target, _value] = args else { return };
    let Some(node) = saved_layer_node(program, &target.value) else {
        return;
    };
    if matches!(node.kind, marrow_schema::NodeKind::Group) {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_CALL_ARGUMENT,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message:
                "`append` target must be a keyed leaf layer, but this path names a group layer"
                    .to_string(),
            span,
            payload: DiagnosticPayload::AppendTarget(AppendTargetDiagnostic::GroupLayer),
        });
    }
}

fn saved_layer_node<'p>(
    program: &'p CheckedProgram,
    expr: &marrow_syntax::Expression,
) -> Option<&'p marrow_schema::Node> {
    let (root, layers) = saved_group_chain(expr)?;
    resolve_store_by_root(program, root)?
        .resource
        .descend_layers(&layers)
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
    diagnostics.push(CheckDiagnostic {
        code: CHECK_CALL_ARGUMENT,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: format!(
            "`{}` cannot convert `{}`; supported sources are {}",
            target.spelling(),
            marrow_type_name(arg_type),
            target.supported_sources_message()
        ),
        span,
        payload: DiagnosticPayload::ConversionUnsupportedSource(
            ConversionUnsupportedSourceDiagnostic {
                target,
                source: arg_type.clone(),
                accepted_sources: target.accepted_source_types(),
            },
        ),
    });
}

fn check_plain_call_modes(
    label: &str,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for arg in args {
        if let Some(mode) = arg.mode {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!(
                    "argument to `{label}` cannot be passed as {}",
                    arg_mode_name(mode)
                ),
            ));
        }
    }
}

fn check_call_mode(
    label: &str,
    arg: &marrow_syntax::Argument,
    param_mode: Option<crate::CheckedParamMode>,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if !call_modes_match(arg.mode, param_mode) {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "argument to `{label}` must be passed as {}",
                call_mode_expectation(param_mode)
            ),
        ));
    }
    if arg.mode.is_some() && !crate::rules::is_assignable(&arg.value) {
        diagnostics.push(CheckDiagnostic {
            code: crate::rules::CHECK_INVALID_ASSIGN_TARGET,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: "inout argument is not a writable place".to_string(),
            span: arg.value.span(),
            payload: DiagnosticPayload::None,
        });
    }
}

fn arg_mode_name(mode: marrow_syntax::ArgMode) -> &'static str {
    match mode {
        marrow_syntax::ArgMode::InOut => "`inout`",
    }
}

fn call_modes_match(
    arg: Option<marrow_syntax::ArgMode>,
    param: Option<crate::CheckedParamMode>,
) -> bool {
    matches!(
        (arg, param),
        (None, None)
            | (
                Some(marrow_syntax::ArgMode::InOut),
                Some(crate::CheckedParamMode::InOut)
            )
    )
}

fn call_mode_expectation(mode: Option<crate::CheckedParamMode>) -> &'static str {
    match mode {
        Some(crate::CheckedParamMode::InOut) => "`inout`",
        None => "a plain argument",
    }
}

fn reversed_type(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> MarrowType {
    if let [arg] = args
        && arg.mode.is_none()
        && arg.name.is_none()
        && let Some((element, None)) = collection_loop_binding_types(program, false, &arg.value)
    {
        return MarrowType::Sequence(Box::new(element));
    }
    arg_types.first().cloned().unwrap_or(MarrowType::Unknown)
}

struct ResourceConstructorCheck<'a> {
    program: &'a CheckedProgram,
    label: &'a str,
    module_name: &'a str,
    resource: &'a ResourceSchema,
    enum_names: &'a [String],
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
        module_name,
        resource,
        enum_names,
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
        let Some(index) = fields.iter().position(|field| &field.name == name) else {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` has no field `{name}`"),
            ));
            continue;
        };
        if supplied[index] {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_CALL_ARGUMENT,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!("field `{name}` is supplied more than once"),
                span,
                payload: DiagnosticPayload::DuplicateNamedArgument(name.clone()),
            });
            continue;
        }
        supplied[index] = true;
        if let Some(ty) = fields[index].plain_field_type() {
            let expected = constructor_field_type(program, module_name, enum_names, ty);
            check_one_arg(label, &expected, arg_type, span, file, diagnostics);
        }
    }

    for (field, supplied) in fields.iter().zip(supplied) {
        if !supplied
            && matches!(
                &field.kind,
                marrow_schema::NodeKind::Slot { required: true, .. }
            )
        {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` requires `{}`", field.name),
            ));
        }
    }
}

fn constructor_field_type(
    program: &CheckedProgram,
    module_name: &str,
    enum_names: &[String],
    ty: &Type,
) -> MarrowType {
    if let Some(resource_type) = resolve_resource_schema_type(program, module_name, ty) {
        return resource_type;
    }
    MarrowType::from_resolved(
        ty.clone(),
        TypeNames {
            module: module_name,
            enums: enum_names,
        },
    )
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
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("argument to `{label}` expects `{expected}`, but found `{found}`"),
            ));
        }
        // Strict typing: an untyped argument against a convertible parameter must be
        // converted first.
        None if matches!(arg_type, MarrowType::Unknown) && expects_conversion(parameter) => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNTYPED_VALUE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "argument to `{label}` has no known type, but `{}` is expected; convert it first",
                    marrow_type_name(parameter),
                ),
                span,
                payload: DiagnosticPayload::None,
            });
        }
        None => {}
    }
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

/// Type `nextId(^root)` and gate it on a single-`int` saved root, which types to
/// `Id(^root)`; any other identity shape reports
/// `check.next_id_requires_single_int`. A non-`^root` or wrong-arity argument is
/// left to the runtime, and an undeclared root is reported elsewhere, so neither is
/// double-reported here.
pub(crate) fn check_next_id(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    let [arg] = args else {
        return MarrowType::Unknown;
    };
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = &arg.value else {
        return MarrowType::Unknown;
    };
    let Some(store) = resolve_store_by_root(program, root) else {
        return MarrowType::Unknown;
    };
    if store.store.single_int_root() {
        return identity_type_for_store(store.store);
    }
    diagnostics.push(CheckDiagnostic {
        code: CHECK_NEXT_ID_REQUIRES_SINGLE_INT,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: format!(
            "`nextId` requires a store with one `int` identity key, but `^{root}` \
             ({}) has no default allocation policy; composite and non-integer \
             identities are application-provided",
            store.store.next_id_shape(),
        ),
        span,
        payload: DiagnosticPayload::None,
    });
    MarrowType::Unknown
}

/// Type `next(<element>)` / `prev(<element>)`. A keyed root or single-key record
/// navigates among record identities (result `Id(^root)`); a keyed or bare child
/// layer navigates among that layer's keys (result the layer's key type). A
/// composite-identity record and an index branch would fault uncatchably at
/// runtime, so each is reported as a compile error. Any other shape is left
/// `Unknown` for the runtime, where a surrounding `??` still types the default.
pub(crate) fn check_neighbor(
    program: &CheckedProgram,
    which: &str,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::Expression;
    let [arg] = args else {
        return MarrowType::Unknown;
    };
    match &arg.value {
        // A bare primary keyed root `^root`: its first/last record is sought. A
        // composite identity has no single returned key value, so reject it before
        // the runtime degrades it to one component.
        Expression::SavedRoot { name: root, .. } => {
            if composite_identity(program, root) {
                return neighbor_unsupported(
                    which,
                    "a composite-identity root (scope a single key level)",
                    span,
                    file,
                    diagnostics,
                );
            }
            record_identity_type(program, root)
        }
        Expression::Call { callee, .. } => match callee.as_ref() {
            // `^root(id…)`: a keyed record, anchoring at one key level, so a composite
            // identity is out of scope.
            Expression::SavedRoot { name: root, .. } => {
                if composite_identity(program, root) {
                    return neighbor_unsupported(
                        which,
                        "a composite-identity record (scope a single key level)",
                        span,
                        file,
                        diagnostics,
                    );
                }
                record_identity_type(program, root)
            }
            // `^root.index(args…)`: an index branch (the callee's base is the root
            // itself). It inspects identities, with no single key position to seek.
            Expression::Field { base, .. }
                if matches!(base.as_ref(), Expression::SavedRoot { .. }) =>
            {
                neighbor_unsupported(which, "an index branch", span, file, diagnostics)
            }
            // `^root(id…).layer(k)`: a keyed layer position; its neighbor is a key.
            Expression::Field { .. } => layer_key_type(program, callee.as_ref()),
            _ => MarrowType::Unknown,
        },
        // A bare child layer `^root(id…).layer`: navigate among the layer's keys.
        Expression::Field { .. } => layer_key_type(program, &arg.value),
        _ if matches!(arg_types.first(), Some(MarrowType::Identity(_))) => neighbor_unsupported(
            which,
            "an identity value (use a saved place)",
            span,
            file,
            diagnostics,
        ),
        _ => MarrowType::Unknown,
    }
}

/// Check `append(layer, value)` against the statically declared layer key kind.
/// `append` allocates an integer position, so accepting a string- or bool-keyed
/// layer would create stored keys the schema cannot address.
pub(crate) fn check_append(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [target, _] = args else {
        return;
    };
    if target.mode.is_some() || target.name.is_some() {
        return;
    }
    let Some(key_type) = saved_path_key_type(program, &target.value) else {
        return;
    };
    if !matches!(as_primitive(&key_type), Some(ScalarType::Int)) {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_CALL_ARGUMENT,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "`append` requires an int-keyed layer, but this layer is keyed by `{}`",
                marrow_type_name(&key_type)
            ),
            span,
            payload: DiagnosticPayload::AppendTarget(AppendTargetDiagnostic::NonIntKeyedLayer {
                key_type,
            }),
        });
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
    diagnostics.push(CheckDiagnostic {
        code: CHECK_NEIGHBOR_UNSUPPORTED,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: format!("`{which}` cannot navigate {shape}"),
        span,
        payload: DiagnosticPayload::None,
    });
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
