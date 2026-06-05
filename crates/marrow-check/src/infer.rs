//! Expression type inference and the saved-path/field type resolution it walks.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{MemberPathResolution, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::{Severity, SourceSpan};

use crate::checks::{
    CallCheck, check_binary, check_call, check_coalesce, check_saved_key_args, check_unary,
    is_saved_index_branch_path, key_type_diagnostic, operator_diagnostic,
};
use crate::enums::{
    IsCheck, check_is, enum_schema_in, join_or, resolve_enum_member_path, resolve_type,
};
use crate::program::TypeNames;
use crate::resolve::resolve_store_by_root;
use crate::typerules::{check_literal_range, marrow_type_name, type_compatible};
use crate::{
    CHECK_AMBIGUOUS_MEMBER, CHECK_CATEGORY_NOT_SELECTABLE, CHECK_COLLECTION_UNSUPPORTED,
    CHECK_PRIVATE_ENUM, CHECK_UNKNOWN_ENUM_MEMBER, CHECK_UNRESOLVED_NAME, CheckDiagnostic,
    CheckedProgram, DiagnosticPayload, MarrowType, build_alias_map, expand_module_alias,
    identity_type_for_store, resolve_resource_schema_type, resolve_resource_type,
    resource_type_name,
};

/// Infer an expression's type without recording diagnostics. Resolution runs after
/// the checking pass, which already reported any type errors, so a throwaway sink
/// keeps it from double-reporting.
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
pub(crate) fn binding_type(
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

/// The `(name, type)` a `const`/`var` statement introduces into its block,
/// computed exactly as [`check_statement_types`] computes it: the annotation when
/// written, otherwise the inferred initializer type, resolved against `scope`. Any
/// other statement introduces no block-frame binding and returns `None`. The
/// checker and the editor scope reconstruction share this so a binding's type is
/// derived in one place. Initializer diagnostics belong to the type-check pass, so
/// inference here discards them.
pub(crate) fn local_binding(
    program: &CheckedProgram,
    statement: &marrow_syntax::Statement,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
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
            infer_type(program, value, scope, aliases, file, &mut sink),
        ),
        Statement::Var {
            name,
            keys,
            ty,
            value,
            ..
        } => {
            let value_type = match value {
                Some(value) => infer_type(program, value, scope, aliases, file, &mut sink),
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
/// any operator whose operands are known to be incompatible. Returns
/// [`MarrowType::Unknown`] whenever the type cannot be determined with certainty,
/// so a containing operator never fires on an uncertain operand.
pub(crate) fn infer_type(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::Expression;
    if let Some(rejection) = saved_access_rejection(program, expr) {
        match rejection {
            SavedAccessRejection::GeneratedIndexBranch => diagnostics.push(CheckDiagnostic {
                code: CHECK_COLLECTION_UNSUPPORTED,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: "generated index branches do not expose resource members or chained calls"
                    .to_string(),
                span: expr.span(),
                payload: DiagnosticPayload::None,
            }),
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
            literal_type(*kind)
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let marrow_syntax::InterpolationPart::Expr(expr) = part {
                    let ty = infer_type(program, expr, scope, aliases, file, diagnostics);
                    if matches!(
                        ty,
                        MarrowType::Primitive(ScalarType::Bytes) | MarrowType::Enum { .. }
                    ) {
                        diagnostics.push(operator_diagnostic(
                            file,
                            expr.span(),
                            format!(
                                "interpolation cannot render `{}`; convert it explicitly",
                                marrow_type_name(&ty)
                            ),
                        ));
                    }
                }
            }
            MarrowType::Primitive(ScalarType::Str)
        }
        Expression::Name { segments, span } if segments.len() == 1 => {
            let name = &segments[0];
            lookup_opt(scope, name).unwrap_or_else(|| {
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNRESOLVED_NAME,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
                    message: format!("`{name}` is not defined"),
                    span: *span,
                    payload: DiagnosticPayload::None,
                });
                MarrowType::Unknown
            })
        }
        Expression::Unary { op, operand, span } => {
            let operand = infer_type(program, operand, scope, aliases, file, diagnostics);
            check_unary(*op, &operand, *span, file, diagnostics)
        }
        Expression::Binary {
            op,
            left,
            right,
            span,
        } => {
            let left_type = infer_type(program, left, scope, aliases, file, diagnostics);
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
            let right_type = infer_type(program, right, scope, aliases, file, diagnostics);
            // `??` only defaults an absent path read, so its left operand must be a
            // path read or `?.` chain — a present non-path value is never absent
            // and has nothing to default. The result is the leaf type of that read.
            if matches!(op, marrow_syntax::BinaryOp::Coalesce) {
                return check_coalesce(
                    program,
                    left,
                    &left_type,
                    &right_type,
                    *span,
                    file,
                    diagnostics,
                );
            }
            check_binary(*op, &left_type, &right_type, *span, file, diagnostics)
        }
        Expression::Call {
            callee, args, span, ..
        } => {
            // Visit the callee subtree (it may hold nested calls, e.g. the
            // `^books(id)` inside `^books(id).tags(pos)`) and infer each argument
            // once. A bare single-segment callee names a function, not a value, so
            // it is left to `check_call` to resolve rather than flagged as an
            // unresolved value name. `check_call` validates the call and yields its
            // return type.
            if !is_bare_name(callee) {
                infer_type(program, callee, scope, aliases, file, diagnostics);
            }
            let arg_types: Vec<MarrowType> = args
                .iter()
                .map(|arg| infer_type(program, &arg.value, scope, aliases, file, diagnostics))
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
                aliases,
                span: *span,
                file,
                diagnostics,
            });
            // A saved access `^root(key…)` or `^root(key…).layer(key…)` carries key
            // arguments the function-call path does not type. Check them against the
            // root's identity keys or the layer's key parameters here, where the
            // saved-path helpers live.
            check_saved_key_args(program, callee, args, &arg_types, *span, file, diagnostics);
            // A keyed-leaf read `^root(key…).layer(key…)` is call-shaped but is not
            // a function call; it types to the layer's declared leaf type. A whole
            // record read `^root(key…)` types to its resource.
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
            let base_type = infer_type(program, base, scope, aliases, file, diagnostics);
            // A saved field read resolves to its declared type: a top-level field
            // `^root(key…).field` or a group-layer field `^root(key…).layer(key…).field`.
            // A field off a resource-typed local (`book.title`) resolves through the
            // resource's schema.
            saved_field_type(program, base, name)
                .or_else(|| singleton_saved_group_field_type(program, base, name))
                .or_else(|| saved_group_field_type(program, base, name))
                .or_else(|| local_field_type(program, &base_type, name))
                .unwrap_or(MarrowType::Unknown)
        }
        // A member-path literal `Enum::seg…` (`Status::active`, `Cat::tiger::bengal`,
        // or a qualified `a::b::Status::active`) resolves to the enum's nominal
        // `{module, name}` identity. The enum is the longest visible-enum prefix and
        // the rest is the member path, walked down the enum's tree. In value position
        // the resolved member must be a concrete leaf: a category groups its
        // descendants and is not selectable, and a bare name duplicated under several
        // parents is ambiguous (the full path always disambiguates).
        Expression::Name { segments, span } if segments.len() >= 2 => {
            enum_member_value_type(program, expr, segments, *span, aliases, file, diagnostics)
        }
        // A bare `^root` naming a keyless singleton store reads the whole record
        // by its root, so it types to that resource shape — the same `Resource` a
        // keyed `^books(key…)` whole read yields through its `Call` form. A keyed
        // root used bare names no value (it needs keys), and any other
        // multi-segment name has no known type.
        Expression::SavedRoot { name, .. } => singleton_resource_type(program, name),
        Expression::Name { .. } => MarrowType::Unknown,
    }
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
        diagnostics.push(CheckDiagnostic {
            code: CHECK_PRIVATE_ENUM,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "enum `{private}` is private to its module; mark it `pub` to use it from another module"
            ),
            span,
            payload: DiagnosticPayload::None,
        });
        return MarrowType::Invalid;
    }
    let enum_name = &resolved.enum_name;
    match resolved.member {
        MemberPathResolution::Found(ordinal) if resolved.schema.is_category(ordinal) => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_CATEGORY_NOT_SELECTABLE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "`{}` is a category and cannot be selected; pick a concrete member under it",
                    segments.join("::")
                ),
                span,
                payload: DiagnosticPayload::None,
            });
            MarrowType::Invalid
        }
        MemberPathResolution::Found(_) => MarrowType::Enum {
            module: resolved.module,
            name: enum_name.clone(),
        },
        MemberPathResolution::Ambiguous(paths) => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_AMBIGUOUS_MEMBER,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "`{}` names more than one member of `{enum_name}`; qualify as {}",
                    segments.join("::"),
                    join_or(&paths)
                ),
                span,
                payload: DiagnosticPayload::None,
            });
            MarrowType::Invalid
        }
        MemberPathResolution::NotFound => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNKNOWN_ENUM_MEMBER,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "`{enum_name}` has no member `{}`",
                    segments[segments.len() - 1]
                ),
                span,
                payload: DiagnosticPayload::None,
            });
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

/// The declared type of a top-level saved field read: `base` is either a keyed
/// record access `^root(key…)` (a call whose callee is the saved root) or — for a
/// keyless singleton store addressed by its root —
/// the saved root `^root` itself. Group-layer fields and keyed-leaf reads are not
/// resolved here.
pub(crate) fn saved_field_type(
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

/// The resource type of a whole-record read `^root(key…)`: the call's callee is
/// the saved root, and the value is the owning resource (mirrors the runtime's
/// whole-resource read producing a `Value::Resource`). Lets field access off a
/// saved read stored in a local be typed.
pub(crate) fn saved_resource_type(
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
pub(crate) fn saved_index_identity_type(
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

/// The declared type of a field read off a resource-typed value, e.g. `book.title`
/// where `book: Book`. `base_type` must be a known resource type; the field is
/// looked up in that resource's schema.
pub(crate) fn local_field_type(
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

/// The declared type of a group field read at any nesting depth, reached through
/// keyed layers (`^root(key…).layer(key…)….field`) or unkeyed groups
/// (`^root(key…).name.field`). `base` is the group entry — the part before the
/// leaf field.
pub(crate) fn saved_group_field_type(
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

/// Extract `(root, [member…])` from a group entry — the base of a group field
/// read — peeling each level outermost-last: a keyed layer `.layer(key…)` (a call
/// whose callee is the layer field) or an unkeyed group hop `.name` (a field off a
/// deeper saved path). The innermost base is the keyed record `^root(key…)` or the
/// singleton root `^root`.
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

/// The checker type of a stored field read named by its saved-path chain — the
/// named segments after the identity, outermost first, terminating in a scalar
/// field. Resolves through the shared schema walk and lifts the result to the
/// checker's lattice. `owning_module` is the module that declares the resource, so
/// an enum-typed field reads as that module's enum rather than `Unknown`.
pub(crate) fn field_member_type(
    program: &CheckedProgram,
    resource: &marrow_schema::ResourceSchema,
    chain: &[&str],
    owning_module: &str,
) -> Option<MarrowType> {
    resource
        .field_type(chain)
        .map(|ty| lift_member_type(program, ty.clone(), owning_module))
}

/// The checker type of a keyed-leaf layer read named by its chain of layer names,
/// outermost first. Resolves through the same shared schema walk as
/// [`field_member_type`], differing only in that the terminal name is a keyed-leaf
/// layer rather than a field.
pub(crate) fn leaf_member_type(
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

/// Look up a name's binding, innermost scope frame first; `None` when unbound.
/// A bound name may still be [`MarrowType::Unknown`] (an `unknown`-typed binding
/// or one whose type could not be inferred), which is distinct from being unbound.
pub(crate) fn lookup_opt(scope: &[HashMap<String, MarrowType>], name: &str) -> Option<MarrowType> {
    scope
        .iter()
        .rev()
        .find_map(|frame| frame.get(name))
        .cloned()
}

/// Whether an expression is a bare single-segment name (`foo`, not `a::b` or
/// `^books`). In callee position such a name is a function name resolved by
/// `check_call`, so it is not treated as an unresolved value reference.
pub(crate) fn is_bare_name(expr: &marrow_syntax::Expression) -> bool {
    matches!(expr, marrow_syntax::Expression::Name { segments, .. } if segments.len() == 1)
}

/// The type of a literal by its lexical kind.
pub(crate) fn literal_type(kind: marrow_syntax::LiteralKind) -> MarrowType {
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
        && arg.mode.is_none()
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

pub(crate) fn starts_from_bare_keyed_root(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
) -> bool {
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
