//! Expression type inference and the saved-path/field type resolution it walks.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{MemberPathResolution, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::checks::{
    CallCheck, CoalesceCheck, check_binary, check_call, check_coalesce, check_saved_key_args,
    check_unary, is_saved_index_branch_path, key_type_diagnostic,
};
use crate::enums::{
    IsCheck, check_is, enum_schema_in, join_or, resolve_enum_member_path, resolve_type,
};
use crate::program::TypeNames;
use crate::resolve::resolve_store_by_root;
use crate::typerules::{
    check_literal_range, marrow_type_name, type_compatible, type_renderable_at_runtime,
};
use crate::{
    CHECK_AMBIGUOUS_MEMBER, CHECK_CATEGORY_NOT_SELECTABLE, CHECK_COLLECTION_UNSUPPORTED,
    CHECK_OPERATOR_TYPE, CHECK_PRIVATE_ENUM, CHECK_UNKNOWN_ENUM_MEMBER, CHECK_UNRESOLVED_NAME,
    CheckDiagnostic, CheckedProgram, DiagnosticPayload, EnumDiagnostic, MarrowType,
    build_alias_map, expand_module_alias, identity_type_for_store, resolve_resource_schema_type,
    resolve_resource_type, resource_type_name,
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
    if let Some(rejection) = saved_access_rejection(program, expr) {
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
            let arg_types: Vec<MarrowType> = args
                .iter()
                .map(|arg| {
                    infer_type_with_read_scope(
                        program,
                        &arg.value,
                        scope,
                        aliases,
                        file,
                        diagnostics,
                        transform_old,
                    )
                })
                .collect();
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
            check_saved_key_args(program, callee, args, &arg_types, *span, file, diagnostics);
            // A call-shaped saved read (keyed-leaf or whole-record) is not a function
            // call; type it through its saved shape once the call path comes back Unknown.
            if matches!(call_type, MarrowType::Unknown) {
                saved_call_type(program, callee, args).unwrap_or(MarrowType::Unknown)
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
            saved_field_type(program, base, name)
                .or_else(|| singleton_saved_group_field_type(program, base, name))
                .or_else(|| saved_group_field_type(program, base, name))
                .or_else(|| local_field_type(program, &base_type, name))
                .unwrap_or(MarrowType::Unknown)
        }
        Expression::Name { segments, span } if segments.len() >= 2 => {
            enum_member_value_type(program, expr, segments, *span, aliases, file, diagnostics)
        }
        Expression::SavedRoot { name, .. } => singleton_resource_type(program, name),
        Expression::Name { .. } => MarrowType::Unknown,
    }
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

/// The declared type of a call-shaped saved read, tried after a function call
/// comes back `Unknown`: a `count(path)`, a keyed-leaf read, a singleton leaf, a
/// unique-index identity, a whole-record read, or a group entry. The first matching
/// shape wins; `None` when the callee names no saved read.
fn saved_call_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
    args: &[marrow_syntax::Argument],
) -> Option<MarrowType> {
    count_builtin_type(program, callee, args)
        .or_else(|| saved_leaf_type(program, callee))
        .or_else(|| singleton_saved_leaf_type(program, callee))
        .or_else(|| saved_index_identity_type(program, callee))
        .or_else(|| saved_resource_type(program, callee))
        .or_else(|| saved_group_entry_type(program, callee))
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

/// The resource type of a bare `^root` whole read, defined only for a keyless
/// singleton (a saved root with no identity keys): reading it by its root yields the
/// whole record. A keyed root needs keys to address a record, so a bare reference to
/// one names no value here.
fn singleton_resource_type(program: &CheckedProgram, root: &str) -> MarrowType {
    match resolve_store_by_root(program, root) {
        Some(store) if store.store.identity_keys.is_empty() => {
            MarrowType::Resource(resource_type_name(&store.module.name, &store.resource.name))
        }
        _ => MarrowType::Unknown,
    }
}

/// The declared type of a top-level saved field read `^root(key…).field`, or the
/// keyless singleton form `^root.field`. `base` is a keyed record access
/// `^root(key…)` (a call whose callee is the saved root) or the singleton saved
/// root `^root`. Group layers and keyed leaves resolve elsewhere.
fn saved_field_type(
    program: &CheckedProgram,
    base: &marrow_syntax::Expression,
    field: &str,
) -> Option<MarrowType> {
    use marrow_syntax::Expression;
    let (root, bare_root) = match base {
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::SavedRoot { name, .. } => (name, false),
            _ => return None,
        },
        Expression::SavedRoot { name, .. } => (name, true),
        _ => return None,
    };
    let store = resolve_store_by_root(program, root)?;
    if bare_root && !store.store.identity_keys.is_empty() {
        return None;
    }
    field_member_type(program, store.resource, &[field], &store.module.name)
}

/// The resource type of a whole-record read `^root(key…)`, where the call's callee
/// is the saved root, mirroring the runtime's whole-resource read producing a
/// `Value::Resource`. Enables field access off a saved read stored in a local.
fn saved_resource_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = callee else {
        return None;
    };
    let store = resolve_store_by_root(program, root)?;
    Some(MarrowType::Resource(resource_type_name(
        &store.module.name,
        &store.resource.name,
    )))
}

/// The record type of a whole group-entry access `^root(key…).layer(key…)` whose
/// terminal layer is a keyed GROUP (not a leaf). A group entry is layer-specific:
/// field reads resolve through its saved layer chain, and it is not a general
/// value of the owning resource type. `callee` is the layer field
/// `^root(key…)….layer`. A leaf layer is handled by `saved_leaf_type`; only a group
/// entry reaches here.
pub(crate) fn saved_group_entry_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let (root, layers) = saved_layer_chain(callee)?;
    let store = resolve_store_by_root(program, root)?;
    // Only a keyed group (a layer holding members) is a whole-entry record; a keyed
    // leaf is a scalar/identity value already typed through `saved_leaf_type`.
    store
        .resource
        .descend_layers(&layers)
        .filter(|node| matches!(node.kind, marrow_schema::NodeKind::Group))
        .map(|_| MarrowType::GroupEntry {
            resource: resource_type_name(&store.module.name, &store.resource.name),
            layers: layers.iter().map(|layer| (*layer).to_string()).collect(),
        })
}

/// The identity type of a unique-index lookup `^root.uniqueIndex(args)`: the
/// owning store's `Id(^root)`. A unique index stores one store identity at the
/// lookup path, so reading it yields that identity (mirrors the runtime's
/// `eval_index_lookup`). A non-unique index has no single identity in value
/// position, so it is not typed here. `callee` is the `^root.index` field.
fn saved_index_identity_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let marrow_syntax::Expression::Field { base, name, .. } = callee else {
        return None;
    };
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = base.as_ref() else {
        return None;
    };
    let store = resolve_store_by_root(program, root)?;
    let index = store
        .store
        .indexes
        .iter()
        .find(|index| &index.name == name)?;
    index.unique.then(|| identity_type_for_store(store.store))
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
        }
        MarrowType::GroupEntry {
            resource: name,
            layers,
        } => {
            let (resource, module) = resolve_resource_type(program, name)?;
            let mut chain: Vec<&str> = layers.iter().map(String::as_str).collect();
            chain.push(field);
            field_member_type(program, resource, &chain, module)
        }
        MarrowType::Error => error_field_type(field),
        _ => None,
    }
}

fn error_field_type(field: &str) -> Option<MarrowType> {
    let descriptor = marrow_schema::error::field(field)?;
    Some(MarrowType::from_resolved(
        descriptor.ty.clone(),
        TypeNames::default(),
    ))
}

/// The declared type of a group field read at any depth, through keyed layers
/// (`^root(key…).layer(key…)….field`) or unkeyed groups (`^root(key…).name.field`).
fn saved_group_field_type(
    program: &CheckedProgram,
    base: &marrow_syntax::Expression,
    field: &str,
) -> Option<MarrowType> {
    let (root, mut chain) = saved_group_chain(base)?;
    let store = resolve_store_by_root(program, root)?;
    if starts_from_bare_keyed_root(program, base) {
        return None;
    }
    chain.push(field);
    field_member_type(program, store.resource, &chain, &store.module.name)
}

fn singleton_saved_group_field_type(
    program: &CheckedProgram,
    base: &marrow_syntax::Expression,
    field: &str,
) -> Option<MarrowType> {
    let marrow_syntax::Expression::Call { callee, .. } = base else {
        return None;
    };
    let (root, layer) = singleton_saved_layer(callee)?;
    let store = resolve_store_by_root(program, root)?;
    if !store.store.identity_keys.is_empty() {
        return None;
    }
    field_member_type(program, store.resource, &[layer, field], &store.module.name)
}

/// Extract `(root, [member…])` from a group entry, members ordered outermost-first.
/// The innermost base is the keyed record `^root(key…)` or the singleton root `^root`.
pub(crate) fn saved_group_chain(expr: &marrow_syntax::Expression) -> Option<(&str, Vec<&str>)> {
    use marrow_syntax::Expression;
    // A keyed layer entry `….layer(key…)`: a call whose callee is the layer field.
    if let Expression::Call { callee, .. } = expr {
        return saved_layer_chain(callee.as_ref());
    }
    // An unkeyed group hop `….name`: a field off the record or a deeper group.
    let (base, name) = match expr {
        Expression::Field { base, name, .. } | Expression::OptionalField { base, name, .. } => {
            (base, name)
        }
        _ => return None,
    };
    match base.as_ref() {
        // The record base: `^root(key…)` (a call on the saved root) or the
        // singleton root `^root`. This `.name` is the first group member.
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::SavedRoot { name: root, .. } => Some((root, vec![name])),
            // A keyed layer entry `Call{callee:Field}` is a deeper group; recurse.
            Expression::Field { .. } => {
                let (root, mut members) = saved_group_chain(base)?;
                members.push(name);
                Some((root, members))
            }
            _ => None,
        },
        Expression::SavedRoot { name: root, .. } => Some((root, vec![name])),
        // A deeper unkeyed group hop: recurse and append this member.
        Expression::Field { .. } | Expression::OptionalField { .. } => {
            let (root, mut members) = saved_group_chain(base)?;
            members.push(name);
            Some((root, members))
        }
        _ => None,
    }
}

/// The declared leaf type of a keyed-leaf read `^root(key…).layer(key…)…` at any
/// nesting depth. `callee` is the layer field `^root(key…)….layer`.
pub(crate) fn saved_leaf_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let (root, layers) = saved_layer_chain(callee)?;
    let store = resolve_store_by_root(program, root)?;
    leaf_member_type(program, store.resource, &layers, &store.module.name)
}

fn singleton_saved_leaf_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let (root, layer) = singleton_saved_layer(callee)?;
    let store = resolve_store_by_root(program, root)?;
    if !store.store.identity_keys.is_empty() {
        return None;
    }
    leaf_member_type(program, store.resource, &[layer], &store.module.name)
}

fn singleton_saved_layer(callee: &marrow_syntax::Expression) -> Option<(&str, &str)> {
    let marrow_syntax::Expression::Field {
        base, name: layer, ..
    } = callee
    else {
        return None;
    };
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = base.as_ref() else {
        return None;
    };
    Some((root, layer))
}

/// Extract `(root, [layer…])` from a keyed layer accessor `^root.layer`,
/// `^root(key…).layer`, or a nested one `^root.layer(key…)….layer`, with the layer
/// names ordered outermost first. Each `Field` peels one layer; its base is the
/// singleton root, a keyed record, or a deeper layer entry.
pub(crate) fn saved_layer_chain(expr: &marrow_syntax::Expression) -> Option<(&str, Vec<&str>)> {
    use marrow_syntax::Expression;
    let Expression::Field {
        base, name: layer, ..
    } = expr
    else {
        return None;
    };
    if let Expression::SavedRoot { name: root, .. } = base.as_ref() {
        return Some((root, vec![layer]));
    }
    let Expression::Call { callee, .. } = base.as_ref() else {
        return None;
    };
    match callee.as_ref() {
        Expression::SavedRoot { name: root, .. } => Some((root, vec![layer])),
        Expression::Field { .. } => {
            let (root, mut layers) = saved_layer_chain(callee)?;
            layers.push(layer);
            Some((root, layers))
        }
        _ => None,
    }
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

/// The checker type of a keyed-leaf layer read named by its `layers`, outermost first.
fn leaf_member_type(
    program: &CheckedProgram,
    resource: &marrow_schema::ResourceSchema,
    layers: &[&str],
    owning_module: &str,
) -> Option<MarrowType> {
    resource
        .leaf_type(layers)
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
) -> Option<MarrowType> {
    let marrow_syntax::Expression::Name { segments, .. } = callee else {
        return None;
    };
    let [arg] = args else {
        return None;
    };
    if segments.as_slice() == ["count"]
        && arg.name.is_none()
        && is_saved_path_expression(program, &arg.value)
    {
        Some(MarrowType::Primitive(ScalarType::Int))
    } else {
        None
    }
}

pub(crate) fn is_saved_path_expression(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
) -> bool {
    use marrow_syntax::Expression;
    match expr {
        Expression::SavedRoot { name, .. } => resolve_store_by_root(program, name).is_some(),
        Expression::Call { callee, .. } => is_saved_path_callee(program, callee),
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            !starts_from_bare_keyed_root(program, expr) && is_saved_path_expression(program, base)
        }
        _ => false,
    }
}

fn is_saved_path_callee(program: &CheckedProgram, callee: &marrow_syntax::Expression) -> bool {
    use marrow_syntax::Expression;
    match callee {
        Expression::SavedRoot { name, .. } => resolve_store_by_root(program, name).is_some(),
        Expression::Call { .. } => is_saved_path_expression(program, callee),
        Expression::Field { base, name, .. } | Expression::OptionalField { base, name, .. } => {
            match base.as_ref() {
                Expression::SavedRoot { name: root, .. } => resolve_store_by_root(program, root)
                    .is_some_and(|store| {
                        store.store.identity_keys.is_empty()
                            || store.store.indexes.iter().any(|index| &index.name == name)
                    }),
                _ => is_saved_path_expression(program, base),
            }
        }
        _ => false,
    }
}

fn starts_from_bare_keyed_root(program: &CheckedProgram, expr: &marrow_syntax::Expression) -> bool {
    starts_from_bare_saved_root(expr)
        .and_then(|root| resolve_store_by_root(program, root))
        .is_some_and(|store| !store.store.identity_keys.is_empty())
}

fn starts_from_bare_saved_root(expr: &marrow_syntax::Expression) -> Option<&str> {
    use marrow_syntax::Expression;
    match expr {
        Expression::SavedRoot { name, .. } => Some(name),
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            starts_from_bare_saved_root(base)
        }
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::SavedRoot { .. } => None,
            _ => starts_from_bare_saved_root(callee),
        },
        _ => None,
    }
}

enum SavedAccessRejection<'a> {
    GeneratedIndexBranch,
    KeyedRootMemberWithoutIdentity(&'a str),
}

fn saved_access_rejection<'a>(
    program: &CheckedProgram,
    expr: &'a marrow_syntax::Expression,
) -> Option<SavedAccessRejection<'a>> {
    use marrow_syntax::Expression;
    match expr {
        Expression::OptionalField { base, name, .. }
            if saved_root_has_index(program, base, name) =>
        {
            Some(SavedAccessRejection::GeneratedIndexBranch)
        }
        Expression::Field { base, name, .. } | Expression::OptionalField { base, name, .. } => {
            if is_saved_index_branch_path(program, base) {
                return Some(SavedAccessRejection::GeneratedIndexBranch);
            }
            match base.as_ref() {
                Expression::SavedRoot { name: root, .. } => {
                    let store = resolve_store_by_root(program, root)?;
                    if store.store.identity_keys.is_empty()
                        || store
                            .store
                            .indexes
                            .iter()
                            .any(|index| index.name.as_str() == name)
                    {
                        None
                    } else {
                        Some(SavedAccessRejection::KeyedRootMemberWithoutIdentity(root))
                    }
                }
                _ => saved_access_rejection(program, base),
            }
        }
        Expression::Call { callee, .. } => {
            if matches!(callee.as_ref(), Expression::Call { .. })
                && is_saved_index_branch_path(program, callee)
            {
                return Some(SavedAccessRejection::GeneratedIndexBranch);
            }
            match callee.as_ref() {
                Expression::SavedRoot { .. } => None,
                _ => saved_access_rejection(program, callee),
            }
        }
        _ => None,
    }
}

fn saved_root_has_index(
    program: &CheckedProgram,
    base: &marrow_syntax::Expression,
    index_name: &str,
) -> bool {
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = base else {
        return false;
    };
    resolve_store_by_root(program, root).is_some_and(|store| {
        store
            .store
            .indexes
            .iter()
            .any(|index| index.name.as_str() == index_name)
    })
}

/// The `Id(^root)` identity type of the store at saved root `root`, or
/// `Unknown` when `root` names no keyed saved root.
pub(crate) fn record_identity_type(program: &CheckedProgram, root: &str) -> MarrowType {
    match resolve_store_by_root(program, root) {
        Some(store) if !store.store.identity_keys.is_empty() => {
            identity_type_for_store(store.store)
        }
        _ => MarrowType::Unknown,
    }
}

/// The single key type of the child layer a `^root(id…).layer` accessor names, or
/// `Unknown` when the layer is undeclared or not single-keyed. The neighbor of a
/// layer position is one of these keys, so `next`/`prev` over the layer type to it.
pub(crate) fn layer_key_type(
    program: &CheckedProgram,
    layer_field: &marrow_syntax::Expression,
) -> MarrowType {
    let Some((root, layers)) = saved_layer_chain(layer_field) else {
        return MarrowType::Unknown;
    };
    let Some(store) = resolve_store_by_root(program, root) else {
        return MarrowType::Unknown;
    };
    match store
        .resource
        .descend_layers(&layers)
        .map(|node| node.key_params.as_slice())
    {
        Some([key]) => MarrowType::from_resolved(key.ty.clone(), TypeNames::default()),
        _ => MarrowType::Unknown,
    }
}
