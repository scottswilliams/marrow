//! Enum resolution and `match` checking, plus cross-module named-type
//! normalization the call boundary relies on.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{MemberPathResolution, ResourceSchema, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::checks::{ConstIntScope, check_block_types};
use crate::infer::infer_type_with_read_scope;
use crate::resolve::resolve_store_by_root;
use crate::typerules::marrow_type_name;
use crate::{
    CHECK_AMBIGUOUS_MATCH_ARM, CHECK_AMBIGUOUS_MEMBER, CHECK_DUPLICATE_MATCH_ARM,
    CHECK_IS_REQUIRES_ENUM, CHECK_IS_TYPE, CHECK_MATCH_REQUIRES_ENUM, CHECK_NONEXHAUSTIVE_MATCH,
    CHECK_PRIVATE_ENUM, CHECK_SCRUTINEE_QUALIFIED_MATCH_ARM, CHECK_UNKNOWN_ENUM_MEMBER,
    CHECK_UNKNOWN_TYPE, CheckDiagnostic, CheckedModule, CheckedProgram, Def, DefItem,
    DiagnosticPayload, EnumDiagnostic, MarrowType, Resolution, ResolvableKind, TypeNames,
    build_alias_map, expand_alias, expand_module_alias, module_of_file, resolve,
    resource_type_name, split_type_path,
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
        // An identity annotation is well formed only against a keyed store. A
        // missing root has no store, and a keyless singleton defines no identity
        // type at all — its root is addressed directly — so `Id(^singleton)` is
        // uninhabitable and rejected here, the same as an undeclared root.
        Type::Identity(identity)
            if resolve_store_by_root(program, identity)
                .is_none_or(|store| store.store.identity_keys.is_empty()) =>
        {
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
    // An uninhabitable `Id(^keyless-singleton)` is rejected with its own
    // diagnostic at the declaration site; here it keeps the store-rooted identity
    // type the store resolves to. Collapsing it to `Invalid` would drop the whole
    // signature's facts when they are later rebuilt without source annotations,
    // shrinking the fact set below its preserved prefix. The resolved type is
    // identical in both build paths, so the facts stay consistent.
    if annotation_type_known(&schema_type, &resolved_type) {
        resolved_type
    } else {
        MarrowType::Invalid
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
    let parsed_by_path: HashMap<&Path, &marrow_syntax::ParsedSource> = parsed_files
        .iter()
        .map(|(file, parsed)| (file.path.as_path(), parsed))
        .collect();
    let mut plan = Vec::new();
    for (module_index, module) in program.modules.iter().enumerate() {
        let Some(parsed) = parsed_by_path.get(module.source_file.as_path()).copied() else {
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
                    MarrowType::keyed(
                        param_decl.keys.iter().map(|key| {
                            resolve_diagnosed_annotation_type(
                                &key.ty,
                                program,
                                &aliases,
                                &module.source_file,
                            )
                        }),
                        resolve_diagnosed_annotation_type(
                            &param_decl.ty,
                            program,
                            &aliases,
                            &module.source_file,
                        ),
                    )
                })
                .collect();
            let return_type = match (function.return_type.as_ref(), decl.return_type.as_ref()) {
                (Some(_), Some(return_ref)) => Some(resolve_diagnosed_annotation_type(
                    return_ref,
                    program,
                    &aliases,
                    &module.source_file,
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
                ty: resolve_diagnosed_annotation_type(
                    const_ref,
                    program,
                    &aliases,
                    &module.source_file,
                ),
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
    pub(crate) const_ints: &'a mut ConstIntScope,
    pub(crate) aliases: &'a HashMap<String, Vec<String>>,
    pub(crate) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

struct MatchEnv<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    return_type: &'a MarrowType,
    scope: &'a mut Vec<HashMap<String, MarrowType>>,
    const_ints: &'a mut ConstIntScope,
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
        const_ints,
        aliases,
        diagnostics,
    } = input;
    let mut env = MatchEnv {
        program,
        file,
        return_type,
        scope,
        const_ints,
        aliases,
        diagnostics,
    };
    let scrutinee_type = scrutinee
        .map(|expr| {
            infer_type_with_read_scope(
                env.program,
                expr,
                env.scope,
                env.aliases,
                env.file,
                env.diagnostics,
                env.const_ints,
                None,
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
            env.const_ints,
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
        // An arm is a member path relative to the scrutinee enum, so writing the
        // enum's own name as a prefix is the redundant `Status::active` mistake —
        // unless the enum genuinely has a top-level member of that name, where the
        // prefix is a real member step and the path resolves normally.
        if let [head, relative @ ..] = segments.as_slice()
            && !relative.is_empty()
            && *head == enum_name
            && !is_top_level_member(schema, head)
        {
            let relative_label = relative.join("::");
            env.diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_SCRUTINEE_QUALIFIED_MATCH_ARM,
                    env.file,
                    arm.path_spans.first().copied().unwrap_or(arm.span),
                    format!(
                        "`match` arms are relative to the scrutinee enum `{enum_name}`; \
                         write the arm as `{relative_label}`, not `{arm_label}`"
                    ),
                )
                .with_payload(DiagnosticPayload::Enum(
                    EnumDiagnostic::ScrutineeQualifiedMatchArm {
                        enum_name: enum_name.to_string(),
                        relative: relative_label,
                    },
                )),
            );
            continue;
        }
        let arm_ordinal = match schema.walk_member_path(&segments) {
            MemberPathResolution::Found(ordinal) => ordinal,
            MemberPathResolution::NotFound => {
                let offending = unresolved_member_segment(schema, &arm.path);
                let segment_span = arm.path_spans.get(offending).copied().unwrap_or(arm.span);
                env.diagnostics.push(unknown_enum_member_diagnostic(
                    env.file,
                    segment_span,
                    enum_name,
                    schema,
                    &arm.path,
                    offending,
                ));
                continue;
            }
            MemberPathResolution::Ambiguous(matches) => {
                // Arms are scrutinee-relative, so a candidate keeps its bare member
                // path. The candidate that spells the rejected arm itself is dropped,
                // so a category sharing its descendant's name offers only the
                // resolvable deeper path rather than echoing the ambiguous arm.
                let candidates: Vec<String> = matches
                    .iter()
                    .map(|&ordinal| schema.member_path(ordinal).join("::"))
                    .filter(|candidate| *candidate != arm_label)
                    .collect();
                env.diagnostics.push(
                    CheckDiagnostic::error(
                        CHECK_AMBIGUOUS_MATCH_ARM,
                        env.file,
                        arm.span,
                        format!(
                            "`{arm_label}` names more than one member of `{enum_name}`; qualify as {}",
                            join_or(&candidates)
                        ),
                    )
                    .with_payload(DiagnosticPayload::Enum(EnumDiagnostic::AmbiguousMatchArm {
                        enum_name: enum_name.to_string(),
                        label: arm_label,
                        candidates,
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

impl ResolvedMemberPath<'_> {
    /// The original-segment index and name of the first written segment that breaks
    /// the member walk, for a [`MemberPathResolution::NotFound`] value-position path.
    /// Shifts the relative offending index by the enum prefix so the caller spans the
    /// right `Name` segment.
    pub(crate) fn unresolved_segment<'s>(&self, segments: &'s [String]) -> (usize, &'s str) {
        let relative = &segments[self.enum_index + 1..];
        let relative_index = unresolved_member_segment(self.schema, relative);
        let index = self.enum_index + 1 + relative_index;
        (index, segments[index].as_str())
    }

    /// The `check.unknown_enum_member` diagnostic for this value-position path, with the
    /// segment span the caller computed. Routes through the shared builder so the value
    /// position and `match` arms blame the same segment and offer the same valid forms.
    pub(crate) fn unknown_member_diagnostic(
        &self,
        file: &Path,
        segment_span: SourceSpan,
        segments: &[String],
        index: usize,
    ) -> CheckDiagnostic {
        let member_segments = &segments[self.enum_index + 1..];
        let offending = index - (self.enum_index + 1);
        unknown_enum_member_diagnostic(
            file,
            segment_span,
            &self.enum_name,
            self.schema,
            member_segments,
            offending,
        )
    }
}

/// The relative index of the first segment that breaks a `NotFound` member walk,
/// resolved by the schema's single downward walk over the borrowed segments.
fn unresolved_member_segment(schema: &marrow_schema::EnumSchema, relative: &[String]) -> usize {
    let segments: Vec<&str> = relative.iter().map(String::as_str).collect();
    schema.first_unresolved_segment(&segments)
}

/// Build the `check.unknown_enum_member` diagnostic for a member path whose written
/// segment at `offending` walks to no member. The single owner of the unknown-member
/// message, span, and payload, shared by the value position and `match` arms, so both
/// blame the same segment and offer the same valid forms.
fn unknown_enum_member_diagnostic(
    file: &Path,
    segment_span: SourceSpan,
    enum_name: &str,
    schema: &marrow_schema::EnumSchema,
    member_segments: &[String],
    offending: usize,
) -> CheckDiagnostic {
    let member = member_segments[offending].clone();
    let suggestions = valid_member_forms(schema, enum_name, &member_segments[offending..]);
    let mut message = format!("`{enum_name}` has no member `{member}`");
    if let [only] = suggestions.as_slice() {
        message.push_str(&format!("; did you mean `{only}`?"));
    } else if !suggestions.is_empty() {
        message.push_str(&format!("; did you mean {}?", join_or(&suggestions)));
    }
    CheckDiagnostic::error(CHECK_UNKNOWN_ENUM_MEMBER, file, segment_span, message).with_payload(
        DiagnosticPayload::Enum(EnumDiagnostic::UnknownMember {
            enum_name: enum_name.to_string(),
            member,
            suggestions,
        }),
    )
}

/// Valid full-path forms the written tail (`["cat", "tabby"]`, starting at the
/// offending segment) could have meant: the qualified path that reaches the same leaf
/// through the offending segment's real parent, and the bare leaf. A mid-path category
/// segment skips its parents, so reattaching the offending name to its true location
/// and the bare-leaf shorthand are the two ways to name the intended value. Forms are
/// returned in `Enum::…` spelling, qualified-first, with no duplicates.
fn valid_member_forms(
    schema: &marrow_schema::EnumSchema,
    enum_name: &str,
    tail: &[String],
) -> Vec<String> {
    let mut forms: Vec<String> = Vec::new();
    let mut push = |path: Vec<&str>| {
        let form = format!("{enum_name}::{}", path.join("::"));
        if !forms.contains(&form) {
            forms.push(form);
        }
    };
    let [head, rest @ ..] = tail else {
        return forms;
    };
    let rest_names: Vec<&str> = rest.iter().map(String::as_str).collect();
    for ordinal in 0..schema.members.len() {
        if schema.member_name(ordinal) != Some(head.as_str()) {
            continue;
        }
        let mut path: Vec<&str> = schema.member_path(ordinal);
        path.extend(rest_names.iter().copied());
        if let MemberPathResolution::Found(leaf) = schema.walk_member_path(&path)
            && schema.is_selectable_leaf(leaf)
        {
            push(schema.member_path(leaf));
        }
    }
    if let Some(last) = tail.last()
        && let MemberPathResolution::Found(leaf) = schema.walk_member_path(&[last.as_str()])
        && schema.is_selectable_leaf(leaf)
    {
        push(vec![last.as_str()]);
    }
    forms
}

/// Whether `name` is the schema's unique top-level member — the only valid start of
/// a qualified member path.
fn is_top_level_member(schema: &marrow_schema::EnumSchema, name: &str) -> bool {
    matches!(
        schema.walk_member_path(&[name]),
        MemberPathResolution::Found(ordinal) if schema.member_path(ordinal) == [name]
    )
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
    // longest enum prefix (shortest member path). Scan the split points from the
    // front, growing the already-expanded module prefix one segment at a time so the
    // whole scan is linear in the path length — rejoining and re-expanding the prefix
    // at every split point is quadratic, which a pathological many-segment path would
    // turn into a hang. Only the leading segment carries an import alias, so the
    // prefix is expanded once at the front and extended verbatim thereafter. The last
    // split that resolves owns the longest prefix.
    let referencing = module_of_file(program, file);
    let mut module_prefix = expand_module_alias(&segments[0], aliases);
    let mut owner = None;
    for enum_index in 1..segments.len() - 1 {
        if enum_index > 1 {
            module_prefix.push_str("::");
            module_prefix.push_str(&segments[enum_index - 1]);
        }
        if let EnumOwnerResolution::Found(found) = resolve_expanded_enum_owner_in_program(
            &module_prefix,
            &segments[enum_index],
            program,
            referencing,
        ) {
            owner = Some((found, enum_index));
        }
    }
    owner.map(|(owner, enum_index)| resolved_member_path(owner, segments, enum_index))
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

/// Build the `check.ambiguous_member` diagnostic for a bare duplicated name reached
/// in a value or `is`-RHS position. There a member is named by its full enum-qualified
/// path, so each candidate carries the enum prefix (`Cat::tiger::paw`) — the spelling
/// the checker confirms, unlike the scrutinee-relative `tiger::paw` that only resolves
/// in a `match` arm. `matches` are the traversal indices the ambiguous walk returned.
///
/// When `value_position`, a category candidate is dropped: a category cannot be
/// selected as a value, so only its selectable descendants are real fixes. The
/// candidate that spells the rejected input itself is always dropped, so a category
/// sharing its descendant leaf's name never offers a dead-end requalification. When
/// that leaves the value hint empty — every match is an unselectable category, or the
/// only selectable match spells the rejected input — it descends to the selectable
/// leaves under the matches, again excluding the rejected spelling, so the hint is the
/// real concrete values the rejected name groups and never the dead-end input itself.
pub(crate) fn ambiguous_member_value_diagnostic(
    file: &Path,
    span: SourceSpan,
    enum_name: &str,
    label: String,
    schema: &marrow_schema::EnumSchema,
    matches: &[usize],
    value_position: bool,
) -> CheckDiagnostic {
    let rejected = format!("{enum_name}::{label}");
    let qualify =
        |ordinal: usize| format!("{enum_name}::{}", schema.member_path(ordinal).join("::"));
    let mut candidates: Vec<String> = matches
        .iter()
        .filter(|&&ordinal| !(value_position && schema.is_category(ordinal)))
        .map(|&ordinal| qualify(ordinal))
        .filter(|candidate| *candidate != rejected)
        .collect();
    if candidates.is_empty() && value_position {
        candidates = matches
            .iter()
            .flat_map(|&category| schema.subtree_ordinals(category))
            .filter(|&ordinal| schema.is_selectable_leaf(ordinal))
            .map(qualify)
            .filter(|candidate| *candidate != rejected)
            .collect();
    }
    CheckDiagnostic::error(
        CHECK_AMBIGUOUS_MEMBER,
        file,
        span,
        format!(
            "`{rejected}` names more than one member of `{enum_name}`; qualify as {}",
            join_or(&candidates)
        ),
    )
    .with_payload(DiagnosticPayload::Enum(EnumDiagnostic::AmbiguousMember {
        enum_name: enum_name.to_string(),
        label,
        candidates,
    }))
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
    // Every right-operand fault below blames the member path itself, so each spans
    // the right operand rather than the whole `left is right` expression.
    let right_span = right.span();
    let resolved = match resolve_enum_member_path(program, right, aliases, file) {
        EnumMemberPathResolution::Resolved(resolved) => resolved,
        EnumMemberPathResolution::AmbiguousBareForeignOwner(ambiguous) => {
            diagnostics.push(ambiguous.diagnostic(file, right_span));
            return bool_type;
        }
        EnumMemberPathResolution::MissingOrNonEnum => {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_IS_TYPE,
                file,
                right_span,
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
                right_span,
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
            right_span,
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
        MemberPathResolution::Ambiguous(matches) => {
            diagnostics.push(ambiguous_member_value_diagnostic(
                file,
                right_span,
                left_name,
                resolved.member_label,
                resolved.schema,
                &matches,
                false,
            ))
        }
        MemberPathResolution::NotFound => diagnostics.push(CheckDiagnostic::error(
            CHECK_IS_TYPE,
            file,
            right_span,
            format!("operator `is` requires a member of `{left_name}` on the right"),
        )),
    }
    bool_type
}

fn enum_visible_in_program(
    program: &CheckedProgram,
    referencing_module: Option<&str>,
    enum_module: &str,
    enum_name: &str,
) -> bool {
    referencing_module == Some(enum_module)
        || program
            .module_by_name(enum_module)
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
    enum_schema_named(program.module_by_name(module)?, name)
}

/// The schema of the enum named `name` declared by `module`, if any. The sole owner
/// of finding an enum within a known module, so the by-name lookup and the candidate
/// scans share one definition.
fn enum_schema_named<'p>(
    module: &'p CheckedModule,
    name: &str,
) -> Option<&'p marrow_schema::EnumSchema> {
    module
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedResourceAnnotation {
    pub(crate) module: String,
    pub(crate) name: String,
}

pub(crate) fn resolve_resource_annotation(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<ResolvedResourceAnnotation> {
    resolve_resource_annotation_type(
        &Type::resolve(ty),
        program,
        aliases,
        module_of_file(program, file).unwrap_or_default(),
    )
}

fn resolve_resource_annotation_type(
    ty: &Type,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    module_name: &str,
) -> Option<ResolvedResourceAnnotation> {
    match ty {
        Type::Sequence(element) => {
            resolve_resource_annotation_type(element, program, aliases, module_name)
        }
        Type::Named(name) => {
            let segments = split_type_path(name);
            resolve_resource_path_in_module(
                program,
                aliases,
                module_name,
                &segments,
                ResolvableKind::Resource,
            )
            .map(|(resource, module)| ResolvedResourceAnnotation {
                module: module.to_string(),
                name: resource.name.clone(),
            })
        }
        _ => None,
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
    match resolve_enum_annotation_type_for_module(ty, program, module) {
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
    program: &CheckedProgram,
    module: &CheckedModule,
) -> EnumAnnotationResolution {
    let aliases = build_alias_map(&module.imports);
    resolve_enum_annotation_type_in_program(ty, program, &aliases, Some(&module.name))
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
        Type::Named(_) => {
            resolve_resource_annotation_type(ty, program, aliases, module_name).map(|resolved| {
                MarrowType::Resource(resource_type_name(&resolved.module, &resolved.name))
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

/// The fully-qualified name of the enum `ty` names when it resolves to a non-`pub`
/// enum owned by the same module as `file`. A same-module reference always sees a
/// private enum (visibility gates only foreign modules), so a `Visible` resolution
/// to a non-`pub` owner is precisely the case a public signature would leak. A
/// foreign private enum resolves to `Private` and is reported elsewhere.
pub(crate) fn same_module_private_enum(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<String> {
    let EnumAnnotationResolution::Visible(resolved) =
        resolve_enum_annotation(ty, program, aliases, file)
    else {
        return None;
    };
    let owner = program.module_by_name(&resolved.module)?;
    (module_of_file(program, file) == Some(resolved.module.as_str())
        && !enum_is_public(owner, &resolved.name))
    .then(|| qualified_enum_name(&resolved.module, &resolved.name))
}

fn qualified_enum_name(module: &str, name: &str) -> String {
    if module.is_empty() {
        return name.to_string();
    }
    format!("{module}::{name}")
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
    resolve_enum_annotation_type_in_program(ty, program, aliases, referencing)
}

fn resolve_enum_annotation_type_in_program(
    ty: &Type,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    referencing: Option<&str>,
) -> EnumAnnotationResolution {
    match ty {
        Type::Sequence(element) => {
            match resolve_enum_annotation_type_in_program(element, program, aliases, referencing) {
                EnumAnnotationResolution::Visible(mut resolved) => {
                    resolved.ty = MarrowType::Sequence(Box::new(resolved.ty));
                    EnumAnnotationResolution::Visible(resolved)
                }
                other => other,
            }
        }
        Type::Named(name) => {
            resolve_named_enum_annotation_in_program(name, program, aliases, referencing)
        }
        _ => EnumAnnotationResolution::MissingOrNonEnum,
    }
}

fn resolve_named_enum_annotation_in_program(
    name: &str,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    referencing: Option<&str>,
) -> EnumAnnotationResolution {
    match resolve_named_enum_owner_in_program(name, program, aliases, referencing) {
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
    resolve_named_enum_owner_in_program(name, program, aliases, module_of_file(program, file))
}

fn resolve_named_enum_owner_in_program<'p>(
    name: &str,
    program: &'p CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    referencing: Option<&str>,
) -> EnumOwnerResolution<'p> {
    if let Some((module, enum_name)) = name.rsplit_once("::") {
        return resolve_qualified_enum_owner_in_program(
            module,
            enum_name,
            program,
            aliases,
            referencing,
        );
    }

    if let Some(module) = referencing
        && let Some(schema) = enum_schema_in(program, module, name)
    {
        return EnumOwnerResolution::Found(ResolvedEnumOwner {
            module: module.to_string(),
            name: name.to_string(),
            schema,
            private: None,
        });
    }

    let public_candidates: Vec<_> = program
        .modules_declaring_enum(name)
        .into_iter()
        .filter_map(|module_index| {
            let module = &program.modules[module_index];
            if Some(module.name.as_str()) == referencing || module.name.is_empty() {
                return None;
            }
            if !enum_is_public(module, name) {
                return None;
            }
            let schema = enum_schema_named(module, name)?;
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
        [] => resolve_private_bare_enum_owner(program, referencing, name),
    }
}

/// Resolve a qualified enum owner from an already-split module prefix and enum name,
/// expanding any leading import alias on the module before lookup.
fn resolve_qualified_enum_owner_in_program<'p>(
    module: &str,
    enum_name: &str,
    program: &'p CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    referencing: Option<&str>,
) -> EnumOwnerResolution<'p> {
    resolve_expanded_enum_owner_in_program(
        &expand_module_alias(module, aliases),
        enum_name,
        program,
        referencing,
    )
}

/// Resolve an enum owner from a module path whose leading alias is already expanded
/// and an enum name. A member-path scan grows the expanded prefix one segment at a
/// time and resolves through here, so the whole scan stays linear instead of
/// rejoining and re-expanding the prefix at every split point.
fn resolve_expanded_enum_owner_in_program<'p>(
    module: &str,
    enum_name: &str,
    program: &'p CheckedProgram,
    referencing: Option<&str>,
) -> EnumOwnerResolution<'p> {
    let Some(schema) = enum_schema_in(program, module, enum_name) else {
        return EnumOwnerResolution::MissingOrNonEnum;
    };
    let private = (!enum_visible_in_program(program, referencing, module, enum_name))
        .then(|| format!("{module}::{enum_name}"));
    EnumOwnerResolution::Found(ResolvedEnumOwner {
        module: module.to_string(),
        name: enum_name.to_string(),
        schema,
        private,
    })
}

fn resolve_private_bare_enum_owner<'p>(
    program: &'p CheckedProgram,
    referencing: Option<&str>,
    name: &str,
) -> EnumOwnerResolution<'p> {
    let private_candidates: Vec<_> = program
        .modules_declaring_enum(name)
        .into_iter()
        .filter_map(|module_index| {
            let module = &program.modules[module_index];
            if Some(module.name.as_str()) == referencing || module.name.is_empty() {
                return None;
            }
            if enum_is_public(module, name) {
                return None;
            }
            let schema = enum_schema_named(module, name)?;
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
