//! The project named-type registry: transparent aliases and the record type.
//!
//! This is the single owner of what a source type name denotes. A transparent
//! `alias Name = Type` denotes exactly its expansion — it mints no identity and
//! no constructor — so every annotation classification calls [`TypeRegistry::expand`]
//! before reading the spelling. A nominal `type Name: int in lo..hi` mints a
//! distinct type: the registry owns its identity — name, inclusive interval, and
//! `supports` capability set — while the image records only its base scalar, so
//! the interval is carried by the guard instructions the compiler emits, not by
//! an image type table. Two product kinds lower into image [`RecordTypeDef`]s,
//! the single canonical product-leaf order owner: the optional `resource` (a
//! durable record with required and sparse scalar fields, no groups and no keyed
//! children) and any number of dense `struct` value types (every field required,
//! non-durable, constructible and read by value).

use std::collections::BTreeMap;

use marrow_codes::Code;
use marrow_image::{EnumId, EnumTypeDef, FieldDef, ImageDraft, RecordTypeDef, TypeId, VariantDef};
use marrow_syntax::{
    AliasDecl, EnumDecl, EnumMember, Expression, LiteralKind, NominalDecl, ResourceDecl,
    ResourceMember, SourceSpan, StructDecl, TypeExpr, UnaryOp, range_expr,
};

use crate::diag::SourceDiagnostic;
use crate::scalar::ScalarType;

/// The identity of a nominal type in [`TypeRegistry`] order, carried by the
/// lowered type so classification never re-reads the source spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NominalId(pub(crate) u16);

/// The closed capability set a nominal declaration's `supports` list unlocks.
/// Each flag independently admits operators over the nominal (see the lowerer's
/// operator mapping); construction and `.checked` need no capability.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SupportSet {
    pub(crate) add: bool,
    pub(crate) subtract: bool,
    pub(crate) step: bool,
    pub(crate) scale: bool,
}

/// One nominal type: a distinct int-based type whose every value lies in the
/// inclusive interval `[lo, hi]`.
pub(crate) struct NominalInfo {
    pub(crate) name: String,
    pub(crate) lo: i64,
    pub(crate) hi: i64,
    pub(crate) supports: SupportSet,
}

/// One resolved record field, in declaration order.
pub(crate) struct FieldInfo {
    pub(crate) name: String,
    pub(crate) scalar: ScalarType,
    pub(crate) required: bool,
}

/// The project's single record type.
pub(crate) struct RecordInfo {
    pub(crate) type_id: TypeId,
    pub(crate) name: String,
    pub(crate) fields: Vec<FieldInfo>,
}

impl RecordInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u16, &FieldInfo)> {
        field_index(&self.fields, name)
    }
}

/// One dense product type: a `struct` whose every field is present inline. It
/// shares the image [`RecordTypeDef`] representation with the resource record —
/// the single canonical product-leaf order owner — but is a distinct value type:
/// non-durable, constructed and read by value, every field required. A struct is
/// admitted as a parameter and a return type (carried as an `ImageType::Record`).
pub(crate) struct StructInfo {
    pub(crate) type_id: TypeId,
    pub(crate) name: String,
    pub(crate) fields: Vec<FieldInfo>,
}

impl StructInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u16, &FieldInfo)> {
        field_index(&self.fields, name)
    }
}

/// One enum-variant payload leaf: a named scalar carried by that variant, in
/// declaration order. The name is used for named construction; the image records
/// only the scalar.
pub(crate) struct EnumPayloadInfo {
    pub(crate) name: String,
    pub(crate) scalar: ScalarType,
}

/// One selectable enum variant: its member name and dense scalar payload.
pub(crate) struct VariantInfo {
    pub(crate) name: String,
    pub(crate) payload: Vec<EnumPayloadInfo>,
}

/// One closed flat enum value type. It lowers to an image [`EnumTypeDef`]; its
/// distinct nominal identity lives here. Hierarchical categories are deferred, so
/// every variant is a selectable leaf.
pub(crate) struct EnumInfo {
    pub(crate) enum_id: EnumId,
    pub(crate) name: String,
    pub(crate) variants: Vec<VariantInfo>,
}

impl EnumInfo {
    /// The index and info of the variant named `name` in declaration order.
    pub(crate) fn variant(&self, name: &str) -> Option<(u16, &VariantInfo)> {
        self.variants
            .iter()
            .enumerate()
            .find(|(_, variant)| variant.name == name)
            .map(|(index, variant)| (index as u16, variant))
    }
}

/// The index and info of the field named `name` in declaration order, shared by
/// the resource record and the dense struct so field lookup has one owner.
fn field_index<'f>(fields: &'f [FieldInfo], name: &str) -> Option<(u16, &'f FieldInfo)> {
    fields
        .iter()
        .enumerate()
        .find(|(_, field)| field.name == name)
        .map(|(index, field)| (index as u16, field))
}

/// The project named-type registry: the transparent aliases, the nominal int
/// types, the dense struct value types, and zero or one durable record type.
#[derive(Default)]
pub(crate) struct TypeRegistry {
    /// `alias name -> alias-free expanded target`. Cyclic aliases are reported
    /// at build and never enter this map.
    aliases: BTreeMap<String, TypeExpr>,
    nominals: Vec<NominalInfo>,
    structs: Vec<StructInfo>,
    enums: Vec<EnumInfo>,
    record: Option<RecordInfo>,
}

impl TypeRegistry {
    pub(crate) fn by_name(&self, name: &str) -> Option<&RecordInfo> {
        self.record.as_ref().filter(|info| info.name == name)
    }

    pub(crate) fn struct_by_name(&self, name: &str) -> Option<&StructInfo> {
        self.structs.iter().find(|info| info.name == name)
    }

    pub(crate) fn struct_by_type(&self, ty: TypeId) -> Option<&StructInfo> {
        self.structs.iter().find(|info| info.type_id == ty)
    }

    pub(crate) fn enum_by_name(&self, name: &str) -> Option<&EnumInfo> {
        self.enums.iter().find(|info| info.name == name)
    }

    pub(crate) fn enum_by_id(&self, id: EnumId) -> Option<&EnumInfo> {
        self.enums.iter().find(|info| info.enum_id == id)
    }

    pub(crate) fn nominal_by_name(&self, name: &str) -> Option<(NominalId, &NominalInfo)> {
        self.nominals
            .iter()
            .position(|info| info.name == name)
            .map(|index| (NominalId(index as u16), &self.nominals[index]))
    }

    pub(crate) fn nominal(&self, id: NominalId) -> &NominalInfo {
        &self.nominals[id.0 as usize]
    }

    pub(crate) fn by_name_for_type(&self, ty: TypeId) -> Option<&RecordInfo> {
        self.record.as_ref().filter(|info| info.type_id == ty)
    }

    /// The alias-free form of a type annotation: every name that is an alias is
    /// replaced by its expanded target, structurally, so classification reads
    /// only scalar spellings and declared type names. Diagnostics stay on the
    /// caller's annotation span — expansion carries the alias target's spans,
    /// which point at another declaration.
    pub(crate) fn expand(&self, ty: &TypeExpr) -> TypeExpr {
        match ty {
            TypeExpr::Name { text, .. } => match self.aliases.get(text) {
                Some(target) => target.clone(),
                None => ty.clone(),
            },
            TypeExpr::Optional { inner, span } => TypeExpr::Optional {
                inner: Box::new(self.expand(inner)),
                span: *span,
            },
            TypeExpr::Sequence { element, span } => TypeExpr::Sequence {
                element: Box::new(self.expand(element)),
                span: *span,
            },
            TypeExpr::Identity(_) => ty.clone(),
        }
    }

    /// Build the registry: the alias table (duplicates, resource-name collisions,
    /// and cycles rejected; targets pre-expanded to alias-free form and validated
    /// against the known types), then the one admitted record type, whose field
    /// annotations resolve through the aliases. More than one resource, groups,
    /// keyed fields, or non-scalar field types are rejected.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        draft: &mut ImageDraft,
        aliases: &[(String, &AliasDecl)],
        nominals: &[(String, &NominalDecl)],
        structs: &[(String, &StructDecl)],
        enums: &[(String, &EnumDecl)],
        resources: &[(String, &ResourceDecl)],
        diagnostics: &mut Vec<SourceDiagnostic>,
    ) -> Self {
        let mut registry = Self {
            aliases: build_alias_table(aliases, resources, structs, enums, diagnostics),
            nominals: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            record: None,
        };
        registry.nominals =
            build_nominals(&registry, nominals, resources, structs, enums, diagnostics);
        // The resource record takes its image type index before the structs, so a
        // project's durable root and sites keep the same record index whether or
        // not dense structs are also declared. Enums take a separate index space
        // (the ENUMS table), so they build last (alias -> nominal -> record ->
        // struct -> enum).
        registry.record = build_record(draft, &registry, resources, diagnostics);
        registry.structs = build_structs(draft, &registry, structs, resources, diagnostics);
        registry.enums = build_enums(draft, &registry, enums, resources, diagnostics);
        validate_alias_targets(&registry, aliases, diagnostics);
        registry
    }
}

/// Resolve the alias declarations to an alias-free name → target map. A
/// duplicate alias name or a collision with a resource name is a
/// `check.name_conflict`; an alias on a cyclic chain is a `check.recursion`
/// and does not enter the map.
fn build_alias_table(
    aliases: &[(String, &AliasDecl)],
    resources: &[(String, &ResourceDecl)],
    structs: &[(String, &StructDecl)],
    enums: &[(String, &EnumDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> BTreeMap<String, TypeExpr> {
    let mut raw: BTreeMap<String, TypeExpr> = BTreeMap::new();
    for (file, decl) in aliases {
        // A parse error blocks compilation before this runs, so a missing target
        // only means the declaration itself was reported; skip it quietly.
        let Some(ty) = &decl.ty else { continue };
        if raw.contains_key(&decl.name) {
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
            .any(|(_, resource)| resource.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a resource", decl.name),
            ));
            continue;
        }
        if structs.iter().any(|(_, item)| item.name == decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a struct", decl.name),
            ));
            continue;
        }
        if enums.iter().any(|(_, item)| item.name == decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as an enum", decl.name),
            ));
            continue;
        }
        raw.insert(decl.name.clone(), ty.clone());
    }

    // Reject every alias that can reach itself through the names its target
    // mentions. Reported per alias on the cycle, at its declaration.
    let cyclic: Vec<String> = raw
        .keys()
        .filter(|name| reaches_itself(name, &raw))
        .cloned()
        .collect();
    for name in &cyclic {
        let (file, decl) = aliases
            .iter()
            .find(|(_, decl)| &decl.name == name)
            .expect("cyclic alias came from the declaration list");
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckRecursion.as_str(),
            file,
            decl.name_span,
            format!("alias `{name}` is part of a cyclic alias chain"),
        ));
        raw.remove(name);
    }

    // The survivors are acyclic; expand each target to alias-free form.
    let expanded: BTreeMap<String, TypeExpr> = raw
        .keys()
        .map(|name| (name.clone(), expand_in(&raw, &raw[name])))
        .collect();
    expanded
}

/// Whether alias `start` can reach itself by following alias-name references.
fn reaches_itself(start: &str, table: &BTreeMap<String, TypeExpr>) -> bool {
    let mut stack: Vec<&str> = Vec::new();
    referenced_names(&table[start], &mut |name| {
        if table.contains_key(name) {
            stack.push(name);
        }
    });
    let mut visited: Vec<&str> = Vec::new();
    while let Some(node) = stack.pop() {
        if node == start {
            return true;
        }
        if visited.contains(&node) {
            continue;
        }
        visited.push(node);
        if let Some(target) = table.get(node) {
            referenced_names(target, &mut |name| {
                if table.contains_key(name) {
                    stack.push(name);
                }
            });
        }
    }
    false
}

/// Visit every type name a target mentions. `referenced_names` and `expand_in`
/// walk the same structure; keeping the traversal here keeps them aligned.
fn referenced_names<'t>(ty: &'t TypeExpr, visit: &mut impl FnMut(&'t str)) {
    match ty {
        TypeExpr::Name { text, .. } => visit(text),
        TypeExpr::Optional { inner, .. } => referenced_names(inner, visit),
        TypeExpr::Sequence { element, .. } => referenced_names(element, visit),
        TypeExpr::Identity(_) => {}
    }
}

/// Expand a target over an acyclic raw table (build-time twin of
/// [`TypeRegistry::expand`], which reads the finished alias-free map).
fn expand_in(table: &BTreeMap<String, TypeExpr>, ty: &TypeExpr) -> TypeExpr {
    match ty {
        TypeExpr::Name { text, .. } => match table.get(text) {
            Some(target) => expand_in(table, target),
            None => ty.clone(),
        },
        TypeExpr::Optional { inner, span } => TypeExpr::Optional {
            inner: Box::new(expand_in(table, inner)),
            span: *span,
        },
        TypeExpr::Sequence { element, span } => TypeExpr::Sequence {
            element: Box::new(expand_in(table, element)),
            span: *span,
        },
        TypeExpr::Identity(_) => ty.clone(),
    }
}

/// Every alias must denote a known type, used or not: its expansion is a scalar,
/// the record type, or one optional over either. An unknown name is a
/// `check.type` at the alias; a well-formed but unadmitted shape is a
/// `check.unsupported`.
fn validate_alias_targets(
    registry: &TypeRegistry,
    aliases: &[(String, &AliasDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    for (file, decl) in aliases {
        let Some(expanded) = registry.aliases.get(&decl.name) else {
            continue; // duplicate or cyclic: already reported
        };
        let head = match expanded {
            TypeExpr::Optional { inner, .. } => inner.as_ref(),
            other => other,
        };
        match head {
            TypeExpr::Name { text, .. } => {
                if ScalarType::from_spelling(text).is_none()
                    && registry.by_name(text).is_none()
                    && registry.nominal_by_name(text).is_none()
                    && registry.struct_by_name(text).is_none()
                    && registry.enum_by_name(text).is_none()
                {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        file,
                        decl.span,
                        format!("alias `{}` does not name a known type: `{text}`", decl.name),
                    ));
                }
            }
            _ => diagnostics.push(unsupported(
                file,
                decl.span,
                &format!("the target type of alias `{}`", decl.name),
            )),
        }
    }
}

/// Resolve the nominal type declarations against the aliases already installed
/// in `registry`. A name collision with an alias, resource, or earlier nominal
/// is a `check.name_conflict`; a base that does not expand to `int` is a
/// `check.unsupported`; a non-literal, stepped, or empty interval is a
/// `check.type`; the capability list must draw from the closed set without
/// repeats. A declaration with a defect is dropped whole rather than admitted
/// half-checked.
#[allow(clippy::too_many_arguments)]
fn build_nominals(
    registry: &TypeRegistry,
    nominals: &[(String, &NominalDecl)],
    resources: &[(String, &ResourceDecl)],
    structs: &[(String, &StructDecl)],
    enums: &[(String, &EnumDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<NominalInfo> {
    let mut built: Vec<NominalInfo> = Vec::new();
    for (file, decl) in nominals {
        // A parse error blocks compilation before this runs; a missing piece
        // only means the declaration itself was reported, so skip it quietly.
        let (Some(base), Some(interval)) = (&decl.base, &decl.interval) else {
            continue;
        };
        // Scalar spellings are keywords the parser already rejects as names;
        // owning them here keeps the conflict predicate self-contained.
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || resources
                .iter()
                .any(|(_, resource)| resource.name == decl.name)
            || structs.iter().any(|(_, item)| item.name == decl.name)
            || enums.iter().any(|(_, item)| item.name == decl.name)
            || built.iter().any(|info| info.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        match scalar_of(&registry.expand(base)) {
            Some(ScalarType::Int) => {}
            Some(other) => {
                diagnostics.push(unsupported(
                    file,
                    base.span(),
                    &format!("a nominal type over `{}`", other.spelling()),
                ));
                continue;
            }
            None => {
                diagnostics.push(unsupported(file, base.span(), "this nominal base type"));
                continue;
            }
        }
        let Some((lo, hi)) = nominal_interval(file, interval, diagnostics) else {
            continue;
        };
        let Some(supports) = support_set(file, decl, diagnostics) else {
            continue;
        };
        built.push(NominalInfo {
            name: decl.name.clone(),
            lo,
            hi,
            supports,
        });
    }
    built
}

/// Evaluate a nominal `in` range to its inclusive `[lo, hi]` bounds. The range
/// follows the language's range operators — `lo..hi` excludes the end, `lo..=hi`
/// includes it — with int-literal bounds (a leading `-` allowed), no step, and
/// at least one admitted value.
fn nominal_interval(
    file: &str,
    interval: &Expression,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(i64, i64)> {
    let error = |diagnostics: &mut Vec<SourceDiagnostic>, span, message: &str| {
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            file,
            span,
            message.to_string(),
        ));
        None
    };
    let Some(range) = range_expr(interval) else {
        return error(
            diagnostics,
            interval.span(),
            "a nominal interval is a range of int literals, such as `0..150`",
        );
    };
    if range.step.is_some() {
        return error(diagnostics, range.span, "a nominal interval takes no step");
    }
    let (Some(start), Some(end)) = (range.start, range.end) else {
        return error(
            diagnostics,
            range.span,
            "a nominal interval needs both bounds",
        );
    };
    let (Some(lo), Some(end_value)) = (literal_int(start), literal_int(end)) else {
        return error(
            diagnostics,
            range.span,
            "a nominal interval's bounds are int literals",
        );
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
        Some(hi) if lo <= hi => Some((lo, hi)),
        _ => error(diagnostics, range.span, "this interval admits no values"),
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
    file: &str,
    decl: &NominalDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<SupportSet> {
    let mut supports = SupportSet::default();
    for spelling in &decl.supports {
        let flag = match spelling.name.as_str() {
            "add" => &mut supports.add,
            "subtract" => &mut supports.subtract,
            "step" => &mut supports.step,
            "scale" => &mut supports.scale,
            other => {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    spelling.span,
                    format!(
                        "unknown capability `{other}`; the capabilities are add, subtract, step, scale"
                    ),
                ));
                return None;
            }
        };
        if *flag {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                spelling.span,
                format!("capability `{}` is repeated", spelling.name),
            ));
            return None;
        }
        *flag = true;
    }
    Some(supports)
}

/// Build the dense struct types. Each becomes an image [`RecordTypeDef`] whose
/// every field is required, resolving field annotations through the aliases
/// already installed in `registry`. A name collision with a scalar, alias,
/// nominal, resource, or earlier struct is a `check.name_conflict`; a group,
/// keyed field, the `required` keyword, an optional field type, or a non-scalar
/// field type is `check.unsupported` (a struct field is the bare `name: Type`
/// form over a scalar). A declaration with a defect is dropped whole.
fn build_structs(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    structs: &[(String, &StructDecl)],
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<StructInfo> {
    let mut built: Vec<StructInfo> = Vec::new();
    for (file, decl) in structs {
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, resource)| resource.name == decl.name)
            || built.iter().any(|info| info.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let Some((fields, field_defs)) = struct_fields(draft, registry, file, decl, diagnostics)
        else {
            continue;
        };
        let name_id = draft.intern_string(&decl.name);
        let type_id = draft.add_record_type(RecordTypeDef {
            name: name_id,
            fields: field_defs,
        });
        built.push(StructInfo {
            type_id,
            name: decl.name.clone(),
            fields,
        });
    }
    built
}

/// Resolve a struct's members to its required scalar fields and their image
/// definitions, or `None` if any member is not the bare `name: scalar` form.
fn struct_fields(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    file: &str,
    decl: &StructDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(Vec<FieldInfo>, Vec<FieldDef>)> {
    let mut fields = Vec::new();
    let mut field_defs = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        let ResourceMember::Field(field) = member else {
            diagnostics.push(unsupported(file, member.span(), "a struct group"));
            ok = false;
            continue;
        };
        if !field.keys.is_empty() {
            diagnostics.push(unsupported(file, field.span, "a keyed struct field"));
            ok = false;
            continue;
        }
        if field.required {
            diagnostics.push(unsupported(
                file,
                field.span,
                "the `required` keyword on a struct field (struct fields are always required)",
            ));
            ok = false;
            continue;
        }
        if matches!(registry.expand(&field.ty), TypeExpr::Optional { .. }) {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "an optional struct field type",
            ));
            ok = false;
            continue;
        }
        let Some(scalar) = scalar_of(&registry.expand(&field.ty)) else {
            diagnostics.push(unsupported(file, field.ty.span(), "this struct field type"));
            ok = false;
            continue;
        };
        let field_name_id = draft.intern_string(&field.name);
        field_defs.push(FieldDef {
            name: field_name_id,
            ty: scalar.image(),
            required: true,
        });
        fields.push(FieldInfo {
            name: field.name.clone(),
            scalar,
            required: true,
        });
    }
    ok.then_some((fields, field_defs))
}

/// Build the closed flat enum types. Each becomes an image [`EnumTypeDef`]. A name
/// collision with a scalar, alias, nominal, resource, struct, or earlier enum is a
/// `check.name_conflict`. Hierarchy is deferred: a `category` member or a member
/// with nested members is `check.unsupported`. A member's payload is the dense
/// `name: Type` form over bare scalars; an optional or non-scalar payload type is
/// `check.unsupported`. A declaration with a defect is dropped whole rather than
/// admitted half-checked, so a later match cannot resolve against a broken enum.
fn build_enums(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    enums: &[(String, &EnumDecl)],
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<EnumInfo> {
    let mut built: Vec<EnumInfo> = Vec::new();
    for (file, decl) in enums {
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || registry.struct_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, resource)| resource.name == decl.name)
            || built.iter().any(|info| info.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let Some((variants, variant_defs)) =
            enum_variants(draft, registry, file, decl, diagnostics)
        else {
            continue;
        };
        let name_id = draft.intern_string(&decl.name);
        let enum_id = draft.add_enum_type(EnumTypeDef {
            name: name_id,
            variants: variant_defs,
        });
        built.push(EnumInfo {
            enum_id,
            name: decl.name.clone(),
            variants,
        });
    }
    built
}

/// Resolve an enum's members to its selectable variants and their image
/// definitions, or `None` if any member is unsupported. On the flat line every
/// member is a leaf: a `category` member or one with nested members is deferred.
fn enum_variants(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    file: &str,
    decl: &EnumDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(Vec<VariantInfo>, Vec<VariantDef>)> {
    let mut variants = Vec::new();
    let mut variant_defs = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        if member.category {
            diagnostics.push(unsupported(
                file,
                member.span,
                "a `category` enum member (hierarchical enums are deferred)",
            ));
            ok = false;
            continue;
        }
        if !member.members.is_empty() {
            diagnostics.push(unsupported(
                file,
                member.span,
                "a nested enum member (hierarchical enums are deferred)",
            ));
            ok = false;
            continue;
        }
        if seen.contains(&member.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                member.name_span,
                format!("enum member `{}` is already declared", member.name),
            ));
            ok = false;
            continue;
        }
        seen.push(member.name.clone());
        let Some((payload, payload_scalars)) = enum_payload(registry, file, member, diagnostics)
        else {
            ok = false;
            continue;
        };
        let name_id = draft.intern_string(&member.name);
        variant_defs.push(VariantDef {
            name: name_id,
            category: false,
            payload: payload_scalars
                .iter()
                .map(|scalar| scalar.image())
                .collect(),
        });
        variants.push(VariantInfo {
            name: member.name.clone(),
            payload,
        });
    }
    ok.then_some((variants, variant_defs))
}

/// Resolve one member's payload fields to their scalars and info, or `None` when
/// a field is not the bare `name: scalar` form.
fn enum_payload(
    registry: &TypeRegistry,
    file: &str,
    member: &EnumMember,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(Vec<EnumPayloadInfo>, Vec<ScalarType>)> {
    let mut payload = Vec::new();
    let mut scalars = Vec::new();
    let mut ok = true;
    for field in &member.payload {
        if matches!(registry.expand(&field.ty), TypeExpr::Optional { .. }) {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "an optional enum payload field type",
            ));
            ok = false;
            continue;
        }
        let Some(scalar) = scalar_of(&registry.expand(&field.ty)) else {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "this enum payload field type",
            ));
            ok = false;
            continue;
        };
        payload.push(EnumPayloadInfo {
            name: field.name.clone(),
            scalar,
        });
        scalars.push(scalar);
    }
    ok.then_some((payload, scalars))
}

/// Build the one admitted record type, resolving field annotations through the
/// aliases already installed in `registry`.
fn build_record(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<RecordInfo> {
    if resources.len() > 1 {
        for (file, resource) in &resources[1..] {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                file,
                resource.name_span,
                "the beta line admits only one resource type per project".to_string(),
            ));
        }
    }
    let (file, resource) = resources.first()?;

    let mut fields = Vec::new();
    let mut field_defs = Vec::new();
    let name_id = draft.intern_string(&resource.name);
    for member in &resource.members {
        let ResourceMember::Field(field) = member else {
            diagnostics.push(unsupported(file, member.span(), "a resource group"));
            continue;
        };
        if !field.keys.is_empty() {
            diagnostics.push(unsupported(file, field.span, "a keyed field"));
            continue;
        }
        let Some(scalar) = scalar_of(&registry.expand(&field.ty)) else {
            diagnostics.push(unsupported(file, field.ty.span(), "this field type"));
            continue;
        };
        let field_name_id = draft.intern_string(&field.name);
        field_defs.push(FieldDef {
            name: field_name_id,
            ty: scalar.image(),
            required: field.required,
        });
        fields.push(FieldInfo {
            name: field.name.clone(),
            scalar,
            required: field.required,
        });
    }

    let type_id = draft.add_record_type(RecordTypeDef {
        name: name_id,
        fields: field_defs,
    });
    Some(RecordInfo {
        type_id,
        name: resource.name.clone(),
        fields,
    })
}

fn scalar_of(ty: &TypeExpr) -> Option<ScalarType> {
    match ty {
        TypeExpr::Name { text, .. } => ScalarType::from_spelling(text),
        _ => None,
    }
}

fn unsupported(file: &str, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}
