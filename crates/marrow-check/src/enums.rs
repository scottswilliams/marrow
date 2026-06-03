//! Enum resolution and `match` checking, plus cross-module named-type
//! normalization the call boundary relies on.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{MemberPathResolution, ResourceSchema, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::{Severity, SourceSpan};

use crate::checks::check_block_types;
use crate::infer::infer_type;
use crate::resolve::resolve_store_by_root;
use crate::typerules::marrow_type_name;
use crate::{
    CHECK_AMBIGUOUS_MATCH_ARM, CHECK_AMBIGUOUS_MEMBER, CHECK_DUPLICATE_MATCH_ARM,
    CHECK_IS_REQUIRES_ENUM, CHECK_IS_TYPE, CHECK_MATCH_REQUIRES_ENUM, CHECK_NONEXHAUSTIVE_MATCH,
    CHECK_PRIVATE_ENUM, CHECK_UNKNOWN_ENUM_MEMBER, CheckDiagnostic, CheckedModule, CheckedProgram,
    Def, DefItem, MarrowType, Resolution, ResolvableKind, TypeNames, build_alias_map, expand_alias,
    expand_module_alias, module_of_file, resolve, resource_type_name,
};

/// Re-resolve every named signature slot in the assembled program against the
/// whole project, so a parameter, return, or constant annotation carries its true
/// enum owner or store identity.
///
/// Each module's signatures are first resolved per-file against that module's own
/// names, which cannot place a qualified `mod::Status` or a bare name owned by
/// another module. This pass revisits those slots with the full program in hand —
/// the same `resolve_type` the in-body checks use — so cross-module enum and
/// resource annotations compare like for like at calls, returns, and constants.
pub(crate) fn normalize_program_named_types(
    program: &mut CheckedProgram,
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
) {
    let resolver = program.clone();
    normalize_program_named_types_against(program, &resolver, parsed_files);
}

/// As [`normalize_program_named_types`], but resolving against an explicit
/// `resolver` program. Test modules normalize against the combined project so a
/// named type a test file imports from a project module resolves to that module.
pub(crate) fn normalize_program_named_types_against(
    program: &mut CheckedProgram,
    resolver: &CheckedProgram,
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
) {
    for module in &mut program.modules {
        let Some((file, parsed)) = parsed_files
            .iter()
            .find(|(file, _)| file.path == module.source_file)
        else {
            continue;
        };
        // The file's import aliases, so an annotation qualified by a short alias
        // (`c::Status` under `use a::b::c`) resolves to the imported module — the
        // same expansion call dispatch applies. Built once, before the mutable
        // borrow of the module's functions and constants.
        let aliases = build_alias_map(&module.imports);
        for function in &mut module.functions {
            let Some(decl) = parsed.file.function(&function.name) else {
                continue;
            };
            for (param, param_decl) in function.params.iter_mut().zip(&decl.params) {
                param.ty = resolve_type(&param_decl.ty, resolver, &aliases, &file.path);
            }
            if let (Some(return_type), Some(return_ref)) =
                (function.return_type.as_mut(), decl.return_type.as_ref())
            {
                *return_type = resolve_type(return_ref, resolver, &aliases, &file.path);
            }
        }
        for constant in &mut module.constants {
            let Some(const_ref) =
                parsed
                    .file
                    .declarations
                    .iter()
                    .find_map(|declaration| match declaration {
                        marrow_syntax::Declaration::Const(decl) if decl.name == constant.name => {
                            decl.ty.as_ref()
                        }
                        _ => None,
                    })
            else {
                continue;
            };
            constant.ty = Some(resolve_type(const_ref, resolver, &aliases, &file.path));
        }
    }
}

pub(crate) struct MatchCheck<'a> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) file: &'a Path,
    pub(crate) return_type: &'a MarrowType,
    pub(crate) scrutinee: Option<&'a marrow_syntax::Expression>,
    pub(crate) arms: &'a [marrow_syntax::MatchArm],
    pub(crate) span: SourceSpan,
    pub(crate) scope: &'a mut Vec<HashMap<String, MarrowType>>,
    pub(crate) aliases: &'a HashMap<String, Vec<String>>,
    pub(crate) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

struct MatchEnv<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    return_type: &'a MarrowType,
    scope: &'a mut Vec<HashMap<String, MarrowType>>,
    aliases: &'a HashMap<String, Vec<String>>,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
}

/// Check a `match` statement over an enum scrutinee.
pub(crate) fn check_match(input: MatchCheck<'_>) {
    let MatchCheck {
        program,
        file,
        return_type,
        scrutinee,
        arms,
        span,
        scope,
        aliases,
        diagnostics,
    } = input;
    let mut env = MatchEnv {
        program,
        file,
        return_type,
        scope,
        aliases,
        diagnostics,
    };
    let scrutinee_type = scrutinee
        .map(|expr| {
            infer_type(
                env.program,
                expr,
                env.scope,
                env.aliases,
                env.file,
                env.diagnostics,
            )
        })
        .unwrap_or(MarrowType::Unknown);
    check_match_arm_bodies(&mut env, arms);

    let MarrowType::Enum {
        module: enum_module,
        name: enum_name,
    } = &scrutinee_type
    else {
        report_non_enum_match(&mut env, &scrutinee_type, span);
        return;
    };
    let Some(schema) = enum_schema_in(program, enum_module, enum_name) else {
        env.diagnostics.push(CheckDiagnostic {
            code: CHECK_MATCH_REQUIRES_ENUM,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "`match` requires an enum value, but the scrutinee's enum `{enum_name}` is not declared"
            ),
            span,
        });
        return;
    };

    check_match_coverage(&mut env, schema, enum_name, arms, span);
}

fn check_match_arm_bodies(env: &mut MatchEnv<'_>, arms: &[marrow_syntax::MatchArm]) {
    for arm in arms {
        check_block_types(
            env.program,
            env.file,
            env.return_type,
            &arm.block,
            env.scope,
            env.aliases,
            env.diagnostics,
        );
    }
}

fn report_non_enum_match(env: &mut MatchEnv<'_>, scrutinee_type: &MarrowType, span: SourceSpan) {
    if !matches!(scrutinee_type, MarrowType::Unknown | MarrowType::Invalid) {
        env.diagnostics.push(CheckDiagnostic {
            code: CHECK_MATCH_REQUIRES_ENUM,
            severity: Severity::Error,
            file: env.file.to_path_buf(),
            message: format!(
                "`match` requires an enum value, but the scrutinee is `{}`",
                marrow_type_name(scrutinee_type)
            ),
            span,
        });
    }
}

fn check_match_coverage(
    env: &mut MatchEnv<'_>,
    schema: &marrow_schema::EnumSchema,
    enum_name: &str,
    arms: &[marrow_syntax::MatchArm],
    span: SourceSpan,
) {
    let mut covered: Vec<usize> = Vec::new();
    let mut had_overlap = false;
    for arm in arms {
        let segments: Vec<&str> = arm.path.iter().map(String::as_str).collect();
        let arm_label = segments.join("::");
        let arm_ordinal = match schema.walk_member_path(&segments) {
            MemberPathResolution::Found(ordinal) => ordinal,
            MemberPathResolution::NotFound => {
                env.diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNKNOWN_ENUM_MEMBER,
                    severity: Severity::Error,
                    file: env.file.to_path_buf(),
                    message: format!("`{enum_name}` has no member `{arm_label}`"),
                    span: arm.span,
                });
                continue;
            }
            MemberPathResolution::Ambiguous(paths) => {
                env.diagnostics.push(CheckDiagnostic {
                    code: CHECK_AMBIGUOUS_MATCH_ARM,
                    severity: Severity::Error,
                    file: env.file.to_path_buf(),
                    message: format!(
                        "`{arm_label}` names more than one member of `{enum_name}`; qualify as {}",
                        join_or(&paths)
                    ),
                    span: arm.span,
                });
                continue;
            }
        };
        let arm_leaves: Vec<usize> = schema
            .subtree_ordinals(arm_ordinal)
            .filter(|&ordinal| schema.is_selectable_leaf(ordinal))
            .collect();
        if arm_leaves.iter().any(|leaf| covered.contains(leaf)) {
            env.diagnostics.push(CheckDiagnostic {
                code: CHECK_DUPLICATE_MATCH_ARM,
                severity: Severity::Error,
                file: env.file.to_path_buf(),
                message: format!("`match` has a duplicate arm for `{arm_label}`"),
                span: arm.span,
            });
            had_overlap = true;
            continue;
        }
        covered.extend(arm_leaves);
    }

    let missing: Vec<String> = schema
        .selectable_leaves()
        .filter(|ordinal| !covered.contains(ordinal))
        .map(|ordinal| schema.member_path(ordinal).join("::"))
        .collect();
    if !missing.is_empty() && !had_overlap {
        env.diagnostics.push(CheckDiagnostic {
            code: CHECK_NONEXHAUSTIVE_MATCH,
            severity: Severity::Error,
            file: env.file.to_path_buf(),
            message: format!(
                "`match` on `{enum_name}` does not cover {}",
                missing
                    .iter()
                    .map(|path| format!("`{path}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            span,
        });
    }
}

/// A member-path expression (`Cat::tiger::bengal` or `mod::Cat::tiger`) resolved
/// against the project's enums: the owning module and enum, plus the walk of the
/// member path relative to that enum. Returned by [`resolve_enum_member_path`] for
/// both the value position and the `is` right operand, so the one place that
/// splits the enum prefix and walks the member tree is shared.
pub(crate) struct ResolvedMemberPath<'p> {
    pub module: String,
    pub enum_name: String,
    pub schema: &'p marrow_schema::EnumSchema,
    pub private: Option<String>,
    /// The walk of the member segments after the enum, by the schema's shared
    /// member-path walk. Each caller applies its own position rule (a value rejects
    /// a category; an `is` operand admits one) and reports ambiguity the same way.
    pub member: MemberPathResolution,
}

/// Resolve a `Cat::tiger::bengal` / `mod::Cat::tiger` member-path expression: split
/// the longest enum prefix (the enum is the segment before the member path, the
/// rest is the path), resolve that enum (same-module first, then aliased module,
/// then project-wide), and walk the remaining segments down its member tree by the
/// schema's shared walk. `None` when the expression is not a member-path of a known
/// enum (too few segments or no such enum); the member walk itself may still be
/// [`MemberPathResolution::NotFound`] or `Ambiguous`, left to the caller.
pub(crate) fn resolve_enum_member_path<'p>(
    program: &'p CheckedProgram,
    expr: &marrow_syntax::Expression,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<ResolvedMemberPath<'p>> {
    let marrow_syntax::Expression::Name { segments, .. } = expr else {
        return None;
    };
    if segments.len() < 2 {
        return None;
    }
    // Find the enum by the longest prefix that names a visible enum, leaving at
    // least one segment for the member path. A bare `Enum::a::b` takes `segments[0]`
    // as a same-module enum; a qualified `mod::Enum::a::b` takes `mod`'s `Enum`.
    let referencing = module_of_file(program, file);
    if let Some((module, schema, private)) =
        resolve_enum_with_visibility(program, referencing, &segments[0])
            .map(|(m, s, p)| (m.to_string(), s, p))
    {
        let path: Vec<&str> = segments[1..].iter().map(String::as_str).collect();
        return Some(ResolvedMemberPath {
            member: schema.walk_member_path(&path),
            module,
            enum_name: segments[0].clone(),
            schema,
            private,
        });
    }
    // The qualified case: the enum name sits at some index, its module is every
    // segment before it, and the member path is everything after. Prefer the
    // longest enum prefix (shortest member path), so scan the split points from the
    // end down — the first that resolves to a known enum wins.
    for enum_index in (1..segments.len() - 1).rev() {
        let module = expand_module_alias(&segments[..enum_index].join("::"), aliases);
        if let Some(schema) = enum_schema_in(program, &module, &segments[enum_index]) {
            let private =
                (!enum_visible_from(program, referencing, &module, &segments[enum_index]))
                    .then(|| format!("{module}::{}", segments[enum_index]));
            let path: Vec<&str> = segments[enum_index + 1..]
                .iter()
                .map(String::as_str)
                .collect();
            return Some(ResolvedMemberPath {
                member: schema.walk_member_path(&path),
                module,
                enum_name: segments[enum_index].clone(),
                schema,
                private,
            });
        }
    }
    None
}

/// Join member paths into an actionable "qualify as `a` or `b`" hint, each path
/// quoted. One path drops the "or" (it should never arise for an ambiguity, but
/// the join is total).
pub(crate) fn join_or(paths: &[String]) -> String {
    let quoted: Vec<String> = paths.iter().map(|path| format!("`{path}`")).collect();
    match quoted.as_slice() {
        [one] => one.clone(),
        [head @ .., last] => format!("{} or {last}", head.join(", ")),
        [] => String::new(),
    }
}

pub(crate) struct IsCheck<'a> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) left_type: &'a MarrowType,
    pub(crate) right: &'a marrow_syntax::Expression,
    pub(crate) aliases: &'a HashMap<String, Vec<String>>,
    pub(crate) span: SourceSpan,
    pub(crate) file: &'a Path,
    pub(crate) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

/// Type-check `left is right`, Marrow's nominal enum-subtree predicate.
pub(crate) fn check_is(input: IsCheck<'_>) -> MarrowType {
    let IsCheck {
        program,
        left_type,
        right,
        aliases,
        span,
        file,
        diagnostics,
    } = input;
    let bool_type = MarrowType::Primitive(ScalarType::Bool);
    let MarrowType::Enum {
        module: left_module,
        name: left_name,
    } = left_type
    else {
        // An untyped left operand defers (an unchecked dynamic value), like the
        // equality path; a known non-enum is rejected.
        if !matches!(left_type, MarrowType::Unknown) {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_IS_REQUIRES_ENUM,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "operator `is` requires an enum value on the left, but found `{}`",
                    marrow_type_name(left_type)
                ),
                span,
            });
        }
        return bool_type;
    };
    let Some(resolved) = resolve_enum_member_path(program, right, aliases, file) else {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_IS_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!("operator `is` requires a member of `{left_name}` on the right"),
            span,
        });
        return bool_type;
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
        });
        return bool_type;
    }
    // Both sides must name the same enum, by owning module and name, so two
    // same-named enums in different modules never alias.
    if &resolved.module != left_module || &resolved.enum_name != left_name {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_IS_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "operator `is` compares within one enum, but the left is `{left_name}` and the right names `{}`",
                resolved.enum_name
            ),
            span,
        });
        return bool_type;
    }
    // The right operand is a member path of the left's enum. As an `is` operand any
    // member is valid (a leaf is exact, a category a subtree), so only an unresolved
    // or ambiguous path is an error. A bare name duplicated under several parents is
    // rejected with the qualifying paths — the symmetric fix to the value footgun.
    match resolved.member {
        MemberPathResolution::Found(_) => {}
        MemberPathResolution::Ambiguous(paths) => diagnostics.push(CheckDiagnostic {
            code: CHECK_AMBIGUOUS_MEMBER,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "`{}` names more than one member of `{left_name}`; qualify as {}",
                member_path_label(right),
                join_or(&paths)
            ),
            span,
        }),
        MemberPathResolution::NotFound => diagnostics.push(CheckDiagnostic {
            code: CHECK_IS_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!("operator `is` requires a member of `{left_name}` on the right"),
            span,
        }),
    }
    bool_type
}

/// The member-path segments of a `Name` expression rendered as written, for a
/// diagnostic that quotes the offending path. A non-name expression renders empty.
fn member_path_label(expr: &marrow_syntax::Expression) -> String {
    match expr {
        marrow_syntax::Expression::Name { segments, .. } => segments.join("::"),
        _ => String::new(),
    }
}

/// Resolve a bare enum `name` referenced from `referencing_module`, returning the
/// owning module's qualified name and its schema. The referencing module's own
/// enum wins; otherwise the first project-wide match, mirroring how a bare
/// function name resolves (same-module declarations before the rest). A
/// module-less or unknown referencing module (`None`) has only the project-wide
/// fallback.
pub(crate) fn resolve_enum<'p>(
    program: &'p CheckedProgram,
    referencing_module: Option<&'p str>,
    name: &str,
) -> Option<(&'p str, &'p marrow_schema::EnumSchema)> {
    resolve_enum_with_visibility(program, referencing_module, name)
        .and_then(|(module, schema, private)| private.is_none().then_some((module, schema)))
}

fn resolve_enum_with_visibility<'p>(
    program: &'p CheckedProgram,
    referencing_module: Option<&'p str>,
    name: &str,
) -> Option<(&'p str, &'p marrow_schema::EnumSchema, Option<String>)> {
    referencing_module
        .and_then(|module| enum_schema_in(program, module, name).map(|schema| (module, schema)))
        .map(|(module, schema)| (module, schema, None))
        .or_else(|| find_project_enum(program, referencing_module, name, true))
        .or_else(|| find_project_enum(program, referencing_module, name, false))
}

fn find_project_enum<'p>(
    program: &'p CheckedProgram,
    referencing_module: Option<&str>,
    name: &str,
    public: bool,
) -> Option<(&'p str, &'p marrow_schema::EnumSchema, Option<String>)> {
    program.modules.iter().find_map(|module| {
        if Some(module.name.as_str()) == referencing_module || module.name.is_empty() {
            return None;
        }
        let schema = module
            .enums
            .iter()
            .find(|enum_schema| enum_schema.name == name)?;
        let is_public = enum_is_public(module, name);
        if is_public != public {
            return None;
        }
        Some((
            module.name.as_str(),
            schema,
            (!is_public).then(|| format!("{}::{name}", module.name)),
        ))
    })
}

fn enum_visible_from(
    program: &CheckedProgram,
    referencing_module: Option<&str>,
    enum_module: &str,
    enum_name: &str,
) -> bool {
    referencing_module == Some(enum_module)
        || program
            .modules
            .iter()
            .find(|module| module.name == enum_module)
            .is_none_or(|module| enum_is_public(module, enum_name))
}

fn enum_is_public(module: &CheckedModule, enum_name: &str) -> bool {
    module.enum_public.get(enum_name).copied().unwrap_or(true)
}

/// The schema of the enum named `name` owned by exactly `module`, if any. Used
/// once an enum's owning module is already resolved (a typed scrutinee or value),
/// so the lookup is by exact identity rather than a fresh name resolution.
pub(crate) fn enum_schema_in<'p>(
    program: &'p CheckedProgram,
    module: &str,
    name: &str,
) -> Option<&'p marrow_schema::EnumSchema> {
    program
        .modules
        .iter()
        .find(|m| m.name == module)?
        .enums
        .iter()
        .find(|enum_schema| enum_schema.name == name)
}

/// Resolve a type annotation against the project's named types.
///
/// Resource annotations resolve first through the module-aware checked resolver.
/// If no resource is named, enum annotations resolve by their true owner: a bare
/// `Status` resolves same-module-first then to the project-wide owner (the
/// symmetry a bare `Status::member` literal already uses), and a qualified
/// `mod::Status` resolves to `mod`'s enum when `mod` declares it.
pub(crate) fn resolve_type(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> MarrowType {
    if let Some(resource_type) = resolve_resource_annotation(ty, program, aliases, file) {
        return resource_type;
    }
    if let Some(enum_type) = resolve_enum_annotation(ty, program, aliases, file) {
        return enum_type;
    }
    MarrowType::resolve(
        ty,
        TypeNames {
            module: module_of_file(program, file).unwrap_or_default(),
            enums: &[],
        },
    )
}

/// Resolve a resource or store-identity type annotation to the checker type.
/// Qualified resource spellings use the same import-alias expansion as calls, so
/// an alias can name a module without minting a second nominal type.
fn resolve_resource_annotation(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<MarrowType> {
    resolve_resource_type(&Type::resolve(ty), program, aliases, file)
}

pub(crate) fn resolve_resource_type(
    ty: &Type,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<MarrowType> {
    match ty {
        Type::Sequence(element) => resolve_resource_type(element, program, aliases, file)
            .map(|element_type| MarrowType::Sequence(Box::new(element_type))),
        Type::Identity(store_root) => resolve_store_by_root(program, store_root)
            .map(|_| MarrowType::Identity(store_root.clone())),
        Type::Named(name) => {
            let segments = split_type_path(name);
            resolve_resource_path(program, aliases, file, &segments, ResolvableKind::Resource).map(
                |(resource, module)| {
                    MarrowType::Resource(resource_type_name(module, &resource.name))
                },
            )
        }
        _ => None,
    }
}

fn split_type_path(path: &str) -> Vec<String> {
    path.split("::").map(str::to_string).collect()
}

fn resolve_resource_path<'p>(
    program: &'p CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    segments: &[String],
    kind: ResolvableKind,
) -> Option<(&'p ResourceSchema, &'p str)> {
    match resolve(
        program,
        module_of_file(program, file).unwrap_or_default(),
        &expand_alias(segments, aliases),
        kind,
    ) {
        Resolution::Found(Def {
            module,
            item: DefItem::Resource(resource),
            ..
        }) => Some((resource, module.name.as_str())),
        _ => None,
    }
}

/// Resolve an enum type annotation to its `Enum { module, name }` identity by the
/// enum's true owner, or `None` when the annotation is not (or does not contain) an
/// enum. A qualified `mod::Name` names `mod`'s enum `Name`; a bare `Name` resolves
/// the way a bare `Name::member` literal does — the referencing module's enum first,
/// then the project-wide owner — so an annotation and a value spelled the same name
/// the same enum. A `sequence[...]` recurses on its element: `sequence[Status]`
/// resolves to `Sequence(Enum { … })` so an enum element keeps its owner, and an
/// element that is not an enum leaves the whole sequence to the structural resolver.
pub(crate) fn resolve_enum_annotation(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<MarrowType> {
    resolve_enum_type(&Type::resolve(ty), program, aliases, file)
}

/// Resolve an already-structured [`Type`] to its enum identity, recursing through
/// `sequence[...]`. Returns `None` for any type with no enum inside, so a non-enum
/// element keeps a sequence on the structural-resolver path.
pub(crate) fn resolve_enum_type(
    ty: &Type,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<MarrowType> {
    match ty {
        Type::Sequence(element) => resolve_enum_type(element, program, aliases, file)
            .map(|element_type| MarrowType::Sequence(Box::new(element_type))),
        Type::Named(name) => {
            // Split on the *last* `::` so a nested module keeps all but the final
            // segment: `a::b::Status` names module `a::b`'s enum `Status`, not
            // module `a`'s `b::Status` (which matches nothing, leaving the slot
            // `Unknown` and every boundary failing open).
            if let Some((module, enum_name)) = name.rsplit_once("::") {
                // Expand a short module alias (`c::Status` under `use a::b::c`)
                // through the file's imports first, mirroring call dispatch, so an
                // aliased annotation resolves to the imported module's enum instead
                // of failing open. A non-alias prefix passes through unchanged.
                let module = expand_module_alias(module, aliases);
                return enum_schema_in(program, &module, enum_name).map(|_| {
                    if enum_visible_from(program, module_of_file(program, file), &module, enum_name)
                    {
                        MarrowType::Enum {
                            module,
                            name: enum_name.to_string(),
                        }
                    } else {
                        MarrowType::Invalid
                    }
                });
            }
            resolve_enum_with_visibility(program, module_of_file(program, file), name).map(
                |(module, _, private)| {
                    if private.is_some() {
                        MarrowType::Invalid
                    } else {
                        MarrowType::Enum {
                            module: module.to_string(),
                            name: name.clone(),
                        }
                    }
                },
            )
        }
        _ => None,
    }
}

pub(crate) fn private_enum_type_reference(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<String> {
    private_enum_type(&Type::resolve(ty), program, aliases, file)
}

fn private_enum_type(
    ty: &Type,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<String> {
    match ty {
        Type::Sequence(element) => private_enum_type(element, program, aliases, file),
        Type::Named(name) => {
            if let Some((module, enum_name)) = name.rsplit_once("::") {
                let module = expand_module_alias(module, aliases);
                return enum_schema_in(program, &module, enum_name).and_then(|_| {
                    (!enum_visible_from(program, module_of_file(program, file), &module, enum_name))
                        .then(|| format!("{module}::{enum_name}"))
                });
            }
            resolve_enum_with_visibility(program, module_of_file(program, file), name)
                .and_then(|(_, _, private)| private)
        }
        _ => None,
    }
}
