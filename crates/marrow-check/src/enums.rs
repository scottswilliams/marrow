//! Enum resolution and `match` checking, plus cross-module named-type
//! normalization the call boundary relies on.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{MemberPathResolution, ResourceSchema, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::checks::check_block_types;
use crate::infer::infer_type;
use crate::resolve::resolve_store_by_root;
use crate::typerules::marrow_type_name;
use crate::{
    CHECK_AMBIGUOUS_MATCH_ARM, CHECK_AMBIGUOUS_MEMBER, CHECK_DUPLICATE_MATCH_ARM,
    CHECK_IS_REQUIRES_ENUM, CHECK_IS_TYPE, CHECK_MATCH_REQUIRES_ENUM, CHECK_NONEXHAUSTIVE_MATCH,
    CHECK_PRIVATE_ENUM, CHECK_UNKNOWN_ENUM_MEMBER, CHECK_UNKNOWN_TYPE, CheckDiagnostic,
    CheckedModule, CheckedProgram, Def, DefItem, DiagnosticPayload, EnumDiagnostic, MarrowType,
    Resolution, ResolvableKind, TypeNames, build_alias_map, expand_alias, expand_module_alias,
    module_of_file, resolve, resource_type_name, split_type_path,
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
    let plan = plan_normalized_named_types(program, parsed_files);
    apply_normalized_named_types(program, plan);
}

pub(crate) fn annotation_type_known(schema_type: &Type, resolved_type: &MarrowType) -> bool {
    match (schema_type, resolved_type) {
        (Type::Unknown, _) => true,
        (Type::Sequence(schema_element), MarrowType::Sequence(resolved_element)) => {
            annotation_type_known(schema_element, resolved_element)
        }
        (_, MarrowType::Unknown) => false,
        _ => true,
    }
}

pub(crate) fn annotation_unknown_identity_name(
    ty: &Type,
    program: &CheckedProgram,
) -> Option<String> {
    match ty {
        Type::Identity(identity) if resolve_store_by_root(program, identity).is_none() => {
            Some(format!("Id(^{identity})"))
        }
        Type::Identity(_) => None,
        Type::Sequence(element) => annotation_unknown_identity_name(element, program),
        Type::Scalar(_) | Type::Named(_) | Type::Unknown => None,
    }
}

pub(crate) fn resolve_diagnosed_annotation_type(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> MarrowType {
    let schema_type = Type::resolve(ty);
    let resolved_type = resolve_type(ty, program, aliases, file);
    if annotation_unknown_identity_name(&schema_type, program).is_some()
        || !annotation_type_known(&schema_type, &resolved_type)
    {
        MarrowType::Invalid
    } else {
        resolved_type
    }
}

struct ModuleTypeNormalization {
    module_index: usize,
    functions: Vec<FunctionTypeNormalization>,
    constants: Vec<ConstantTypeNormalization>,
}

struct FunctionTypeNormalization {
    function_index: usize,
    params: Vec<MarrowType>,
    return_type: Option<MarrowType>,
}

struct ConstantTypeNormalization {
    constant_index: usize,
    ty: MarrowType,
}

fn plan_normalized_named_types(
    program: &CheckedProgram,
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
) -> Vec<ModuleTypeNormalization> {
    let mut plan = Vec::new();
    for (module_index, module) in program.modules.iter().enumerate() {
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
        // The checked functions zip positionally with the parse's function
        // declarations (one checked function per declaration, in source order); a
        // by-name lookup would re-resolve a duplicate-named function against the
        // first declaration's annotations.
        let function_decls =
            parsed
                .file
                .declarations
                .iter()
                .filter_map(|declaration| match declaration {
                    marrow_syntax::Declaration::Function(function) => Some(function),
                    _ => None,
                });
        let mut functions = Vec::new();
        for ((function_index, function), decl) in
            module.functions.iter().enumerate().zip(function_decls)
        {
            let params = function
                .params
                .iter()
                .zip(&decl.params)
                .map(|(_, param_decl)| {
                    resolve_diagnosed_annotation_type(&param_decl.ty, program, &aliases, &file.path)
                })
                .collect();
            let return_type = match (function.return_type.as_ref(), decl.return_type.as_ref()) {
                (Some(_), Some(return_ref)) => Some(resolve_diagnosed_annotation_type(
                    return_ref, program, &aliases, &file.path,
                )),
                _ => None,
            };
            functions.push(FunctionTypeNormalization {
                function_index,
                params,
                return_type,
            });
        }
        let mut constants = Vec::new();
        for (constant_index, constant) in module.constants.iter().enumerate() {
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
            constants.push(ConstantTypeNormalization {
                constant_index,
                ty: resolve_diagnosed_annotation_type(const_ref, program, &aliases, &file.path),
            });
        }
        plan.push(ModuleTypeNormalization {
            module_index,
            functions,
            constants,
        });
    }
    plan
}

fn apply_normalized_named_types(program: &mut CheckedProgram, plan: Vec<ModuleTypeNormalization>) {
    for module_plan in plan {
        let Some(module) = program.modules.get_mut(module_plan.module_index) else {
            continue;
        };
        for function_plan in module_plan.functions {
            let Some(function) = module.functions.get_mut(function_plan.function_index) else {
                continue;
            };
            for (param, ty) in function.params.iter_mut().zip(function_plan.params) {
                param.ty = ty;
            }
            if let (Some(return_type), Some(ty)) =
                (function.return_type.as_mut(), function_plan.return_type)
            {
                *return_type = ty;
            }
        }
        for constant_plan in module_plan.constants {
            let Some(constant) = module.constants.get_mut(constant_plan.constant_index) else {
                continue;
            };
            constant.ty = Some(constant_plan.ty);
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
        env.diagnostics.push(CheckDiagnostic::error(
            CHECK_MATCH_REQUIRES_ENUM,
            file,
            span,
            format!(
                "`match` requires an enum value, but the scrutinee's enum `{enum_name}` is not declared"
            ),
        ));
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
        env.diagnostics.push(CheckDiagnostic::error(
            CHECK_MATCH_REQUIRES_ENUM,
            env.file,
            span,
            format!(
                "`match` requires an enum value, but the scrutinee is `{}`",
                marrow_type_name(scrutinee_type)
            ),
        ));
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
                env.diagnostics.push(
                    CheckDiagnostic::error(
                        CHECK_UNKNOWN_ENUM_MEMBER,
                        env.file,
                        arm.span,
                        format!("`{enum_name}` has no member `{arm_label}`"),
                    )
                    .with_payload(DiagnosticPayload::Enum(
                        EnumDiagnostic::UnknownMember {
                            enum_name: enum_name.to_string(),
                            member: arm_label,
                        },
                    )),
                );
                continue;
            }
            MemberPathResolution::Ambiguous(paths) => {
                env.diagnostics.push(
                    CheckDiagnostic::error(
                        CHECK_AMBIGUOUS_MATCH_ARM,
                        env.file,
                        arm.span,
                        format!(
                            "`{arm_label}` names more than one member of `{enum_name}`; qualify as {}",
                            join_or(&paths)
                        ),
                    )
                    .with_payload(DiagnosticPayload::Enum(EnumDiagnostic::AmbiguousMatchArm {
                        enum_name: enum_name.to_string(),
                        label: arm_label,
                        candidates: paths,
                    })),
                );
                continue;
            }
        };
        let arm_leaves: Vec<usize> = schema
            .subtree_ordinals(arm_ordinal)
            .filter(|&ordinal| schema.is_selectable_leaf(ordinal))
            .collect();
        if arm_leaves.iter().any(|leaf| covered.contains(leaf)) {
            env.diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_DUPLICATE_MATCH_ARM,
                    env.file,
                    arm.span,
                    format!("`match` has a duplicate arm for `{arm_label}`"),
                )
                .with_payload(DiagnosticPayload::Enum(
                    EnumDiagnostic::DuplicateMatchArm { label: arm_label },
                )),
            );
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
        env.diagnostics.push(
            CheckDiagnostic::error(
                CHECK_NONEXHAUSTIVE_MATCH,
                env.file,
                span,
                format!(
                    "`match` on `{enum_name}` does not cover {}",
                    missing
                        .iter()
                        .map(|path| format!("`{path}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_payload(DiagnosticPayload::Enum(
                EnumDiagnostic::NonexhaustiveMatch {
                    enum_name: enum_name.to_string(),
                    missing,
                },
            )),
        );
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
    pub member_label: String,
    pub schema: &'p marrow_schema::EnumSchema,
    pub private: Option<String>,
    /// The index of the enum segment within the original `Name` segments: `0` for a
    /// bare `Enum::a::b`, the split point for a qualified `mod::Enum::a::b`. The member
    /// path begins at `enum_index + 1`, so a consumer that needs the written member
    /// segments reads them without recomputing the prefix split.
    pub enum_index: usize,
    /// The walk of the member segments after the enum, by the schema's shared
    /// member-path walk. Each caller applies its own position rule (a value rejects
    /// a category; an `is` operand admits one) and reports ambiguity the same way.
    pub member: MemberPathResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AmbiguousEnumMemberPath {
    enum_name: String,
    member_label: String,
    candidates: Vec<String>,
}

impl AmbiguousEnumMemberPath {
    pub(crate) fn diagnostic(self, file: &Path, span: SourceSpan) -> CheckDiagnostic {
        let path = format!("{}::{}", self.enum_name, self.member_label);
        CheckDiagnostic::error(
            CHECK_AMBIGUOUS_MEMBER,
            file,
            span,
            format!(
                "`{path}` is ambiguous; qualify as {}",
                join_or(&self.candidates)
            ),
        )
        .with_payload(DiagnosticPayload::Enum(EnumDiagnostic::AmbiguousMember {
            enum_name: self.enum_name,
            label: self.member_label,
            candidates: self.candidates,
        }))
    }
}

pub(crate) enum EnumMemberPathResolution<'p> {
    Resolved(ResolvedMemberPath<'p>),
    AmbiguousBareForeignOwner(AmbiguousEnumMemberPath),
    MissingOrNonEnum,
}

/// Resolve a `Cat::tiger::bengal` / `mod::Cat::tiger` member-path expression: split
/// the longest enum prefix (the enum is the segment before the member path, the
/// rest is the path), resolve that enum (same-module first, then aliased module,
/// then project-wide), and walk the remaining segments down its member tree by the
/// schema's shared walk. A bare enum name exposed by several visible foreign
/// modules fails closed before any member is picked; otherwise the member walk
/// itself may still be [`MemberPathResolution::NotFound`] or `Ambiguous`, left to
/// the caller.
pub(crate) fn resolve_enum_member_path<'p>(
    program: &'p CheckedProgram,
    expr: &marrow_syntax::Expression,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> EnumMemberPathResolution<'p> {
    let marrow_syntax::Expression::Name { segments, .. } = expr else {
        return EnumMemberPathResolution::MissingOrNonEnum;
    };
    if segments.len() < 2 {
        return EnumMemberPathResolution::MissingOrNonEnum;
    }
    // Find the enum by the longest prefix that names a visible enum, leaving at
    // least one segment for the member path. A qualified `mod::Enum::a::b` takes
    // `mod`'s `Enum`; if no qualified prefix resolves, a bare `Enum::a::b` takes
    // `segments[0]` as the enum owner.
    if let Some(resolved) = resolve_qualified_enum_member_path(program, aliases, file, segments) {
        return resolved;
    }
    let bare_owner = resolve_named_enum_owner(&segments[0], program, aliases, file);
    resolve_bare_foreign_member_path(bare_owner, segments)
}

fn resolve_qualified_enum_member_path<'p>(
    program: &'p CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    segments: &[String],
) -> Option<EnumMemberPathResolution<'p>> {
    // The qualified case: the enum name sits at some index, its module is every
    // segment before it, and the member path is everything after. Prefer the
    // longest enum prefix (shortest member path), so scan the split points from the
    // end down — the first that resolves to a known enum wins.
    for enum_index in (1..segments.len() - 1).rev() {
        let owner_name = segments[..=enum_index].join("::");
        let EnumOwnerResolution::Found(owner) =
            resolve_named_enum_owner(&owner_name, program, aliases, file)
        else {
            continue;
        };
        return Some(resolved_member_path(owner, segments, enum_index));
    }
    None
}

fn resolve_bare_foreign_member_path<'p>(
    owner: EnumOwnerResolution<'p>,
    segments: &[String],
) -> EnumMemberPathResolution<'p> {
    match owner {
        EnumOwnerResolution::Found(owner) => resolved_member_path(owner, segments, 0),
        EnumOwnerResolution::AmbiguousBareForeign { name, candidates } => {
            let member_label = segments[1..].join("::");
            EnumMemberPathResolution::AmbiguousBareForeignOwner(AmbiguousEnumMemberPath {
                enum_name: name,
                candidates: candidates
                    .into_iter()
                    .map(|candidate| format!("{candidate}::{member_label}"))
                    .collect(),
                member_label,
            })
        }
        EnumOwnerResolution::MissingOrNonEnum => EnumMemberPathResolution::MissingOrNonEnum,
    }
}

fn resolved_member_path<'p>(
    owner: ResolvedEnumOwner<'p>,
    segments: &[String],
    enum_index: usize,
) -> EnumMemberPathResolution<'p> {
    let path: Vec<&str> = segments[enum_index + 1..]
        .iter()
        .map(String::as_str)
        .collect();
    EnumMemberPathResolution::Resolved(ResolvedMemberPath {
        member: owner.schema.walk_member_path(&path),
        module: owner.module,
        enum_name: owner.name,
        member_label: segments[enum_index + 1..].join("::"),
        schema: owner.schema,
        private: owner.private,
        enum_index,
    })
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

pub(crate) fn ambiguous_enum_annotation_diagnostic(
    file: &Path,
    span: SourceSpan,
    name: String,
    ty: Type,
) -> CheckDiagnostic {
    CheckDiagnostic::error(
        CHECK_UNKNOWN_TYPE,
        file,
        span,
        format!("type annotation `{name}` is ambiguous; qualify the enum name"),
    )
    .with_payload(DiagnosticPayload::AmbiguousType { ty, name })
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
            diagnostics.push(CheckDiagnostic::error(
                CHECK_IS_REQUIRES_ENUM,
                file,
                span,
                format!(
                    "operator `is` requires an enum value on the left, but found `{}`",
                    marrow_type_name(left_type)
                ),
            ));
        }
        return bool_type;
    };
    let resolved = match resolve_enum_member_path(program, right, aliases, file) {
        EnumMemberPathResolution::Resolved(resolved) => resolved,
        EnumMemberPathResolution::AmbiguousBareForeignOwner(ambiguous) => {
            diagnostics.push(ambiguous.diagnostic(file, span));
            return bool_type;
        }
        EnumMemberPathResolution::MissingOrNonEnum => {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_IS_TYPE,
                file,
                span,
                format!("operator `is` requires a member of `{left_name}` on the right"),
            ));
            return bool_type;
        }
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
        return bool_type;
    }
    // Both sides must name the same enum, by owning module and name, so two
    // same-named enums in different modules never alias.
    if &resolved.module != left_module || &resolved.enum_name != left_name {
        diagnostics.push(CheckDiagnostic::error(
            CHECK_IS_TYPE,
            file,
            span,
            format!(
                "operator `is` compares within one enum, but the left is `{left_name}` and the right names `{}`",
                resolved.enum_name
            ),
        ));
        return bool_type;
    }
    // The right operand is a member path of the left's enum. As an `is` operand any
    // member is valid (a leaf is exact, a category a subtree), so only an unresolved
    // or ambiguous path is an error. A bare name duplicated under several parents is
    // rejected with the qualifying paths — the symmetric fix to the value footgun.
    match resolved.member {
        MemberPathResolution::Found(_) => {}
        MemberPathResolution::Ambiguous(paths) => diagnostics.push(
            CheckDiagnostic::error(
                CHECK_AMBIGUOUS_MEMBER,
                file,
                span,
                format!(
                    "`{}` names more than one member of `{left_name}`; qualify as {}",
                    member_path_label(right),
                    join_or(&paths)
                ),
            )
            .with_payload(DiagnosticPayload::Enum(EnumDiagnostic::AmbiguousMember {
                enum_name: left_name.clone(),
                label: resolved.member_label,
                candidates: paths,
            })),
        ),
        MemberPathResolution::NotFound => diagnostics.push(CheckDiagnostic::error(
            CHECK_IS_TYPE,
            file,
            span,
            format!("operator `is` requires a member of `{left_name}` on the right"),
        )),
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

fn enum_visible_from_modules(
    modules: &[CheckedModule],
    referencing_module: Option<&str>,
    enum_module: &str,
    enum_name: &str,
) -> bool {
    referencing_module == Some(enum_module)
        || modules
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
    enum_schema_in_modules(&program.modules, module, name)
}

fn enum_schema_in_modules<'p>(
    modules: &'p [CheckedModule],
    module: &str,
    name: &str,
) -> Option<&'p marrow_schema::EnumSchema> {
    modules
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
/// `Status` resolves same-module-first, then to the sole visible foreign owner.
/// If several visible foreign modules expose that name, the annotation fails
/// closed instead of picking one. A qualified `mod::Status` resolves to `mod`'s
/// enum when `mod` declares it.
pub(crate) fn resolve_type(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> MarrowType {
    let schema_type = Type::resolve(ty);
    if let Some(resource_type) = resolve_resource_type_ref(&schema_type, program, aliases, file) {
        return resource_type;
    }
    match resolve_enum_annotation_type(&schema_type, program, aliases, file) {
        EnumAnnotationResolution::Visible(resolved) => resolved.ty,
        EnumAnnotationResolution::Private(_) => MarrowType::Invalid,
        EnumAnnotationResolution::AmbiguousBareForeign(_) => MarrowType::Unknown,
        EnumAnnotationResolution::MissingOrNonEnum => MarrowType::resolve(
            ty,
            TypeNames {
                module: module_of_file(program, file).unwrap_or_default(),
                enums: &[],
            },
        ),
    }
}

pub(crate) fn resolve_schema_type_for_module(
    ty: &Type,
    program: &CheckedProgram,
    module: &CheckedModule,
) -> MarrowType {
    let aliases = build_alias_map(&module.imports);
    if let Some(resource_type) =
        resolve_resource_type_ref_in_module(ty, program, &aliases, &module.name)
    {
        return resource_type;
    }
    match resolve_enum_annotation_type_for_module(ty, &program.modules, module) {
        EnumAnnotationResolution::Visible(resolved) => resolved.ty,
        EnumAnnotationResolution::Private(_) => MarrowType::Invalid,
        EnumAnnotationResolution::AmbiguousBareForeign(_) => MarrowType::Unknown,
        EnumAnnotationResolution::MissingOrNonEnum => MarrowType::from_resolved(
            ty.clone(),
            TypeNames {
                module: &module.name,
                enums: &[],
            },
        ),
    }
}

pub(crate) fn resolve_enum_annotation_type_for_module(
    ty: &Type,
    modules: &[CheckedModule],
    module: &CheckedModule,
) -> EnumAnnotationResolution {
    let aliases = build_alias_map(&module.imports);
    resolve_enum_annotation_type_in_modules(ty, modules, &aliases, Some(&module.name))
}

fn resolve_resource_type_ref(
    ty: &Type,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<MarrowType> {
    resolve_resource_type_ref_in_module(
        ty,
        program,
        aliases,
        module_of_file(program, file).unwrap_or_default(),
    )
}

fn resolve_resource_type_ref_in_module(
    ty: &Type,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    module_name: &str,
) -> Option<MarrowType> {
    match ty {
        Type::Sequence(element) => {
            resolve_resource_type_ref_in_module(element, program, aliases, module_name)
                .map(|element_type| MarrowType::Sequence(Box::new(element_type)))
        }
        Type::Identity(store_root) => resolve_store_by_root(program, store_root)
            .map(|_| MarrowType::Identity(store_root.clone())),
        Type::Named(name) => {
            let segments = split_type_path(name);
            resolve_resource_path_in_module(
                program,
                aliases,
                module_name,
                &segments,
                ResolvableKind::Resource,
            )
            .map(|(resource, module)| {
                MarrowType::Resource(resource_type_name(module, &resource.name))
            })
        }
        _ => None,
    }
}

fn resolve_resource_path_in_module<'p>(
    program: &'p CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    module_name: &str,
    segments: &[String],
    kind: ResolvableKind,
) -> Option<(&'p ResourceSchema, &'p str)> {
    match resolve(program, module_name, &expand_alias(segments, aliases), kind) {
        Resolution::Found(Def {
            module,
            item: DefItem::Resource(resource),
            ..
        }) => Some((resource, module.name.as_str())),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedEnumAnnotation {
    pub(crate) module: String,
    pub(crate) name: String,
    pub(crate) ty: MarrowType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnumAnnotationResolution {
    Visible(ResolvedEnumAnnotation),
    Private(String),
    AmbiguousBareForeign(String),
    MissingOrNonEnum,
}

pub(crate) fn resolve_enum_annotation(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> EnumAnnotationResolution {
    let schema_type = Type::resolve(ty);
    if resolve_resource_type_ref(&schema_type, program, aliases, file).is_some() {
        return EnumAnnotationResolution::MissingOrNonEnum;
    }
    resolve_enum_annotation_type(&schema_type, program, aliases, file)
}

fn visible_enum_annotation(
    module: String,
    name: String,
    ty: MarrowType,
) -> EnumAnnotationResolution {
    EnumAnnotationResolution::Visible(ResolvedEnumAnnotation { module, name, ty })
}

struct ResolvedEnumOwner<'p> {
    module: String,
    name: String,
    schema: &'p marrow_schema::EnumSchema,
    private: Option<String>,
}

enum EnumOwnerResolution<'p> {
    Found(ResolvedEnumOwner<'p>),
    AmbiguousBareForeign {
        name: String,
        candidates: Vec<String>,
    },
    MissingOrNonEnum,
}

fn resolve_enum_annotation_type(
    ty: &Type,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> EnumAnnotationResolution {
    resolve_enum_annotation_type_in_module(ty, program, aliases, module_of_file(program, file))
}

fn resolve_enum_annotation_type_in_module(
    ty: &Type,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    referencing: Option<&str>,
) -> EnumAnnotationResolution {
    resolve_enum_annotation_type_in_modules(ty, &program.modules, aliases, referencing)
}

fn resolve_enum_annotation_type_in_modules(
    ty: &Type,
    modules: &[CheckedModule],
    aliases: &HashMap<String, Vec<String>>,
    referencing: Option<&str>,
) -> EnumAnnotationResolution {
    match ty {
        Type::Sequence(element) => {
            match resolve_enum_annotation_type_in_modules(element, modules, aliases, referencing) {
                EnumAnnotationResolution::Visible(mut resolved) => {
                    resolved.ty = MarrowType::Sequence(Box::new(resolved.ty));
                    EnumAnnotationResolution::Visible(resolved)
                }
                other => other,
            }
        }
        Type::Named(name) => {
            resolve_named_enum_annotation_in_modules(name, modules, aliases, referencing)
        }
        _ => EnumAnnotationResolution::MissingOrNonEnum,
    }
}

fn resolve_named_enum_annotation_in_modules(
    name: &str,
    modules: &[CheckedModule],
    aliases: &HashMap<String, Vec<String>>,
    referencing: Option<&str>,
) -> EnumAnnotationResolution {
    match resolve_named_enum_owner_in_modules(name, modules, aliases, referencing) {
        EnumOwnerResolution::Found(owner) => match owner.private {
            Some(private) => EnumAnnotationResolution::Private(private),
            None => visible_enum_annotation(
                owner.module.clone(),
                owner.name.clone(),
                MarrowType::Enum {
                    module: owner.module,
                    name: owner.name,
                },
            ),
        },
        EnumOwnerResolution::AmbiguousBareForeign { name, .. } => {
            EnumAnnotationResolution::AmbiguousBareForeign(name)
        }
        EnumOwnerResolution::MissingOrNonEnum => EnumAnnotationResolution::MissingOrNonEnum,
    }
}

fn resolve_named_enum_owner<'p>(
    name: &str,
    program: &'p CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> EnumOwnerResolution<'p> {
    resolve_named_enum_owner_in_modules(
        name,
        &program.modules,
        aliases,
        module_of_file(program, file),
    )
}

fn resolve_named_enum_owner_in_modules<'p>(
    name: &str,
    modules: &'p [CheckedModule],
    aliases: &HashMap<String, Vec<String>>,
    referencing: Option<&str>,
) -> EnumOwnerResolution<'p> {
    if let Some((module, enum_name)) = name.rsplit_once("::") {
        let module = expand_module_alias(module, aliases);
        let Some(schema) = enum_schema_in_modules(modules, &module, enum_name) else {
            return EnumOwnerResolution::MissingOrNonEnum;
        };
        let private = (!enum_visible_from_modules(modules, referencing, &module, enum_name))
            .then(|| format!("{module}::{enum_name}"));
        return EnumOwnerResolution::Found(ResolvedEnumOwner {
            module,
            name: enum_name.to_string(),
            schema,
            private,
        });
    }

    if let Some(module) = referencing
        && let Some(schema) = enum_schema_in_modules(modules, module, name)
    {
        return EnumOwnerResolution::Found(ResolvedEnumOwner {
            module: module.to_string(),
            name: name.to_string(),
            schema,
            private: None,
        });
    }

    let public_candidates: Vec<_> = modules
        .iter()
        .filter_map(|module| {
            if Some(module.name.as_str()) == referencing || module.name.is_empty() {
                return None;
            }
            if !enum_is_public(module, name) {
                return None;
            }
            let schema = enum_schema_in_modules(modules, &module.name, name)?;
            Some((module.name.as_str(), schema))
        })
        .collect();
    match public_candidates.as_slice() {
        [(module, schema)] => EnumOwnerResolution::Found(ResolvedEnumOwner {
            module: (*module).to_string(),
            name: name.to_string(),
            schema,
            private: None,
        }),
        [_, _, ..] => EnumOwnerResolution::AmbiguousBareForeign {
            name: name.to_string(),
            candidates: public_candidates
                .iter()
                .map(|(module, _)| format!("{module}::{name}"))
                .collect(),
        },
        [] => resolve_private_bare_enum_owner(modules, referencing, name),
    }
}

fn resolve_private_bare_enum_owner<'p>(
    modules: &'p [CheckedModule],
    referencing: Option<&str>,
    name: &str,
) -> EnumOwnerResolution<'p> {
    let private_candidates: Vec<_> = modules
        .iter()
        .filter_map(|module| {
            if Some(module.name.as_str()) == referencing || module.name.is_empty() {
                return None;
            }
            if enum_is_public(module, name) {
                return None;
            }
            let schema = enum_schema_in_modules(modules, &module.name, name)?;
            Some((module.name.as_str(), schema))
        })
        .collect();

    match private_candidates.as_slice() {
        [(module, schema)] => EnumOwnerResolution::Found(ResolvedEnumOwner {
            module: (*module).to_string(),
            name: name.to_string(),
            schema,
            private: Some(format!("{module}::{name}")),
        }),
        [_, _, ..] => EnumOwnerResolution::AmbiguousBareForeign {
            name: name.to_string(),
            candidates: private_candidates
                .iter()
                .map(|(module, _)| format!("{module}::{name}"))
                .collect(),
        },
        [] => EnumOwnerResolution::MissingOrNonEnum,
    }
}
