//! Enum resolution and `match` checking, plus the cross-module enum-signature
//! normalization the call boundary relies on.

use super::*;

/// Re-resolve every enum-typed signature slot in the assembled program against the
/// whole project, so a parameter, return, or constant annotation carries its
/// enum's true `{module, name}` owner.
///
/// Each module's signatures are first resolved per-file against that module's own
/// names, which cannot place a qualified `mod::Status` or a bare name owned by
/// another module. This pass revisits those slots with the full program in hand —
/// the same `resolve_type` the in-body checks use — so a cross-module enum
/// parameter is the same `Enum { module, name }` value its caller's argument is,
/// and the call boundary compares like for like. Non-enum slots are left untouched.
pub(crate) fn normalize_program_enum_types(
    program: &mut CheckedProgram,
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
) {
    let resolver = program.clone();
    normalize_program_enum_types_against(program, &resolver, parsed_files);
}

/// As [`normalize_program_enum_types`], but resolving against an explicit
/// `resolver` program. Test modules normalize against the combined project so an
/// enum a test file imports from a project module resolves to that module.
pub(crate) fn normalize_program_enum_types_against(
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
        // The file's import aliases, so an enum annotation qualified by a short
        // alias (`c::Status` under `use a::b::c`) resolves to the imported module —
        // the same expansion call dispatch applies. Built once, before the mutable
        // borrow of the module's functions and constants.
        let aliases = build_alias_map(&module.imports);
        for function in &mut module.functions {
            let Some(decl) = parsed.file.function(&function.name) else {
                continue;
            };
            for (param, param_decl) in function.params.iter_mut().zip(&decl.params) {
                if let Some(enum_type) =
                    resolve_enum_annotation(&param_decl.ty, resolver, &aliases, &file.path)
                {
                    param.ty = enum_type;
                }
            }
            if let (Some(return_type), Some(return_ref)) =
                (function.return_type.as_mut(), decl.return_type.as_ref())
                && let Some(enum_type) =
                    resolve_enum_annotation(return_ref, resolver, &aliases, &file.path)
            {
                *return_type = enum_type;
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
            if let Some(enum_type) =
                resolve_enum_annotation(const_ref, resolver, &aliases, &file.path)
            {
                constant.ty = Some(enum_type);
            }
        }
    }
}

/// The enum names declared across every parsed file, including error-bearing
/// ones, so a type annotation that names an enum is recognized regardless of
/// whether its file passed.
pub(crate) fn collect_enum_names(
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
) -> HashSet<String> {
    parsed_files
        .iter()
        .flat_map(|(_, parsed)| parsed.file.declarations.iter())
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Enum(decl) => Some(decl.name.clone()),
            _ => None,
        })
        .collect()
}

/// Check a `match` statement over an enum scrutinee: the scrutinee must be an
/// enum, every arm must name a member of that enum, no member may be matched
/// twice, and every member must be covered (exhaustive, no wildcard). Each arm
/// block is checked regardless, so type errors inside an arm still surface.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_match(
    program: &CheckedProgram,
    file: &Path,
    return_type: &MarrowType,
    scrutinee: Option<&marrow_syntax::Expression>,
    arms: &[marrow_syntax::MatchArm],
    span: SourceSpan,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let scrutinee_type = scrutinee
        .map(|expr| infer_type(program, expr, scope, aliases, file, diagnostics))
        .unwrap_or(MarrowType::Unknown);

    // Check every arm body up front so type errors inside an arm surface even when
    // the scrutinee is not an enum or an arm names an unknown member.
    for arm in arms {
        check_block_types(
            program,
            file,
            return_type,
            &arm.block,
            scope,
            aliases,
            diagnostics,
        );
    }

    let MarrowType::Enum {
        module: enum_module,
        name: enum_name,
    } = &scrutinee_type
    else {
        // An unresolved scrutinee (an untyped call, a saved read) is left alone:
        // the check never fires on an uncertain type. A known non-enum is rejected.
        if !matches!(scrutinee_type, MarrowType::Unknown | MarrowType::Invalid) {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_MATCH_REQUIRES_ENUM,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "`match` requires an enum value, but the scrutinee is `{}`",
                    marrow_type_name(&scrutinee_type)
                ),
                span,
            });
        }
        return;
    };
    let Some(schema) = enum_schema_in(program, enum_module, enum_name) else {
        // The scrutinee typed as an enum, but no such enum is declared. Rather than
        // silently skip exhaustiveness (which would let the match fault at runtime),
        // reject it: a `match` needs a known enum to dispatch over.
        diagnostics.push(CheckDiagnostic {
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

    // Coverage is over the enum's selectable leaves: each must be covered by
    // exactly one arm. An arm is a member path relative to the scrutinee enum —
    // a concrete leaf covers itself, a category covers every selectable leaf under
    // it. A bare arm name duplicated under several parents is ambiguous; the full
    // path always disambiguates. A leaf covered twice (a repeated arm, or a leaf
    // already covered by an enclosing category) is an overlap; an uncovered leaf is
    // non-exhaustive.
    let mut covered: Vec<usize> = Vec::new();
    // An arm rejected as an overlap is already the one clear diagnostic for that arm.
    // Reporting non-exhaustiveness on top — because the rejected arm's leaves were
    // dropped from coverage — would be noise, so the exhaustiveness pass is skipped
    // when any overlap fired. A genuinely uncovered leaf with no overlap still reports.
    let mut had_overlap = false;
    for arm in arms {
        let segments: Vec<&str> = arm.path.iter().map(String::as_str).collect();
        let arm_label = segments.join("::");
        let arm_ordinal = match schema.walk_member_path(&segments) {
            MemberPathResolution::Found(ordinal) => ordinal,
            MemberPathResolution::NotFound => {
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNKNOWN_ENUM_MEMBER,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
                    message: format!("`{enum_name}` has no member `{arm_label}`"),
                    span: arm.span,
                });
                continue;
            }
            // A bare name names a member under more than one parent; name the
            // qualifying paths so the dev can pick one (`tiger::paw` or `lion::paw`).
            MemberPathResolution::Ambiguous(paths) => {
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_AMBIGUOUS_MATCH_ARM,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
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
            diagnostics.push(CheckDiagnostic {
                code: CHECK_DUPLICATE_MATCH_ARM,
                severity: Severity::Error,
                file: file.to_path_buf(),
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
        diagnostics.push(CheckDiagnostic {
            code: CHECK_NONEXHAUSTIVE_MATCH,
            severity: Severity::Error,
            file: file.to_path_buf(),
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

/// Record each `match`'s resolved scrutinee enum on its statement, so the runtime
/// dispatches arms by that enum's ordinals rather than guessing the enum from the
/// arm member set. `target` is rewritten in place: every `match` in its function
/// bodies is stamped with its scrutinee enum. `program` is the read-only resolved
/// snapshot inference reads from. The two are separate borrows because inference
/// reads the whole program while `target`'s bodies are rewritten.
pub fn resolve_match_enums(target: &mut CheckedProgram, program: &CheckedProgram) {
    for module in &mut target.modules {
        let aliases = build_alias_map(&module.imports);
        let constants: HashMap<String, MarrowType> = module
            .constants
            .iter()
            .map(|constant| {
                (
                    constant.name.clone(),
                    constant.ty.clone().unwrap_or(MarrowType::Unknown),
                )
            })
            .collect();
        let source_file = module.source_file.clone();
        for function in &mut module.functions {
            let mut scope = vec![constants.clone()];
            scope.push(
                function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.ty.clone()))
                    .collect(),
            );
            resolve_block_matches(
                &mut function.body,
                program,
                &aliases,
                &mut scope,
                &source_file,
            );
        }
    }
}

/// Walk a block, tracking the bindings it introduces, and resolve every `match`'s
/// scrutinee enum. Mirrors the binding rules [`check_block_types`] uses so a
/// scrutinee that names a local resolves the same way.
pub(crate) fn resolve_block_matches(
    block: &mut marrow_syntax::Block,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    file: &Path,
) {
    use marrow_syntax::Statement;
    scope.push(HashMap::new());
    for statement in &mut block.statements {
        match statement {
            Statement::Const {
                name, ty, value, ..
            } => {
                let value_type = infer_only(program, value, scope, aliases, file);
                bind(
                    scope,
                    name,
                    binding_type(ty.as_ref(), value_type, program, aliases, file),
                );
            }
            Statement::Var {
                name, ty, value, ..
            } => {
                let value_type = value.as_ref().map_or(MarrowType::Unknown, |value| {
                    infer_only(program, value, scope, aliases, file)
                });
                bind(
                    scope,
                    name,
                    binding_type(ty.as_ref(), value_type, program, aliases, file),
                );
            }
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                resolve_block_matches(then_block, program, aliases, scope, file);
                for else_if in else_ifs {
                    resolve_block_matches(&mut else_if.block, program, aliases, scope, file);
                }
                if let Some(block) = else_block {
                    resolve_block_matches(block, program, aliases, scope, file);
                }
            }
            Statement::While { body, .. }
            | Statement::Transaction { body, .. }
            | Statement::Lock { body, .. } => {
                resolve_block_matches(body, program, aliases, scope, file);
            }
            Statement::For {
                binding,
                iterable,
                body,
                ..
            } => {
                let element = match infer_only(program, iterable, scope, aliases, file) {
                    MarrowType::Sequence(element) if binding.second.is_none() => *element,
                    _ => MarrowType::Unknown,
                };
                let mut frame = HashMap::new();
                frame.insert(binding.first.clone(), element);
                if let Some(second) = &binding.second {
                    frame.insert(second.clone(), MarrowType::Unknown);
                }
                scope.push(frame);
                resolve_block_matches(body, program, aliases, scope, file);
                scope.pop();
            }
            Statement::Try {
                body,
                catch,
                finally,
                ..
            } => {
                resolve_block_matches(body, program, aliases, scope, file);
                if let Some(clause) = catch {
                    let mut frame = HashMap::new();
                    frame.insert(clause.name.clone(), MarrowType::Error);
                    scope.push(frame);
                    resolve_block_matches(&mut clause.block, program, aliases, scope, file);
                    scope.pop();
                }
                if let Some(finally) = finally {
                    resolve_block_matches(finally, program, aliases, scope, file);
                }
            }
            Statement::Match {
                scrutinee,
                arms,
                enum_name,
                enum_module,
                ..
            } => {
                if let Some(scrutinee) = scrutinee.as_ref()
                    && let MarrowType::Enum { module, name } =
                        infer_only(program, scrutinee, scope, aliases, file)
                {
                    *enum_name = Some(name);
                    *enum_module = Some(module);
                }
                for arm in arms.iter_mut() {
                    resolve_block_matches(&mut arm.block, program, aliases, scope, file);
                }
            }
            _ => {}
        }
    }
    scope.pop();
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

/// Type-check `left is right`. `left` must be an enum value; `right` must be a
/// member-path of the same enum (a concrete member or a category — a category is
/// the whole point, testing subtree membership). The result is always `bool`. `is`
/// is a separate nominal predicate, not an assignability relaxation: a value's type
/// stays its exact enum, so no subtyping lattice is introduced and the totality of
/// the type model is untouched.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_is(
    program: &CheckedProgram,
    left_type: &MarrowType,
    right: &marrow_syntax::Expression,
    aliases: &HashMap<String, Vec<String>>,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
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

/// Resolve a type annotation against the project's named types, so a resource
/// type like `Book` resolves to `MarrowType::Resource("Book")` and an enum type to
/// the enum it names — carrying that enum's owning module, so two same-named enums
/// never alias and a foreign enum is never stamped with the referencing module.
///
/// An enum annotation is resolved by its true owner, never the referencing module:
/// a bare `Status` resolves same-module-first then to the project-wide owner (the
/// symmetry a bare `Status::member` literal already uses), and a qualified
/// `mod::Status` resolves to `mod`'s enum when `mod` declares it. Resources are
/// placed by `MarrowType::resolve` with no enum names, so it cannot mint a phantom
/// enum from a foreign-only bare name.
pub(crate) fn resolve_type(
    ty: &marrow_syntax::TypeRef,
    program: &CheckedProgram,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> MarrowType {
    if let Some(enum_type) = resolve_enum_annotation(ty, program, aliases, file) {
        return enum_type;
    }
    let resources: Vec<String> = program
        .modules
        .iter()
        .flat_map(|module| {
            module
                .resources
                .iter()
                .map(|resource| resource.name.clone())
        })
        .collect();
    MarrowType::resolve(
        ty,
        TypeNames {
            module: module_of_file(program, file).unwrap_or_default(),
            resources: &resources,
            enums: &[],
        },
    )
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
