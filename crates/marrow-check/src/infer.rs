//! Expression type inference and the saved-path/field type resolution it walks.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{MemberPathResolution, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::checks::{
    CallCheck, CoalesceCheck, SavedKeyArgCheck, check_binary, check_call, check_coalesce,
    check_saved_key_args, check_unary, key_type_diagnostic,
};
use crate::enums::{
    IsCheck, check_is, enum_schema_in, join_or, resolve_enum_member_path, resolve_type,
};
use crate::executable::{SavedAccessRejection, SavedPlaceResolver, lower_expr_for_file};
use crate::program::TypeNames;
use crate::typerules::{
    check_literal_range, marrow_type_name, type_compatible, type_renderable_at_runtime,
};
use crate::{
    CHECK_AMBIGUOUS_MEMBER, CHECK_CATEGORY_NOT_SELECTABLE, CHECK_COLLECTION_UNSUPPORTED,
    CHECK_OPERATOR_TYPE, CHECK_PRIVATE_ENUM, CHECK_UNKNOWN_ENUM_MEMBER, CHECK_UNRESOLVED_NAME,
    CheckDiagnostic, CheckedProgram, DiagnosticPayload, EnumDiagnostic, MarrowType,
    build_alias_map, expand_module_alias, resolve_resource_schema_type, resolve_resource_type,
};

/// Infer a type during post-check resolution, discarding diagnostics the checking
/// pass already reported so they are not double-counted.
pub(crate) fn infer_only(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> MarrowType {
    infer_type(program, expr, scope, aliases, file, &mut Vec::new())
}

/// The declared type of a binding: its annotation when written, otherwise the
/// inferred type of its initializer.
fn binding_type(
    annotation: Option<&marrow_syntax::TypeRef>,
    value_type: MarrowType,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> MarrowType {
    match annotation {
        Some(ty) => resolve_type(ty, program, aliases, file),
        None => value_type,
    }
}

/// Record `name`'s type in the innermost scope frame.
pub(crate) fn bind(scope: &mut [HashMap<String, MarrowType>], name: &str, ty: MarrowType) {
    if let Some(frame) = scope.last_mut() {
        frame.insert(name.to_string(), ty);
    }
}

/// The `(name, type)` a `const`/`var` statement introduces into its block:
/// annotation when written, else inferred initializer type. `None` for any other
/// statement. The checker and the editor scope reconstruction share this so a
/// binding's type is derived in one place.
pub(crate) fn local_binding(
    program: &CheckedProgram,
    statement: &marrow_syntax::Statement,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<(String, MarrowType)> {
    local_binding_with_read_scope(program, statement, scope, aliases, file, None)
}

pub(crate) fn local_binding_with_read_scope(
    program: &CheckedProgram,
    statement: &marrow_syntax::Statement,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    transform_old: Option<crate::presence::TransformOldReadScope<'_>>,
) -> Option<(String, MarrowType)> {
    use marrow_syntax::Statement;
    let mut sink = Vec::new();
    let (name, keys, annotation, value_type) = match statement {
        Statement::Const {
            name, ty, value, ..
        } => (
            name,
            &[][..],
            ty,
            infer_type_with_read_scope(
                program,
                value,
                scope,
                aliases,
                file,
                &mut sink,
                transform_old,
            ),
        ),
        Statement::Var {
            name,
            keys,
            ty,
            value,
            ..
        } => {
            let value_type = match value {
                Some(value) => infer_type_with_read_scope(
                    program,
                    value,
                    scope,
                    aliases,
                    file,
                    &mut sink,
                    transform_old,
                ),
                None => MarrowType::Unknown,
            };
            (name, keys.as_slice(), ty, value_type)
        }
        _ => return None,
    };
    let value = binding_type(annotation.as_ref(), value_type, program, aliases, file);
    let ty = if keys.is_empty() {
        value
    } else {
        MarrowType::LocalTree {
            keys: keys
                .iter()
                .map(|key| MarrowType::resolve(&key.ty, TypeNames::default()))
                .collect(),
            value: Box::new(value),
        }
    };
    Some((name.clone(), ty))
}

/// Infer an expression's type, recording a `check.operator_type` diagnostic for
/// any operator with known-incompatible operands. Returns [`MarrowType::Unknown`]
/// whenever the type is uncertain, so a containing operator never fires on it.
pub(crate) fn infer_type(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    infer_type_with_read_scope(program, expr, scope, aliases, file, diagnostics, None)
}

pub(crate) fn infer_type_with_read_scope(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    transform_old: Option<crate::presence::TransformOldReadScope<'_>>,
) -> MarrowType {
    use marrow_syntax::Expression;
    if let Some(rejection) = checked_expr(program, expr, scope, file)
        .and_then(|expr| SavedPlaceResolver::new(program).access_rejection(&expr))
    {
        match rejection {
            SavedAccessRejection::GeneratedIndexBranch => diagnostics.push(CheckDiagnostic::error(
                CHECK_COLLECTION_UNSUPPORTED,
                file,
                expr.span(),
                "generated index branches do not expose resource members or chained calls",
            )),
            SavedAccessRejection::KeyedRootMemberWithoutIdentity(root) => {
                diagnostics.push(key_type_diagnostic(
                    file,
                    expr.span(),
                    format!(
                        "`^{root}` must be addressed with an identity before using its members"
                    ),
                ))
            }
        }
        return MarrowType::Unknown;
    }
    match expr {
        Expression::Literal { kind, text, span } => {
            check_literal_range(*kind, text, *span, file, diagnostics);
            if matches!(kind, marrow_syntax::LiteralKind::String) {
                check_string_escapes(text, *span, file, diagnostics);
            }
            literal_type(*kind)
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                match part {
                    marrow_syntax::InterpolationPart::Text { text, span } => {
                        check_interpolation_text_escapes(text, *span, file, diagnostics);
                    }
                    marrow_syntax::InterpolationPart::Expr(expr) => {
                        let ty = infer_type_with_read_scope(
                            program,
                            expr,
                            scope,
                            aliases,
                            file,
                            diagnostics,
                            transform_old,
                        );
                        if type_renderable_at_runtime(&ty) == Some(false) {
                            diagnostics.push(interpolation_unsupported_source_diagnostic(
                                file,
                                expr.span(),
                                ty,
                            ));
                        }
                    }
                }
            }
            MarrowType::Primitive(ScalarType::Str)
        }
        Expression::Name { segments, span } if segments.len() == 1 => {
            let name = &segments[0];
            lookup_opt(scope, name).unwrap_or_else(|| {
                diagnostics.push(CheckDiagnostic::error(
                    CHECK_UNRESOLVED_NAME,
                    file,
                    *span,
                    format!("`{name}` is not defined"),
                ));
                MarrowType::Unknown
            })
        }
        Expression::Unary { op, operand, span } => {
            let operand = infer_type_with_read_scope(
                program,
                operand,
                scope,
                aliases,
                file,
                diagnostics,
                transform_old,
            );
            check_unary(*op, &operand, *span, file, diagnostics)
        }
        Expression::Binary {
            op,
            left,
            right,
            span,
        } => {
            let left_type = infer_type_with_read_scope(
                program,
                left,
                scope,
                aliases,
                file,
                diagnostics,
                transform_old,
            );
            // `is` is the enum-subtree predicate: its right is a member-path naming a
            // member or category, not a value, so it is resolved inside `check_is`
            // rather than inferred as a value here — inferring it would reject a
            // category right operand as non-selectable.
            if matches!(op, marrow_syntax::BinaryOp::Is) {
                return check_is(IsCheck {
                    program,
                    left_type: &left_type,
                    right,
                    aliases,
                    span: *span,
                    file,
                    diagnostics,
                });
            }
            let right_type = infer_type_with_read_scope(
                program,
                right,
                scope,
                aliases,
                file,
                diagnostics,
                transform_old,
            );
            // `??` only defaults an absent path read, so its left operand must be a
            // path read or `?.` chain — a present non-path value is never absent
            // and has nothing to default. The result is the leaf type of that read.
            if matches!(op, marrow_syntax::BinaryOp::Coalesce) {
                return check_coalesce(CoalesceCheck {
                    program,
                    left,
                    left_type: &left_type,
                    right_type: &right_type,
                    span: *span,
                    file,
                    scope,
                    transform_old,
                    diagnostics,
                });
            }
            check_binary(*op, &left_type, &right_type, *span, file, diagnostics)
        }
        Expression::Range {
            start,
            end,
            step,
            span,
            ..
        } => {
            let start_type = start.as_ref().map(|start| {
                infer_type_with_read_scope(
                    program,
                    start,
                    scope,
                    aliases,
                    file,
                    diagnostics,
                    transform_old,
                )
            });
            let end_type = end.as_ref().map(|end| {
                infer_type_with_read_scope(
                    program,
                    end,
                    scope,
                    aliases,
                    file,
                    diagnostics,
                    transform_old,
                )
            });
            if let Some(step) = step {
                infer_type_with_read_scope(
                    program,
                    step,
                    scope,
                    aliases,
                    file,
                    diagnostics,
                    transform_old,
                );
            }
            match (start_type, end_type) {
                (Some(start), Some(end)) => check_binary(
                    marrow_syntax::BinaryOp::RangeExclusive,
                    &start,
                    &end,
                    *span,
                    file,
                    diagnostics,
                ),
                (Some(ty), None) | (None, Some(ty)) => ty,
                (None, None) => MarrowType::Unknown,
            }
        }
        Expression::Call {
            callee, args, span, ..
        } => {
            // A bare single-segment callee names a function, not a value, so it is
            // left to `check_call` rather than flagged as an unresolved value name.
            if !is_bare_name(callee) {
                infer_type_with_read_scope(
                    program,
                    callee,
                    scope,
                    aliases,
                    file,
                    diagnostics,
                    transform_old,
                );
            }
            let mut arg_types = Vec::with_capacity(args.len());
            for arg in args {
                arg_types.push(infer_call_arg_type(CallArgInfer {
                    program,
                    callee,
                    arg: &arg.value,
                    scope,
                    aliases,
                    file,
                    diagnostics,
                    transform_old,
                }));
            }
            if let Some(ty) = local_collection_access_type(
                callee,
                args,
                &arg_types,
                scope,
                *span,
                file,
                diagnostics,
            ) {
                return ty;
            }
            let call_type = check_call(CallCheck {
                program,
                callee,
                args,
                arg_types: &arg_types,
                scope,
                aliases,
                span: *span,
                file,
                transform_old,
                diagnostics,
            });
            // A saved access carries key arguments the function-call path does not
            // type; check them against the root identity or layer key parameters.
            check_saved_key_args(SavedKeyArgCheck {
                program,
                callee,
                args,
                arg_types: &arg_types,
                scope,
                span: *span,
                file,
                diagnostics,
            });
            // A call-shaped saved read (keyed-leaf or whole-record) is not a function
            // call; type it through its saved shape once the call path comes back Unknown.
            if matches!(call_type, MarrowType::Unknown) {
                count_builtin_type(program, callee, args, scope, file)
                    .or_else(|| saved_expr_type(program, expr, scope, file))
                    .unwrap_or(MarrowType::Unknown)
            } else {
                call_type
            }
        }
        // A plain field read and an optional (`?.`) field read resolve to the same
        // declared leaf type: the short-circuit only changes the read's runtime
        // behavior on absence, not the type of a populated leaf.
        Expression::Field { base, name, .. } | Expression::OptionalField { base, name, .. } => {
            let base_type = infer_type_with_read_scope(
                program,
                base,
                scope,
                aliases,
                file,
                diagnostics,
                transform_old,
            );
            saved_expr_type(program, expr, scope, file)
                .or_else(|| local_field_type(program, &base_type, name))
                .unwrap_or(MarrowType::Unknown)
        }
        Expression::Name { segments, span } if segments.len() >= 2 => {
            enum_member_value_type(program, expr, segments, *span, aliases, file, diagnostics)
        }
        Expression::SavedRoot { .. } => {
            saved_expr_type(program, expr, scope, file).unwrap_or(MarrowType::Unknown)
        }
        Expression::Name { .. } => MarrowType::Unknown,
    }
}

struct CallArgInfer<'a, 'd> {
    program: &'a CheckedProgram,
    callee: &'a marrow_syntax::Expression,
    arg: &'a marrow_syntax::Expression,
    scope: &'a [HashMap<String, MarrowType>],
    aliases: &'a HashMap<String, Vec<String>>,
    file: &'a Path,
    diagnostics: &'d mut Vec<CheckDiagnostic>,
    transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
}

fn infer_call_arg_type(input: CallArgInfer<'_, '_>) -> MarrowType {
    if checked_expr(input.program, input.callee, input.scope, input.file)
        .is_some_and(|callee| SavedPlaceResolver::new(input.program).is_saved_path_callee(&callee))
        && let Some(ty) = infer_saved_key_range_arg_type(
            input.program,
            input.arg,
            input.scope,
            input.aliases,
            input.file,
            input.diagnostics,
            input.transform_old,
        )
    {
        return ty;
    }
    infer_type_with_read_scope(
        input.program,
        input.arg,
        input.scope,
        input.aliases,
        input.file,
        input.diagnostics,
        input.transform_old,
    )
}

fn infer_saved_key_range_arg_type(
    program: &CheckedProgram,
    arg: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    transform_old: Option<crate::presence::TransformOldReadScope<'_>>,
) -> Option<MarrowType> {
    let range = marrow_syntax::range_expr(arg)?;
    if let Some(step) = range.step {
        infer_type_with_read_scope(
            program,
            step,
            scope,
            aliases,
            file,
            diagnostics,
            transform_old,
        );
    }
    let start = range.start.map(|expr| {
        infer_type_with_read_scope(
            program,
            expr,
            scope,
            aliases,
            file,
            diagnostics,
            transform_old,
        )
    });
    let end = range.end.map(|expr| {
        infer_type_with_read_scope(
            program,
            expr,
            scope,
            aliases,
            file,
            diagnostics,
            transform_old,
        )
    });
    Some(match (start, end) {
        (Some(start), Some(end)) if type_compatible(&start, &end) != Some(false) => start,
        (Some(_), Some(_)) => MarrowType::Unknown,
        (Some(ty), None) | (None, Some(ty)) => ty,
        (None, None) => MarrowType::Unknown,
    })
}

fn interpolation_unsupported_source_diagnostic(
    file: &Path,
    span: SourceSpan,
    source: MarrowType,
) -> CheckDiagnostic {
    let message = format!(
        "interpolation cannot render `{}`; convert it explicitly",
        marrow_type_name(&source)
    );
    CheckDiagnostic::error(CHECK_OPERATOR_TYPE, file, span, message)
        .with_payload(DiagnosticPayload::InterpolationUnsupportedSource { source })
}

/// Reject a string literal whose escape decoding fails — an escape outside the
/// recognized set or a trailing lone backslash. The escape set is owned by
/// `marrow_syntax`; decoding here through the same function keeps the checker and
/// runtime in lockstep, catching at check what the runtime would otherwise fault on.
fn check_string_escapes(
    text: &str,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if marrow_syntax::decode_string_literal(text).is_err() {
        diagnostics.push(string_escape_diagnostic(file, span));
    }
}

/// Like [`check_string_escapes`] but for an interpolation literal text segment,
/// which carries no surrounding quotes.
fn check_interpolation_text_escapes(
    text: &str,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if marrow_syntax::decode_string_escapes(text).is_err() {
        diagnostics.push(string_escape_diagnostic(file, span));
    }
}

fn string_escape_diagnostic(file: &Path, span: SourceSpan) -> CheckDiagnostic {
    CheckDiagnostic::error(
        crate::CHECK_STRING_ESCAPE,
        file,
        span,
        "unsupported string escape; only `\\\\`, `\\\"`, `\\n`, `\\r`, and `\\t` are recognized",
    )
}

fn checked_expr(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> Option<crate::CheckedExpr> {
    lower_expr_for_file(program, file, expr, scope)
}

fn saved_expr_type(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> Option<MarrowType> {
    let expr = checked_expr(program, expr, scope, file)?;
    SavedPlaceResolver::new(program).value_type(&expr)
}

/// The type of a member-path literal `Enum::seg…` in value position, owning the
/// private-enum, category-not-selectable, ambiguous-member, and unknown-member
/// diagnostics. A category groups its descendants and is not selectable, and a bare
/// name duplicated under several parents is ambiguous (the full path always
/// disambiguates); a concrete leaf yields the enum's nominal `{module, name}`
/// identity. A non-enum multi-segment name stays `Unknown` with no diagnostic.
fn enum_member_value_type(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    segments: &[String],
    span: SourceSpan,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    let Some(resolved) = resolve_enum_member_path(program, expr, aliases, file) else {
        return MarrowType::Unknown;
    };
    if let Some(private) = resolved.private {
        diagnostics.push(
            CheckDiagnostic::error(
                CHECK_PRIVATE_ENUM,
                file,
                span,
                format!(
                    "enum `{private}` is private to its module; mark it `pub` to use it from another module"
                ),
            )
            .with_payload(DiagnosticPayload::PrivateEnum(private)),
        );
        return MarrowType::Invalid;
    }
    let enum_name = &resolved.enum_name;
    match resolved.member {
        MemberPathResolution::Found(ordinal) if resolved.schema.is_category(ordinal) => {
            diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_CATEGORY_NOT_SELECTABLE,
                    file,
                    span,
                    format!(
                        "`{}` is a category and cannot be selected; pick a concrete member under it",
                        segments.join("::")
                    ),
                )
                .with_payload(DiagnosticPayload::Enum(
                    EnumDiagnostic::CategoryNotSelectable {
                        label: resolved.member_label.clone(),
                    },
                )),
            );
            MarrowType::Invalid
        }
        MemberPathResolution::Found(_) => MarrowType::Enum {
            module: resolved.module,
            name: enum_name.clone(),
        },
        MemberPathResolution::Ambiguous(paths) => {
            diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_AMBIGUOUS_MEMBER,
                    file,
                    span,
                    format!(
                        "`{}` names more than one member of `{enum_name}`; qualify as {}",
                        segments.join("::"),
                        join_or(&paths)
                    ),
                )
                .with_payload(DiagnosticPayload::Enum(
                    EnumDiagnostic::AmbiguousMember {
                        enum_name: enum_name.clone(),
                        label: resolved.member_label,
                        candidates: paths,
                    },
                )),
            );
            MarrowType::Invalid
        }
        MemberPathResolution::NotFound => {
            diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_UNKNOWN_ENUM_MEMBER,
                    file,
                    span,
                    format!(
                        "`{enum_name}` has no member `{}`",
                        segments[segments.len() - 1]
                    ),
                )
                .with_payload(DiagnosticPayload::Enum(
                    EnumDiagnostic::UnknownMember {
                        enum_name: enum_name.clone(),
                        member: resolved.member_label,
                    },
                )),
            );
            MarrowType::Invalid
        }
    }
}

fn local_collection_access_type(
    callee: &marrow_syntax::Expression,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    scope: &[HashMap<String, MarrowType>],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<MarrowType> {
    let marrow_syntax::Expression::Name { segments, .. } = callee else {
        return None;
    };
    let [name] = segments.as_slice() else {
        return None;
    };
    match lookup_opt(scope, name)? {
        MarrowType::Sequence(element) => {
            check_local_key_count(name, 1, args.len(), span, file, diagnostics);
            if let [arg_type] = arg_types
                && !matches!(
                    type_compatible(&MarrowType::Primitive(ScalarType::Int), arg_type),
                    Some(true) | None
                )
            {
                diagnostics.push(key_type_diagnostic(
                    file,
                    span,
                    format!(
                        "key `pos` expects `int`, but this value is `{}`",
                        marrow_type_name(arg_type)
                    ),
                ));
            }
            Some(*element)
        }
        MarrowType::LocalTree { keys, value } => {
            check_local_key_count(name, keys.len(), args.len(), span, file, diagnostics);
            if keys.len() == arg_types.len() {
                for (index, (expected, actual)) in keys.iter().zip(arg_types).enumerate() {
                    if matches!(type_compatible(expected, actual), Some(false)) {
                        diagnostics.push(key_type_diagnostic(
                            file,
                            span,
                            format!(
                                "key {} expects `{}`, but this value is `{}`",
                                index + 1,
                                marrow_type_name(expected),
                                marrow_type_name(actual)
                            ),
                        ));
                    }
                }
            }
            Some(*value)
        }
        _ => None,
    }
}

fn check_local_key_count(
    name: &str,
    expected: usize,
    actual: usize,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if expected != actual {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "local collection `{name}` expects {expected} key argument(s), but {actual} were given"
            ),
        ));
    }
}

/// The declared type of a field read off a resource-typed value (`book.title`),
/// looked up in that resource's schema.
fn local_field_type(
    program: &CheckedProgram,
    base_type: &MarrowType,
    field: &str,
) -> Option<MarrowType> {
    match base_type {
        MarrowType::Resource(name) => {
            let (resource, module) = resolve_resource_type(program, name)?;
            field_member_type(program, resource, &[field], module)
                .or_else(|| group_entry_unkeyed_group_type(resource, name, &[field], &[], field))
        }
        MarrowType::GroupEntry {
            resource: name,
            layers,
        } => {
            let (resource, module) = resolve_resource_type(program, name)?;
            let mut chain: Vec<&str> = layers.iter().map(String::as_str).collect();
            chain.push(field);
            field_member_type(program, resource, &chain, module)
                .or_else(|| group_entry_unkeyed_group_type(resource, name, &chain, layers, field))
        }
        MarrowType::Error => error_field_type(field),
        _ => None,
    }
}

fn group_entry_unkeyed_group_type(
    resource: &marrow_schema::ResourceSchema,
    resource_name: &str,
    chain: &[&str],
    layers: &[String],
    field: &str,
) -> Option<MarrowType> {
    let node = resource.descend_layers(chain)?;
    if !node.key_params.is_empty() || !matches!(node.kind, marrow_schema::NodeKind::Group) {
        return None;
    }
    let mut nested = layers.to_vec();
    nested.push(field.to_string());
    Some(MarrowType::GroupEntry {
        resource: resource_name.to_string(),
        layers: nested,
    })
}

fn error_field_type(field: &str) -> Option<MarrowType> {
    let descriptor = marrow_schema::error::field(field)?;
    Some(MarrowType::from_resolved(
        descriptor.ty.clone(),
        TypeNames::default(),
    ))
}

/// The checker type of a stored field read named by its `chain` of segments,
/// outermost first. `owning_module` is the resource's declaring module, so an
/// enum-typed field reads as that module's enum rather than `Unknown`.
fn field_member_type(
    program: &CheckedProgram,
    resource: &marrow_schema::ResourceSchema,
    chain: &[&str],
    owning_module: &str,
) -> Option<MarrowType> {
    resource
        .field_type(chain)
        .map(|ty| lift_member_type(program, ty.clone(), owning_module))
}

/// Lift a schema member [`Type`] through the same nominal resource placement used
/// by annotations and constructors; enum members resolve only by the declaring
/// module or an explicit qualified owner.
pub(crate) fn lift_member_type(
    program: &CheckedProgram,
    ty: Type,
    owning_module: &str,
) -> MarrowType {
    if let Some(resource_type) = resolve_resource_schema_type(program, owning_module, &ty) {
        return resource_type;
    }
    if let Some(enum_type) = resolve_member_enum_type(program, owning_module, &ty) {
        return enum_type;
    }
    MarrowType::from_resolved(ty, TypeNames::default())
}

fn resolve_member_enum_type(
    program: &CheckedProgram,
    owning_module: &str,
    ty: &Type,
) -> Option<MarrowType> {
    match ty {
        Type::Sequence(element) => resolve_member_enum_type(program, owning_module, element)
            .map(|element_type| MarrowType::Sequence(Box::new(element_type))),
        Type::Named(name) => resolve_member_enum_name(program, owning_module, name),
        _ => None,
    }
}

fn resolve_member_enum_name(
    program: &CheckedProgram,
    owning_module: &str,
    name: &str,
) -> Option<MarrowType> {
    let (module, enum_name) = if let Some((module, enum_name)) = name.rsplit_once("::") {
        let aliases = program
            .modules
            .iter()
            .find(|module| module.name == owning_module)
            .map(|module| build_alias_map(&module.imports))
            .unwrap_or_default();
        (expand_module_alias(module, &aliases), enum_name.to_string())
    } else {
        (owning_module.to_string(), name.to_string())
    };
    enum_schema_in(program, &module, &enum_name).map(|_| MarrowType::Enum {
        module,
        name: enum_name,
    })
}

/// Look up a name's binding, innermost frame first; `None` when unbound. A bound
/// name may still be [`MarrowType::Unknown`], which is distinct from being unbound.
fn lookup_opt(scope: &[HashMap<String, MarrowType>], name: &str) -> Option<MarrowType> {
    scope
        .iter()
        .rev()
        .find_map(|frame| frame.get(name))
        .cloned()
}

/// Whether an expression is a bare single-segment name (`foo`, not `a::b` or `^books`).
fn is_bare_name(expr: &marrow_syntax::Expression) -> bool {
    matches!(expr, marrow_syntax::Expression::Name { segments, .. } if segments.len() == 1)
}

fn literal_type(kind: marrow_syntax::LiteralKind) -> MarrowType {
    use marrow_syntax::LiteralKind;
    MarrowType::Primitive(match kind {
        LiteralKind::Integer => ScalarType::Int,
        LiteralKind::Decimal => ScalarType::Decimal,
        LiteralKind::Duration => ScalarType::Duration,
        LiteralKind::String => ScalarType::Str,
        LiteralKind::Bytes => ScalarType::Bytes,
        LiteralKind::Bool => ScalarType::Bool,
    })
}

fn count_builtin_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
    args: &[marrow_syntax::Argument],
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> Option<MarrowType> {
    let marrow_syntax::Expression::Name { segments, .. } = callee else {
        return None;
    };
    let [arg] = args else {
        return None;
    };
    if segments.as_slice() == ["count"]
        && arg.name.is_none()
        && checked_expr(program, &arg.value, scope, file)
            .is_some_and(|expr| SavedPlaceResolver::new(program).is_saved_path(&expr))
    {
        Some(MarrowType::Primitive(ScalarType::Int))
    } else {
        None
    }
}
