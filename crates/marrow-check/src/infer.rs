//! Expression type inference and the saved-path/field type resolution it walks.

use super::*;

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
    let (name, annotation, value_type) = match statement {
        Statement::Const {
            name, ty, value, ..
        } => (
            name,
            ty,
            infer_type(program, value, scope, aliases, file, &mut sink),
        ),
        Statement::Var {
            name, ty, value, ..
        } => {
            let value_type = match value {
                Some(value) => infer_type(program, value, scope, aliases, file, &mut sink),
                None => MarrowType::Unknown,
            };
            (name, ty, value_type)
        }
        _ => return None,
    };
    Some((
        name.clone(),
        binding_type(annotation.as_ref(), value_type, program, aliases, file),
    ))
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
    match expr {
        Expression::Literal { kind, text, span } => {
            check_literal_range(*kind, text, *span, file, diagnostics);
            literal_type(*kind)
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let marrow_syntax::InterpolationPart::Expr(expr) = part {
                    infer_type(program, expr, scope, aliases, file, diagnostics);
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
                return check_is(
                    program,
                    &left_type,
                    right,
                    aliases,
                    *span,
                    file,
                    diagnostics,
                );
            }
            let right_type = infer_type(program, right, scope, aliases, file, diagnostics);
            // `??` only defaults an absent path read, so its left operand must be a
            // path read or `?.` chain — a present non-path value is never absent
            // and has nothing to default. The result is the leaf type of that read.
            if matches!(op, marrow_syntax::BinaryOp::Coalesce) {
                return check_coalesce(left, &left_type, &right_type, *span, file, diagnostics);
            }
            check_binary(*op, &left_type, &right_type, *span, file, diagnostics)
        }
        Expression::Call { callee, args, span } => {
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
            let call_type = check_call(
                program,
                callee,
                args,
                &arg_types,
                aliases,
                *span,
                file,
                diagnostics,
            );
            // A saved access `^root(key…)` or `^root(key…).layer(key…)` carries key
            // arguments the function-call path does not type. Check them against the
            // root's identity keys or the layer's key parameters here, where the
            // saved-path helpers live.
            check_saved_key_args(program, callee, &arg_types, *span, file, diagnostics);
            // A keyed-leaf read `^root(key…).layer(key…)` is call-shaped but is not
            // a function call; it types to the layer's declared leaf type. A whole
            // record read `^root(key…)` types to its resource.
            if matches!(call_type, MarrowType::Unknown) {
                saved_leaf_type(program, callee)
                    .or_else(|| saved_index_identity_type(program, callee))
                    .or_else(|| saved_resource_type(program, callee))
                    .or_else(|| saved_group_entry_type(program, callee))
                    .unwrap_or(MarrowType::Unknown)
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
            let Some(resolved) = resolve_enum_member_path(program, expr, aliases, file) else {
                // Not an enum: a cross-module name or identity path stays unknown.
                return MarrowType::Unknown;
            };
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
                        span: *span,
                    });
                    MarrowType::Unknown
                }
                MemberPathResolution::Found(_) => MarrowType::Enum {
                    module: resolved.module,
                    name: enum_name.clone(),
                },
                // A bare name under several parents cannot pick one in value
                // position either; the full path (`Cat::tiger::paw`) disambiguates.
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
                        span: *span,
                    });
                    MarrowType::Unknown
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
                        span: *span,
                    });
                    MarrowType::Unknown
                }
            }
        }
        // A bare `^root` naming a keyless singleton (`Settings at ^settings`) reads
        // the whole record by its root, so it types to that resource — the same
        // `Resource` a keyed `^books(key…)` whole read yields through its `Call`
        // form. A keyed root used bare names no value (it needs keys), and any other
        // multi-segment name has no known type.
        Expression::SavedRoot { name, .. } => singleton_resource_type(program, name),
        Expression::Name { .. } => MarrowType::Unknown,
    }
}

/// The resource type of a bare `^root` whole read, defined only for a keyless
/// singleton (a saved root with no identity keys): reading it by its root yields the
/// whole record. A keyed root needs keys to address a record, so a bare reference to
/// one names no value here.
fn singleton_resource_type(program: &CheckedProgram, root: &str) -> MarrowType {
    match find_resource_schema(program, root) {
        Some(resource)
            if resource
                .saved_root
                .as_ref()
                .is_some_and(|saved_root| saved_root.identity_keys.is_empty()) =>
        {
            MarrowType::Resource(resource.name.clone())
        }
        _ => MarrowType::Unknown,
    }
}

/// The declared type of a top-level saved field read: `base` is either a keyed
/// record access `^root(key…)` (a call whose callee is the saved root) or — for a
/// keyless singleton resource (`Settings at ^settings`) addressed by its root —
/// the saved root `^root` itself. Group-layer fields and keyed-leaf reads are not
/// resolved here.
pub(crate) fn saved_field_type(
    program: &CheckedProgram,
    base: &marrow_syntax::Expression,
    field: &str,
) -> Option<MarrowType> {
    use marrow_syntax::Expression;
    let root = match base {
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::SavedRoot { name, .. } => name,
            _ => return None,
        },
        Expression::SavedRoot { name, .. } => name,
        _ => return None,
    };
    let resource = find_resource_schema(program, root)?;
    field_member_type(resource, &[field], resource_module(program, &resource.name))
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
    let resource = find_resource_schema(program, root)?;
    Some(MarrowType::Resource(resource.name.clone()))
}

/// The record type of a whole group-entry access `^root(key…).layer(key…)` whose
/// terminal layer is a keyed GROUP (not a leaf): the owning resource. A group entry
/// is a record value — read as a `Value::Resource`, written from one — so it shares
/// the resource type, which gives an `unknown`-typed whole-entry write the same
/// conversion boundary a whole-resource write has (a raw scalar or foreign identity
/// must not silently land in one of the entry's typed fields). `callee` is the layer
/// field `^root(key…)….layer`. A leaf layer is handled by `saved_leaf_type`; only a
/// group entry reaches here.
pub(crate) fn saved_group_entry_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let (root, layers) = saved_layer_chain(callee)?;
    let resource = find_resource_schema(program, root)?;
    // Only a keyed group (a layer holding members) is a whole-entry record; a keyed
    // leaf is a scalar/identity value already typed through `saved_leaf_type`.
    resource
        .descend_layers(&layers)
        .filter(|node| matches!(node.element, marrow_schema::Element::Group))
        .map(|_| MarrowType::Resource(resource.name.clone()))
}

/// The identity type of a unique-index lookup `^root.uniqueIndex(args)`: the
/// owning resource's `Resource::Id`. A unique index stores one resource identity
/// at the lookup path, so reading it yields that identity (mirrors the runtime's
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
    let resource = find_resource_schema(program, root)?;
    let index = resource.indexes.iter().find(|index| &index.name == name)?;
    index
        .unique
        .then(|| MarrowType::Identity(resource.name.clone()))
}

/// The declared type of a field read off a resource-typed value, e.g. `book.title`
/// where `book: Book`. `base_type` must be a known resource type; the field is
/// looked up in that resource's schema.
pub(crate) fn local_field_type(
    program: &CheckedProgram,
    base_type: &MarrowType,
    field: &str,
) -> Option<MarrowType> {
    let MarrowType::Resource(name) = base_type else {
        return None;
    };
    let resource = resolve::resolve_resource_by_name_any(program, name)?;
    field_member_type(resource, &[field], resource_module(program, &resource.name))
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
    let resource = find_resource_schema(program, root)?;
    chain.push(field);
    field_member_type(resource, &chain, resource_module(program, &resource.name))
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
    let Expression::Field { base, name, .. } = expr else {
        return None;
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
        // A deeper unkeyed group `Field`: recurse and append this member.
        Expression::Field { .. } => {
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
    let resource = find_resource_schema(program, root)?;
    leaf_member_type(resource, &layers, resource_module(program, &resource.name))
}

/// Extract `(root, [layer…])` from a keyed layer accessor `^root(key…).layer` or a
/// nested one `^root(key…).layer(key…)….layer`, with the layer names ordered
/// outermost first. Each `Field` peels one layer; its base is either the keyed
/// record `^root(key…)` (a call on a saved root) or a deeper layer entry.
pub(crate) fn saved_layer_chain(expr: &marrow_syntax::Expression) -> Option<(&str, Vec<&str>)> {
    use marrow_syntax::Expression;
    let Expression::Field {
        base, name: layer, ..
    } = expr
    else {
        return None;
    };
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
    resource: &marrow_schema::ResourceSchema,
    chain: &[&str],
    owning_module: &str,
) -> Option<MarrowType> {
    resource
        .field_type(chain)
        .map(|ty| lift_member_type(ty.clone(), owning_module))
}

/// The checker type of a keyed-leaf layer read named by its chain of layer names,
/// outermost first. Resolves through the same shared schema walk as
/// [`field_member_type`], differing only in that the terminal name is a keyed-leaf
/// layer rather than a field.
pub(crate) fn leaf_member_type(
    resource: &marrow_schema::ResourceSchema,
    layers: &[&str],
    owning_module: &str,
) -> Option<MarrowType> {
    resource
        .leaf_type(layers)
        .map(|ty| lift_member_type(ty.clone(), owning_module))
}

/// Lift a saved-member schema [`Type`] to the checker's lattice. A saved member is
/// a scalar, sequence, identity, or — for a bare [`Type::Named`] — an enum, which
/// the saved-field rule guarantees is declared in `owning_module`. So a `Named`
/// member reads as that module's enum, carrying its nominal `{module, name}`
/// identity, rather than collapsing to `Unknown`.
pub(crate) fn lift_member_type(ty: Type, owning_module: &str) -> MarrowType {
    match ty {
        Type::Named(name) => MarrowType::Enum {
            module: owning_module.to_string(),
            name,
        },
        other => MarrowType::from_resolved(other, TypeNames::default()),
    }
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

/// The `Resource::Id` identity type of the resource at saved root `root`, or
/// `Unknown` when `root` names no keyed saved root.
pub(crate) fn record_identity_type(program: &CheckedProgram, root: &str) -> MarrowType {
    match find_resource_schema(program, root) {
        Some(resource) if resource.saved_root.is_some() => {
            MarrowType::Identity(resource.name.clone())
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
    let Some(resource) = find_resource_schema(program, root) else {
        return MarrowType::Unknown;
    };
    match resource
        .descend_layers(&layers)
        .map(|node| node.key_params.as_slice())
    {
        Some([key]) => MarrowType::from_resolved(key.ty.clone(), TypeNames::default()),
        _ => MarrowType::Unknown,
    }
}

/// The type produced by a resource constructor callee, if `segments` name a
/// resource visible to `from_module`: `Book(...)` constructs the resource value
/// (its [`MarrowType::Resource`]), and `Book::Id(...)` constructs its identity
/// ([`MarrowType::Identity`]). The resource *name* is module-scoped (own module
/// first), so two modules can each declare a `Book`. Any other callee returns
/// `None`, so a genuinely unresolved call is still reported.
pub(crate) fn resource_constructor_type(
    program: &CheckedProgram,
    from_module: &str,
    segments: &[String],
) -> Option<MarrowType> {
    match segments {
        [name] => match resolve(program, from_module, segments, ResolvableKind::Resource) {
            Resolution::Found(_) => Some(MarrowType::Resource(name.clone())),
            _ => None,
        },
        [name, id] if id == "Id" => {
            match resolve(
                program,
                from_module,
                segments,
                ResolvableKind::ResourceIdentity,
            ) {
                Resolution::Found(_) => Some(MarrowType::Identity(name.clone())),
                _ => None,
            }
        }
        _ => None,
    }
}
