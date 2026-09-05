//! Building the [`TypeRegistry`](super::TypeRegistry) from declarations: reserved
//! templates, the transparent-alias table, nominal intervals, and the
//! declare-then-fill passes for structs, enums, records, and materialized groups.

use super::*;

/// The reserved toolchain generic templates, in fixed order (`Option` then
/// `Result`), registered before any user template. They are ordinary generic enums
/// defined here rather than by user source: the `some`/`none`/`ok`/`err` payload
/// leaves reference the templates' own type parameters, so instantiation
/// monomorphizes them exactly like a user generic enum, and the lowerer recovers
/// their reserved constructor/`try`/spelling behavior from the minting template.
pub(super) fn reserved_templates() -> Vec<TypeTemplate> {
    let param = |name: &str| TypeExpr::Name {
        text: name.to_string(),
        segment_spans: Vec::new(),
        span: SourceSpan::default(),
    };
    let payload = |ty: TypeExpr| TemplatePayload {
        name: "value".to_string(),
        ty,
    };
    vec![
        TypeTemplate {
            name: "Option".to_string(),
            file: None,
            name_span: SourceSpan::default(),
            reserved: Some(Reserved::Option),
            type_params: vec![("T".to_string(), None)],
            body: TemplateBody::Enum(
                vec![
                    TemplateVariant {
                        name: "none".to_string(),
                        payload: Vec::new(),
                    },
                    TemplateVariant {
                        name: "some".to_string(),
                        payload: vec![payload(param("T"))],
                    },
                ]
                .into(),
            ),
        },
        TypeTemplate {
            name: "Result".to_string(),
            file: None,
            name_span: SourceSpan::default(),
            reserved: Some(Reserved::Result),
            type_params: vec![("T".to_string(), None), ("E".to_string(), None)],
            body: TemplateBody::Enum(
                vec![
                    TemplateVariant {
                        name: "ok".to_string(),
                        payload: vec![payload(param("T"))],
                    },
                    TemplateVariant {
                        name: "err".to_string(),
                        payload: vec![payload(param("E"))],
                    },
                ]
                .into(),
            ),
        },
    ]
}

/// Register every generic `struct`/`enum` (one carrying type parameters) as a
/// value-type template, after the reserved toolchain generics. A template mints no
/// concrete image type; a name collision with a scalar, reserved name, alias,
/// nominal, resource, or another declared type is a `check.name_conflict`, and a
/// structurally unadmitted member (a group, key, `required` keyword, optional field,
/// or category/nested enum member) is a `check.unsupported`; a defective template is
/// dropped so no `Name<Args>` use resolves against it.
pub(super) fn register_type_templates(
    registry: &mut TypeRegistry,
    structs: &[(FileRef, FileIdentity, &StructDecl)],
    enums: &[(FileRef, FileIdentity, &EnumDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), DeclareError> {
    let type_param_names =
        |params: &[marrow_syntax::TypeParamDecl]| -> Vec<(String, Option<TypeConstraint>)> {
            params
                .iter()
                .map(|param| {
                    (
                        param.name.clone(),
                        param.constraint.map(TypeConstraint::from_syntax),
                    )
                })
                .collect()
        };
    let name_taken = |registry: &TypeRegistry, name: &str| -> bool {
        ScalarType::from_spelling(name).is_some()
            || registry.aliases.contains_key(name)
            || registry.nominal_by_name(name).is_some()
            || resources.iter().any(|(_, _, r)| r.name == name)
            || structs
                .iter()
                .filter(|(_, _, d)| d.type_params.is_empty())
                .any(|(_, _, d)| d.name == name)
            || enums
                .iter()
                .filter(|(_, _, d)| d.type_params.is_empty())
                .any(|(_, _, d)| d.name == name)
            || registry
                .type_templates
                .iter()
                .any(|template| template.name == name)
    };
    for (at, file, decl) in structs {
        if decl.type_params.is_empty() {
            continue;
        }
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        if name_taken(registry, &decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let mut refusal = None;
        let fields = template_struct_fields(file, decl, diagnostics, declared, &mut refusal);
        if let Some(fields) = fields.as_ref() {
            for (_, ty) in fields {
                if let Some(row) = unknown_template_member(
                    registry,
                    structs,
                    enums,
                    resources,
                    &decl.type_params,
                    ty,
                    file,
                ) {
                    refuse_first(&mut refusal, diagnostics, declared, row);
                }
            }
        }
        let fields = match (fields, refusal) {
            (Some(fields), None) => fields,
            // Every arm that drops the members, and every member type that names
            // nothing declared, reported through the accumulator, so a refused
            // template always carries the cause a use is steered to.
            (_, Some(refusal)) => {
                registry
                    .named
                    .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
            (None, None) => continue,
        };
        registry.named.declare(
            decl.name.clone(),
            DeclarationOccurrence::Accepted(NamedTypeKind::Template),
        )?;
        registry.type_templates.push(TypeTemplate {
            name: decl.name.clone(),
            file: Some(file.clone()),
            name_span: decl.name_span,
            reserved: None,
            type_params: type_param_names(&decl.type_params),
            body: TemplateBody::Struct(fields.into()),
        });
    }
    for (at, file, decl) in enums {
        if decl.type_params.is_empty() {
            continue;
        }
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        if name_taken(registry, &decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let mut refusal = None;
        let variants = template_enum_variants(file, decl, diagnostics, declared, &mut refusal);
        if let Some(variants) = variants.as_ref() {
            for variant in variants {
                for payload in &variant.payload {
                    if let Some(row) = unknown_template_member(
                        registry,
                        structs,
                        enums,
                        resources,
                        &decl.type_params,
                        &payload.ty,
                        file,
                    ) {
                        refuse_first(&mut refusal, diagnostics, declared, row);
                    }
                }
            }
        }
        let variants = match (variants, refusal) {
            (Some(variants), None) => variants,
            // Every arm that drops the members, and every member type that names
            // nothing declared, reported through the accumulator, so a refused
            // template always carries the cause a use is steered to.
            (_, Some(refusal)) => {
                registry
                    .named
                    .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
            (None, None) => continue,
        };
        registry.named.declare(
            decl.name.clone(),
            DeclarationOccurrence::Accepted(NamedTypeKind::Template),
        )?;
        registry.type_templates.push(TypeTemplate {
            name: decl.name.clone(),
            file: Some(file.clone()),
            name_span: decl.name_span,
            reserved: None,
            type_params: type_param_names(&decl.type_params),
            body: TemplateBody::Enum(variants.into()),
        });
    }
    Ok(())
}

/// The row refusing a generic template's member type that names nothing this
/// project declares, or `None` when the spelling is resolvable.
///
/// A template's member types are resolved per instantiation, so without this check
/// a template whose member names an undeclared type is registered whole and its
/// defect is first reported at a *construction* site — blaming the construction for
/// a declaration's error, and never reporting the declaration at all. The
/// declaration set is read raw because templates register before the concrete types
/// reserve, which is also what lets one template name another declared later.
fn unknown_template_member(
    registry: &TypeRegistry,
    structs: &[(FileRef, FileIdentity, &StructDecl)],
    enums: &[(FileRef, FileIdentity, &EnumDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    params: &[marrow_syntax::TypeParamDecl],
    ty: &TypeExpr,
    file: &FileIdentity,
) -> Option<SourceDiagnostic> {
    let declares = |name: &str| {
        params.iter().any(|param| param.name == name)
            || ScalarType::from_spelling(name).is_some()
            || registry.aliases.contains_key(name)
            || registry.nominal_by_name(name).is_some()
            || resources.iter().any(|(_, _, decl)| decl.name == name)
            || structs.iter().any(|(_, _, decl)| decl.name == name)
            || enums.iter().any(|(_, _, decl)| decl.name == name)
            || registry
                .type_templates
                .iter()
                .any(|template| template.name == name)
            || matches!(name, "List" | "Map")
    };
    match ty {
        TypeExpr::Name { text, span, .. } => (!declares(text)).then(|| {
            SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                *span,
                format!("`{text}` does not name a known type"),
            )
        }),
        TypeExpr::Optional { inner, .. } => {
            unknown_template_member(registry, structs, enums, resources, params, inner, file)
        }
        TypeExpr::Apply {
            head,
            head_span,
            args,
            ..
        } => {
            if !declares(head) {
                return Some(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    *head_span,
                    format!("`{head}` does not name a known type"),
                ));
            }
            args.iter().find_map(|arg| {
                unknown_template_member(registry, structs, enums, resources, params, arg, file)
            })
        }
        // An entry identity names a store root, resolved by the durable owner, and a
        // parse-recovery leaf never reaches a `!has_errors` tree.
        TypeExpr::Identity(_) | TypeExpr::Incomplete { .. } => None,
    }
}

/// The named field-type expressions of a generic struct template, or `None` if any
/// member is not the bare `name: Type` form (matching the concrete-struct rule; the
/// field types themselves are resolved per instantiation).
fn template_struct_fields(
    file: &FileIdentity,
    decl: &StructDecl,
    diagnostics: &mut DiagnosticCollector,
    declared: DeclarationSite<'_>,
    refusal: &mut Option<DeclarationRefusalSummary>,
) -> Option<Vec<(String, TypeExpr)>> {
    let mut fields = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        let ResourceMember::Field(field) = member else {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(file, member.span(), "a struct group"),
            );
            ok = false;
            continue;
        };
        if !field.keys.is_empty() {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(file, field.span, "a keyed struct field"),
            );
            ok = false;
            continue;
        }
        if field.required {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    field.span,
                    "the `required` keyword on a struct field (struct fields are always required)",
                ),
            );
            ok = false;
            continue;
        }
        if matches!(field.ty, TypeExpr::Optional { .. }) {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(file, field.ty.span(), "an optional struct field type"),
            );
            ok = false;
            continue;
        }
        fields.push((field.name.clone(), field.ty.clone()));
    }
    ok.then_some(fields)
}

/// The variants (name plus named payload leaves) of a generic enum template, or
/// `None` if any member is a `category` or a nested member (a generic enum is flat;
/// its payload field types are resolved per instantiation).
fn template_enum_variants(
    file: &FileIdentity,
    decl: &EnumDecl,
    diagnostics: &mut DiagnosticCollector,
    declared: DeclarationSite<'_>,
    refusal: &mut Option<DeclarationRefusalSummary>,
) -> Option<Vec<TemplateVariant>> {
    let mut variants = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        if member.category || !member.members.is_empty() {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    member.span,
                    "a category or nested member on a generic enum",
                ),
            );
            ok = false;
            continue;
        }
        variants.push(TemplateVariant {
            name: member.name.clone(),
            payload: member
                .payload
                .iter()
                .map(|field| TemplatePayload {
                    name: field.name.clone(),
                    ty: field.ty.clone(),
                })
                .collect(),
        });
    }
    ok.then_some(variants)
}

/// Resolve the alias declarations to shared global terminal targets. A
/// duplicate alias name or a collision with a resource name is a
/// `check.name_conflict`; an alias on a cyclic chain is a `check.recursion`
/// and does not enter the map.
pub(super) fn build_alias_table(
    named: &mut DeclarationLedger<String, NamedTypeKind>,
    aliases: &[(FileRef, FileIdentity, &AliasDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    structs: &[(FileRef, FileIdentity, &StructDecl)],
    enums: &[(FileRef, FileIdentity, &EnumDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<AliasTable, DeclareError> {
    let mut raw = BTreeMap::new();
    for (at, file, decl) in aliases {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        // A parse error blocks compilation before this runs, so a missing target
        // only means the declaration itself was reported; skip it quietly.
        let Some(ty) = &decl.ty else { continue };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            named.declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        if raw.contains_key(&decl.name) || named.declared(&decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("an alias named `{}` is already declared", decl.name),
            ));
            continue;
        }
        if resources
            .iter()
            .any(|(_, _, resource)| resource.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a resource", decl.name),
            ));
            continue;
        }
        if structs.iter().any(|(_, _, item)| item.name == decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a struct", decl.name),
            ));
            continue;
        }
        if enums.iter().any(|(_, _, item)| item.name == decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as an enum", decl.name),
            ));
            continue;
        }
        #[cfg(test)]
        bump_alias_cycle(|counts| counts.target_visits += 1);
        let target = match ty {
            TypeExpr::Name { text, .. } => Some((text.as_str(), AliasPresence::Bare)),
            TypeExpr::Optional { inner, .. } => match inner.as_ref() {
                TypeExpr::Name { text, .. } => Some((text.as_str(), AliasPresence::Optional)),
                _ => None,
            },
            _ => None,
        };
        let Some((target, presence)) = target else {
            let refusal = refuse_row(
                diagnostics,
                DeclarationSite {
                    span: decl.span,
                    ..declared
                },
                unsupported(
                    file,
                    decl.span,
                    &format!("the target type of alias `{}`", decl.name),
                ),
            );
            named.declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        };
        raw.insert(
            decl.name.clone(),
            AliasInput {
                at: *at,
                file,
                decl,
                target,
                presence,
            },
        );
    }

    AliasTable::normalize(named, raw, diagnostics)
}

/// Validate global targets after concrete declarations have their fill verdicts.
/// An unknown target is `check.type`; a refused declaration retains its cause.
pub(super) fn validate_alias_targets(
    registry: &mut TypeRegistry,
    aliases: &[(FileRef, FileIdentity, &AliasDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), DeclareError> {
    let mut refused: Vec<String> = Vec::new();
    for (at, file, decl) in aliases {
        let Some(target) = registry.aliases.get(&decl.name) else {
            continue; // duplicate or cyclic: already reported
        };
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.span,
        };
        let text = target.name;
        let refusal = if ScalarType::from_spelling(text).is_none()
            && registry.by_name(text).is_none()
            && registry.nominal_by_name(text).is_none()
            && registry.struct_by_name(text).is_none()
            && registry.enum_by_name(text).is_none()
        {
            Some(match registry.named.lookup(text)? {
                Binding::Refused(_, summary) => refuse_row(
                    diagnostics,
                    declared,
                    declaration_refused(file, decl.span, summary),
                ),
                Binding::Accepted(_) | Binding::Absent => refuse(
                    diagnostics,
                    declared,
                    Code::CheckType.as_str(),
                    format!("alias `{}` does not name a known type: `{text}`", decl.name),
                ),
            })
        } else {
            None
        };
        let occurrence = match refusal {
            Some(refusal) => {
                refused.push(decl.name.clone());
                DeclarationOccurrence::Refused(refusal)
            }
            None => DeclarationOccurrence::Accepted(NamedTypeKind::Alias),
        };
        registry.named.declare(decl.name.clone(), occurrence)?;
    }
    // Uses of a refused alias reach its own ledger cause, preserving the name
    // the annotation actually wrote rather than blaming the terminal spelling.
    for name in refused {
        registry.aliases.remove(&name);
    }
    Ok(())
}

/// Resolve the nominal type declarations against the aliases already installed
/// in `registry`. A name collision with an alias, resource, or earlier nominal
/// is a `check.name_conflict`; a base that does not denote `int` is a
/// `check.unsupported`; a non-literal, stepped, or empty interval is a
/// `check.type`; the capability list must draw from the closed set without
/// repeats. A declaration with a defect is dropped whole rather than admitted
/// half-checked.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_nominals(
    registry: &mut TypeRegistry,
    nominals: &[(FileRef, FileIdentity, &NominalDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    structs: &[(FileRef, FileIdentity, &StructDecl)],
    enums: &[(FileRef, FileIdentity, &EnumDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<NominalInfo>, BuildError> {
    let mut built: Vec<NominalInfo> = Vec::new();
    for (at, file, decl) in nominals {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        // A parse error blocks compilation before this runs; a missing piece
        // only means the declaration itself was reported, so skip it quietly.
        let (Some(base), Some(interval)) = (&decl.base, &decl.interval) else {
            continue;
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        // Scalar spellings are keywords the parser already rejects as names;
        // owning them here keeps the conflict predicate self-contained. A nominal
        // this pass already refused holds its name too, so the repeat conflicts
        // whichever of the two the compiler could admit.
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || resources
                .iter()
                .any(|(_, _, resource)| resource.name == decl.name)
            || structs.iter().any(|(_, _, item)| item.name == decl.name)
            || enums.iter().any(|(_, _, item)| item.name == decl.name)
            || registry.named.declared(&decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let refused = match registry.scalar_annotation(base) {
            Ok(ScalarType::Int) => None,
            Ok(other) => Some(refuse_row(
                diagnostics,
                declared,
                unsupported(
                    file,
                    base.span(),
                    &format!("a nominal type over `{}`", other.spelling()),
                ),
            )),
            Err(ResolveError::Refusal(refusal)) => Some(refuse_row(
                diagnostics,
                declared,
                registry.scalar_refusal_row(
                    refusal,
                    file,
                    base.span(),
                    "this nominal base type",
                )?,
            )),
            Err(ResolveError::Invariant(invariant)) => return Err(invariant.into()),
        };
        if let Some(refusal) = refused {
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        let interval = match nominal_interval(file, interval) {
            Ok(bounds) => Ok(bounds),
            Err(row) => Err(refuse_row(diagnostics, declared, *row)),
        };
        let (lo, hi) = match interval {
            Ok(bounds) => bounds,
            Err(refusal) => {
                registry
                    .named
                    .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
        };
        let supports = match support_set(file, decl) {
            Ok(supports) => supports,
            Err(row) => {
                let refusal = refuse_row(diagnostics, declared, *row);
                registry
                    .named
                    .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
        };
        registry.named.declare(
            decl.name.clone(),
            DeclarationOccurrence::Accepted(NamedTypeKind::Nominal),
        )?;
        built.push(NominalInfo {
            name: decl.name.clone(),
            lo,
            hi,
            supports,
        });
    }
    Ok(built)
}

/// Evaluate a nominal `in` range to its inclusive `[lo, hi]` bounds. The range
/// follows the language's range operators — `lo..hi` excludes the end, `lo..=hi`
/// includes it — with int-literal bounds (a leading `-` allowed), no step, and
/// at least one admitted value.
/// The interval's inclusive bounds, or the row that refuses it. The row is
/// returned rather than pushed so the caller can retain it as the declaration's
/// cause in the same statement that reports it.
fn nominal_interval(
    file: &FileIdentity,
    interval: &Expression,
) -> Result<(i64, i64), Box<SourceDiagnostic>> {
    let error = |span, message: &str| {
        Err(Box::new(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            file,
            span,
            message.to_string(),
        )))
    };
    let Some(range) = range_expr(interval) else {
        return error(
            interval.span(),
            "a nominal interval is a range of int literals, such as `0..150`",
        );
    };
    if range.step.is_some() {
        return error(range.span, "a nominal interval takes no step");
    }
    let (Some(start), Some(end)) = (range.start, range.end) else {
        return error(range.span, "a nominal interval needs both bounds");
    };
    let (Some(lo), Some(end_value)) = (literal_int(start), literal_int(end)) else {
        return error(range.span, "a nominal interval's bounds are int literals");
    };
    // Normalize the end-exclusive spelling to the inclusive upper bound. A
    // literal never spells `i64::MIN`, so the exclusive form always has a
    // representable predecessor; the checked form keeps that self-evident.
    let hi = if range.inclusive_end {
        Some(end_value)
    } else {
        end_value.checked_sub(1)
    };
    match hi {
        Some(hi) if lo <= hi => Ok((lo, hi)),
        _ => error(range.span, "this interval admits no values"),
    }
}

/// The value of an int literal, or a negated int literal, or `None`.
fn literal_int(expr: &Expression) -> Option<i64> {
    match expr {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => crate::lower::parse_int(text),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => match &**operand {
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } => crate::lower::parse_int(text).and_then(i64::checked_neg),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve a declaration's `supports` spellings against the closed capability
/// set, rejecting an unknown or repeated capability.
fn support_set(
    file: &FileIdentity,
    decl: &NominalDecl,
) -> Result<SupportSet, Box<SourceDiagnostic>> {
    let mut supports = SupportSet::default();
    for spelling in &decl.supports {
        let flag = match spelling.name.as_str() {
            "add" => &mut supports.add,
            "subtract" => &mut supports.subtract,
            "step" => &mut supports.step,
            "scale" => &mut supports.scale,
            other => {
                return Err(Box::new(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    spelling.span,
                    format!(
                        "unknown capability `{other}`; the capabilities are add, subtract, step, scale"
                    ),
                )));
            }
        };
        if *flag {
            return Err(Box::new(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                spelling.span,
                format!("capability `{}` is repeated", spelling.name),
            )));
        }
        *flag = true;
    }
    Ok(supports)
}

/// One struct reserved in pass one: the file it was declared in, its declaration,
/// and the image record index it will fill in pass two.
pub(super) struct ReservedStruct<'a> {
    pub(super) file: FileIdentity,
    pub(super) at: FileRef,
    pub(super) decl: &'a StructDecl,
    pub(super) type_id: TypeId,
}

/// Pass one for the dense struct types: reserve each admitted struct's image
/// [`RecordTypeDef`] index (empty for now) and register its name, so pass two may
/// resolve a field that names any other struct or enum. A name collision with a
/// scalar, alias, nominal, resource, or earlier struct is a `check.name_conflict`;
/// a colliding or reserved-name struct is dropped and never reserved.
pub(super) fn declare_structs<'a>(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    structs: &'a [(FileRef, FileIdentity, &StructDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<ReservedStruct<'a>>, DeclareError> {
    let mut reserved: Vec<ReservedStruct<'a>> = Vec::new();
    for (at, file, decl) in structs {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, _, resource)| resource.name == decl.name)
            || registry.struct_by_name(&decl.name).is_some()
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let name_id = draft.intern_string(&decl.name)?;
        let type_id = draft.reserve_record_type(name_id)?;
        registry
            .coordinates
            .declare(type_id, *at, file, decl.name_span);
        registry.structs.push(StructInfo {
            type_id,
            name: decl.name.clone(),
            fields: Vec::new(),
            verdict: DeclarationVerdict::Accepted,
        });
        reserved.push(ReservedStruct {
            file: file.clone(),
            at: *at,
            decl,
            type_id,
        });
    }
    Ok(reserved)
}

/// Pass two for the dense struct types: resolve each reserved struct's fields
/// against the full registry and fill both the registry info and the image record.
/// A struct field is the bare `name: Type` form over any value type — a scalar,
/// nominal, another struct, or a closed enum (`Option`/`Result`/a user `enum`);
/// a group, keyed field, the `required` keyword, an optional type, or an unknown
/// type is `check.unsupported`. A declaration with a member defect is refused whole
/// (its reserved image record stays empty and its name leaves the accepted set) so
/// a later construction or match cannot resolve against a broken struct. Its
/// reserved row stays in place carrying [`DeclarationVerdict::Refused`], so a
/// reference an earlier fill pass minted against the reservation addresses a
/// refused declaration rather than dangling.
pub(super) fn fill_structs(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    reserved: &[ReservedStruct<'_>],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    for item in reserved {
        let declared = DeclarationSite {
            name: &item.decl.name,
            file: &item.file,
            at: item.at,
            span: item.decl.name_span,
        };
        let occurrence = struct_fields(draft, registry, declared, item.decl, diagnostics)?
            .map_accepted(|(fields, field_defs)| {
                #[expect(
                    clippy::expect_used,
                    reason = "reserve-then-fill law: the row was reserved in this batch and fills exactly once"
                )]
                draft.set_record_fields(item.type_id, field_defs)
                    .expect("a reserved row fills once");
                if let Some(info) = registry
                    .structs
                    .iter_mut()
                    .find(|info| info.type_id == item.type_id)
                {
                    info.fields = fields;
                }
                NamedTypeKind::Struct
            });
        if matches!(occurrence, DeclarationOccurrence::Refused(_))
            && let Some(info) = registry
                .structs
                .iter_mut()
                .find(|info| info.type_id == item.type_id)
        {
            info.verdict = DeclarationVerdict::Refused;
        }
        registry.named.declare(item.decl.name.clone(), occurrence)?;
    }
    Ok(())
}

/// Resolve a struct's members to its required value fields and their image
/// definitions, or `None` if any member is not the bare `name: Type` form over a
/// value type.
type ResolvedStructFields = (Vec<FieldInfo>, Vec<FieldDef>);

fn struct_fields(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    declared: DeclarationSite<'_>,
    decl: &StructDecl,
    diagnostics: &mut DiagnosticCollector,
) -> Result<DeclarationOccurrence<ResolvedStructFields>, GenericInvariant> {
    let file = declared.file;
    let mut fields = Vec::new();
    let mut field_defs = Vec::new();
    let mut refusal = None;
    let mut limited = false;
    for member in &decl.members {
        let ResourceMember::Field(field) = member else {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(file, member.span(), "a struct group"),
            );
            continue;
        };
        if !field.keys.is_empty() {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(file, field.span, "a keyed struct field"),
            );
            continue;
        }
        if field.required {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    field.span,
                    "the `required` keyword on a struct field (struct fields are always required)",
                ),
            );
            continue;
        }
        let field_ty = match registry.resolve_garg(
            draft,
            &field.ty,
            MintSite {
                file,
                span: field.ty.span(),
            },
        ) {
            Ok(ty) => ty,
            Err(ResolveError::Refusal(refused)) => {
                match registry.member_refusal_row(
                    refused,
                    file,
                    field.ty.span(),
                    if registry.optional_annotation(&field.ty) {
                        "an optional struct field type"
                    } else {
                        "this struct field type"
                    },
                )? {
                    Some(row) => refuse_first(&mut refusal, diagnostics, declared, row),
                    None => limited = true,
                }
                continue;
            }
            Err(ResolveError::Invariant(invariant)) => return Err(invariant),
        };
        let field_name_id = draft.intern_string(&field.name)?;
        field_defs.push(FieldDef {
            name: field_name_id,
            ty: field_ty.image(),
            required: true,
        });
        fields.push(FieldInfo {
            name: field.name.clone(),
            ty: field_ty,
            required: true,
        });
    }
    Ok(match (refusal, limited) {
        (Some(refusal), _) => DeclarationOccurrence::Refused(refusal),
        // The shared instantiation limit reports once, at the monomorphization
        // owner; this declaration is refused for a cause that pass owns.
        (None, true) => DeclarationOccurrence::Refused(refuse_covered(
            declared,
            Code::CheckInstantiationLimit.as_str(),
        )),
        (None, false) => DeclarationOccurrence::Accepted((fields, field_defs)),
    })
}

/// One enum reserved in pass one: the file it was declared in, its declaration,
/// and the image ENUMS index it will fill in pass two.
pub(super) struct ReservedEnum<'a> {
    pub(super) file: FileIdentity,
    pub(super) at: FileRef,
    pub(super) decl: &'a EnumDecl,
    pub(super) enum_id: EnumId,
}

/// Pass one for the closed flat enum types: reserve each admitted enum's image
/// [`EnumTypeDef`] index (empty for now) and register its name. A name collision
/// with a scalar, alias, nominal, resource, struct, or earlier enum is a
/// `check.name_conflict`; a colliding or reserved-name enum is dropped and never
/// reserved. Reserving user enums before pass two resolves any field types keeps
/// their image indices ahead of the `Option`/`Result` instantiations minted lazily
/// during field resolution.
pub(super) fn declare_enums<'a>(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    enums: &'a [(FileRef, FileIdentity, &EnumDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<ReservedEnum<'a>>, DeclareError> {
    let mut reserved: Vec<ReservedEnum<'a>> = Vec::new();
    for (at, file, decl) in enums {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || registry.struct_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, _, resource)| resource.name == decl.name)
            || registry.enum_by_name(&decl.name).is_some()
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        if decl.members.len() > marrow_image::bounds::MAX_VARIANTS {
            let refusal = refuse(
                diagnostics,
                declared,
                Code::CheckResourceLimit.as_str(),
                format!(
                    "an enum declares {} members; the fixed limit is {}",
                    decl.members.len(),
                    marrow_image::bounds::MAX_VARIANTS
                ),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        let name_id = draft.intern_string(&decl.name)?;
        let enum_id = draft.reserve_enum_type(name_id)?;
        registry.enums.push(EnumInfo {
            enum_id,
            name: decl.name.clone(),
            variants: Vec::new(),
            verdict: DeclarationVerdict::Accepted,
        });
        reserved.push(ReservedEnum {
            file: file.clone(),
            at: *at,
            decl,
            enum_id,
        });
    }
    Ok(reserved)
}

/// Pass two for the closed flat enum types: resolve each reserved enum's variants
/// and fill both the registry info and the image ENUMS entry. Hierarchy is
/// deferred: a `category` member or a member with nested members is
/// `check.unsupported`. A member's payload is the dense `name: Type` form over bare
/// scalars; an optional or non-scalar payload type is `check.unsupported`. A
/// declaration with a defect is refused whole (its reserved image entry stays empty
/// and its name leaves the accepted set) so a later match cannot resolve against a
/// broken enum. Its reserved row stays in place carrying
/// [`DeclarationVerdict::Refused`], for the reason given at [`fill_structs`].
pub(super) fn fill_enums(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    reserved: &[ReservedEnum<'_>],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    for item in reserved {
        let declared = DeclarationSite {
            name: &item.decl.name,
            file: &item.file,
            at: item.at,
            span: item.decl.name_span,
        };
        let occurrence = enum_variants(draft, registry, declared, item.decl, diagnostics)?
            .map_accepted(|(variants, variant_defs)| {
                #[expect(
                    clippy::expect_used,
                    reason = "reserve-then-fill law: the row was reserved in this batch and fills exactly once"
                )]
                draft.set_enum_variants(item.enum_id, variant_defs)
                    .expect("a reserved row fills once");
                if let Some(info) = registry
                    .enums
                    .iter_mut()
                    .find(|info| info.enum_id == item.enum_id)
                {
                    info.variants = variants;
                }
                NamedTypeKind::Enum
            });
        if matches!(occurrence, DeclarationOccurrence::Refused(_))
            && let Some(info) = registry
                .enums
                .iter_mut()
                .find(|info| info.enum_id == item.enum_id)
        {
            info.verdict = DeclarationVerdict::Refused;
        }
        registry.named.declare(item.decl.name.clone(), occurrence)?;
    }
    Ok(())
}

/// One enum's selectable variants and the image definitions that carry them.
type EnumVariants = (Vec<VariantInfo>, Vec<VariantDef>);

/// One enum member's payload fields, as info and as the scalars the image holds.
type EnumPayload = (Vec<EnumPayloadInfo>, Vec<ScalarType>);

/// Resolve an enum's members to its selectable variants and their image
/// definitions, or `None` if any member is unsupported. On the flat line every
/// member is a leaf: a `category` member or one with nested members is deferred.
fn enum_variants(
    draft: &mut DraftTxn<'_>,
    registry: &TypeRegistry,
    declared: DeclarationSite<'_>,
    decl: &EnumDecl,
    diagnostics: &mut DiagnosticCollector,
) -> Result<DeclarationOccurrence<EnumVariants>, BuildError> {
    let file = declared.file;
    let mut variants = Vec::new();
    let mut variant_defs = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut refusal = None;
    for member in &decl.members {
        if member.category {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    member.span,
                    "a `category` enum member (hierarchical enums are deferred)",
                ),
            );
            continue;
        }
        if !member.members.is_empty() {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    member.span,
                    "a nested enum member (hierarchical enums are deferred)",
                ),
            );
            continue;
        }
        if seen.contains(&member.name) {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                SourceDiagnostic::at(
                    Code::CheckNameConflict.as_str(),
                    file,
                    member.name_span,
                    format!("enum member `{}` is already declared", member.name),
                ),
            );
            continue;
        }
        seen.push(member.name.clone());
        let Some((payload, payload_scalars)) =
            enum_payload(registry, declared, member, diagnostics, &mut refusal)?
        else {
            continue;
        };
        let name_id = draft.intern_string(&member.name)?;
        variant_defs.push(VariantDef {
            name: name_id,
            category: false,
            payload: payload_scalars
                .iter()
                .map(|scalar| ImageType::scalar(scalar.image()))
                .collect(),
        });
        variants.push(VariantInfo {
            name: member.name.clone(),
            payload,
        });
    }
    Ok(match refusal {
        Some(refusal) => DeclarationOccurrence::Refused(refusal),
        None => DeclarationOccurrence::Accepted((variants, variant_defs)),
    })
}

/// Resolve one member's payload fields to their scalars and info, or `None` when
/// a field is not the bare `name: scalar` form. A defect refuses the whole
/// declaration, so it is recorded in the enum's shared refusal rather than
/// returned separately.
fn enum_payload(
    registry: &TypeRegistry,
    declared: DeclarationSite<'_>,
    member: &EnumMember,
    diagnostics: &mut DiagnosticCollector,
    refusal: &mut Option<DeclarationRefusalSummary>,
) -> Result<Option<EnumPayload>, BuildError> {
    let file = declared.file;
    if member.payload.len() > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
        refuse_first(
            refusal,
            diagnostics,
            declared,
            SourceDiagnostic::at(
                Code::CheckResourceLimit.as_str(),
                file,
                member.span,
                format!(
                    "an enum member carries {} payload fields; the fixed limit is {}",
                    member.payload.len(),
                    marrow_image::bounds::MAX_PAYLOAD_FIELDS
                ),
            ),
        );
        return Ok(None);
    }
    let mut payload = Vec::new();
    let mut scalars = Vec::new();
    let mut ok = true;
    for field in &member.payload {
        let scalar = match registry.scalar_annotation(&field.ty) {
            Ok(scalar) => scalar,
            Err(ResolveError::Refusal(refused)) => {
                let subject = if registry.optional_annotation(&field.ty) {
                    "an optional enum payload field type"
                } else {
                    "this enum payload field type"
                };
                let row = registry.scalar_refusal_row(refused, file, field.ty.span(), subject)?;
                refuse_first(refusal, diagnostics, declared, row);
                ok = false;
                continue;
            }
            Err(ResolveError::Invariant(invariant)) => return Err(invariant.into()),
        };
        payload.push(EnumPayloadInfo {
            name: field.name.clone(),
            scalar,
        });
        scalars.push(scalar);
    }
    Ok(ok.then_some((payload, scalars)))
}

/// Pass one for the admitted record types: reserve each resource's image
/// [`RecordTypeDef`] index (empty for now, ahead of the structs) and register its
/// name, returning the surviving resource declarations for pass two in the same
/// order as [`TypeRegistry::records`]. A reserved resource name, or a name a prior
/// resource already declared, drops that resource with a precise diagnostic; the
/// first declaration of a name stands. The durable graph still admits one store
/// this line, so a second resource is a value record type, never a second root.
pub(super) fn declare_records<'a>(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    resources: &'a [(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<(FileRef, FileIdentity, &'a ResourceDecl)>, DeclareError> {
    let mut survivors = Vec::new();
    for (ordinal, (at, file, resource)) in resources.iter().enumerate() {
        let declared = DeclarationSite {
            name: &resource.name,
            file,
            at: *at,
            span: resource.name_span,
        };
        if is_reserved_type_name(&resource.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, resource.name_span, &resource.name),
            );
            registry.named.declare(
                resource.name.clone(),
                DeclarationOccurrence::Refused(refusal),
            )?;
            continue;
        }
        // Two resources of the same name have no unambiguous record identity, so a
        // repeat is a precise typed rejection and the first declaration stands.
        if registry
            .records
            .iter()
            .any(|info| info.name == resource.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                resource.name_span,
                format!("`{}` is already declared as a resource", resource.name),
            ));
            continue;
        }
        let name_id = draft.intern_string(&resource.name)?;
        let type_id = draft.reserve_record_type(name_id)?;
        registry
            .coordinates
            .declare(type_id, *at, file, resource.name_span);
        // The ordinal is admitted with the record, never separately: the durable build
        // reads index `i` of one against index `i` of the other.
        registry.records.admit(
            RecordInfo {
                type_id,
                name: resource.name.clone(),
                fields: Vec::new(),
                groups: Vec::new(),
            },
            ordinal,
        );
        registry.named.declare(
            resource.name.clone(),
            DeclarationOccurrence::Accepted(NamedTypeKind::Resource),
        )?;
        survivors.push((*at, file.clone(), *resource));
    }
    Ok(survivors)
}

/// Pass two for the record types: fill each reserved record from its surviving
/// declaration, in the reserved order.
pub(super) fn fill_records(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    record_decls: &[(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    // The survivors are in the same order as the reserved records, so record `index`
    // is the one this declaration reserved.
    for (index, (at, file, resource)) in record_decls.iter().enumerate() {
        let declared = DeclarationSite {
            name: &resource.name,
            file,
            at: *at,
            span: resource.name_span,
        };
        fill_record(draft, registry, index, declared, resource, diagnostics)?;
    }
    Ok(())
}

/// Fill one reserved record (`registry.records[index]`) from its resource
/// declaration: declare each member into the registry's member ledger and fill both
/// the registry info and the image record from what the ledger accepted. A resource
/// field is a scalar, nominal scalar, dense struct, or closed enum value
/// (`Option`/`Result`/a user `enum`). A collection, keyed field, or unknown spelling
/// is not admitted; an unkeyed group is materialized separately below.
///
/// A refused member is `check.unsupported` at its own span and only that member
/// leaves the accepted set — the record keeps its other members. The refusal stays
/// in the ledger, so a later use of that member is steered to the cause rather than
/// told the record has no such field.
fn fill_record(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    index: usize,
    declared: DeclarationSite<'_>,
    resource: &ResourceDecl,
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    let file = declared.file;
    let type_id = registry.records[index].type_id;
    let mut groups = Vec::new();
    let mut group_slot_defs = Vec::new();
    for member in &resource.members {
        match member {
            ResourceMember::Field(field) => {
                let at = DeclarationSite {
                    name: &field.name,
                    file,
                    at: declared.at,
                    span: field.span,
                };
                let occurrence = if registry
                    .members
                    .declared(&MemberKey::field(&resource.name, &field.name))
                {
                    // Two members of one name have no unambiguous slot in the
                    // record; the first declaration stands and the repeat is a
                    // precise rejection rather than a silently dropped member.
                    DeclarationOccurrence::Refused(refuse_row(
                        diagnostics,
                        at,
                        member_conflict(file, field.span, &resource.name, &field.name),
                    ))
                } else if field.keys.is_empty() {
                    resource_member(draft, registry, at, field, "this field type", diagnostics)?
                } else {
                    // A keyed scalar leaf (`tags(pos: int): string`) is a keyed
                    // positional layer, not yet part of the beta durable graph. It is
                    // refused so the shape is a precise rejection, not a silent drop.
                    DeclarationOccurrence::Refused(refuse_row(
                        diagnostics,
                        at,
                        unsupported(file, field.span, "a keyed field"),
                    ))
                };
                registry
                    .members
                    .declare(MemberKey::field(&resource.name, &field.name), occurrence)?;
            }
            ResourceMember::Group(group) if group.keys.is_empty() => {
                // An unkeyed `group` is a nested sub-record value: its scalar/enum
                // leaves become a group record type, and the containing value gains one
                // required slot holding that record. Its durable identity is owned
                // separately by `durable.rs`; this is the materialized-value side only.
                let (leaf_fields, leaf_defs) = build_group_leaves(
                    draft,
                    registry,
                    &resource.name,
                    group,
                    declared,
                    diagnostics,
                )?;
                let anchor = format!("{}.{}", resource.name, group.name);
                let group_name_id = draft.intern_string(&anchor)?;
                let group_type_id = draft.add_record_type(RecordTypeDef {
                    name: group_name_id,
                    fields: leaf_defs,
                })?;
                group_slot_defs.push(FieldDef {
                    name: draft.intern_string(&group.name)?,
                    ty: ImageType::Record {
                        idx: group_type_id,
                        optional: false,
                    },
                    required: true,
                });
                groups.push(GroupInfo {
                    name: group.name.clone(),
                    type_id: group_type_id,
                    fields: leaf_fields,
                });
            }
            ResourceMember::Group(_) => {
                // A keyed `branch` (a `group` with key parameters) is a durable-graph
                // member, resolved by `durable.rs`; it is an addressed collection, not
                // part of the materialized value.
            }
        }
    }
    // The ledger is the authority for which members survived and in what order, so
    // the record's fields and the image slots are read out of it rather than
    // accumulated beside it.
    let fields = registry.accepted_members(&resource.name);
    let mut field_defs: Vec<FieldDef> = fields
        .iter()
        .map(|field| {
            Ok(FieldDef {
                name: draft.intern_string(&field.name)?,
                ty: field.ty.image(),
                required: field.required,
            })
        })
        .collect::<Result<_, BuildError>>()?;
    // The record is group-inclusive: its top-level field slots followed by one
    // group-record slot per unkeyed group, in declaration order. The verifier ties the
    // field slots to the durable member tree's fields and each trailing group slot to a
    // `Group` member, so this one record type serves both the durable graph and the
    // storeless value model.
    field_defs.extend(group_slot_defs);
    #[expect(
        clippy::expect_used,
        reason = "reserve-then-fill law: the row was reserved in this batch and fills exactly once"
    )]
    draft
        .set_record_fields(type_id, field_defs)
        .expect("a reserved row fills once");
    let info = registry.records.at_mut(index);
    info.fields = fields;
    info.groups = groups;
    Ok(())
}

/// The row rejecting a second member of one name in `owner`, which has no
/// unambiguous slot in the record the owner materializes.
fn member_conflict(
    file: &FileIdentity,
    span: SourceSpan,
    owner: &str,
    member: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckNameConflict.as_str(),
        file,
        span,
        format!("`{owner}` already declares a member `{member}`"),
    )
}

/// Resolve one resource member's declared type to the value it binds, or to the
/// refusal the member ledger retains.
///
/// A resource member is a value drawn from the closed acyclic durable value set: a
/// scalar, a nominal scalar, a dense struct, or a closed enum (`Option`/`Result`/a
/// user `enum`). A collection is not a durable member value; an abstract parameter
/// never reaches a concrete record.
fn resource_member(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    at: DeclarationSite<'_>,
    field: &FieldDecl,
    subject: &str,
    diagnostics: &mut DiagnosticCollector,
) -> Result<DeclarationOccurrence<FieldInfo>, GenericInvariant> {
    let file = at.file;
    Ok(
        match registry.resolve_garg(
            draft,
            &field.ty,
            MintSite {
                file,
                span: field.ty.span(),
            },
        ) {
            Ok(ty @ (GArg::Scalar(_) | GArg::Nominal(_) | GArg::Struct(_) | GArg::Enum(_))) => {
                DeclarationOccurrence::Accepted(FieldInfo {
                    name: field.name.clone(),
                    ty,
                    required: field.required,
                })
            }
            // A member type that resolves but is outside the durable value set is a
            // genuine subset gap; one that names a refused declaration is steered to
            // that declaration's own cause.
            Ok(_) => DeclarationOccurrence::Refused(refuse_row(
                diagnostics,
                at,
                unsupported(file, field.ty.span(), subject),
            )),
            Err(ResolveError::Refusal(refused)) => {
                match registry.member_refusal_row(refused, file, field.ty.span(), subject)? {
                    Some(row) => DeclarationOccurrence::Refused(refuse_row(diagnostics, at, row)),
                    // The shared instantiation limit reports once, at the
                    // monomorphization owner; this member is refused for a cause that
                    // pass owns.
                    None => DeclarationOccurrence::Refused(refuse_covered(
                        at,
                        Code::CheckInstantiationLimit.as_str(),
                    )),
                }
            }
            Err(ResolveError::Invariant(invariant)) => return Err(invariant),
        },
    )
}

/// The direct scalar/enum leaves of an unkeyed group, in declaration order,
/// returning both the registry field infos and the image field defs. A keyed leaf,
/// a nested group or keyed branch inside the group, or a non-value leaf type is a
/// precise `check.unsupported` that refuses only that leaf. Nested groups and
/// group-scoped branches are deferred; refusing them keeps them from silently
/// dropping, and keeps the leaf name answerable at its uses.
fn build_group_leaves(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    record: &str,
    group: &GroupDecl,
    declared: DeclarationSite<'_>,
    diagnostics: &mut DiagnosticCollector,
) -> Result<(Vec<FieldInfo>, Vec<FieldDef>), BuildError> {
    let file = declared.file;
    let anchor = format!("{record}.{}", group.name);
    for member in &group.members {
        let field = match member {
            ResourceMember::Field(field) => field,
            ResourceMember::Group(inner) => {
                let at = DeclarationSite {
                    name: &inner.name,
                    file,
                    at: declared.at,
                    span: inner.span,
                };
                let key = MemberKey::leaf(record, &group.name, &inner.name);
                // A member occupies its name whether or not it was accepted, so a
                // repeat here is a name conflict exactly as it is at a leaf below.
                // The nested group is refused either way; the repeat is the thing
                // the reader has to fix first.
                let row = if registry.members.declared(&key) {
                    member_conflict(file, inner.span, &anchor, &inner.name)
                } else {
                    let what = if inner.keys.is_empty() {
                        "a nested group"
                    } else {
                        "a keyed branch inside a group"
                    };
                    unsupported(file, inner.span, what)
                };
                let refusal = refuse_row(diagnostics, at, row);
                registry
                    .members
                    .declare(key, DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
        };
        let at = DeclarationSite {
            name: &field.name,
            file,
            at: declared.at,
            span: field.span,
        };
        let occurrence =
            if registry
                .members
                .declared(&MemberKey::leaf(record, &group.name, &field.name))
            {
                DeclarationOccurrence::Refused(refuse_row(
                    diagnostics,
                    at,
                    member_conflict(file, field.span, &anchor, &field.name),
                ))
            } else if field.keys.is_empty() {
                resource_member(
                    draft,
                    registry,
                    at,
                    field,
                    "this group field type",
                    diagnostics,
                )?
            } else {
                DeclarationOccurrence::Refused(refuse_row(
                    diagnostics,
                    at,
                    unsupported(file, field.span, "a keyed field"),
                ))
            };
        registry.members.declare(
            MemberKey::leaf(record, &group.name, &field.name),
            occurrence,
        )?;
    }
    let fields = registry.accepted_members(&anchor);
    let field_defs = fields
        .iter()
        .map(|leaf| {
            Ok(FieldDef {
                name: draft.intern_string(&leaf.name)?,
                ty: leaf.ty.image(),
                required: leaf.required,
            })
        })
        .collect::<Result<_, BuildError>>()?;
    Ok((fields, field_defs))
}
