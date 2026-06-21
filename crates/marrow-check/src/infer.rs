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
    EnumMemberPathResolution, IsCheck, check_is, join_or, resolve_diagnosed_annotation_type,
    resolve_enum_member_path,
};
use crate::executable::{
    CheckedBuiltinCall, CheckedBuiltinValueShape, CheckedLiteralKind, SavedAccessRejection,
    SavedPlaceResolver, lower_expr_for_file,
};
use crate::program::TypeNames;
use crate::typerules::{
    LiteralSign, check_literal_range, marrow_type_name, negated_integer_literal, type_compatible,
    type_renderable_at_runtime,
};
use crate::{
    CHECK_AMBIGUOUS_MEMBER, CHECK_CATEGORY_NOT_SELECTABLE, CHECK_COLLECTION_UNSUPPORTED,
    CHECK_LAYER_NOT_VALUE, CHECK_OPERATOR_TYPE, CHECK_PRIVATE_ENUM, CHECK_UNKNOWN_FIELD,
    CHECK_UNRESOLVED_NAME, CheckDiagnostic, CheckedProgram, DiagnosticPayload, EnumDiagnostic,
    MarrowType, resolve_resource_type,
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
        Some(ty) => resolve_diagnosed_annotation_type(ty, program, aliases, file),
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
    let ty = MarrowType::keyed(
        keys.iter()
            .map(|key| resolve_diagnosed_annotation_type(&key.ty, program, aliases, file)),
        value,
    );
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

pub(crate) fn infer_assignment_target_type_with_read_scope(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    transform_old: Option<crate::presence::TransformOldReadScope<'_>>,
) -> MarrowType {
    use marrow_syntax::Expression;
    match expr {
        Expression::Field {
            base, name, span, ..
        }
        | Expression::OptionalField {
            base, name, span, ..
        } => infer_field_access(FieldAccessInfer {
            program,
            expr,
            base,
            name,
            span: *span,
            scope,
            aliases,
            file,
            diagnostics,
            transform_old,
            context: FieldAccessContext::AssignmentTarget,
            position: ValuePosition::Value,
        }),
        // A write or delete target is an address, not a value read. A partially keyed
        // composite layer there names an inner sub-layer, which the dedicated
        // invalid-target rejection owns; routing through the collection-subject
        // position keeps the value-read partial-key gate from stacking a second
        // diagnostic on the same span.
        _ => infer_collection_subject_type_with_read_scope(
            program,
            expr,
            scope,
            aliases,
            file,
            diagnostics,
            transform_old,
        ),
    }
}

/// Where a saved access sits relative to its consumer. A value position binds,
/// returns, renders, or passes the access as a scalar, so a partially keyed composite
/// layer there is a non-value misuse. A collection-subject position streams it — a
/// `for` iterable or a collection builtin's argument — where a partial key is the valid
/// inner sub-layer to traverse, so the non-value rejection must not fire.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValuePosition {
    Value,
    CollectionSubject,
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
    infer_value(
        program,
        expr,
        ValuePosition::Value,
        scope,
        aliases,
        file,
        diagnostics,
        transform_old,
    )
}

/// Infer the type of a saved access that its consumer streams as a collection — a
/// `for` iterable or a collection builtin's argument. The result is discarded or
/// replaced by the builtin's own type; this surfaces the subject's key-argument and
/// structural diagnostics without the value-position partial-key rejection, since a
/// partially keyed composite layer is a valid inner sub-layer to stream.
pub(crate) fn infer_collection_subject_type_with_read_scope(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    transform_old: Option<crate::presence::TransformOldReadScope<'_>>,
) -> MarrowType {
    infer_value(
        program,
        expr,
        ValuePosition::CollectionSubject,
        scope,
        aliases,
        file,
        diagnostics,
        transform_old,
    )
}

#[allow(clippy::too_many_arguments)]
fn infer_value(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    position: ValuePosition,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    transform_old: Option<crate::presence::TransformOldReadScope<'_>>,
) -> MarrowType {
    use marrow_syntax::Expression;
    if reject_saved_access(program, expr, scope, file, diagnostics) {
        return MarrowType::Unknown;
    }
    match expr {
        Expression::Literal { kind, text, span } => {
            check_literal_range(*kind, text, LiteralSign::Bare, *span, file, diagnostics);
            match kind {
                marrow_syntax::LiteralKind::String => {
                    check_string_escapes(text, *span, file, diagnostics);
                }
                marrow_syntax::LiteralKind::Bytes => {
                    check_bytes_escapes(text, *span, file, diagnostics);
                }
                _ => {}
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
                        let before = diagnostics.len();
                        let ty = infer_type_with_read_scope(
                            program,
                            expr,
                            scope,
                            aliases,
                            file,
                            diagnostics,
                            transform_old,
                        );
                        if saved_collection_render_unowned(
                            program,
                            expr,
                            scope,
                            file,
                            diagnostics,
                            before,
                        ) {
                            diagnostics.push(saved_collection_render_diagnostic(file, expr.span()));
                        } else if type_renderable_at_runtime(&ty) == Some(false) {
                            diagnostics.push(render_unsupported_source_diagnostic(
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
        Expression::Name { segments, span, .. } if segments.len() == 1 => {
            let name = &segments[0];
            lookup_opt(scope, name).unwrap_or_else(|| {
                diagnostics.push(
                    CheckDiagnostic::error(
                        CHECK_UNRESOLVED_NAME,
                        file,
                        *span,
                        format!("`{name}` is not defined"),
                    )
                    .with_payload(DiagnosticPayload::UnresolvedName { name: name.clone() }),
                );
                MarrowType::Unknown
            })
        }
        Expression::Unary { op, operand, span } => {
            // A `-` over an integer literal range-checks against the negated bound, so
            // `i64::MIN` is in range though its bare magnitude is not. Checking the
            // operand here keeps the literal arm from rejecting that magnitude on its own.
            let operand = if let Some((text, literal_span)) = negated_integer_literal(*op, operand)
            {
                check_literal_range(
                    marrow_syntax::LiteralKind::Integer,
                    text,
                    LiteralSign::Negated,
                    literal_span,
                    file,
                    diagnostics,
                );
                literal_type(marrow_syntax::LiteralKind::Integer)
            } else {
                infer_type_with_read_scope(
                    program,
                    operand,
                    scope,
                    aliases,
                    file,
                    diagnostics,
                    transform_old,
                )
            };
            check_unary(*op, &operand, *span, file, diagnostics)
        }
        Expression::Binary {
            op,
            left,
            right,
            span,
        } => {
            // A saved collection is an in-place stream with no materialized value, so it
            // is never an operator operand — arithmetic, comparison, `is`, or `??` alike.
            // Such an operand infers `Unknown`, so the operator check would otherwise
            // defer and the program would fault clean-then-runtime; reject it here at the
            // operator. A saved scalar read is a single value and stays a legal operand.
            if binary_operand_is_saved_collection(program, left, scope, file)
                || binary_operand_is_saved_collection(program, right, scope, file)
            {
                diagnostics.push(CheckDiagnostic::error(
                    CHECK_OPERATOR_TYPE,
                    file,
                    *span,
                    "operator cannot be applied to a saved collection; iterate it instead",
                ));
                return MarrowType::Invalid;
            }
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
            // A keyed callee whose base is already a definite error (a descent off a
            // partial-key layer or a materialized record) makes the whole keyed access
            // invalid: that owning error is the sole diagnostic, so the result stays
            // `Invalid` and a surrounding `??`/return suppresses its cascade. The callee
            // is inferred in collection-subject position: a partial-key layer callee
            // (`^cubes(1).cells` in `^cubes(1).cells(x)`) is the valid descent target the
            // arguments complete, not a value-read that the partial-key gate may reject.
            if !is_bare_name(callee) {
                let callee_type = infer_collection_subject_type_with_read_scope(
                    program,
                    callee,
                    scope,
                    aliases,
                    file,
                    diagnostics,
                    transform_old,
                );
                if matches!(callee_type, MarrowType::Invalid) {
                    return MarrowType::Invalid;
                }
            }
            let mut arg_types = Vec::with_capacity(args.len());
            for (index, arg) in args.iter().enumerate() {
                arg_types.push(infer_call_arg_type(CallArgInfer {
                    program,
                    callee,
                    arg_index: index,
                    arg_name: arg.name.as_deref(),
                    arg: &arg.value,
                    scope,
                    aliases,
                    file,
                    diagnostics,
                    transform_old,
                }));
            }
            check_print_argument_renderable(
                program,
                callee,
                args,
                &arg_types,
                scope,
                file,
                diagnostics,
            );
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
                bare_saved_value_type(program, expr, *span, position, scope, file, diagnostics)
            } else {
                call_type
            }
        }
        // A plain field read and an optional (`?.`) field read resolve to the same
        // declared leaf type: the short-circuit only changes the read's runtime
        // behavior on absence, not the type of a populated leaf.
        Expression::Field {
            base, name, span, ..
        }
        | Expression::OptionalField {
            base, name, span, ..
        } => infer_field_access(FieldAccessInfer {
            program,
            expr,
            base,
            name,
            span: *span,
            scope,
            aliases,
            file,
            diagnostics,
            transform_old,
            context: FieldAccessContext::Read,
            position,
        }),
        Expression::Name { segments, span, .. } if segments.len() >= 2 => {
            enum_member_value_type(program, expr, segments, *span, aliases, file, diagnostics)
        }
        Expression::SavedRoot { span, .. } => {
            bare_saved_value_type(program, expr, *span, position, scope, file, diagnostics)
        }
        Expression::Name { .. } => MarrowType::Unknown,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldAccessContext {
    Read,
    AssignmentTarget,
}

struct FieldAccessInfer<'a, 'd> {
    program: &'a CheckedProgram,
    expr: &'a marrow_syntax::Expression,
    base: &'a marrow_syntax::Expression,
    name: &'a str,
    span: SourceSpan,
    scope: &'a [HashMap<String, MarrowType>],
    aliases: &'a HashMap<String, Vec<String>>,
    file: &'a Path,
    diagnostics: &'d mut Vec<CheckDiagnostic>,
    transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
    context: FieldAccessContext,
    position: ValuePosition,
}

fn infer_field_access(input: FieldAccessInfer<'_, '_>) -> MarrowType {
    if reject_saved_access(
        input.program,
        input.expr,
        input.scope,
        input.file,
        input.diagnostics,
    ) {
        return MarrowType::Unknown;
    }
    // A `.field` or child-layer descends off a record value. A partially keyed
    // composite layer is an iterable inner sub-layer, not a record, so descending
    // off it would address durable data with an unfilled key column elided. Reject
    // the descent on the field span before resolving its type.
    if descends_off_partial_key_layer(input.program, input.base, input.scope, input.file) {
        input.diagnostics.push(layer_not_value_diagnostic(
            input.file,
            input.span,
            input.name,
            LayerNotValueReason::PartialKeyLayer,
        ));
        return MarrowType::Invalid;
    }
    let base_type = match input.context {
        FieldAccessContext::Read => infer_type_with_read_scope(
            input.program,
            input.base,
            input.scope,
            input.aliases,
            input.file,
            input.diagnostics,
            input.transform_old,
        ),
        FieldAccessContext::AssignmentTarget => infer_assignment_target_type_with_read_scope(
            input.program,
            input.base,
            input.scope,
            input.aliases,
            input.file,
            input.diagnostics,
            input.transform_old,
        ),
    };
    // A bare-key field access in a value position is itself a value-read entry, like the
    // call and saved-root arms. A partially keyed composite layer there — `^grids(1).cells`,
    // every key column unfilled — names an iterable inner sub-layer, never a scalar, so
    // the one value-read gate rejects it here too rather than letting it fall through to
    // the untyped catch-all and fault `run.unsupported` in a position that imposes no
    // type expectation. A collection-subject access streams the layer, so the gate's
    // position guard skips it there.
    if input.context == FieldAccessContext::Read
        && let Some(checked) = checked_expr(input.program, input.expr, input.scope, input.file)
        && SavedPlaceResolver::new(input.program)
            .partial_key_layer_name(&checked)
            .is_some()
    {
        return bare_saved_value_type(
            input.program,
            input.expr,
            input.span,
            input.position,
            input.scope,
            input.file,
            input.diagnostics,
        );
    }
    if let Some(ty) = saved_expr_type(input.program, input.expr, input.scope, input.file) {
        return ty;
    }
    match local_field_resolution(input.program, &base_type, input.name) {
        FieldResolution::Resolved(ty) => ty,
        FieldResolution::UnknownField | FieldResolution::NoFields
            if input.context == FieldAccessContext::Read =>
        {
            input
                .diagnostics
                .push(unknown_field_diagnostic(input.file, input.span, input.name));
            MarrowType::Invalid
        }
        // A keyed child layer is reached only through its saved address, never
        // pulled into a materialized record value. Reading it off a materialized
        // value can never resolve, so a Read names a definite error. A saved-path
        // access of the same shape (`^outers(1).groups`) resolves through its
        // address and is handled above, so it is excluded here.
        FieldResolution::NonValueMember
            if input.context == FieldAccessContext::Read
                && !reads_through_saved_place(
                    input.program,
                    input.expr,
                    input.scope,
                    input.file,
                ) =>
        {
            input.diagnostics.push(layer_not_value_diagnostic(
                input.file,
                input.span,
                input.name,
                LayerNotValueReason::MaterializedValue,
            ));
            MarrowType::Invalid
        }
        FieldResolution::UnknownField
        | FieldResolution::NoFields
        | FieldResolution::NonValueMember => MarrowType::Unknown,
        FieldResolution::InvalidBase => MarrowType::Invalid,
        FieldResolution::UnresolvedBase => MarrowType::Unknown,
    }
}

fn reject_saved_access(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> bool {
    reject_saved_access_inner(program, expr, scope, file, diagnostics, false)
}

pub(crate) fn reject_saved_access_with_suggested_index(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> bool {
    reject_saved_access_inner(program, expr, scope, file, diagnostics, true)
}

fn reject_saved_access_inner(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    suggested_index: bool,
) -> bool {
    let Some(rejection) = checked_expr(program, expr, scope, file).and_then(|expr| {
        let resolver = SavedPlaceResolver::new(program);
        if suggested_index {
            resolver.access_rejection_with_suggested_index(&expr, scope)
        } else {
            resolver.access_rejection(&expr)
        }
    }) else {
        return false;
    };
    match rejection {
        SavedAccessRejection::GeneratedIndexBranch => diagnostics.push(CheckDiagnostic::error(
            CHECK_COLLECTION_UNSUPPORTED,
            file,
            expr.span(),
            "generated index branches do not expose resource members or chained calls",
        )),
        SavedAccessRejection::NoMatchingIndex { declaration } => diagnostics.push(
            CheckDiagnostic::error(
                CHECK_COLLECTION_UNSUPPORTED,
                file,
                expr.span(),
                "lookup has no matching declared index",
            )
            .with_payload(DiagnosticPayload::SuggestedIndex { declaration }),
        ),
        SavedAccessRejection::KeyedRootMemberWithoutIdentity(root) => {
            diagnostics.push(key_type_diagnostic(
                file,
                expr.span(),
                format!("`^{root}` must be addressed with an identity before using its members"),
            ))
        }
    }
    true
}

struct CallArgInfer<'a, 'd> {
    program: &'a CheckedProgram,
    callee: &'a marrow_syntax::Expression,
    arg_index: usize,
    arg_name: Option<&'a str>,
    arg: &'a marrow_syntax::Expression,
    scope: &'a [HashMap<String, MarrowType>],
    aliases: &'a HashMap<String, Vec<String>>,
    file: &'a Path,
    diagnostics: &'d mut Vec<CheckDiagnostic>,
    transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
}

fn infer_call_arg_type(input: CallArgInfer<'_, '_>) -> MarrowType {
    if input.arg_name.is_none()
        && callee_accepts_missing_index_suggestion(input.callee)
        && reject_saved_access_with_suggested_index(
            input.program,
            input.arg,
            input.scope,
            input.file,
            input.diagnostics,
        )
    {
        return MarrowType::Unknown;
    }
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
    if input.arg_name.is_none() && callee_streams_collection_argument(input.callee, input.arg_index)
    {
        return infer_collection_subject_type_with_read_scope(
            input.program,
            input.arg,
            input.scope,
            input.aliases,
            input.file,
            input.diagnostics,
            input.transform_old,
        );
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

fn callee_accepts_missing_index_suggestion(callee: &marrow_syntax::Expression) -> bool {
    matches!(
        callee,
        marrow_syntax::Expression::Name { segments, .. } if segments.as_slice() == ["count"]
    )
}

/// Whether the `arg_index`-th positional argument of builtin `callee` accepts a saved
/// subject streamed as a collection rather than read as a scalar. The builtin descriptor
/// table is the sole owner of this argument shape: a parameter typed as a collection,
/// saved path, or saved layer takes its subject in streamed position, so a partially keyed
/// composite layer is inferred there instead of being rejected as a non-value.
fn callee_streams_collection_argument(
    callee: &marrow_syntax::Expression,
    arg_index: usize,
) -> bool {
    let marrow_syntax::Expression::Name { segments, .. } = callee else {
        return false;
    };
    let [name] = segments.as_slice() else {
        return false;
    };
    CheckedBuiltinCall::descriptor_for_name(name).is_some_and(|descriptor| {
        descriptor.params.get(arg_index).is_some_and(|param| {
            matches!(
                param.shape,
                CheckedBuiltinValueShape::Collection
                    | CheckedBuiltinValueShape::SavedPath
                    | CheckedBuiltinValueShape::SavedLayer
            )
        })
    })
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

/// Reject a `print` argument whose type has no direct text form, the same set
/// string interpolation rejects, so a non-renderable value faults at check rather
/// than at runtime. Only a single positional argument is examined; arity and other
/// builtins are handled on the call path.
fn check_print_argument_renderable(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let marrow_syntax::Expression::Name { segments, .. } = callee else {
        return;
    };
    if segments.as_slice() != ["print"] {
        return;
    }
    let ([arg], [ty]) = (args, arg_types) else {
        return;
    };
    // The argument was already inferred, so a partially-keyed composite layer here
    // already carries the more precise `check.layer_not_value` at this span; defer to it
    // by treating `before` as the diagnostics already accumulated.
    if saved_collection_render_unowned(program, &arg.value, scope, file, diagnostics, 0) {
        diagnostics.push(saved_collection_render_diagnostic(file, arg.value.span()));
        return;
    }
    if type_renderable_at_runtime(ty) == Some(false) {
        diagnostics.push(render_unsupported_source_diagnostic(
            file,
            arg.value.span(),
            ty.clone(),
        ));
    }
}

/// Whether a render-surface value (`print` or interpolation) is a saved collection
/// the render check itself must own. Such a value is an in-place stream with no text
/// form and infers `Unknown`, so the type-based renderable check would otherwise defer
/// and the program would fault clean-then-runtime. A saved scalar read is a single
/// renderable value and is excluded by the shared classifier. `inferring` already
/// rejected a partially-keyed composite layer here with the more precise
/// `check.layer_not_value`, so a diagnostic produced for this span during inference
/// (everything from `before` on) defers to that owner rather than stacking a second.
fn saved_collection_render_unowned(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
    diagnostics: &[CheckDiagnostic],
    before: usize,
) -> bool {
    binary_operand_is_saved_collection(program, expr, scope, file)
        && !diagnostics[before..]
            .iter()
            .any(|diagnostic| diagnostic.file == file && diagnostic.span == expr.span())
}

/// The render rejection raised for a saved collection in `print` or interpolation: it
/// has no text form, so it must be iterated rather than rendered as a value.
fn saved_collection_render_diagnostic(file: &Path, span: SourceSpan) -> CheckDiagnostic {
    CheckDiagnostic::error(
        CHECK_OPERATOR_TYPE,
        file,
        span,
        "cannot render a saved collection; iterate it instead",
    )
}

/// The rejection both render surfaces — `print` and string interpolation — raise
/// for a value type that has no direct text form. The two surfaces accept and
/// reject the same set, so they share one diagnostic.
fn render_unsupported_source_diagnostic(
    file: &Path,
    span: SourceSpan,
    source: MarrowType,
) -> CheckDiagnostic {
    let message = format!(
        "cannot render `{}`; convert it explicitly",
        marrow_type_name(&source)
    );
    CheckDiagnostic::error(CHECK_OPERATOR_TYPE, file, span, message)
        .with_payload(DiagnosticPayload::RenderUnsupportedSource { source })
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

/// Reject a bytes literal whose escape decoding fails — an escape outside the
/// recognized set, a trailing lone backslash, or a malformed or truncated
/// `\xNN`. The escape grammar is owned by `marrow_syntax`; decoding here through
/// the same function keeps the checker and runtime in lockstep.
fn check_bytes_escapes(
    text: &str,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if marrow_syntax::decode_bytes_literal(text).is_err() {
        diagnostics.push(bytes_escape_diagnostic(file, span));
    }
}

fn bytes_escape_diagnostic(file: &Path, span: SourceSpan) -> CheckDiagnostic {
    CheckDiagnostic::error(
        crate::CHECK_BYTES_ESCAPE,
        file,
        span,
        "unsupported bytes escape; only `\\\\`, `\\\"`, `\\n`, `\\r`, `\\t`, and `\\xNN` are recognized",
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

/// Whether `expr` resolves to a saved place — a path rooted at saved data that the
/// runtime reads through its address. A field access that lowers to a saved place
/// (`^outers(1).groups`) is a saved-path descent, distinct from the same-shaped
/// access read off a materialized record value (`inner.groups`).
fn reads_through_saved_place(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    checked_expr(program, expr, scope, file).is_some_and(|expr| expr.saved_place().is_some())
}

/// Whether a binary operand is a saved collection — a store root, a saved keyed
/// sub-layer, or an index branch, bare or laundered through a traversal combinator.
/// Such an operand has no materialized value, so it can never feed an operator. This
/// is the same place-based classifier every by-value boundary shares, so the operator
/// rejection and the binding/argument/return rejections agree on what a saved
/// collection is — a saved scalar read (a single stored value) is excluded.
fn binary_operand_is_saved_collection(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    crate::checks::materializes_saved_collection_by_value(program, expr, scope, file)
}

/// Whether `base` is a saved place whose innermost keyed layer is only partially
/// addressed. Such a base names an iterable inner sub-layer, not a record value, so
/// a `.field` or child-layer access off it cannot descend into a value.
fn descends_off_partial_key_layer(
    program: &CheckedProgram,
    base: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    checked_expr(program, base, scope, file)
        .is_some_and(|base| SavedPlaceResolver::new(program).is_partial_key_layer_path(&base))
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

/// The type of a saved access read in a bare value position — a scalar bind without
/// `??`, an interpolation, a plain call argument, or a function return. A partially
/// keyed composite layer names an iterable inner sub-layer, never a scalar, so reading
/// it as a value is rejected here with the same `LayerNotValue` diagnostic a `.field`
/// descent off a partial key raises; otherwise the access resolves through its saved
/// shape. This is the single value-read entry the strict partial-key gate guards, so a
/// one-remaining-column prefix cannot leak as a value and fault `run.absent_element`.
/// A collection-subject position streams the layer, so the rejection is skipped there.
fn bare_saved_value_type(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    span: SourceSpan,
    position: ValuePosition,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    if position == ValuePosition::Value
        && let Some(checked) = checked_expr(program, expr, scope, file)
        && let Some(layer) = SavedPlaceResolver::new(program).partial_key_layer_name(&checked)
    {
        diagnostics.push(layer_not_value_diagnostic(
            file,
            span,
            layer,
            LayerNotValueReason::PartialKeyValue,
        ));
        return MarrowType::Invalid;
    }
    saved_expr_type(program, expr, scope, file).unwrap_or(MarrowType::Unknown)
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
    let resolved = match resolve_enum_member_path(program, expr, aliases, file) {
        EnumMemberPathResolution::Resolved(resolved) => resolved,
        EnumMemberPathResolution::AmbiguousBareForeignOwner(ambiguous) => {
            diagnostics.push(ambiguous.diagnostic(file, span));
            return MarrowType::Invalid;
        }
        EnumMemberPathResolution::MissingOrNonEnum => return MarrowType::Unknown,
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
            let (index, _) = resolved.unresolved_segment(segments);
            let segment_span = name_segment_span(expr, index).unwrap_or(span);
            diagnostics.push(resolved.unknown_member_diagnostic(
                file,
                segment_span,
                segments,
                index,
            ));
            MarrowType::Invalid
        }
    }
}

/// The span of the segment at `index` within a `Name` expression, for a diagnostic
/// that blames one written path segment rather than the whole reference.
fn name_segment_span(expr: &marrow_syntax::Expression, index: usize) -> Option<SourceSpan> {
    let marrow_syntax::Expression::Name { segment_spans, .. } = expr else {
        return None;
    };
    segment_spans.get(index).copied()
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

enum FieldResolution {
    Resolved(MarrowType),
    UnknownField,
    NonValueMember,
    InvalidBase,
    /// The base is a concrete value with no resource fields — a scalar, enum,
    /// identity, sequence, or keyed map. A field read off it can never resolve, so
    /// it is a definite error rather than a deferred one.
    NoFields,
    UnresolvedBase,
}

/// Whether an assignment target names a place declared `ErrorCode`, so a value
/// stored into it must satisfy the dotted-lowercase grammar. Resolves the place's
/// declaring schema node from its base type, covering a plain field (`entry.code`,
/// `^logs(1).code`) and a keyed-leaf write (`^logs(1).tags(0)`) alike. `false` for
/// any target that is not a resolvable resource field or keyed leaf.
pub(crate) fn assignment_target_is_error_code(
    program: &CheckedProgram,
    target: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    transform_old: Option<crate::presence::TransformOldReadScope<'_>>,
) -> bool {
    use marrow_syntax::Expression;
    // A keyed-leaf write `place.layer(key) = value` carries the leaf name on the
    // `Field` callee; a plain-field write carries it on the target itself.
    let member = match target {
        Expression::Field { base, name, .. } | Expression::OptionalField { base, name, .. } => {
            Some((base.as_ref(), name.as_str()))
        }
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::Field { base, name, .. } | Expression::OptionalField { base, name, .. } => {
                Some((base.as_ref(), name.as_str()))
            }
            _ => None,
        },
        _ => None,
    };
    let Some((base, name)) = member else {
        return false;
    };
    let mut sink = Vec::new();
    let base_type = infer_assignment_target_type_with_read_scope(
        program,
        base,
        scope,
        aliases,
        file,
        &mut sink,
        transform_old,
    );
    resolved_field_node(program, &base_type, name).is_some_and(marrow_schema::Node::is_error_code)
}

/// The schema node declaring `field` on a resource-shaped base, or `None` when the
/// base is not a resolvable resource value or carries no such field. The resource
/// tree walk itself is owned by [`marrow_schema::ResourceSchema::node_at`]; this
/// only maps the base type into the resource name and saved-path chain it reads.
fn resolved_field_node<'a>(
    program: &'a CheckedProgram,
    base_type: &MarrowType,
    field: &str,
) -> Option<&'a marrow_schema::Node> {
    let (name, chain): (&str, Vec<&str>) = match base_type {
        MarrowType::Resource(name) => (name, vec![field]),
        MarrowType::GroupEntry {
            resource: name,
            layers,
        } => {
            let mut chain: Vec<&str> = layers.iter().map(String::as_str).collect();
            chain.push(field);
            (name, chain)
        }
        _ => return None,
    };
    let (resource, _) = resolve_resource_type(program, name)?;
    resource.node_at(&chain)
}

/// Resolve a field read off a resource-shaped value (`book.title`) through that
/// resource's schema while distinguishing a missing member from an untyped base.
fn local_field_resolution(
    program: &CheckedProgram,
    base_type: &MarrowType,
    field: &str,
) -> FieldResolution {
    match base_type {
        MarrowType::Resource(name) => {
            let Some((resource, module)) = resolve_resource_type(program, name) else {
                return FieldResolution::UnresolvedBase;
            };
            resource_field_resolution(program, resource, name, module, &[field], &[])
        }
        MarrowType::GroupEntry {
            resource: name,
            layers,
        } => {
            let Some((resource, module)) = resolve_resource_type(program, name) else {
                return FieldResolution::UnresolvedBase;
            };
            let mut chain: Vec<&str> = layers.iter().map(String::as_str).collect();
            chain.push(field);
            resource_field_resolution(program, resource, name, module, &chain, layers)
        }
        MarrowType::Error => error_field_type(field)
            .map(FieldResolution::Resolved)
            .unwrap_or(FieldResolution::UnknownField),
        MarrowType::Invalid => FieldResolution::InvalidBase,
        // A scalar, enum, identity, sequence, or keyed map carries no resource
        // fields, so a field read off it can never resolve. `Unknown` alone defers,
        // keeping cross-module unresolved bases free of false positives.
        MarrowType::Primitive(_)
        | MarrowType::Enum { .. }
        | MarrowType::Identity(_)
        | MarrowType::Sequence(_)
        | MarrowType::LocalTree { .. } => FieldResolution::NoFields,
        MarrowType::Unknown => FieldResolution::UnresolvedBase,
    }
}

/// The materialized type of `field` read off a value of `base_type`, when it
/// resolves to a concrete member type — a plain field's scalar or a nested
/// unkeyed group's `GroupEntry`. The presence walk uses this to descend a chained
/// group base such as `p.address` before classifying its sparse fields.
pub(crate) fn member_value_type(
    program: &CheckedProgram,
    base_type: &MarrowType,
    field: &str,
) -> Option<MarrowType> {
    match local_field_resolution(program, base_type, field) {
        FieldResolution::Resolved(ty) => Some(ty),
        _ => None,
    }
}

fn resource_field_resolution(
    program: &CheckedProgram,
    resource: &marrow_schema::ResourceSchema,
    resource_name: &str,
    owning_module: &str,
    chain: &[&str],
    layers: &[String],
) -> FieldResolution {
    let Some(member) = chain.last() else {
        return FieldResolution::UnresolvedBase;
    };
    let Some(node) = resource.node_at(chain) else {
        return FieldResolution::UnknownField;
    };
    if let Some(ty) = node.plain_field_type() {
        return FieldResolution::Resolved(lift_member_type(program, ty.clone(), owning_module));
    }
    if node.key_params.is_empty() && matches!(node.kind, marrow_schema::NodeKind::Group) {
        let mut nested = layers.to_vec();
        nested.push((*member).to_string());
        return FieldResolution::Resolved(MarrowType::GroupEntry {
            resource: resource_name.to_string(),
            layers: nested,
        });
    }
    FieldResolution::NonValueMember
}

fn error_field_type(field: &str) -> Option<MarrowType> {
    let descriptor = marrow_schema::error::field(field)?;
    Some(MarrowType::from_resolved(
        descriptor.ty.clone(),
        TypeNames::default(),
    ))
}

/// Whether `field` read off a materialized value of `base_type` is a sparse
/// (maybe-present) plain field — a resource field declared without `required`, or
/// an `Error`'s optional `help`/`data`. A required field, a layer, an unknown
/// field, or a base that is not a materialized resource value is not sparse, so a
/// presence guard only widens to fields that can genuinely be absent at runtime.
pub(crate) fn sparse_member(program: &CheckedProgram, base_type: &MarrowType, field: &str) -> bool {
    match base_type {
        MarrowType::Resource(name) => resolve_resource_type(program, name)
            .is_some_and(|(resource, _)| resource_member_sparse(resource, &[field])),
        MarrowType::GroupEntry {
            resource: name,
            layers,
        } => resolve_resource_type(program, name).is_some_and(|(resource, _)| {
            let mut chain: Vec<&str> = layers.iter().map(String::as_str).collect();
            chain.push(field);
            resource_member_sparse(resource, &chain)
        }),
        MarrowType::Error => {
            marrow_schema::error::field(field).is_some_and(|descriptor| !descriptor.required)
        }
        MarrowType::Primitive(_)
        | MarrowType::Enum { .. }
        | MarrowType::Identity(_)
        | MarrowType::Sequence(_)
        | MarrowType::LocalTree { .. }
        | MarrowType::Invalid
        | MarrowType::Unknown => false,
    }
}

/// Whether the resource node addressed by `chain` is a sparse plain field. A
/// plain field carries `required` on its `Slot`; a layer or absent node is not a
/// sparse field.
fn resource_member_sparse(resource: &marrow_schema::ResourceSchema, chain: &[&str]) -> bool {
    let Some((member, parents)) = chain.split_last() else {
        return false;
    };
    let members = match parents {
        [] => &resource.members,
        _ => match resource.descend_layers(parents) {
            Some(node) => &node.members,
            None => return false,
        },
    };
    members.iter().any(|node| {
        node.name == *member
            && matches!(
                node.kind,
                marrow_schema::NodeKind::Slot {
                    required: false,
                    ..
                }
            )
            && node.key_params.is_empty()
    })
}

fn unknown_field_diagnostic(file: &Path, span: SourceSpan, field: &str) -> CheckDiagnostic {
    CheckDiagnostic::error(
        CHECK_UNKNOWN_FIELD,
        file,
        span,
        format!("field `{field}` is not declared on this value's type"),
    )
}

/// Why a `.field`/child-layer access or a bare value read names a sub-layer rather
/// than a value.
#[derive(Clone, Copy)]
enum LayerNotValueReason {
    /// The base is a materialized record value; keyed child layers are not pulled
    /// into it and are reached only through their saved address.
    MaterializedValue,
    /// The base is a saved address whose innermost composite layer is only partially
    /// keyed, so it names an iterable inner sub-layer with key columns still to fill.
    PartialKeyLayer,
    /// A value-read position reads a partially keyed composite layer directly. The
    /// address names an iterable inner sub-layer, so reading it as a scalar would
    /// check clean and fault `run.absent_element`.
    PartialKeyValue,
}

fn layer_not_value_diagnostic(
    file: &Path,
    span: SourceSpan,
    field: &str,
    reason: LayerNotValueReason,
) -> CheckDiagnostic {
    let message = match reason {
        LayerNotValueReason::MaterializedValue => format!(
            "`{field}` is a keyed child layer; read it through its saved address, not through a materialized value"
        ),
        LayerNotValueReason::PartialKeyLayer => format!(
            "`{field}` descends off a partially keyed layer; supply every key column to reach a record before descending into it"
        ),
        LayerNotValueReason::PartialKeyValue => format!(
            "`{field}` is a partially keyed layer, not a value; supply every key column to read one entry"
        ),
    };
    CheckDiagnostic::error(CHECK_LAYER_NOT_VALUE, file, span, message).with_payload(
        DiagnosticPayload::LayerNotValue {
            field: field.to_string(),
        },
    )
}

/// Lift a schema member [`Type`] through the same nominal placement used by
/// annotations and constructors.
pub(crate) fn lift_member_type(
    program: &CheckedProgram,
    ty: Type,
    owning_module: &str,
) -> MarrowType {
    if let Some(module) = program
        .modules
        .iter()
        .find(|module| module.name == owning_module)
    {
        return crate::enums::resolve_schema_type_for_module(&ty, program, module);
    }
    MarrowType::from_resolved(ty, TypeNames::default())
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
    CheckedLiteralKind::lower(kind).marrow_type()
}
