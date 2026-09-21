//! Building the [`TypeRegistry`](super::TypeRegistry) from declarations: reserved
//! templates, the transparent-alias table, nominal intervals, and the
//! declare-then-fill passes for structs, enums, records, and materialized groups.

use super::*;

/// Report that `name` cannot be declared because `holder` already took it.
fn name_conflict(
    diagnostics: &mut DiagnosticCollector,
    file: &ProjectFile,
    span: SourceSpan,
    name: &str,
    holder: NameHolder,
) {
    diagnostics.push(SourceDiagnostic::at(
        Code::CheckNameConflict,
        file,
        span,
        format!("`{name}` is already declared as {}", holder.spelling()),
    ));
}

/// The declarations of one tree, for a name-conflict probe scoped to that tree.
fn in_origin<'a, T>(
    declarations: &'a [(FileRef, ProjectFile, &'a T)],
    origin: &'a SourceOrigin,
) -> impl Iterator<Item = &'a T> {
    declarations
        .iter()
        .filter(move |(_, file, _)| file.origin() == origin)
        .map(|(_, _, decl)| *decl)
}

/// What a declaration pass that has not run yet will bind `name` to.
///
/// The passes run alias, nominal, template, record, struct, enum, and each holds its
/// names against the later ones; once a pass has run, [`TypeRegistry::name_conflict`]
/// is the authority. Callers pass the source lists their own pass yields to, so this
/// one predicate serves every pass.
fn pending_name<'a>(
    name: &str,
    resources: impl IntoIterator<Item = &'a str>,
    structs: impl IntoIterator<Item = &'a str>,
    enums: impl IntoIterator<Item = &'a str>,
) -> Option<NameHolder> {
    let kind = if resources.into_iter().any(|other| other == name) {
        NamedTypeKind::Resource
    } else if structs.into_iter().any(|other| other == name) {
        NamedTypeKind::Struct
    } else if enums.into_iter().any(|other| other == name) {
        NamedTypeKind::Enum
    } else {
        return None;
    };
    Some(NameHolder::Kind(kind))
}

/// The reserved toolchain generic templates, in fixed order (`Option` then `Result`),
/// registered before any user template. They are ordinary generic enums defined here
/// rather than by user source: their payload leaves reference the templates' own type
/// parameters, so instantiation monomorphizes them exactly like a user generic enum,
/// and the lowerer recovers their reserved behavior from the minting template.
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
/// concrete image type; a name collision with a scalar, reserved name, alias, nominal,
/// resource, or another declared type is a `check.name_conflict`, and a structurally
/// unadmitted member is a `check.unsupported`. A defective template is dropped so no
/// `Name<Args>` use resolves against it.
pub(super) fn register_type_templates(
    registry: &mut TypeRegistry,
    structs: &[(FileRef, ProjectFile, &StructDecl)],
    enums: &[(FileRef, ProjectFile, &EnumDecl)],
    resources: &[(FileRef, ProjectFile, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), DeclareError> {
    // Templates yield to the concrete declarations of the same name; the generic rows
    // of these lists are this pass's own, held by the ledger as it declares them.
    // A pending declaration holds a name only in its own tree: type namespaces are
    // origin-scoped, so a dependency's `Pair` never conflicts with the root's.
    let taken = |registry: &TypeRegistry, declared: &DeclarationSite<'_>| {
        let scope = ScopedName::declared(declared);
        let origin = scope.origin();
        Ok::<_, DeclarationIndexDrift>(registry.name_conflict(&scope)?.or_else(|| {
            pending_name(
                declared.name,
                in_origin(resources, origin).map(|r| r.name.as_str()),
                in_origin(structs, origin)
                    .filter(|d| d.type_params.is_empty())
                    .map(|d| d.name.as_str()),
                in_origin(enums, origin)
                    .filter(|d| d.type_params.is_empty())
                    .map(|d| d.name.as_str()),
            )
        }))
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
        let taken = taken(registry, &declared)?;
        if !claim_name(&mut registry.named, declared, taken, diagnostics)? {
            continue;
        }
        let mut refusal = None;
        refuse_repeated_type_params(
            file,
            &decl.name,
            &decl.type_params,
            declared,
            diagnostics,
            &mut refusal,
        );
        let fields = declared_template_fields(file, decl, diagnostics, declared, &mut refusal);
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
        let body = fields.map(|fields| TemplateBody::Struct(fields.into()));
        settle_template(registry, declared, &decl.type_params, refusal, body)?;
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
        let taken = taken(registry, &declared)?;
        if !claim_name(&mut registry.named, declared, taken, diagnostics)? {
            continue;
        }
        let mut refusal = None;
        refuse_repeated_type_params(
            file,
            &decl.name,
            &decl.type_params,
            declared,
            diagnostics,
            &mut refusal,
        );
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
        let body = variants.map(|variants| TemplateBody::Enum(variants.into()));
        settle_template(registry, declared, &decl.type_params, refusal, body)?;
    }
    Ok(())
}

/// Whether a declaration may take its name: a reserved spelling is refused into the
/// ledger, and a name `taken` already holds is reported as a conflict. The one owner
/// of the checks every declaration pass runs before it reserves a row, so the passes
/// cannot drift on what a reserved or conflicting name reports.
fn claim_name(
    named: &mut DeclarationLedger<ScopedName, NamedTypeKind>,
    declared: DeclarationSite<'_>,
    taken: Option<NameHolder>,
    diagnostics: &mut DiagnosticCollector,
) -> Result<bool, DeclareError> {
    if is_reserved_type_name(declared.name) {
        let refusal = refuse_row(
            diagnostics,
            declared,
            reserved_name(declared.file, declared.span, declared.name),
        );
        named.declare(
            ScopedName::declared(&declared),
            DeclarationOccurrence::Refused(refusal),
        )?;
        return Ok(false);
    }
    if let Some(holder) = taken {
        name_conflict(
            diagnostics,
            declared.file,
            declared.span,
            declared.name,
            holder,
        );
        return Ok(false);
    }
    Ok(true)
}

/// Record this template's verdict: its refusal, or the accepted body.
///
/// Every arm that drops the members reports through the refusal accumulator, so a
/// refused template always carries the cause a use is steered to.
fn settle_template(
    registry: &mut TypeRegistry,
    declared: DeclarationSite<'_>,
    type_params: &[marrow_syntax::TypeParamDecl],
    refusal: Option<DeclarationRefusalSummary>,
    body: Option<TemplateBody>,
) -> Result<(), DeclareError> {
    let body = match (body, refusal) {
        (Some(body), None) => body,
        (_, Some(refusal)) => {
            return registry.named.declare(
                ScopedName::declared(&declared),
                DeclarationOccurrence::Refused(refusal),
            );
        }
        (None, None) => return Ok(()),
    };
    registry.named.declare(
        ScopedName::declared(&declared),
        DeclarationOccurrence::Accepted(NamedTypeKind::Template),
    )?;
    registry.type_templates.push(TypeTemplate {
        name: declared.name.to_string(),
        file: Some(declared.file.clone()),
        name_span: declared.span,
        reserved: None,
        type_params: type_params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    param.constraint.map(TypeConstraint::from_syntax),
                )
            })
            .collect(),
        body,
    });
    Ok(())
}

/// Refuse every type parameter of `owner` that repeats an earlier one, in order.
fn refuse_repeated_type_params(
    file: &ProjectFile,
    owner: &str,
    params: &[marrow_syntax::TypeParamDecl],
    declared: DeclarationSite<'_>,
    diagnostics: &mut DiagnosticCollector,
    refusal: &mut Option<DeclarationRefusalSummary>,
) {
    let mut names = MemberNamespace::new(owner);
    for param in params {
        if let Some(row) = names.claim(file, &param.name, param.name_span) {
            refuse_first(refusal, diagnostics, declared, row);
        }
    }
}

/// The row refusing a generic template's member type that names nothing this
/// project declares, or `None` when the spelling is resolvable.
///
/// A template's member types are resolved per instantiation, so without this check a
/// template naming an undeclared type is registered whole and its defect is first
/// reported at a *construction* site, blaming the construction for a declaration's
/// error. The declaration set is read raw because templates register before the
/// concrete types reserve, which also lets one template name another declared later.
fn unknown_template_member(
    registry: &TypeRegistry,
    structs: &[(FileRef, ProjectFile, &StructDecl)],
    enums: &[(FileRef, ProjectFile, &EnumDecl)],
    resources: &[(FileRef, ProjectFile, &ResourceDecl)],
    params: &[marrow_syntax::TypeParamDecl],
    ty: &TypeExpr,
    file: &ProjectFile,
) -> Option<SourceDiagnostic> {
    let declares = |written: &str| {
        if params.iter().any(|param| param.name == written) || matches!(written, "List" | "Map") {
            return true;
        }
        let Some(scope) = registry.scoped(file.origin(), written) else {
            return false;
        };
        let origin = scope.origin();
        let name = scope.name();
        ScalarType::from_spelling(name).is_some()
            || registry.aliases.contains_key(&scope)
            || registry.nominal_by_name(&scope).is_some()
            || in_origin(resources, origin).any(|decl| decl.name == name)
            || in_origin(structs, origin).any(|decl| decl.name == name)
            || in_origin(enums, origin).any(|decl| decl.name == name)
            || registry.type_template_by_name(&scope).is_some()
    };
    match ty {
        TypeExpr::Name { text, span, .. } => (!declares(text)).then(|| {
            SourceDiagnostic::at(
                Code::CheckType,
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
                    Code::CheckType,
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
/// Admit one struct member as the bare `name: Type` form, or report why it is not.
///
/// The single owner of which members a struct declaration may carry. The template pass
/// and the concrete fill pass differ only in what they do with an admitted field's
/// type, so a refusal spelled here is the one a reader sees from both.
fn admit_struct_member<'a>(
    member: &'a ResourceMember,
    file: &ProjectFile,
    names: &mut MemberNamespace<'a>,
    declared: DeclarationSite<'_>,
    diagnostics: &mut DiagnosticCollector,
    refusal: &mut Option<DeclarationRefusalSummary>,
) -> Option<&'a marrow_syntax::FieldDecl> {
    let ResourceMember::Field(field) = member else {
        refuse_first(
            refusal,
            diagnostics,
            declared,
            unsupported(file, member.span(), "a struct group"),
        );
        return None;
    };
    if let Some(row) = names.claim(file, &field.name, field.name_span) {
        refuse_first(refusal, diagnostics, declared, row);
        return None;
    }
    if !field.keys.is_empty() {
        refuse_first(
            refusal,
            diagnostics,
            declared,
            unsupported(file, field.span, "a keyed struct field"),
        );
        return None;
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
        return None;
    }
    Some(field)
}

fn declared_template_fields(
    file: &ProjectFile,
    decl: &StructDecl,
    diagnostics: &mut DiagnosticCollector,
    declared: DeclarationSite<'_>,
    refusal: &mut Option<DeclarationRefusalSummary>,
) -> Option<Vec<(String, TypeExpr)>> {
    let mut fields = Vec::new();
    let mut names = MemberNamespace::new(&decl.name);
    let mut ok = true;
    for member in &decl.members {
        let Some(field) =
            admit_struct_member(member, file, &mut names, declared, diagnostics, refusal)
        else {
            ok = false;
            continue;
        };
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
    file: &ProjectFile,
    decl: &EnumDecl,
    diagnostics: &mut DiagnosticCollector,
    declared: DeclarationSite<'_>,
    refusal: &mut Option<DeclarationRefusalSummary>,
) -> Option<Vec<TemplateVariant>> {
    let mut variants = Vec::new();
    let mut names = MemberNamespace::new(&decl.name);
    let mut ok = true;
    for member in &decl.members {
        if let Some(row) = names.claim(file, &member.name, member.name_span) {
            refuse_first(refusal, diagnostics, declared, row);
            ok = false;
            continue;
        }
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
        let mut payload_names = MemberNamespace::new(format!("{}.{}", decl.name, member.name));
        let mut payload = Vec::with_capacity(member.payload.len());
        for field in &member.payload {
            if let Some(row) = payload_names.claim(file, &field.name, field.name_span) {
                refuse_first(refusal, diagnostics, declared, row);
                ok = false;
                continue;
            }
            payload.push(TemplatePayload {
                name: field.name.clone(),
                ty: field.ty.clone(),
            });
        }
        variants.push(TemplateVariant {
            name: member.name.clone(),
            payload,
        });
    }
    ok.then_some(variants)
}

/// Resolve the alias declarations to shared global terminal targets. A
/// duplicate alias name or a collision with a resource name is a
/// `check.name_conflict`; an alias on a cyclic chain is a `check.recursion`
/// and does not enter the map.
pub(super) fn build_alias_table(
    named: &mut DeclarationLedger<ScopedName, NamedTypeKind>,
    origins: &CapturedOrigins,
    aliases: &[(FileRef, ProjectFile, &AliasDecl)],
    resources: &[(FileRef, ProjectFile, &ResourceDecl)],
    structs: &[(FileRef, ProjectFile, &StructDecl)],
    enums: &[(FileRef, ProjectFile, &EnumDecl)],
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
        let scope = ScopedName::declared(&declared);
        // A parse error blocks compilation before this runs, so a missing target means
        // the declaration was already reported; skip it quietly.
        let Some(ty) = &decl.ty else { continue };
        // Aliases resolve first, so every other declaration form is still only source
        // here and an alias yields its name to all of them.
        let taken = if raw.contains_key(&scope) || named.declared(&scope) {
            Some(NameHolder::Kind(NamedTypeKind::Alias))
        } else {
            pending_name(
                &decl.name,
                in_origin(resources, scope.origin()).map(|r| r.name.as_str()),
                in_origin(structs, scope.origin()).map(|d| d.name.as_str()),
                in_origin(enums, scope.origin()).map(|d| d.name.as_str()),
            )
        };
        if !claim_name(named, declared, taken, diagnostics)? {
            continue;
        }
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
            named.declare(scope, DeclarationOccurrence::Refused(refusal))?;
            continue;
        };
        let target = ScopedName::written(origins, file.origin(), target);
        raw.insert(
            scope,
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
    aliases: &[(FileRef, ProjectFile, &AliasDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), DeclareError> {
    let mut refused: Vec<ScopedName> = Vec::new();
    for (at, file, decl) in aliases {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.span,
        };
        let scope = ScopedName::declared(&declared);
        let Some(target) = registry.aliases.get(&scope) else {
            continue; // duplicate or cyclic: already reported
        };
        let terminal = target.terminal.clone();
        let text = terminal.name();
        let refusal = if ScalarType::from_spelling(text).is_none()
            && registry.by_name(&terminal).is_none()
            && registry.nominal_by_name(&terminal).is_none()
            && registry.struct_by_name(&terminal).is_none()
            && registry.enum_by_name(&terminal).is_none()
        {
            Some(match registry.named.lookup(&terminal)? {
                Binding::Refused(id, summary) => refuse_row(
                    diagnostics,
                    declared,
                    declaration_refused(file, decl.span, id.namespace(), summary),
                ),
                Binding::Accepted(_) | Binding::Absent => refuse(
                    diagnostics,
                    declared,
                    Code::CheckType,
                    format!("alias `{}` does not name a known type: `{text}`", decl.name),
                ),
            })
        } else {
            None
        };
        let occurrence = match refusal {
            Some(refusal) => {
                refused.push(scope.clone());
                DeclarationOccurrence::Refused(refusal)
            }
            None => DeclarationOccurrence::Accepted(NamedTypeKind::Alias),
        };
        registry.named.declare(scope, occurrence)?;
    }
    // Uses of a refused alias reach its own ledger cause, preserving the name the
    // annotation wrote rather than blaming the terminal spelling.
    for scope in refused {
        registry.aliases.remove(&scope);
    }
    Ok(())
}

/// Resolve the nominal type declarations against the aliases already installed in
/// `registry`. A name collision with an alias, resource, or earlier nominal is a
/// `check.name_conflict`; a base that does not denote `int` is a `check.unsupported`; a
/// non-literal, stepped, or empty interval is a `check.type`; the capability list must
/// draw from the closed set without repeats. A declaration with a defect is dropped
/// whole rather than admitted half-checked.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_nominals(
    registry: &mut TypeRegistry,
    nominals: &[(FileRef, ProjectFile, &NominalDecl)],
    resources: &[(FileRef, ProjectFile, &ResourceDecl)],
    structs: &[(FileRef, ProjectFile, &StructDecl)],
    enums: &[(FileRef, ProjectFile, &EnumDecl)],
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
        let scope = ScopedName::declared(&declared);
        // A parse error blocks compilation before this runs, so a missing piece means
        // the declaration was already reported; skip it quietly.
        let (Some(base), Some(interval)) = (&decl.base, &decl.interval) else {
            continue;
        };
        // Nominals yield to every declaration form the later passes bind, and a nominal
        // this pass already refused holds its name too.
        let taken = registry.name_conflict(&scope)?.or_else(|| {
            pending_name(
                &decl.name,
                in_origin(resources, scope.origin()).map(|r| r.name.as_str()),
                in_origin(structs, scope.origin()).map(|d| d.name.as_str()),
                in_origin(enums, scope.origin()).map(|d| d.name.as_str()),
            )
        });
        if !claim_name(&mut registry.named, declared, taken, diagnostics)? {
            continue;
        }
        let refused = match registry.scalar_annotation(file.origin(), base) {
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
                .declare(scope, DeclarationOccurrence::Refused(refusal))?;
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
                    .declare(scope.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
        };
        let supports = match support_set(file, decl) {
            Ok(supports) => supports,
            Err(row) => {
                let refusal = refuse_row(diagnostics, declared, *row);
                registry
                    .named
                    .declare(scope.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
        };
        registry.named.declare(
            scope.clone(),
            DeclarationOccurrence::Accepted(NamedTypeKind::Nominal),
        )?;
        built.push(NominalInfo {
            origin: file.origin().clone(),
            name: decl.name.clone(),
            lo,
            hi,
            supports,
        });
    }
    Ok(built)
}

/// Evaluate a nominal `in` range to its inclusive `[lo, hi]` bounds. The range follows
/// the language's range operators — `lo..hi` excludes the end, `lo..=hi` includes it —
/// with int-literal bounds (a leading `-` allowed), no step, and at least one admitted
/// value. The refusal row is returned rather than pushed, so the caller retains it as
/// the declaration's cause in the same statement that reports it.
fn nominal_interval(
    file: &ProjectFile,
    interval: &Expression,
) -> Result<(i64, i64), Box<SourceDiagnostic>> {
    let error = |span, message: &str| {
        Err(Box::new(SourceDiagnostic::at(
            Code::CheckType,
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
    // Normalize the end-exclusive spelling to the inclusive upper bound. A literal
    // never spells `i64::MIN`, so the exclusive form always has a representable
    // predecessor; the checked form keeps that self-evident.
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
    file: &ProjectFile,
    decl: &NominalDecl,
) -> Result<SupportSet, Box<SourceDiagnostic>> {
    let mut supports = SupportSet::default();
    for spelling in &decl.supports {
        let flag = match spelling.name.as_str() {
            "add" => &mut supports.add,
            "subtract" => &mut supports.subtract,
            other => {
                return Err(Box::new(SourceDiagnostic::at(
                    Code::CheckType,
                    file,
                    spelling.span,
                    format!("unknown capability `{other}`; the capabilities are add, subtract"),
                )));
            }
        };
        if *flag {
            return Err(Box::new(SourceDiagnostic::at(
                Code::CheckType,
                file,
                spelling.span,
                format!("capability `{}` is repeated", spelling.name),
            )));
        }
        *flag = true;
    }
    Ok(supports)
}

/// A struct or enum declaration whose image row pass one reserved, addressed by its
/// position in the registry table its kind fills.
pub(super) struct ReservedRow<'a, D> {
    file: ProjectFile,
    at: FileRef,
    decl: &'a D,
    index: usize,
}

/// A declaration kind pass two fills: how its members resolve, how an accepted
/// resolution commits to the image row and the registry table, and how a refusal
/// marks the row it leaves in place.
pub(super) trait DeclaredRow {
    type Members;
    const KIND: NamedTypeKind;
    fn site<'a>(&'a self, file: &'a ProjectFile, at: FileRef) -> DeclarationSite<'a>;
    fn resolve(
        &self,
        draft: &mut DraftTxn<'_>,
        registry: &mut TypeRegistry,
        declared: DeclarationSite<'_>,
        diagnostics: &mut DiagnosticCollector,
    ) -> Result<DeclarationOccurrence<Self::Members>, BuildError>;
    fn commit(
        draft: &mut DraftTxn<'_>,
        registry: &mut TypeRegistry,
        index: usize,
        members: Self::Members,
    );
    fn refuse(registry: &mut TypeRegistry, index: usize);
}

impl DeclaredRow for StructDecl {
    type Members = ResolvedStructFields;
    const KIND: NamedTypeKind = NamedTypeKind::Struct;

    fn site<'a>(&'a self, file: &'a ProjectFile, at: FileRef) -> DeclarationSite<'a> {
        DeclarationSite {
            name: &self.name,
            file,
            at,
            span: self.name_span,
        }
    }

    fn resolve(
        &self,
        draft: &mut DraftTxn<'_>,
        registry: &mut TypeRegistry,
        declared: DeclarationSite<'_>,
        diagnostics: &mut DiagnosticCollector,
    ) -> Result<DeclarationOccurrence<Self::Members>, BuildError> {
        Ok(struct_fields(draft, registry, declared, self, diagnostics)?)
    }

    fn commit(
        draft: &mut DraftTxn<'_>,
        registry: &mut TypeRegistry,
        index: usize,
        (fields, field_defs): Self::Members,
    ) {
        let info = &mut registry.structs[index];
        #[expect(
            clippy::expect_used,
            reason = "reserve-then-fill law: the row was reserved in this batch and fills exactly once"
        )]
        draft
            .set_record_fields(info.type_id, field_defs)
            .expect("a reserved row fills once");
        info.fields = fields;
    }

    fn refuse(registry: &mut TypeRegistry, index: usize) {
        registry.structs[index].verdict = DeclarationVerdict::Refused;
    }
}

impl DeclaredRow for EnumDecl {
    type Members = EnumVariants;
    const KIND: NamedTypeKind = NamedTypeKind::Enum;

    fn site<'a>(&'a self, file: &'a ProjectFile, at: FileRef) -> DeclarationSite<'a> {
        DeclarationSite {
            name: &self.name,
            file,
            at,
            span: self.name_span,
        }
    }

    fn resolve(
        &self,
        draft: &mut DraftTxn<'_>,
        registry: &mut TypeRegistry,
        declared: DeclarationSite<'_>,
        diagnostics: &mut DiagnosticCollector,
    ) -> Result<DeclarationOccurrence<Self::Members>, BuildError> {
        enum_variants(draft, registry, declared, self, diagnostics)
    }

    fn commit(
        draft: &mut DraftTxn<'_>,
        registry: &mut TypeRegistry,
        index: usize,
        (variants, variant_defs): Self::Members,
    ) {
        let info = &mut registry.enums[index];
        #[expect(
            clippy::expect_used,
            reason = "reserve-then-fill law: the row was reserved in this batch and fills exactly once"
        )]
        draft
            .set_enum_variants(info.enum_id, variant_defs)
            .expect("a reserved row fills once");
        info.variants = variants;
    }

    fn refuse(registry: &mut TypeRegistry, index: usize) {
        registry.enums[index].verdict = DeclarationVerdict::Refused;
    }
}

/// Pass two for the dense struct and closed enum types: resolve each reserved row's
/// members against the full registry and fill both the registry info and the image
/// row, addressed by the index the reservation recorded. A declaration with a member
/// defect is refused whole (its reserved image row stays empty and its name leaves
/// the accepted set) so a later construction or match cannot resolve against a broken
/// type. Its reserved row stays in place carrying [`DeclarationVerdict::Refused`], so
/// a reference an earlier fill pass minted against the reservation addresses a refused
/// declaration rather than dangling.
pub(super) fn fill_rows<D: DeclaredRow>(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    reserved: &[ReservedRow<'_, D>],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    for item in reserved {
        let declared = item.decl.site(&item.file, item.at);
        let occurrence = item
            .decl
            .resolve(draft, registry, declared, diagnostics)?
            .map_accepted(|members| {
                D::commit(draft, registry, item.index, members);
                D::KIND
            });
        if matches!(occurrence, DeclarationOccurrence::Refused(_)) {
            D::refuse(registry, item.index);
        }
        registry
            .named
            .declare(ScopedName::declared(&declared), occurrence)?;
    }
    Ok(())
}

/// Pass one for the dense struct types: reserve each admitted struct's image
/// [`RecordTypeDef`] index (empty for now) and register its name, so pass two may
/// resolve a field that names any other struct or enum. A name collision with a scalar,
/// alias, nominal, resource, or earlier struct is a `check.name_conflict`, and that
/// struct is dropped and never reserved.
pub(super) fn declare_structs<'a>(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    structs: &'a [(FileRef, ProjectFile, &StructDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<ReservedRow<'a, StructDecl>>, DeclareError> {
    let mut reserved: Vec<ReservedRow<'a, StructDecl>> = Vec::new();
    for (at, file, decl) in structs {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        let taken = registry.name_conflict(&ScopedName::declared(&declared))?;
        if !claim_name(&mut registry.named, declared, taken, diagnostics)? {
            continue;
        }
        let name_id = draft.intern_string(&decl.name)?;
        let type_id = draft.reserve_record_type(name_id)?;
        registry
            .coordinates
            .declare(type_id, *at, file, decl.name_span);
        let index = registry.structs.len();
        registry.structs.push(StructInfo {
            type_id,
            origin: file.origin().clone(),
            name: decl.name.clone(),
            fields: Vec::new(),
            verdict: DeclarationVerdict::Accepted,
        });
        reserved.push(ReservedRow {
            file: file.clone(),
            at: *at,
            decl,
            index,
        });
    }
    Ok(reserved)
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
    let mut names = MemberNamespace::new(&decl.name);
    let mut refusal = None;
    let mut limited = false;
    for member in &decl.members {
        let Some(field) = admit_struct_member(
            member,
            file,
            &mut names,
            declared,
            diagnostics,
            &mut refusal,
        ) else {
            continue;
        };
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
                    if registry.optional_annotation(file.origin(), &field.ty) {
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
        // The shared instantiation limit reports once, at the monomorphization owner;
        // this declaration is refused for that cause.
        (None, true) => {
            DeclarationOccurrence::Refused(refuse_covered(declared, Code::CheckInstantiationLimit))
        }
        (None, false) => DeclarationOccurrence::Accepted((fields, field_defs)),
    })
}

/// Pass one for the closed flat enum types: reserve each admitted enum's image
/// [`EnumTypeDef`] index (empty for now) and register its name. A name collision with a
/// scalar, alias, nominal, resource, struct, or earlier enum is a `check.name_conflict`,
/// and that enum is dropped and never reserved. Reserving user enums before pass two
/// resolves any field types keeps their image indices ahead of the `Option`/`Result`
/// instantiations minted lazily during field resolution.
pub(super) fn declare_enums<'a>(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    enums: &'a [(FileRef, ProjectFile, &EnumDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<ReservedRow<'a, EnumDecl>>, DeclareError> {
    let mut reserved: Vec<ReservedRow<'a, EnumDecl>> = Vec::new();
    for (at, file, decl) in enums {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        let scope = ScopedName::declared(&declared);
        let taken = registry.name_conflict(&scope)?;
        if !claim_name(&mut registry.named, declared, taken, diagnostics)? {
            continue;
        }
        if decl.members.len() > marrow_image::bounds::MAX_VARIANTS {
            let refusal = refuse(
                diagnostics,
                declared,
                Code::CheckResourceLimit,
                format!(
                    "an enum declares {} members; the fixed limit is {}",
                    decl.members.len(),
                    marrow_image::bounds::MAX_VARIANTS
                ),
            );
            registry
                .named
                .declare(scope, DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        let name_id = draft.intern_string(&decl.name)?;
        let enum_id = draft.reserve_enum_type(name_id)?;
        registry
            .coordinates
            .declare_enum(enum_id, *at, file, decl.name_span);
        let index = registry.enums.len();
        registry.enums.push(EnumInfo {
            enum_id,
            origin: file.origin().clone(),
            name: decl.name.clone(),
            variants: Vec::new(),
            verdict: DeclarationVerdict::Accepted,
        });
        reserved.push(ReservedRow {
            file: file.clone(),
            at: *at,
            decl,
            index,
        });
    }
    Ok(reserved)
}

/// One enum's selectable variants and the image definitions that carry them.
type EnumVariants = (Vec<VariantInfo>, Vec<VariantDef>);

/// One enum member's payload fields, as info and as the leaf types the image holds.
type EnumPayload = (Vec<EnumPayloadInfo>, Vec<ImageType>);

/// Resolve an enum's members to its selectable variants and their image
/// definitions, or `None` if any member is unsupported. On the flat line every
/// member is a leaf: a `category` member or one with nested members is deferred.
fn enum_variants(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    declared: DeclarationSite<'_>,
    decl: &EnumDecl,
    diagnostics: &mut DiagnosticCollector,
) -> Result<DeclarationOccurrence<EnumVariants>, BuildError> {
    let file = declared.file;
    let mut variants = Vec::new();
    let mut variant_defs = Vec::new();
    let mut names = MemberNamespace::new(&decl.name);
    let mut refusal = None;
    let mut limited = false;
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
        if let Some(row) = names.claim(file, &member.name, member.name_span) {
            refuse_first(&mut refusal, diagnostics, declared, row);
            continue;
        }
        let Some((payload, payload_leaves)) = enum_payload(
            draft,
            registry,
            declared,
            member,
            diagnostics,
            &mut refusal,
            &mut limited,
        )?
        else {
            continue;
        };
        let name_id = draft.intern_string(&member.name)?;
        variant_defs.push(VariantDef {
            name: name_id,
            category: false,
            payload: payload_leaves,
        });
        variants.push(VariantInfo {
            name: member.name.clone(),
            payload,
        });
    }
    Ok(match (refusal, limited) {
        (Some(refusal), _) => DeclarationOccurrence::Refused(refusal),
        // The shared instantiation limit reports once, at the monomorphization owner;
        // this declaration is refused for that cause.
        (None, true) => {
            DeclarationOccurrence::Refused(refuse_covered(declared, Code::CheckInstantiationLimit))
        }
        (None, false) => DeclarationOccurrence::Accepted((variants, variant_defs)),
    })
}

/// Resolve one member's payload fields to their leaf types and info, or `None`
/// when a field is not the bare `name: Type` form over a payload type. A defect
/// refuses the whole declaration, so it is recorded in the enum's shared refusal
/// rather than returned separately.
fn enum_payload(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    declared: DeclarationSite<'_>,
    member: &EnumMember,
    diagnostics: &mut DiagnosticCollector,
    refusal: &mut Option<DeclarationRefusalSummary>,
    limited: &mut bool,
) -> Result<Option<EnumPayload>, BuildError> {
    let file = declared.file;
    if member.payload.len() > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
        refuse_first(
            refusal,
            diagnostics,
            declared,
            SourceDiagnostic::at(
                Code::CheckResourceLimit,
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
    let mut leaves = Vec::new();
    let mut names = MemberNamespace::new(format!("{}.{}", declared.name, member.name));
    let mut ok = true;
    for field in &member.payload {
        if let Some(row) = names.claim(file, &field.name, field.name_span) {
            refuse_first(refusal, diagnostics, declared, row);
            ok = false;
            continue;
        }
        let site = MintSite {
            file,
            span: field.ty.span(),
        };
        // A payload annotation is written in the tree that declares the enum, so it
        // resolves in that tree's namespace.
        let ty = match registry.enum_payload_leaf(draft, file.origin(), &field.ty, &[], site) {
            Ok((ty, None)) => ty,
            Ok((_, Some(coll))) => {
                let row =
                    registry.collection_payload_refusal(site, declared.name, &member.name, coll);
                refuse_first(refusal, diagnostics, declared, row);
                ok = false;
                continue;
            }
            Err(ResolveError::Refusal(refused)) => {
                let subject = if registry.optional_annotation(file.origin(), &field.ty) {
                    "an optional enum payload field type"
                } else {
                    "this enum payload field type"
                };
                match registry.member_refusal_row(refused, file, field.ty.span(), subject)? {
                    Some(row) => refuse_first(refusal, diagnostics, declared, row),
                    None => *limited = true,
                }
                ok = false;
                continue;
            }
            Err(ResolveError::Invariant(invariant)) => return Err(invariant.into()),
        };
        payload.push(EnumPayloadInfo {
            name: field.name.clone(),
            ty,
        });
        leaves.push(ty.image());
    }
    Ok(ok.then_some((payload, leaves)))
}

/// Pass one for the admitted record types: reserve each resource's image
/// [`RecordTypeDef`] index (empty for now, ahead of the structs) and register its name,
/// returning the surviving resource declarations for pass two in the same order as
/// [`TypeRegistry::records`]. A reserved name, or one a prior resource already declared,
/// drops that resource with a precise diagnostic; the first declaration stands.
pub(super) fn declare_records<'a>(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    resources: &'a [(FileRef, ProjectFile, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<(FileRef, ProjectFile, &'a ResourceDecl)>, DeclareError> {
    let mut survivors = Vec::new();
    for (ordinal, (at, file, resource)) in resources.iter().enumerate() {
        let declared = DeclarationSite {
            name: &resource.name,
            file,
            at: *at,
            span: resource.name_span,
        };
        let scope = ScopedName::declared(&declared);
        let taken = match registry.name_conflict(&scope)? {
            // Two resources of the same name have no unambiguous record identity,
            // so a repeat is a precise typed rejection and the first stands.
            Some(NameHolder::Kind(NamedTypeKind::Resource)) => {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckType,
                    file,
                    resource.name_span,
                    format!("`{}` is already declared as a resource", resource.name),
                ));
                continue;
            }
            taken => taken,
        };
        if !claim_name(&mut registry.named, declared, taken, diagnostics)? {
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
                origin: file.origin().clone(),
                name: resource.name.clone(),
                fields: Vec::new(),
                groups: Vec::new(),
            },
            ordinal,
        );
        registry.named.declare(
            scope,
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
    record_decls: &[(FileRef, ProjectFile, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    // The survivors are in the same order as the reserved records, so record `index`
    // is the one this declaration reserved.
    let mut published_groups = false;
    for (index, (at, file, resource)) in record_decls.iter().enumerate() {
        let declared = DeclarationSite {
            name: &resource.name,
            file,
            at: *at,
            span: resource.name_span,
        };
        fill_record(draft, registry, index, declared, resource, diagnostics)?;
        published_groups |= !registry.records[index].groups.is_empty();
    }
    if published_groups {
        registry.invalidate_row_directory();
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
/// A refused member is `check.unsupported` at its own span and only that member leaves
/// the accepted set. The refusal stays in the ledger, so a later use of that member is
/// steered to the cause rather than told the record has no such field.
fn fill_record(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    index: usize,
    declared: DeclarationSite<'_>,
    resource: &ResourceDecl,
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    let file = declared.file;
    let owner = ScopedName::declared(&declared);
    let mut groups = Vec::new();
    let mut group_slot_defs = Vec::new();
    // Fields, groups, and branches share the resource's one member layer: a group or
    // branch is declared here as a name even though its value or placement is built
    // elsewhere, so a repeat across kinds is refused at the repeat.
    let mut names = MemberNamespace::new(&resource.name);
    for member in &resource.members {
        let (name, name_span) = match member {
            ResourceMember::Field(field) => (&field.name, field.name_span),
            ResourceMember::Group(group) => (&group.name, group.name_span),
        };
        let at = DeclarationSite {
            name,
            file,
            at: declared.at,
            span: member.span(),
        };
        if let Some(row) = names.claim(file, name, name_span) {
            let refusal = refuse_row(diagnostics, at, row);
            registry.members.declare(
                MemberKey::new(&owner, name),
                DeclarationOccurrence::Refused(refusal),
            )?;
            continue;
        }
        match member {
            ResourceMember::Field(field) => {
                let occurrence = if field.keys.is_empty() {
                    resource_member(draft, registry, at, field, "this field type", diagnostics)?
                } else {
                    // A keyed scalar leaf (`tags(pos: int): string`) is a keyed
                    // positional layer, outside the durable graph. It is refused so
                    // the shape is a precise rejection, not a silent drop.
                    DeclarationOccurrence::Refused(refuse_row(
                        diagnostics,
                        at,
                        unsupported(file, field.span, "a keyed field"),
                    ))
                };
                registry
                    .members
                    .declare(MemberKey::new(&owner, &field.name), occurrence)?;
            }
            ResourceMember::Group(group) if group.keys.is_empty() => {
                let (info, slot) =
                    admit_unkeyed_group(draft, registry, &owner, group, declared, diagnostics)?;
                groups.push(info);
                group_slot_defs.push(slot);
            }
            ResourceMember::Group(branch) => {
                // A keyed `branch` is a durable-graph member resolved by `durable.rs`:
                // an addressed collection, not part of the materialized value. Its
                // layers are claimed here once per declaration, whatever number of
                // stores reach it.
                let mut refusal = None;
                refuse_branch_layer_repeats(
                    file,
                    &format!("{}.{}", resource.name, branch.name),
                    branch,
                    declared,
                    diagnostics,
                    &mut refusal,
                );
                if let Some(refusal) = refusal {
                    registry.members.declare(
                        MemberKey::new(&owner, &branch.name),
                        DeclarationOccurrence::Refused(refusal),
                    )?;
                }
            }
        }
    }
    seal_record_slots(draft, registry, index, &owner, groups, group_slot_defs)
}

/// Build one unkeyed `group` as a nested sub-record value: its scalar and enum leaves
/// become a group record type, and the containing record gains one required slot
/// holding that record. The group's durable identity is owned separately by
/// `durable.rs`; this is the materialized-value side only.
fn admit_unkeyed_group(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    owner: &ScopedName,
    group: &GroupDecl,
    declared: DeclarationSite<'_>,
    diagnostics: &mut DiagnosticCollector,
) -> Result<(GroupInfo, FieldDef), BuildError> {
    let (leaf_fields, leaf_defs) =
        build_group_leaves(draft, registry, owner, group, declared, diagnostics)?;
    let group_name_id = draft.intern_string(owner.below(&group.name).name())?;
    let group_type_id = draft.add_record_type(RecordTypeDef {
        name: group_name_id,
        fields: leaf_defs,
    })?;
    let slot = FieldDef {
        name: draft.intern_string(&group.name)?,
        ty: ImageType::Record {
            idx: group_type_id,
            optional: false,
        },
        required: true,
    };
    Ok((
        GroupInfo {
            name: group.name.clone(),
            type_id: group_type_id,
            fields: leaf_fields,
        },
        slot,
    ))
}

/// Fill the reserved record from the member ledger.
///
/// The ledger is the authority for which members survived and in what order, so the
/// record's fields and the image slots are read out of it rather than accumulated
/// beside it. The record is group-inclusive: its top-level field slots followed by one
/// group-record slot per unkeyed group, in declaration order, so this one record type
/// serves both the durable graph and the storeless value model.
fn seal_record_slots(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    index: usize,
    owner: &ScopedName,
    groups: Vec<GroupInfo>,
    group_slot_defs: Vec<FieldDef>,
) -> Result<(), BuildError> {
    let type_id = registry.records[index].type_id;
    let fields = registry.accepted_members(owner);
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

/// Claim one branch's layer — its key columns, then its members in declaration
/// order — and every group's layer below it, refusing each repeat at the repeat.
///
/// A static group nested in a branch claims its own leaves here too: the type registry
/// materializes only the resource's top-level groups, so this is the one owner for
/// every layer below a branch, and it runs once per declaration rather than once per
/// store that binds the resource.
fn refuse_branch_layer_repeats(
    file: &ProjectFile,
    anchor: &str,
    group: &GroupDecl,
    declared: DeclarationSite<'_>,
    diagnostics: &mut DiagnosticCollector,
    refusal: &mut Option<DeclarationRefusalSummary>,
) {
    let mut names = MemberNamespace::new(anchor);
    for column in &group.keys {
        if let Some(row) = names.claim(file, &column.name, column.name_span) {
            refuse_first(refusal, diagnostics, declared, row);
        }
    }
    for member in &group.members {
        let (name, name_span) = match member {
            ResourceMember::Field(field) => (&field.name, field.name_span),
            ResourceMember::Group(inner) => (&inner.name, inner.name_span),
        };
        if let Some(row) = names.claim(file, name, name_span) {
            refuse_first(refusal, diagnostics, declared, row);
            continue;
        }
        if let ResourceMember::Group(inner) = member {
            refuse_branch_layer_repeats(
                file,
                &format!("{anchor}.{}", inner.name),
                inner,
                declared,
                diagnostics,
                refusal,
            );
        }
    }
}

/// Resolve one resource member's declared type to the value it binds, or to the
/// refusal the member ledger retains.
///
/// A resource member is a value drawn from the closed acyclic durable value set: a
/// scalar, a nominal scalar, a dense struct, or a closed enum. A collection is not a
/// durable member value; an abstract parameter never reaches a concrete record.
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
            // A member type outside the durable value set is a genuine subset gap; one
            // naming a refused declaration is steered to that declaration's own cause.
            Ok(_) => DeclarationOccurrence::Refused(refuse_row(
                diagnostics,
                at,
                unsupported(file, field.ty.span(), subject),
            )),
            Err(ResolveError::Refusal(refused)) => {
                match registry.member_refusal_row(refused, file, field.ty.span(), subject)? {
                    Some(row) => DeclarationOccurrence::Refused(refuse_row(diagnostics, at, row)),
                    // The shared instantiation limit reports once, at the
                    // monomorphization owner; this member is refused for that cause.
                    None => DeclarationOccurrence::Refused(refuse_covered(
                        at,
                        Code::CheckInstantiationLimit,
                    )),
                }
            }
            Err(ResolveError::Invariant(invariant)) => return Err(invariant),
        },
    )
}

/// The direct scalar/enum leaves of an unkeyed group, in declaration order, returning
/// both the registry field infos and the image field defs. A keyed leaf, a nested group
/// or keyed branch inside the group, or a non-value leaf type is a precise
/// `check.unsupported` that refuses only that leaf, so a deferred shape neither drops
/// silently nor leaves its name unanswerable at a use.
fn build_group_leaves(
    draft: &mut DraftTxn<'_>,
    registry: &mut TypeRegistry,
    record: &ScopedName,
    group: &GroupDecl,
    declared: DeclarationSite<'_>,
    diagnostics: &mut DiagnosticCollector,
) -> Result<(Vec<FieldInfo>, Vec<FieldDef>), BuildError> {
    let file = declared.file;
    let anchor = record.below(&group.name);
    let mut names = MemberNamespace::new(anchor.name());
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
                let key = MemberKey::new(&anchor, &inner.name);
                // The nested group is refused either way; a repeated name is the
                // thing the reader has to fix first.
                let row = names
                    .claim(file, &inner.name, inner.name_span)
                    .unwrap_or_else(|| {
                        let what = if inner.keys.is_empty() {
                            "a nested group"
                        } else {
                            "a keyed branch inside a group"
                        };
                        unsupported(file, inner.span, what)
                    });
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
        let occurrence = if let Some(row) = names.claim(file, &field.name, field.name_span) {
            DeclarationOccurrence::Refused(refuse_row(diagnostics, at, row))
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
        registry
            .members
            .declare(MemberKey::new(&anchor, &field.name), occurrence)?;
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
