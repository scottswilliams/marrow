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
        if !matches!(scrutinee_type, MarrowType::Unknown) {
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

    let mut covered: Vec<&str> = Vec::new();
    for arm in arms {
        if schema.ordinal(&arm.member).is_none() {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNKNOWN_ENUM_MEMBER,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!("`{enum_name}` has no member `{}`", arm.member),
                span: arm.span,
            });
            continue;
        }
        if covered.contains(&arm.member.as_str()) {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_DUPLICATE_MATCH_ARM,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!("`match` has a duplicate arm for `{}`", arm.member),
                span: arm.span,
            });
            continue;
        }
        covered.push(&arm.member);
    }

    let missing: Vec<&str> = schema
        .members
        .iter()
        .map(|member| member.name.as_str())
        .filter(|name| !covered.contains(name))
        .collect();
    if !missing.is_empty() {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_NONEXHAUSTIVE_MATCH,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "`match` on `{enum_name}` does not cover {}",
                missing
                    .iter()
                    .map(|name| format!("`{name}`"))
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
    referencing_module
        .and_then(|module| enum_schema_in(program, module, name).map(|schema| (module, schema)))
        .or_else(|| {
            program.modules.iter().find_map(|module| {
                module
                    .enums
                    .iter()
                    .find(|enum_schema| enum_schema.name == name)
                    .map(|schema| (module.name.as_str(), schema))
            })
        })
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
                return enum_schema_in(program, &module, enum_name).map(|_| MarrowType::Enum {
                    module,
                    name: enum_name.to_string(),
                });
            }
            resolve_enum(program, module_of_file(program, file), name).map(|(module, _)| {
                MarrowType::Enum {
                    module: module.to_string(),
                    name: name.clone(),
                }
            })
        }
        _ => None,
    }
}
