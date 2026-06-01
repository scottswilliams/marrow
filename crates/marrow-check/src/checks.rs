//! The type-check driver passes over a parsed file: return placement, operator,
//! condition, assignment, call, and saved-key argument checks.

use std::cmp::Ordering;

use marrow_store::Decimal;

use super::*;

/// Resolve every `use` against `resolvable`, run the type pass over each parsed
/// file against `program` with `project_resources`, then suppress resolution
/// reports that target modules whose files failed to parse or read. This is the
/// shared tail of check_project and check_tests: pass 1 (parse plus each caller's
/// module and ownership construction) differs and stays in the caller, but once
/// the resolvable module set, program, and resource set are known every step is
/// identical.
pub(crate) struct ResolvedFileCheck<'a> {
    pub(crate) files: &'a [marrow_project::ModuleFile],
    pub(crate) parsed_files: &'a [(&'a marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    pub(crate) module_name_policy: ModuleNamePolicy,
    pub(crate) resolvable: &'a HashMap<String, PathBuf>,
    pub(crate) program: &'a CheckedProgram,
    pub(crate) project_resources: &'a HashSet<String>,
    pub(crate) project_enums: &'a HashSet<String>,
}

pub(crate) fn check_resolved_files(input: ResolvedFileCheck<'_>, report: &mut CheckReport) {
    let ResolvedFileCheck {
        files,
        parsed_files,
        module_name_policy,
        resolvable,
        program,
        project_resources,
        project_enums,
    } = input;

    // Pass 2: every `use` must name a project module, a sibling module, or a
    // standard-library module, now that the full resolvable module set is known.
    for (file, parsed) in parsed_files {
        for use_decl in &parsed.file.uses {
            if !is_resolved_import(&use_decl.name, resolvable) {
                report.diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNRESOLVED_IMPORT,
                    severity: Severity::Error,
                    file: file.path.clone(),
                    message: format!("cannot resolve import `{}`", use_decl.name),
                    span: use_decl.span,
                });
            }
        }
    }

    // Pass 3: flag type annotations that name an unknown type.
    for (file, parsed) in parsed_files {
        check_file_types(
            program,
            project_resources,
            project_enums,
            &file.path,
            parsed,
            &mut report.diagnostics,
        );
    }

    // A file that failed to parse or read is excluded from the program, so exact
    // imports of its module and qualified calls into it would look unresolved
    // even though the source may contain the definition. Suppress only those
    // reports; other clean files' local resolution diagnostics remain trustworthy.
    let incomplete_modules =
        incomplete_module_names(files, parsed_files, module_name_policy, program);
    if !incomplete_modules.is_empty() {
        report.diagnostics.retain(|diagnostic| {
            if diagnostic.code == CHECK_UNRESOLVED_IMPORT
                && unresolved_import_name(&diagnostic.message)
                    .is_some_and(|name| incomplete_modules.contains(name))
            {
                return false;
            }
            if diagnostic.code == CHECK_UNRESOLVED_CALL
                && unresolved_call_name(&diagnostic.message).is_some_and(|name| {
                    references_incomplete_module_member(name, &incomplete_modules)
                })
            {
                return false;
            }
            true
        });
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ModuleNamePolicy {
    DeclaredOrPath,
    PathOnly,
}

fn incomplete_module_names(
    files: &[marrow_project::ModuleFile],
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    module_name_policy: ModuleNamePolicy,
    program: &CheckedProgram,
) -> HashSet<String> {
    let complete_modules: HashSet<&str> = program
        .modules
        .iter()
        .map(|module| module.name.as_str())
        .collect();
    let parsed_paths: HashSet<&Path> = parsed_files
        .iter()
        .map(|(file, _)| file.path.as_path())
        .collect();
    let mut modules = HashSet::new();
    for file in files {
        if !parsed_paths.contains(file.path.as_path())
            && let Some(module) = &file.module_name
        {
            insert_incomplete_module(&mut modules, &complete_modules, module);
        }
    }
    for (file, parsed) in parsed_files {
        if parsed.has_errors() {
            if matches!(module_name_policy, ModuleNamePolicy::DeclaredOrPath)
                && let Some(module) = &parsed.file.module
            {
                insert_incomplete_module(&mut modules, &complete_modules, &module.name);
            }
            if let Some(module) = &file.module_name {
                insert_incomplete_module(&mut modules, &complete_modules, module);
            }
        }
    }
    modules
}

fn insert_incomplete_module(
    modules: &mut HashSet<String>,
    complete_modules: &HashSet<&str>,
    name: &str,
) {
    if !complete_modules.contains(name) {
        modules.insert(name.to_string());
    }
}

fn unresolved_call_name(message: &str) -> Option<&str> {
    message
        .strip_prefix("function `")?
        .strip_suffix("` is not defined")
}

fn unresolved_import_name(message: &str) -> Option<&str> {
    message
        .strip_prefix("cannot resolve import `")?
        .strip_suffix('`')
}

fn references_incomplete_module_member(name: &str, modules: &HashSet<String>) -> bool {
    modules.iter().any(|module| {
        name.strip_prefix(module)
            .is_some_and(|rest| rest.starts_with("::"))
    })
}

/// The file-level prelude every function body in a file shares: its short→full
/// import aliases and its module-level constants, both of which are in scope
/// before any function's own parameters and locals.
pub(crate) struct FilePrelude {
    pub(crate) aliases: HashMap<String, Vec<String>>,
    pub(crate) module_constants: HashMap<String, MarrowType>,
}

/// Build a file's [`FilePrelude`]: the alias map from its imports and the typed
/// module constants, in source order so an earlier constant is in scope for a
/// later one. The type-check pass and the editor queries both start from this,
/// so the bindings a function body sees are derived in exactly one place.
pub(crate) fn file_prelude(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
) -> FilePrelude {
    // Short→full import aliases, used to expand short-form calls
    // (`clock::now()` → `std::clock::now`) before resolution. The runtime
    // rebuilds the same map from `CheckedModule::imports`.
    let aliases = build_alias_map(
        &parsed
            .file
            .uses
            .iter()
            .map(|use_decl| use_decl.name.clone())
            .collect::<Vec<_>>(),
    );
    // A module's top-level constants are in scope (bare) for its functions, an
    // annotated one carrying its annotation and an unannotated one its inferred
    // type, so a typed use like `var x: int = M` resolves rather than
    // false-positiving `check.untyped_value`. Initializer diagnostics
    // (constant-expression validity, literal range) come from `check_const_value`,
    // so inference diagnostics are discarded here.
    let mut module_constants: HashMap<String, MarrowType> = HashMap::new();
    for declaration in &parsed.file.declarations {
        if let marrow_syntax::Declaration::Const(constant) = declaration {
            let ty = match (&constant.ty, &constant.value) {
                (Some(ty), _) => resolve_type(ty, program, &aliases, file),
                (None, Some(value)) => infer_type(
                    program,
                    value,
                    std::slice::from_ref(&module_constants),
                    &aliases,
                    file,
                    &mut Vec::new(),
                ),
                // The value did not parse; the parser already reported the error.
                (None, None) => MarrowType::Unknown,
            };
            module_constants.insert(constant.name.clone(), ty);
        }
    }
    FilePrelude {
        aliases,
        module_constants,
    }
}

/// Run the type-inference pass over one parsed file against the resolved
/// `program`: unknown-type annotations, return-value placement, the
/// expression/statement type checks (operator/condition/assignment/call/argument
/// types, std arity, the `nextId` single-`int` gate), and missing-return
/// analysis. Library files (via [`check_project`]) and test scripts (via
/// [`check_tests`]) share this pass. `project_resources` and `project_enums` are
/// the project-wide name sets used to recognize type annotations.
pub(crate) fn check_file_types(
    program: &CheckedProgram,
    project_resources: &HashSet<String>,
    project_enums: &HashSet<String>,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let has_parse_errors = parsed.has_errors();
    let FilePrelude {
        aliases,
        module_constants,
    } = file_prelude(program, file, parsed);
    check_prototype_only(program, file, parsed, diagnostics);
    let annotation_context = TypeAnnotationContext {
        program,
        aliases: &aliases,
        file,
        resources: project_resources,
        enums: project_enums,
    };
    for declaration in &parsed.file.declarations {
        match declaration {
            marrow_syntax::Declaration::Function(function) => {
                for param in &function.params {
                    check_type_annotation(
                        &param.ty,
                        function.span,
                        &annotation_context,
                        diagnostics,
                    );
                }
                if let Some(return_type) = &function.return_type {
                    check_type_annotation(
                        return_type,
                        function.span,
                        &annotation_context,
                        diagnostics,
                    );
                }
                if has_parse_errors {
                    continue;
                }
                check_return_values(
                    file,
                    &function.body,
                    function.return_type.is_some(),
                    diagnostics,
                );
                check_block_type_annotations(&function.body, &annotation_context, diagnostics);
                check_function_types(
                    program,
                    file,
                    function,
                    &module_constants,
                    &aliases,
                    diagnostics,
                );
                if function.return_type.is_some() && !block_returns(&function.body) {
                    diagnostics.push(CheckDiagnostic {
                        code: CHECK_MISSING_RETURN,
                        severity: Severity::Error,
                        file: file.to_path_buf(),
                        message: format!(
                            "function `{}` may reach its end without returning a value",
                            function.name
                        ),
                        span: function.span,
                    });
                }
            }
            marrow_syntax::Declaration::Const(constant) => {
                if let Some(ty) = &constant.ty {
                    check_type_annotation(ty, constant.span, &annotation_context, diagnostics);
                }
            }
            // Resource and enum member types are validated by schema compilation.
            marrow_syntax::Declaration::Resource(_) | marrow_syntax::Declaration::Enum(_) => {}
        }
    }
}

/// Record a `check.unknown_type` diagnostic when `ty` names a type the checker
/// does not recognize or uses unsupported map syntax outside saved-resource
/// member sugar. Located at `span` (the declaration), since a type annotation
/// carries no span of its own.
struct TypeAnnotationContext<'a> {
    program: &'a CheckedProgram,
    aliases: &'a HashMap<String, Vec<String>>,
    file: &'a Path,
    resources: &'a HashSet<String>,
    enums: &'a HashSet<String>,
}

fn check_type_annotation(
    ty: &marrow_syntax::TypeRef,
    span: SourceSpan,
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if let Some(private) =
        private_enum_type_reference(ty, context.program, context.aliases, context.file)
    {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_PRIVATE_ENUM,
            severity: Severity::Error,
            file: context.file.to_path_buf(),
            message: format!(
                "enum `{private}` is private to its module; mark it `pub` to use it from another module"
            ),
            span,
        });
        return;
    }
    if marrow_schema::contains_map_type_syntax(&ty.text)
        || !MarrowType::names_known_type(ty, context.resources, context.enums)
    {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_UNKNOWN_TYPE,
            severity: Severity::Error,
            file: context.file.to_path_buf(),
            message: format!("unknown type `{}`", ty.text.trim()),
            span,
        });
    }
}

fn check_block_type_annotations(
    block: &marrow_syntax::Block,
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        check_statement_type_annotations(statement, context, diagnostics);
    }
}

fn check_statement_type_annotations(
    statement: &marrow_syntax::Statement,
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Statement;
    match statement {
        Statement::Const {
            ty: Some(ty), span, ..
        } => {
            check_type_annotation(ty, *span, context, diagnostics);
        }
        Statement::Var { keys, ty, span, .. } => {
            for key in keys {
                check_type_annotation(&key.ty, *span, context, diagnostics);
            }
            if let Some(ty) = ty {
                check_type_annotation(ty, *span, context, diagnostics);
            }
        }
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            check_block_type_annotations(then_block, context, diagnostics);
            for else_if in else_ifs {
                check_block_type_annotations(&else_if.block, context, diagnostics);
            }
            if let Some(block) = else_block {
                check_block_type_annotations(block, context, diagnostics);
            }
        }
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. }
        | Statement::Lock { body, .. } => {
            check_block_type_annotations(body, context, diagnostics);
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            check_block_type_annotations(body, context, diagnostics);
            if let Some(catch) = catch {
                if let Some(ty) = &catch.ty {
                    check_type_annotation(ty, catch.block.span, context, diagnostics);
                }
                check_block_type_annotations(&catch.block, context, diagnostics);
            }
            if let Some(finally) = finally {
                check_block_type_annotations(finally, context, diagnostics);
            }
        }
        Statement::Match { arms, .. } => {
            for arm in arms {
                check_block_type_annotations(&arm.block, context, diagnostics);
            }
        }
        _ => {}
    }
}

/// Flag each `return` whose value presence does not match the function's declared
/// return type: a value-returning function must return a value, and a function
/// with no return type must not return one. Recurses into nested blocks; `finally`
/// is left to `check.finally_control_flow`.
pub(crate) fn check_return_values(
    file: &Path,
    body: &marrow_syntax::Block,
    returns_value: bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Statement;
    for statement in &body.statements {
        match statement {
            Statement::Return { value, span } => {
                let message = match (returns_value, value.is_some()) {
                    (true, false) => "a value-returning function must return a value",
                    (false, true) => "a function with no return type cannot return a value",
                    _ => continue,
                };
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_RETURN_VALUE,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
                    message: message.to_string(),
                    span: *span,
                });
            }
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                check_return_values(file, then_block, returns_value, diagnostics);
                for else_if in else_ifs {
                    check_return_values(file, &else_if.block, returns_value, diagnostics);
                }
                if let Some(block) = else_block {
                    check_return_values(file, block, returns_value, diagnostics);
                }
            }
            Statement::While { body, .. }
            | Statement::For { body, .. }
            | Statement::Transaction { body, .. }
            | Statement::Lock { body, .. } => {
                check_return_values(file, body, returns_value, diagnostics);
            }
            Statement::Try { body, catch, .. } => {
                check_return_values(file, body, returns_value, diagnostics);
                if let Some(clause) = catch {
                    check_return_values(file, &clause.block, returns_value, diagnostics);
                }
                // `finally` cannot contain `return` (check.finally_control_flow).
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    check_return_values(file, &arm.block, returns_value, diagnostics);
                }
            }
            _ => {}
        }
    }
}

/// Whether `block` definitely returns (or otherwise diverges) on every path —
/// a sound under-approximation of "every reachable path returns". It is
/// conservative: a function ending in a call or a loop may diverge, so it is not
/// flagged; only a clearly falling-through end is. This favors no false positives
/// over catching every genuine case.
pub(crate) fn block_returns(block: &marrow_syntax::Block) -> bool {
    block.statements.last().is_some_and(statement_returns)
}

pub(crate) fn statement_returns(statement: &marrow_syntax::Statement) -> bool {
    use marrow_syntax::{Expression, Statement};
    match statement {
        Statement::Return { .. } | Statement::Throw { .. } => true,
        // A call may throw or loop forever, so a function ending in one is allowed.
        Statement::Expr { value, .. } => matches!(value, Expression::Call { .. }),
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => else_block.as_ref().is_some_and(|else_block| {
            block_returns(then_block)
                && else_ifs.iter().all(|else_if| block_returns(&else_if.block))
                && block_returns(else_block)
        }),
        Statement::Transaction { body, .. } | Statement::Lock { body, .. } => block_returns(body),
        Statement::Try { body, catch, .. } => {
            block_returns(body)
                && catch
                    .as_ref()
                    .is_none_or(|clause| block_returns(&clause.block))
        }
        // A `match` is exhaustive with no fall-through, so it returns on every
        // path exactly when every arm does. An empty (member-less) match cannot
        // arise, so `all` over no arms is not a spurious "returns".
        Statement::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|arm| block_returns(&arm.block))
        }
        // A loop may not run or may run forever; conservatively treat a function
        // ending in one as diverging rather than risk a false positive.
        Statement::While { .. } | Statement::For { .. } => true,
        _ => false,
    }
}

/// Type-check a function body: flag operators applied to operands they do not
/// accept, `if`/`while` conditions that are not `bool`, and calls whose arguments
/// do not match the function they resolve to. Walks the body tracking the type of
/// each in-scope binding (parameters and `const`/`var` locals) and inferring the
/// type of each expression. A check fires only when a type or signature is known
/// to be wrong, so an unresolved value — a saved-data read, a cross-module value,
/// an unresolved call — is never a false positive. The operator rules are:
/// matching numeric operands for `+ - * /`, `int` for
/// `%`, `string` for `_`, ordered same-typed operands for comparisons, and `bool`
/// for `and`/`or`/`not`.
pub(crate) fn check_function_types(
    program: &CheckedProgram,
    file: &Path,
    function: &marrow_syntax::FunctionDecl,
    module_constants: &HashMap<String, MarrowType>,
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    // The base scope frame is the module's constants overlaid with the parameter
    // list (a parameter shadows a like-named constant). Types resolve against the
    // project's resources so resource-typed bindings feed field-type inference.
    let mut base = module_constants.clone();
    for param in &function.params {
        base.insert(
            param.name.clone(),
            resolve_type(&param.ty, program, aliases, file),
        );
    }
    let mut scope: Vec<HashMap<String, MarrowType>> = vec![base];
    // The declared return type (unknown for a void function), used to check each
    // `return` expression's type as the walk reaches it.
    let return_type = function
        .return_type
        .as_ref()
        .map_or(MarrowType::Unknown, |ty| {
            resolve_type(ty, program, aliases, file)
        });
    check_block_types(
        program,
        file,
        &return_type,
        &function.body,
        &mut scope,
        aliases,
        diagnostics,
    );
}

/// Type-check every statement in a block, with a scope frame for the
/// `const`/`var` bindings the block introduces.
pub(crate) fn check_block_types(
    program: &CheckedProgram,
    file: &Path,
    return_type: &MarrowType,
    block: &marrow_syntax::Block,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    scope.push(HashMap::new());
    for statement in &block.statements {
        check_statement_types(
            program,
            file,
            return_type,
            statement,
            scope,
            aliases,
            diagnostics,
        );
    }
    scope.pop();
}

/// Check one statement: type-check the expressions it contains, recurse into any
/// nested blocks, and record the type of any binding it introduces.
pub(crate) fn check_statement_types(
    program: &CheckedProgram,
    file: &Path,
    return_type: &MarrowType,
    statement: &marrow_syntax::Statement,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Statement;
    match statement {
        Statement::Const {
            ty, value, span, ..
        } => {
            let value_type = infer_type(program, value, scope, aliases, file, diagnostics);
            check_range_value(file, value, diagnostics);
            if let Some(ty) = ty {
                check_assignment(
                    file,
                    *span,
                    &resolve_type(ty, program, aliases, file),
                    &value_type,
                    diagnostics,
                );
            }
            if let Some((name, ty)) = local_binding(program, statement, scope, aliases, file) {
                bind(scope, &name, ty);
            }
        }
        Statement::Var {
            ty, value, span, ..
        } => {
            let value_type = match value {
                Some(value) => {
                    let value_type = infer_type(program, value, scope, aliases, file, diagnostics);
                    check_range_value(file, value, diagnostics);
                    value_type
                }
                None => MarrowType::Unknown,
            };
            // An annotated initializer must match the declared type.
            if let (Some(ty), Some(_)) = (ty, value) {
                check_assignment(
                    file,
                    *span,
                    &resolve_type(ty, program, aliases, file),
                    &value_type,
                    diagnostics,
                );
            }
            if let Some((name, ty)) = local_binding(program, statement, scope, aliases, file) {
                bind(scope, &name, ty);
            }
        }
        Statement::Assign {
            target,
            value,
            span,
        } => {
            // The target's type is known for a local variable or a saved field;
            // for other places (a local resource field, a whole resource) it is
            // unknown and the assignment is left alone.
            let target_type = infer_type(program, target, scope, aliases, file, diagnostics);
            let value_type = infer_type(program, value, scope, aliases, file, diagnostics);
            check_range_value(file, value, diagnostics);
            check_assignment(file, *span, &target_type, &value_type, diagnostics);
        }
        Statement::Merge { .. } => {}
        Statement::Delete { path, .. } => {
            infer_type(program, path, scope, aliases, file, diagnostics);
        }
        Statement::Return { value, span } => {
            if let Some(value) = value {
                let value_type = infer_type(program, value, scope, aliases, file, diagnostics);
                check_range_value(file, value, diagnostics);
                check_return_type(file, *span, return_type, &value_type, diagnostics);
            }
        }
        Statement::Throw { value, span } => {
            let value_type = infer_type(program, value, scope, aliases, file, diagnostics);
            check_range_value(file, value, diagnostics);
            check_throw_type(file, *span, &value_type, diagnostics);
        }
        Statement::Expr { value, .. } => {
            infer_type(program, value, scope, aliases, file, diagnostics);
            check_range_value(file, value, diagnostics);
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if let Some(condition) = condition {
                check_condition(program, file, condition, scope, aliases, diagnostics);
                check_range_value(file, condition, diagnostics);
            }
            check_block_types(
                program,
                file,
                return_type,
                then_block,
                scope,
                aliases,
                diagnostics,
            );
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    check_condition(program, file, condition, scope, aliases, diagnostics);
                    check_range_value(file, condition, diagnostics);
                }
                check_block_types(
                    program,
                    file,
                    return_type,
                    &else_if.block,
                    scope,
                    aliases,
                    diagnostics,
                );
            }
            if let Some(block) = else_block {
                check_block_types(
                    program,
                    file,
                    return_type,
                    block,
                    scope,
                    aliases,
                    diagnostics,
                );
            }
        }
        Statement::While {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_condition(program, file, condition, scope, aliases, diagnostics);
                check_range_value(file, condition, diagnostics);
            }
            check_block_types(
                program,
                file,
                return_type,
                body,
                scope,
                aliases,
                diagnostics,
            );
        }
        Statement::For {
            binding,
            iterable,
            step,
            body,
            ..
        } => {
            // Inferring the iterable here also operator-checks it; its diagnostics
            // belong to the type pass, so `for_frame` re-infers with a discard sink.
            infer_type(program, iterable, scope, aliases, file, diagnostics);
            check_range_iterable_value_parts(file, iterable, diagnostics);
            if let Some(step) = step {
                check_range_value(file, step, diagnostics);
            }
            check_range_header(
                program,
                file,
                iterable,
                step.as_ref(),
                scope,
                aliases,
                diagnostics,
            );
            check_for_collection_support(program, file, binding, iterable, diagnostics);
            let frame = for_frame(program, binding, iterable, scope, aliases, file);
            scope.push(frame);
            check_block_types(
                program,
                file,
                return_type,
                body,
                scope,
                aliases,
                diagnostics,
            );
            scope.pop();
        }
        Statement::Transaction { body, .. } => {
            check_block_types(
                program,
                file,
                return_type,
                body,
                scope,
                aliases,
                diagnostics,
            );
        }
        Statement::Lock { body, .. } => {
            check_block_types(
                program,
                file,
                return_type,
                body,
                scope,
                aliases,
                diagnostics,
            );
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            check_block_types(
                program,
                file,
                return_type,
                body,
                scope,
                aliases,
                diagnostics,
            );
            if let Some(clause) = catch {
                // The catch clause binds an Error value for the duration of its block.
                let mut frame = HashMap::new();
                frame.insert(clause.name.clone(), MarrowType::Error);
                scope.push(frame);
                check_block_types(
                    program,
                    file,
                    return_type,
                    &clause.block,
                    scope,
                    aliases,
                    diagnostics,
                );
                scope.pop();
            }
            if let Some(finally) = finally {
                check_block_types(
                    program,
                    file,
                    return_type,
                    finally,
                    scope,
                    aliases,
                    diagnostics,
                );
            }
        }
        Statement::Match {
            scrutinee,
            arms,
            span,
            ..
        } => {
            if let Some(scrutinee) = scrutinee {
                check_range_value(file, scrutinee, diagnostics);
            }
            check_match(
                program,
                file,
                return_type,
                scrutinee.as_ref(),
                arms,
                *span,
                scope,
                aliases,
                diagnostics,
            );
        }
        Statement::Break { .. } | Statement::Continue { .. } => {}
    }
}

/// The scope frame a `for` loop's body runs under, mirroring
/// [`check_statement_types`]: the loop binding(s) in scope for the body.
/// Collection loops bind the collection's element, with `keys(...)` preserving
/// address-only traversal and two-name loops binding address plus element.
/// Inference here discards diagnostics; the type pass emits the iterable's
/// separately.
pub(crate) fn for_frame(
    program: &CheckedProgram,
    binding: &marrow_syntax::ForBinding,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> HashMap<String, MarrowType> {
    let iterable_type = infer_type(program, iterable, scope, aliases, file, &mut Vec::new());
    if let Some((first_type, second_type)) =
        collection_loop_binding_types(program, binding.second.is_some(), iterable)
    {
        let mut frame = HashMap::new();
        frame.insert(binding.first.clone(), first_type);
        if let Some(second) = &binding.second {
            frame.insert(second.clone(), second_type.unwrap_or(MarrowType::Unknown));
        }
        return frame;
    }
    let first_type = match (&binding.second, &iterable_type) {
        (None, MarrowType::Sequence(element)) => (**element).clone(),
        // A range binds its single variable to its endpoint type, so the body type-
        // checks (`for x in lo..hi`: `x` is the endpoint scalar). Only a same-typed
        // steppable-endpoint range types its variable; anything else stays unknown.
        (None, _) => range_endpoint_type(program, iterable, scope, aliases, file)
            .unwrap_or(MarrowType::Unknown),
        _ => MarrowType::Unknown,
    };
    let mut frame = HashMap::new();
    frame.insert(binding.first.clone(), first_type);
    if let Some(second) = &binding.second {
        frame.insert(second.clone(), MarrowType::Unknown);
    }
    frame
}

fn collection_loop_binding_types(
    program: &CheckedProgram,
    two_name: bool,
    iterable: &marrow_syntax::Expression,
) -> Option<(MarrowType, Option<MarrowType>)> {
    let iterable = reversed_call_arg(iterable).unwrap_or(iterable);
    if let Some(path) = collection_wrapper_arg(iterable, "keys") {
        if two_name || is_saved_unique_index_branch_path(program, path) {
            return None;
        }
        return Some((saved_path_key_type(program, path)?, None));
    }
    if let Some(path) = collection_wrapper_arg(iterable, "values") {
        if two_name || is_saved_index_branch_path(program, path) {
            return None;
        }
        return Some((saved_path_value_type(program, path), None));
    }
    if let Some(path) = collection_wrapper_arg(iterable, "entries") {
        if !two_name || is_saved_index_branch_path(program, path) {
            return None;
        }
        return Some((
            saved_path_key_type(program, path)?,
            Some(saved_path_value_type(program, path)),
        ));
    }
    if !is_saved_path_syntax(program, iterable) {
        return None;
    }
    if is_saved_index_branch_path(program, iterable) {
        if two_name {
            let (resource, index, arg_count) = saved_index_branch(program, iterable)?;
            if non_unique_index_branch_yields_identity(resource, index, arg_count) {
                return Some((
                    saved_path_key_type(program, iterable)?,
                    Some(MarrowType::Resource(resource.name.clone())),
                ));
            }
            return None;
        }
        return Some((saved_path_key_type(program, iterable)?, None));
    }
    if two_name {
        return Some((
            saved_path_key_type(program, iterable)?,
            saved_path_direct_value_type(program, iterable),
        ));
    }
    Some((saved_path_key_type(program, iterable)?, None))
}

fn check_for_collection_support(
    program: &CheckedProgram,
    file: &Path,
    binding: &marrow_syntax::ForBinding,
    iterable: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if binding.second.is_none() {
        return;
    }
    let iterable = reversed_call_arg(iterable).unwrap_or(iterable);
    let Some((resource, index, arg_count)) = saved_index_branch(program, iterable) else {
        return;
    };
    if non_unique_index_branch_yields_identity(resource, index, arg_count) {
        return;
    }
    diagnostics.push(CheckDiagnostic {
        code: CHECK_COLLECTION_UNSUPPORTED,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: "a two-name loop over an index branch must yield resource identities".to_string(),
        span: iterable.span(),
    });
}

fn reversed_call_arg(expr: &marrow_syntax::Expression) -> Option<&marrow_syntax::Expression> {
    collection_wrapper_arg(expr, "reversed")
}

fn collection_wrapper_arg<'a>(
    expr: &'a marrow_syntax::Expression,
    wrapper: &str,
) -> Option<&'a marrow_syntax::Expression> {
    let marrow_syntax::Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let marrow_syntax::Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.as_slice() != [wrapper] {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

fn is_saved_path_syntax(program: &CheckedProgram, expr: &marrow_syntax::Expression) -> bool {
    use marrow_syntax::Expression;
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Field { .. } => {
            is_saved_index_branch_path(program, expr) || saved_layer_chain(expr).is_some()
        }
        Expression::Call { .. } => is_saved_index_branch_path(program, expr),
        _ => false,
    }
}

fn saved_path_key_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    use marrow_syntax::Expression;
    match path {
        Expression::SavedRoot { name, .. } => {
            let resource = find_resource_schema(program, name)?;
            resource
                .saved_root
                .as_ref()
                .filter(|root| !root.identity_keys.is_empty())?;
            Some(MarrowType::Identity(resource.name.clone()))
        }
        Expression::Call { .. } => saved_index_branch_type(program, path),
        Expression::Field { .. } if is_saved_index_branch_path(program, path) => {
            saved_index_branch_type(program, path)
        }
        Expression::Field { .. } => Some(layer_key_type(program, path)),
        _ => None,
    }
}

fn saved_path_value_type(program: &CheckedProgram, path: &marrow_syntax::Expression) -> MarrowType {
    saved_path_direct_value_type(program, path).unwrap_or(MarrowType::Unknown)
}

fn saved_path_direct_value_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    use marrow_syntax::Expression;
    match path {
        Expression::SavedRoot { name, .. } => {
            let resource = find_resource_schema(program, name)?;
            resource
                .saved_root
                .as_ref()
                .filter(|root| !root.identity_keys.is_empty())?;
            Some(MarrowType::Resource(resource.name.clone()))
        }
        Expression::Field { .. } => saved_leaf_type(program, path)
            .or_else(|| saved_group_entry_type(program, path))
            .or(Some(MarrowType::Unknown)),
        _ => None,
    }
}

fn saved_index_branch_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let (resource, index, arg_count) = saved_index_branch(program, path)?;
    if index.unique {
        return Some(MarrowType::Identity(resource.name.clone()));
    }
    let identity_arity = resource
        .saved_root
        .as_ref()
        .map_or(0, |root| root.identity_keys.len());
    let identity_start = index.args.len().saturating_sub(identity_arity);
    if arg_count < identity_start {
        return index
            .args
            .get(arg_count)
            .map(|name| index_component_type(program, resource, name));
    }
    Some(MarrowType::Identity(resource.name.clone()))
}

fn non_unique_index_branch_yields_identity(
    resource: &ResourceSchema,
    index: &IndexSchema,
    arg_count: usize,
) -> bool {
    if index.unique {
        return false;
    }
    let identity_arity = resource
        .saved_root
        .as_ref()
        .map_or(0, |root| root.identity_keys.len());
    let identity_start = index.args.len().saturating_sub(identity_arity);
    arg_count >= identity_start
}

fn index_component_type(
    program: &CheckedProgram,
    resource: &ResourceSchema,
    name: &str,
) -> MarrowType {
    if let Some(key) = resource
        .saved_root
        .as_ref()
        .and_then(|root| root.identity_keys.iter().find(|key| key.name == name))
    {
        return MarrowType::from_resolved(key.ty.clone(), TypeNames::default());
    }
    resource
        .field_type(&[name])
        .map(|ty| lift_member_type(ty.clone(), resource_module(program, &resource.name)))
        .unwrap_or(MarrowType::Unknown)
}

fn saved_index_branch<'p>(
    program: &'p CheckedProgram,
    path: &marrow_syntax::Expression,
) -> Option<(&'p ResourceSchema, &'p IndexSchema, usize)> {
    match path {
        marrow_syntax::Expression::Call { callee, args, .. } => {
            if args
                .iter()
                .any(|arg| arg.mode.is_some() || arg.name.is_some())
            {
                return None;
            }
            let (resource, index) = saved_index_schema(program, callee)?;
            (args.len() <= index.args.len()).then_some((resource, index, args.len()))
        }
        marrow_syntax::Expression::Field { .. } => {
            saved_index_schema(program, path).map(|(resource, index)| (resource, index, 0))
        }
        _ => None,
    }
}

fn saved_index_schema<'p>(
    program: &'p CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<(&'p ResourceSchema, &'p IndexSchema)> {
    let marrow_syntax::Expression::Field { base, name, .. } = callee else {
        return None;
    };
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = base.as_ref() else {
        return None;
    };
    let resource = find_resource_schema(program, root)?;
    let index = resource.indexes.iter().find(|index| &index.name == name)?;
    Some((resource, index))
}

fn is_saved_index_branch_path(program: &CheckedProgram, path: &marrow_syntax::Expression) -> bool {
    saved_index_branch(program, path).is_some()
}

fn is_saved_unique_index_branch_path(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
) -> bool {
    saved_index_branch(program, path).is_some_and(|(_, index, _)| index.unique)
}

/// The endpoint scalar type of a range iterable when both endpoints are the same
/// steppable type, or `None` for any other iterable or a mismatched/non-steppable
/// pair. A range binds its loop variable to this type.
fn range_endpoint_type(
    program: &CheckedProgram,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<MarrowType> {
    let (left, right) = range_endpoints(iterable)?;
    let left = infer_only(program, left, scope, aliases, file);
    let right = infer_only(program, right, scope, aliases, file);
    match (as_primitive(&left), as_primitive(&right)) {
        (Some(lo), Some(hi)) if lo == hi && is_steppable(lo) => Some(MarrowType::Primitive(lo)),
        _ => None,
    }
}

/// The two endpoint expressions of a range, or `None` if the iterable is not a
/// range.
fn range_endpoints(
    iterable: &marrow_syntax::Expression,
) -> Option<(&marrow_syntax::Expression, &marrow_syntax::Expression)> {
    use marrow_syntax::{BinaryOp, Expression};
    match iterable {
        Expression::Binary {
            op: BinaryOp::RangeExclusive | BinaryOp::RangeInclusive,
            left,
            right,
            ..
        } => Some((left, right)),
        _ => None,
    }
}

/// Reject ranges outside `for` iterables. A range is a loop shape, not a value
/// that can be stored, returned, thrown, passed, or evaluated for its own sake.
pub(crate) fn check_range_value(
    file: &Path,
    expr: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::{BinaryOp, Expression, InterpolationPart};
    match expr {
        Expression::Binary {
            op: BinaryOp::RangeExclusive | BinaryOp::RangeInclusive,
            left,
            right,
            span,
        } => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_RANGE_VALUE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: "a range can only be used as a `for` iterable".to_string(),
                span: *span,
            });
            check_range_value(file, left, diagnostics);
            check_range_value(file, right, diagnostics);
        }
        Expression::Binary { left, right, .. } => {
            check_range_value(file, left, diagnostics);
            check_range_value(file, right, diagnostics);
        }
        Expression::Unary { operand, .. } => check_range_value(file, operand, diagnostics),
        Expression::Call { callee, args, .. } => {
            check_range_value(file, callee, diagnostics);
            for arg in args {
                check_range_value(file, &arg.value, diagnostics);
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            check_range_value(file, base, diagnostics);
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    check_range_value(file, expr, diagnostics);
                }
            }
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {}
    }
}

fn check_range_iterable_value_parts(
    file: &Path,
    iterable: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if range_endpoints(iterable).is_none() {
        check_range_value(file, iterable, diagnostics);
    }
}

/// Validate a range-for header beyond what the operator and binding checks cover:
/// the endpoints must be the same steppable type, the `by` step (if any) must
/// match — a number for int/decimal, a duration for date/instant — decimal and
/// instant ranges require an explicit step, and a step that statically cannot run
/// (a literal wrong-direction step, or a zero step) is rejected as a dead loop. A
/// step on a non-range iterable is also rejected. The endpoint operator-typing is
/// already reported by the type pass, so a non-steppable or mismatched endpoint
/// pair is left to it; this pass owns the step and direction rules.
pub(crate) fn check_range_header(
    program: &CheckedProgram,
    file: &Path,
    iterable: &marrow_syntax::Expression,
    step: Option<&marrow_syntax::Expression>,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some((left, right)) = range_endpoints(iterable) else {
        // A step is only meaningful on a range; reject `by` on any other iterable.
        if let Some(step) = step {
            diagnostics.push(range_diagnostic(
                file,
                step.span(),
                "a `by` step applies only to a range".to_string(),
            ));
        }
        return;
    };
    let endpoint = match (
        as_primitive(&infer_only(program, left, scope, aliases, file)),
        as_primitive(&infer_only(program, right, scope, aliases, file)),
    ) {
        // A same-typed steppable endpoint pair is the only shape with step rules;
        // a non-steppable or mismatched pair is reported by the operator check.
        (Some(lo), Some(hi)) if lo == hi && is_steppable(lo) => lo,
        _ => return,
    };
    let step_type = step.map(|step| as_primitive(&infer_only(program, step, scope, aliases, file)));
    check_step_type(
        file,
        iterable.span(),
        endpoint,
        step,
        step_type,
        diagnostics,
    );
    check_temporal_step_sign(file, endpoint, step, diagnostics);
    check_date_step_whole_days(file, endpoint, step, diagnostics);
    check_dead_loop(file, iterable, left, right, step, diagnostics);
}

/// Reject a negated duration step on a `date`/`instant` range. A duration is always
/// non-negative — `-1.day` faults, duration subtraction is rejected, and
/// `parseDuration` rejects negatives — so a descending temporal range can never be
/// produced at runtime: such a loop only faults. Rather than green-light a guaranteed
/// fault, the check reports it now. Descending date/instant ranges are not yet
/// expressible; int/decimal ranges still descend by a negative step.
fn check_temporal_step_sign(
    file: &Path,
    endpoint: ScalarType,
    step: Option<&marrow_syntax::Expression>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if !matches!(endpoint, ScalarType::Date | ScalarType::Instant) {
        return;
    }
    let Some(step) = step else { return };
    if matches!(literal_int_sign(step), Some(sign) if sign < 0) {
        diagnostics.push(range_diagnostic(
            file,
            step.span(),
            format!(
                "{} range step must be a positive duration; descending temporal ranges are not yet supported",
                article_for(endpoint)
            ),
        ));
    }
}

/// Reject a literal duration step on a `date` range that is not a whole number of
/// days. A date has no time of day, so a sub-day or fractional-day step (`by 1.hour`,
/// `by 25.hours`) faults at runtime; the checker reports the guaranteed fault now. An
/// `instant` range carries a time component, so any positive duration steps it — this
/// rule is `date`-only. Only a literal step is statically known; a variable step that
/// is not a whole-day multiple still faults at runtime, which is correct.
fn check_date_step_whole_days(
    file: &Path,
    endpoint: ScalarType,
    step: Option<&marrow_syntax::Expression>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if endpoint != ScalarType::Date {
        return;
    }
    let Some(step) = step else { return };
    let Some(total_seconds) = literal_duration_seconds(step) else {
        return;
    };
    const SECONDS_PER_DAY: i64 = 86_400;
    if total_seconds % SECONDS_PER_DAY != 0 {
        diagnostics.push(range_diagnostic(
            file,
            step.span(),
            "a date range step must be a whole number of days".to_string(),
        ));
    }
}

/// The total seconds of a literal duration step (`1.hour` => 3600), or `None` for a
/// non-literal or non-duration step. A negated duration is read through the negation
/// so its magnitude is measured; the sign is handled separately.
fn literal_duration_seconds(expr: &marrow_syntax::Expression) -> Option<i64> {
    use marrow_syntax::{Expression, LiteralKind, UnaryOp, duration_unit_seconds};
    match expr {
        Expression::Literal {
            kind: LiteralKind::Duration,
            text,
            ..
        } => {
            let (magnitude, unit) = text.split_once('.')?;
            let magnitude: i64 = magnitude.parse().ok()?;
            magnitude.checked_mul(duration_unit_seconds(unit)?)
        }
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => literal_duration_seconds(operand),
        _ => None,
    }
}

/// The step-type rule: int/decimal endpoints step by a same-typed number;
/// date/instant endpoints step by a duration. Decimal and instant have no safe
/// default step, so omitting `by` there is an error; int defaults to 1 and date to
/// one calendar day. An untyped (`unknown`) step defers.
fn check_step_type(
    file: &Path,
    range_span: SourceSpan,
    endpoint: ScalarType,
    step: Option<&marrow_syntax::Expression>,
    step_type: Option<Option<ScalarType>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let expected = match endpoint {
        ScalarType::Int | ScalarType::Decimal => endpoint,
        // date and instant step by a duration span.
        _ => ScalarType::Duration,
    };
    match (step, step_type) {
        (Some(step), Some(Some(actual))) if actual != expected => {
            diagnostics.push(range_diagnostic(
                file,
                step.span(),
                format!(
                    "{} range steps by `{}`, not `{}`",
                    article_for(endpoint),
                    expected.name(),
                    actual.name(),
                ),
            ));
        }
        (Some(_), _) => {}
        // No `by`: decimal and instant require one; int and date have a default.
        (None, _) => {
            if matches!(endpoint, ScalarType::Decimal | ScalarType::Instant) {
                diagnostics.push(range_diagnostic(
                    file,
                    range_span,
                    format!(
                        "{} range needs an explicit `by` step",
                        article_for(endpoint)
                    ),
                ));
            }
        }
    }
}

/// A scalar named with its indefinite article and backtick spelling — `an `int``,
/// `a `decimal``, `an `instant`` — so a range diagnostic reads naturally for both
/// consonant- and vowel-initial type names. The two vowel-initial steppable
/// spellings are `int` and `instant`.
fn article_for(scalar: ScalarType) -> String {
    let article = if matches!(scalar, ScalarType::Int | ScalarType::Instant) {
        "an"
    } else {
        "a"
    };
    format!("{article} `{}`", scalar.name())
}

/// Reject a step that statically can never run. A zero step never progresses; a
/// literal wrong-direction step over literal endpoints (`1..10 by -1`,
/// `0.0..1.0 by -0.5`) is a dead loop. A variable endpoint or step is left to the
/// runtime, where a wrong direction is simply an empty loop and a zero step faults.
fn check_dead_loop(
    file: &Path,
    iterable: &marrow_syntax::Expression,
    left: &marrow_syntax::Expression,
    right: &marrow_syntax::Expression,
    step: Option<&marrow_syntax::Expression>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(step_sign) = literal_step_sign(step) else {
        return;
    };
    if step_sign == Ordering::Equal {
        diagnostics.push(range_diagnostic(
            file,
            iterable.span(),
            "a range step cannot be zero".to_string(),
        ));
        return;
    }
    // The endpoints' relative order: integer endpoints compare as integers, decimal
    // endpoints as decimals. A mismatched or non-literal pair yields `None` and is
    // left to the runtime.
    let endpoints = literal_int_value(left)
        .zip(literal_int_value(right))
        .map(|(lo, hi)| lo.cmp(&hi))
        .or_else(|| {
            literal_decimal_value(left)
                .zip(literal_decimal_value(right))
                .map(|(lo, hi)| lo.cmp(&hi))
        });
    let Some(endpoints) = endpoints else {
        return;
    };
    // An ascending step needs lo <= hi to run; a descending step needs lo >= hi.
    // Equal endpoints with `..` are also empty, but that is a legitimate empty loop,
    // not a wrong-direction bug, so only a provably wrong direction is flagged.
    let wrong_direction = (step_sign == Ordering::Greater && endpoints == Ordering::Greater)
        || (step_sign == Ordering::Less && endpoints == Ordering::Less);
    if wrong_direction {
        diagnostics.push(range_diagnostic(
            file,
            iterable.span(),
            "this range steps away from its end and never runs".to_string(),
        ));
    }
}

/// The direction of a literal step against zero — `Greater` ascending, `Less`
/// descending, `Equal` for a zero step — or `None` for a non-literal step (or an
/// omitted one, which defaults to the ascending unit step). Reads both the
/// int/duration sign and a decimal literal's sign so a dead decimal loop is caught.
fn literal_step_sign(step: Option<&marrow_syntax::Expression>) -> Option<Ordering> {
    let Some(step) = step else {
        return Some(Ordering::Greater);
    };
    literal_int_sign(step)
        .map(|sign| sign.cmp(&0))
        .or_else(|| literal_decimal_value(step).map(|value| value.cmp(&Decimal::ZERO)))
}

/// The value of a literal decimal expression (`0.5`, `-0.5`), or `None` for a
/// non-literal or non-decimal literal. Used to decide a static decimal range's
/// direction and step sign.
fn literal_decimal_value(expr: &marrow_syntax::Expression) -> Option<Decimal> {
    use marrow_syntax::{Expression, LiteralKind, UnaryOp};
    match expr {
        Expression::Literal {
            kind: LiteralKind::Decimal,
            text,
            ..
        } => Decimal::parse(text),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => literal_decimal_value(operand).and_then(|value| Decimal::ZERO.checked_sub(value)),
        _ => None,
    }
}

/// The signed value of a literal integer expression (`5`, `-1`), or `None` for a
/// non-literal or a duration/other literal. Used to decide a static range
/// direction; a duration step's sign is read separately.
fn literal_int_value(expr: &marrow_syntax::Expression) -> Option<i64> {
    use marrow_syntax::{Expression, LiteralKind, UnaryOp};
    match expr {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => text.parse().ok(),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => literal_int_value(operand).and_then(i64::checked_neg),
        _ => None,
    }
}

/// The sign (-1, 0, +1) of a literal step — an integer literal or a duration
/// literal, optionally negated — or `None` for a non-literal step. A duration
/// literal's magnitude carries the sign through the unary negation.
fn literal_int_sign(expr: &marrow_syntax::Expression) -> Option<i64> {
    use marrow_syntax::{Expression, LiteralKind, UnaryOp};
    match expr {
        Expression::Literal {
            kind: LiteralKind::Integer | LiteralKind::Duration,
            text,
            ..
        } => duration_or_int_magnitude(text).map(i64::signum),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => literal_int_sign(operand).map(|sign| -sign),
        _ => None,
    }
}

/// The leading magnitude of an int or duration literal as a signed `i64` for a
/// sign test: an int literal's value, or a duration literal's count before its
/// unit (`1.day` => 1). Saturates so a huge magnitude still reports its sign.
fn duration_or_int_magnitude(text: &str) -> Option<i64> {
    let digits = text.split('.').next().unwrap_or(text);
    digits
        .parse::<i64>()
        .ok()
        .or(Some(if digits.is_empty() { 0 } else { i64::MAX }))
}

fn range_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_RANGE,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        span,
    }
}

/// Type-check an `if`/`while` condition. Inferring it also operator-checks it;
/// then a condition whose type is a known primitive other than `bool` is flagged,
/// since conditions must be `bool`. An
/// unknown type — an unresolved call such as `exists(...)`, a saved-data read — is
/// left alone, so the check never fires on an uncertain condition.
pub(crate) fn check_condition(
    program: &CheckedProgram,
    file: &Path,
    condition: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let condition_type = infer_type(program, condition, scope, aliases, file, diagnostics);
    let span = condition.span();
    match as_primitive(&condition_type) {
        Some(primitive) if primitive != ScalarType::Bool => diagnostics.push(CheckDiagnostic {
            code: CHECK_CONDITION_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!("condition must be `bool`, found `{}`", primitive.name()),
            span,
        }),
        // Strict typing: a condition whose type cannot be resolved cannot be shown
        // to be `bool`.
        None if matches!(condition_type, MarrowType::Unknown) => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNTYPED_VALUE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: "condition has no known type; it must be `bool`".to_string(),
                span,
            });
        }
        // `Error` is a concrete (non-scalar) type, not an unknown one, so it cannot
        // be `bool`: flag it just like a wrong scalar (not as an untyped value).
        None if matches!(condition_type, MarrowType::Error) => diagnostics.push(CheckDiagnostic {
            code: CHECK_CONDITION_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: "condition must be `bool`, found `Error`".to_string(),
            span,
        }),
        // A concrete non-scalar — an identity, whole record, or sequence — is not
        // `bool`, so it is flagged like a wrong scalar rather than swallowed.
        None if is_concrete_nonscalar(&condition_type) => diagnostics.push(CheckDiagnostic {
            code: CHECK_CONDITION_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "condition must be `bool`, found `{}`",
                marrow_type_name(&condition_type)
            ),
            span,
        }),
        _ => {}
    }
}

/// Flag a `throw` whose operand is known to be something other than `Error`.
/// Unknown operands are left to the runtime backstop, as with other unresolved
/// values in this pass.
pub(crate) fn check_throw_type(
    file: &Path,
    span: SourceSpan,
    value_type: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match value_type {
        MarrowType::Error | MarrowType::Unknown => {}
        _ => diagnostics.push(CheckDiagnostic {
            code: CHECK_THROW_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "`throw` requires an `Error` value, found `{}`",
                marrow_type_name(value_type)
            ),
            span,
        }),
    }
}

/// Flag a `return` value whose type does not match the function's declared
/// return type. Fires only when both are known, incompatible primitives, so a
/// void function (unknown return type), a non-primitive return (a resource or
/// identity), or an unresolved returned value is left alone. Value presence is
/// checked separately by `check.return_value`.
pub(crate) fn check_return_type(
    file: &Path,
    span: SourceSpan,
    return_type: &MarrowType,
    value_type: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match type_compatible(return_type, value_type) {
        Some(true) => {}
        Some(false) => diagnostics.push(CheckDiagnostic {
            code: CHECK_RETURN_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "function returns `{}`, but this value is `{}`",
                marrow_type_name(return_type),
                marrow_type_name(value_type),
            ),
            span,
        }),
        // Strict typing: a value with no known type returned where a convertible type
        // is declared must be converted first. A void function (unknown return type),
        // or one returning a whole resource or a sequence (no conversion boundary),
        // places no such constraint and is left alone.
        None if matches!(value_type, MarrowType::Unknown) && expects_conversion(return_type) => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNTYPED_VALUE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "this `return` value has no known type, but the function returns `{}`; convert it first",
                    marrow_type_name(return_type),
                ),
                span,
            });
        }
        None => {}
    }
}

/// Flag a value stored into a concrete place when its type is wrong or cannot be
/// resolved. A known-incompatible value is a `check.assignment_type` mismatch; an
/// `Unknown` value stored into a place with a conversion boundary (a scalar, an
/// identity, an enum, a whole resource) is a `check.untyped_value` error (strict
/// typing: dynamic data must be converted before typed use). An untyped place (a
/// sequence, `unknown`) is left alone. A whole group-entry assignment may take a
/// value of the owning resource type because the runtime writes matching fields
/// from that resource value into the addressed group entry.
pub(crate) fn check_assignment(
    file: &Path,
    span: SourceSpan,
    place: &MarrowType,
    value: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let compatible = match (place, value) {
        (MarrowType::GroupEntry { resource, .. }, MarrowType::Resource(value_resource)) => {
            Some(resource == value_resource)
        }
        _ => type_compatible(place, value),
    };
    match compatible {
        Some(true) => {}
        Some(false) => {
            let (expected, found) = mismatch_display(place, value);
            diagnostics.push(CheckDiagnostic {
                code: CHECK_ASSIGNMENT_TYPE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!("expected `{expected}`, but the value is `{found}`"),
                span,
            });
        }
        // A value the checker could not resolve, stored into a convertible place. An
        // untyped place (a sequence, `unknown`) has no conversion boundary and is
        // left alone.
        None if matches!(value, MarrowType::Unknown) && expects_conversion(place) => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNTYPED_VALUE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "the value stored into `{}` has no known type; convert it before typed use",
                    marrow_type_name(place),
                ),
                span,
            });
        }
        None => {}
    }
}

/// Type-check the key arguments of a saved access against the keys it addresses.
/// A record lookup `^root(key…)` is checked against the root's identity keys; a
/// keyed-layer access `^root(key…).layer(key…)` against that layer's key
/// parameters. A foreign identity spliced into a keyspace, or a scalar of the
/// wrong type, is a `check.key_type`. Non-saved callees (a function call, an index
/// lookup) and unresolved roots are left alone.
pub(crate) fn check_saved_key_args(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Expression;
    // A whole-record lookup `^root(key…)`: the sole identity argument may be the
    // resource's own identity value (a splice), checked nominally; otherwise the
    // per-key scalars are checked against the declared identity keys.
    if let Expression::SavedRoot { name: root, .. } = callee {
        let Some(resource) = find_resource_schema(program, root) else {
            return;
        };
        let Some(saved_root) = &resource.saved_root else {
            return;
        };
        if let [MarrowType::Identity(spliced)] = arg_types {
            // A bare identity names a resource in the accessor's own module and is
            // matched nominally against the root. A qualified identity imported from
            // another module keeps its module path, which cannot be placed against
            // the root's bare resource name without the unified type IR, so it
            // defers to the runtime key guard rather than being rejected here.
            if !spliced.contains("::") && spliced != &resource.name {
                diagnostics.push(key_type_diagnostic(
                    file,
                    span,
                    format!(
                        "`^{root}` is addressed by `{}::Id`, but this value is `{spliced}::Id`",
                        resource.name,
                    ),
                ));
            }
            return;
        }
        check_keys_against(
            &saved_root.identity_keys,
            arg_types,
            span,
            file,
            diagnostics,
        );
        return;
    }
    // A keyed-layer access `^root(key…).layer(key…)`: check this layer's key
    // parameters. The layer chain peels the named layers from the accessor.
    if let Some((root, layers)) = saved_layer_chain(callee)
        && let Some(resource) = find_resource_schema(program, root)
        && let Some(node) = resource.descend_layers(&layers)
    {
        check_keys_against(&node.key_params, arg_types, span, file, diagnostics);
    }
}

/// Compare a saved access's argument types against the declared key parameters
/// they fill. A count mismatch is reported once (the per-key mapping is then
/// undefined); otherwise each argument is checked nominally against its key's
/// type, with an `unknown` argument deferred to the runtime.
pub(crate) fn check_keys_against(
    keys: &[marrow_schema::KeyDef],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if keys.len() != arg_types.len() {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "this keyed access expects {} key argument(s), but {} were given",
                keys.len(),
                arg_types.len(),
            ),
        ));
        return;
    }
    for (key, arg_type) in keys.iter().zip(arg_types) {
        let expected = MarrowType::from_resolved(key.ty.clone(), TypeNames::default());
        if type_compatible(&expected, arg_type) == Some(false) {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "key `{}` expects `{}`, but this value is `{}`",
                    key.name,
                    marrow_type_name(&expected),
                    marrow_type_name(arg_type),
                ),
            ));
        }
    }
}

/// Type-check the key arguments of an identity constructor `Name::Id(key…)`
/// against the referenced resource's declared identity keys. Positional keys map
/// by position; named (composite) keys map to the declared key of the same name,
/// so a swapped type under the right name is still caught. A wrong-scalar key, or
/// a non-key value such as another identity, is a `check.key_type` — the same
/// keyspace guard a record lookup passes. Malformed constructor argument shape
/// (wrong count, mixed positional/named keys, unknown key names, duplicate key
/// names, or missing named keys) is a `check.call_argument`.
pub(crate) fn check_identity_constructor_keys(
    label: &str,
    keys: &[marrow_schema::KeyDef],
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let named = args.iter().filter(|arg| arg.name.is_some()).count();
    if named != 0 && named != args.len() {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!("`{label}` takes either positional or named keys, not both"),
        ));
        return;
    }

    if named == 0 {
        if keys.len() != args.len() {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!(
                    "`{label}` expects {} key argument(s), but {} were given",
                    keys.len(),
                    args.len(),
                ),
            ));
            return;
        }
        for (key, arg_type) in keys.iter().zip(arg_types) {
            check_identity_key_type(key, arg_type, span, file, diagnostics);
        }
        return;
    }

    let mut supplied = vec![false; keys.len()];
    let mut malformed_names = false;
    for (arg, arg_type) in args.iter().zip(arg_types) {
        let Some(name) = &arg.name else {
            continue;
        };
        let Some(index) = keys.iter().position(|key| &key.name == name) else {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` has no identity key `{name}`"),
            ));
            malformed_names = true;
            continue;
        };
        if supplied[index] {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("identity key `{name}` is supplied more than once"),
            ));
            malformed_names = true;
            continue;
        }
        supplied[index] = true;
        check_identity_key_type(&keys[index], arg_type, span, file, diagnostics);
    }

    if malformed_names {
        return;
    }
    for (key, supplied) in keys.iter().zip(supplied) {
        if !supplied {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` requires identity key `{}`", key.name),
            ));
        }
    }
}

fn check_identity_key_type(
    key: &marrow_schema::KeyDef,
    arg_type: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let expected = MarrowType::from_resolved(key.ty.clone(), TypeNames::default());
    if type_compatible(&expected, arg_type) == Some(false) {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "identity key `{}` expects `{}`, but this value is `{}`",
                key.name,
                marrow_type_name(&expected),
                marrow_type_name(arg_type),
            ),
        ));
    }
}

/// A `check.key_type` diagnostic located at a saved access's span.
pub(crate) fn key_type_diagnostic(
    file: &Path,
    span: SourceSpan,
    message: String,
) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_KEY_TYPE,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        span,
    }
}

/// Validate a unary operator against its operand type, returning the result type,
/// or [`MarrowType::Unknown`] when the operand is not a known primitive or the
/// operator is misused (which records a diagnostic).
pub(crate) fn check_unary(
    op: marrow_syntax::UnaryOp,
    operand: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::UnaryOp;
    if matches!(operand, MarrowType::Invalid) {
        return MarrowType::Invalid;
    }
    // A concrete non-scalar operand — an identity, record, sequence, or the
    // checker-only `Error` — has no unary operator, so flag it as an operator
    // misuse rather than silently passing it through. This must precede the
    // `as_primitive` gate, which treats every non-primitive as `None` and would
    // otherwise drop these to `Unknown`.
    if matches!(operand, MarrowType::Error) || is_concrete_nonscalar(operand) {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}`",
                unary_symbol(op),
                marrow_type_name(operand),
            ),
        ));
        return MarrowType::Invalid;
    }
    let Some(operand) = as_primitive(operand) else {
        return MarrowType::Unknown;
    };
    let valid = match op {
        UnaryOp::Neg => is_numeric(operand),
        UnaryOp::Not => operand == ScalarType::Bool,
    };
    if !valid {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}`",
                unary_symbol(op),
                operand.name(),
            ),
        ));
        return MarrowType::Unknown;
    }
    MarrowType::Primitive(operand)
}

/// Validate a binary operator against its operand types, returning the result
/// type, or [`MarrowType::Unknown`] when either operand is not a known primitive
/// or the operator is misused (which records a diagnostic).
pub(crate) fn check_binary(
    op: marrow_syntax::BinaryOp,
    left: &MarrowType,
    right: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::BinaryOp;
    if matches!(left, MarrowType::Invalid) || matches!(right, MarrowType::Invalid) {
        return MarrowType::Invalid;
    }
    // `Error` is a concrete type, not an untyped one: no binary operator applies to
    // it, so flag it as an operator misuse rather than silently passing it through
    // (matching the unary case). This must come before the `as_primitive` gate,
    // which treats `Error` as a non-primitive `None` and would otherwise skip it.
    if matches!(left, MarrowType::Error) || matches!(right, MarrowType::Error) {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `Error`",
                binary_symbol(op)
            ),
        ));
        return MarrowType::Invalid;
    }
    // Equality is decided over concrete non-scalar types before the `as_primitive`
    // gate, which would otherwise drop them to `Unknown`. Whole records and
    // sequences have no equality; identities and enums compare nominally, so a
    // same-resource identity or same-enum pair is equatable (`bool`) while a
    // cross-resource pair, a different enum, or either against a scalar is a
    // category error. An `Unknown` operand defers to the scalar path, where untyped
    // values are handled.
    if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual)
        && let Some(result) = check_equality(op, left, right, span, file, diagnostics)
    {
        return result;
    }
    // No non-equality operator applies to a concrete non-scalar operand — an
    // identity, whole record, sequence, or enum. Flag it as an operator misuse
    // rather than letting the scalar gate below drop it to `Unknown`. An `Unknown`
    // operand still defers there, where untyped values are handled.
    if is_concrete_nonscalar(left) || is_concrete_nonscalar(right) {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}` and `{}`",
                binary_symbol(op),
                marrow_type_name(left),
                marrow_type_name(right),
            ),
        ));
        return MarrowType::Invalid;
    }
    let (Some(left), Some(right)) = (as_primitive(left), as_primitive(right)) else {
        return MarrowType::Unknown;
    };
    // Each arm is (operator accepts these operands, result type when it does).
    let (valid, result) = match op {
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply => (
            is_numeric(left) && left == right,
            MarrowType::Primitive(left),
        ),
        BinaryOp::Divide => (
            is_numeric(left) && left == right,
            MarrowType::Primitive(ScalarType::Decimal),
        ),
        BinaryOp::Remainder => (
            left == ScalarType::Int && right == ScalarType::Int,
            MarrowType::Primitive(ScalarType::Int),
        ),
        BinaryOp::Concat => (
            left == ScalarType::Str && right == ScalarType::Str,
            MarrowType::Primitive(ScalarType::Str),
        ),
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => (
            is_ordered(left) && left == right,
            MarrowType::Primitive(ScalarType::Bool),
        ),
        BinaryOp::Equal | BinaryOp::NotEqual => {
            (left == right, MarrowType::Primitive(ScalarType::Bool))
        }
        BinaryOp::And | BinaryOp::Or => (
            left == ScalarType::Bool && right == ScalarType::Bool,
            MarrowType::Primitive(ScalarType::Bool),
        ),
        // A range is not a value an operator consumes; accept two endpoints of the
        // same steppable type. The endpoint typing, step, and direction rules are a
        // separate range-for check, so this only rejects a non-steppable or
        // mismatched endpoint pairing.
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => {
            (is_steppable(left) && left == right, MarrowType::Unknown)
        }
        // `??` constrains its operands by the path's leaf type, not by scalar
        // shape alone, so it is typed in `check_coalesce` before reaching here.
        BinaryOp::Coalesce => (left == right, MarrowType::Primitive(left)),
        // `is` is the nominal enum-subtree predicate, typed in `check_is` before
        // reaching here; a scalar operand never satisfies it.
        BinaryOp::Is => (false, MarrowType::Primitive(ScalarType::Bool)),
    };
    if !valid {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}` and `{}`",
                binary_symbol(op),
                left.name(),
                right.name(),
            ),
        ));
        return MarrowType::Unknown;
    }
    result
}

/// Decide `==`/`!=` over concrete non-scalar operands, returning `Some(result)`
/// once a verdict is reached and `None` to defer to the scalar path. Whole records
/// and sequences are not equatable; identities and enums compare nominally, so a
/// same-resource identity or same-enum pair is `bool` and any other pairing —
/// different identities, different enums, or either against a scalar — is a
/// category error. An `Unknown` operand defers (the untyped-value path owns it); a
/// scalar pair defers to the ordinary scalar-equality check. A diagnostic is pushed
/// on the rejected cases, which still yield `bool` so a surrounding expression sees
/// the natural result type of a comparison.
pub(crate) fn check_equality(
    op: marrow_syntax::BinaryOp,
    left: &MarrowType,
    right: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<MarrowType> {
    let reject = |diagnostics: &mut Vec<CheckDiagnostic>| {
        let (left_name, right_name) = mismatch_display(left, right);
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot compare `{left_name}` and `{right_name}`",
                binary_symbol(op),
            ),
        ));
        Some(MarrowType::Primitive(ScalarType::Bool))
    };
    match (left, right) {
        (MarrowType::Invalid, _) | (_, MarrowType::Invalid) => None,
        // An untyped operand defers: the scalar path handles untyped values.
        (MarrowType::Unknown, _) | (_, MarrowType::Unknown) => None,
        // Whole records and sequences have no equality at all.
        (
            MarrowType::Resource(_)
            | MarrowType::GroupEntry { .. }
            | MarrowType::Sequence(_)
            | MarrowType::LocalTree { .. },
            _,
        )
        | (
            _,
            MarrowType::Resource(_)
            | MarrowType::GroupEntry { .. }
            | MarrowType::Sequence(_)
            | MarrowType::LocalTree { .. },
        ) => reject(diagnostics),
        // Identities compare nominally: equatable only against the same resource.
        (MarrowType::Identity(a), MarrowType::Identity(b)) => {
            if a == b {
                Some(MarrowType::Primitive(ScalarType::Bool))
            } else {
                reject(diagnostics)
            }
        }
        // An identity against a scalar, enum, or `Error` is a category error.
        (MarrowType::Identity(_), _) | (_, MarrowType::Identity(_)) => reject(diagnostics),
        // Enums compare nominally: equatable only against the same enum, by owning
        // module and name, so two same-named enums in different modules are not.
        (MarrowType::Enum { .. }, MarrowType::Enum { .. }) => {
            if left == right {
                Some(MarrowType::Primitive(ScalarType::Bool))
            } else {
                reject(diagnostics)
            }
        }
        // An enum against a scalar or `Error` is a category error.
        (MarrowType::Enum { .. }, _) | (_, MarrowType::Enum { .. }) => reject(diagnostics),
        // Two scalars (or `Error`, which the caller already rejected) defer to the
        // ordinary scalar-equality path.
        (
            MarrowType::Primitive(_) | MarrowType::Error,
            MarrowType::Primitive(_) | MarrowType::Error,
        ) => None,
    }
}

/// Type-check `path ?? default`. The result is the leaf type of the path read on
/// the left (a populated read yields that value; an absent one yields the
/// default), so the default must be the same scalar type. A non-path left operand
/// is rejected: only a read that can be absent has anything to default.
pub(crate) fn check_coalesce(
    left: &marrow_syntax::Expression,
    left_type: &MarrowType,
    right_type: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    if matches!(left_type, MarrowType::Invalid) || matches!(right_type, MarrowType::Invalid) {
        return MarrowType::Invalid;
    }
    if !is_path_read(left) {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            "operator `??` applies only to a path read or `?.` chain".to_string(),
        ));
        return MarrowType::Unknown;
    }
    // A concrete non-scalar leaf (an identity, record, or sequence read) defaults
    // only with a value of the same nominal type, so a `Book::Id` leaf cannot take
    // a `Magazine::Id` default, and a non-scalar paired with a scalar is a category
    // error either way. The scalar path below would drop the non-scalar to `Unknown`
    // and silently accept the mismatch, so resolve any pairing with a non-scalar
    // side here; an `Unknown` operand still defers there.
    if is_concrete_nonscalar(left_type) || is_concrete_nonscalar(right_type) {
        return match type_compatible(left_type, right_type) {
            Some(true) => left_type.clone(),
            Some(false) => {
                diagnostics.push(operator_diagnostic(
                    file,
                    span,
                    format!(
                        "operator `??` cannot default `{}` with `{}`",
                        marrow_type_name(left_type),
                        marrow_type_name(right_type),
                    ),
                ));
                MarrowType::Unknown
            }
            None => left_type.clone(),
        };
    }
    // Both sides must be the same scalar, like the other value operators. When
    // either is still untyped, defer rather than guess, yielding the known side
    // (or `Unknown`) so a surrounding operator never fires on an uncertain operand.
    match (as_primitive(left_type), as_primitive(right_type)) {
        (Some(leaf), Some(default)) if leaf == default => MarrowType::Primitive(leaf),
        (Some(leaf), Some(default)) => {
            diagnostics.push(operator_diagnostic(
                file,
                span,
                format!(
                    "operator `??` cannot default `{}` with `{}`",
                    leaf.name(),
                    default.name(),
                ),
            ));
            MarrowType::Unknown
        }
        // An untyped leaf falls back to the default's type; an untyped default
        // leaves the result the leaf type. Either way an unknown stays unknown.
        (None, _) => right_type.clone(),
        (Some(leaf), None) => MarrowType::Primitive(leaf),
    }
}

/// Whether an expression is a path read whose value can be absent — the only
/// left operand `??` accepts. A field read (`book.title`, `^books(id).title`),
/// an optional field read (`book?.shelf`), or a call-shaped saved read
/// (`^books(id)`, `^books(id).tags(1)`) can be absent; a bare local, literal, or
/// computed value is always present and has nothing to default.
pub(crate) fn is_path_read(expr: &marrow_syntax::Expression) -> bool {
    use marrow_syntax::Expression;
    matches!(
        expr,
        Expression::Field { .. } | Expression::OptionalField { .. } | Expression::Call { .. }
    )
}

pub(crate) fn operator_diagnostic(
    file: &Path,
    span: SourceSpan,
    message: String,
) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_OPERATOR_TYPE,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        span,
    }
}

/// Validate a call against the user function it resolves to and return that
/// function's declared return type (or [`MarrowType::Unknown`]). Only a plain
/// name call that resolves to a declared function is checked; a builtin, std
/// helper, `Error` constructor, key-lookup (non-name callee), or
/// unresolved name is left alone — mirroring the runtime's dispatch order — so the
/// check never fires on a non-function or a call the checker cannot resolve.
///
/// It flags the argument count (every parameter is required), named arguments that
/// name no parameter, out/inout marker mismatches, non-place out/inout arguments,
/// and arguments whose type does not match the corresponding parameter.
// Each argument is an independent input threaded through the type-check pipeline
// (program/aliases/file/diagnostics are the cross-cutting context every node
// carries, like `scope`); bundling them would not aid clarity here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_call(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    aliases: &HashMap<String, Vec<String>>,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    let marrow_syntax::Expression::Name { segments, .. } = callee else {
        check_plain_call_modes("call", args, span, file, diagnostics);
        return MarrowType::Unknown;
    };
    // Expand a short-form leading segment through the file's imports once, up front,
    // so `clock::now()` resolves like `std::clock::now()` and a project `books::add`
    // like `shelf::books::add`. All downstream resolution uses the expanded form.
    let expanded = expand_alias(segments, aliases);
    let segments = expanded.as_slice();
    // `nextId(^root)` needs the argument *expression* (the `^root`), not just its
    // type, to know which resource is allocated — so it is handled here, before the
    // generic builtin branch typed only from `arg_types`. It types to `Resource::Id`
    // for a single-`int` root and reports `check.next_id_requires_single_int` for
    // any other shape (composite/non-int/singleton).
    if let [name] = segments
        && name == "nextId"
    {
        check_plain_call_modes(name, args, span, file, diagnostics);
        return check_next_id(program, args, span, file, diagnostics);
    }
    // `reversed` needs the argument expression for saved collections, whose element
    // sequence type is not always the argument's direct value type. `next`/`prev`
    // likewise need the `^path` expression to find the resource/layer they navigate,
    // so these resolve here before the generic builtin branch typed only from
    // `arg_types`.
    if let [name] = segments {
        match name.as_str() {
            "reversed" => {
                check_plain_call_modes(name, args, span, file, diagnostics);
                check_arity(name, 1, args, span, file, diagnostics);
                return reversed_type(program, args, arg_types);
            }
            "next" | "prev" => {
                check_plain_call_modes(name, args, span, file, diagnostics);
                check_arity(name, 1, args, span, file, diagnostics);
                return check_neighbor(program, name, args, arg_types, span, file, diagnostics);
            }
            "values" | "entries" => {
                check_plain_call_modes(name, args, span, file, diagnostics);
                check_arity(name, 1, args, span, file, diagnostics);
                check_value_materialization_args(program, name, args, span, file, diagnostics);
                return MarrowType::Unknown;
            }
            "append" => {
                check_plain_call_modes(name, args, span, file, diagnostics);
                check_arity(name, 2, args, span, file, diagnostics);
                check_append_args(program, args, span, file, diagnostics);
                check_append(program, args, span, file, diagnostics);
                return MarrowType::Primitive(ScalarType::Int);
            }
            _ => {}
        }
    }
    // Builtins dispatch before user functions. Std helpers have fixed signatures,
    // and a few single-name builtins have static shape rules; the rest leave their
    // arguments to the runtime. A std helper's return type feeds the surrounding
    // type checks.
    if is_builtin_call(segments) {
        check_plain_call_modes(&segments.join("::"), args, span, file, diagnostics);
        check_builtin_call_args(segments, arg_types, span, file, diagnostics);
        if let Some(params) = std_call_params(segments) {
            check_args_against(
                &segments.join("::"),
                &params,
                arg_types,
                span,
                file,
                diagnostics,
            );
        }
        return std_call_return_type(segments)
            .or_else(|| conversion_return_type(segments))
            .or_else(|| builtin_return_type(segments, arg_types))
            // The `Error(...)` constructor builds a builtin Error value, so it types
            // as `Error` (not `Unknown`) — e.g. `std::log::error(Error(...))` and
            // `throw Error(...)` both expect an `Error`.
            .or_else(|| (segments == ["Error"]).then_some(MarrowType::Error))
            .unwrap_or(MarrowType::Unknown);
    }
    // Calls resolve from the module the file contributes: a bare name in its own
    // module, a qualified `mod::name` in the named module (cross-module needs the
    // qualifier and the target must be `pub`). A module-less script contributes the
    // empty module, so its bare calls resolve against its own functions.
    let from_module = module_of_file(program, file).unwrap_or_default();
    // `Book::Id(...)` is the identity constructor even if `Book::Id` could also
    // spell a qualified resource name. Keep the checker aligned with runtime
    // dispatch, where identity constructors run before resource values.
    if let Some((ty @ MarrowType::Identity(_), resource)) =
        resource_constructor_resource(program, from_module, segments)
    {
        check_plain_call_modes(&segments.join("::"), args, span, file, diagnostics);
        if let Some(saved_root) = &resource.saved_root {
            check_identity_constructor_keys(
                &format!("{}::Id", resource.name),
                &saved_root.identity_keys,
                args,
                arg_types,
                span,
                file,
                diagnostics,
            );
        }
        return ty;
    }
    // A callee naming a declared resource is a value constructor, not a function:
    // `Book(...)` and `module::Book(...)` build resource values. Recognize it so
    // a valid constructor is not a false `check.unresolved_call`.
    if let Resolution::Found(Def {
        module,
        item: DefItem::Resource(resource),
        ..
    }) = resolve(program, from_module, segments, ResolvableKind::Resource)
    {
        check_plain_call_modes(&segments.join("::"), args, span, file, diagnostics);
        let resource_names: Vec<String> = module
            .resources
            .iter()
            .map(|resource| resource.name.clone())
            .collect();
        let enum_names: Vec<String> = module
            .enums
            .iter()
            .map(|enum_| enum_.name.clone())
            .collect();
        check_resource_constructor_args(
            &resource.name,
            resource,
            TypeNames {
                module: &module.name,
                resources: &resource_names,
                enums: &enum_names,
            },
            args,
            arg_types,
            span,
            file,
            diagnostics,
        );
        return MarrowType::Resource(resource.name.clone());
    }
    // Resolving as a `Function` can only ever Find a function, so the other arms
    // never carry a non-function item. Only calls in a file that is part of the
    // program are reported, and a project that did not fully parse has its
    // unresolved calls suppressed in `check_project` (the missing definition may
    // live in an excluded module).
    let function = match resolve(program, from_module, segments, ResolvableKind::Function) {
        Resolution::Found(Def {
            item: DefItem::Function(function),
            ..
        }) => function,
        // The function exists but is not `pub` to this module: a distinct
        // visibility error, not "unresolved" (the name resolved, the access did
        // not).
        Resolution::NotVisible(name) => {
            if file_in_program(program, file) {
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_PRIVATE_FUNCTION,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
                    message: format!(
                        "function `{name}` is private to its module; mark it `pub` to call it \
                         from another module"
                    ),
                    span,
                });
            }
            return MarrowType::Unknown;
        }
        // A bare name that names a `pub` function in two or more modules: each is
        // reachable, but only as `module::fn`; the bare spelling must be qualified.
        Resolution::Ambiguous(candidates) => {
            if file_in_program(program, file) {
                let leaf = segments.join("::");
                let options = candidates
                    .iter()
                    .map(|module| format!("`{module}::{leaf}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_AMBIGUOUS_CALL,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
                    message: format!(
                        "call to `{leaf}` is ambiguous; qualify it as one of {options}"
                    ),
                    span,
                });
            }
            return MarrowType::Unknown;
        }
        // A non-builtin call that resolves to no declared function is unresolved.
        Resolution::Found(_) | Resolution::Unresolved => {
            if file_in_program(program, file) {
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNRESOLVED_CALL,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
                    message: format!("function `{}` is not defined", segments.join("::")),
                    span,
                });
            }
            return MarrowType::Unknown;
        }
    };
    // Every parameter is required (no defaults), so the argument count must match.
    if args.len() != function.params.len() {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "function `{}` expects {} argument(s), but {} were given",
                segments.join("::"),
                function.params.len(),
                args.len(),
            ),
        ));
    }
    // Match each argument to its parameter — positional by position, named by name
    // (the parser guarantees positional arguments precede named ones) — flagging a
    // named argument that names no parameter, a parameter supplied more than once,
    // and an argument whose known primitive type differs from the parameter's.
    let callee = segments.join("::");
    let mut supplied = vec![false; function.params.len()];
    for (index, (arg, arg_type)) in args.iter().zip(arg_types).enumerate() {
        let param_index = match &arg.name {
            Some(name) => {
                let param_index = function.params.iter().position(|param| &param.name == name);
                if param_index.is_none() {
                    diagnostics.push(call_diagnostic(
                        file,
                        span,
                        format!("function `{callee}` has no parameter `{name}`"),
                    ));
                }
                param_index
            }
            None => function.params.get(index).map(|_| index),
        };
        // Every concrete parameter type — scalar, identity, resource, sequence,
        // enum, or the checker-only `Error` — is checked nominally; an `unknown`
        // parameter places no constraint and is left to the runtime.
        if let Some(param_index) = param_index {
            let param = &function.params[param_index];
            if supplied[param_index] {
                diagnostics.push(call_diagnostic(
                    file,
                    span,
                    format!(
                        "function `{callee}` parameter `{}` is supplied more than once",
                        param.name
                    ),
                ));
                continue;
            }
            supplied[param_index] = true;
            check_call_mode(&callee, arg, param.mode, span, file, diagnostics);
            check_one_arg(&callee, &param.ty, arg_type, span, file, diagnostics);
        }
    }
    function.return_type.clone().unwrap_or(MarrowType::Unknown)
}

fn check_builtin_call_args(
    segments: &[String],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [name] = segments else { return };
    if let Some(target) = ScalarType::from_scalar_name(name) {
        check_conversion_arg(name, target, arg_types, span, file, diagnostics);
    }
}

fn check_value_materialization_args(
    program: &CheckedProgram,
    name: &str,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [arg] = args else { return };
    if arg.mode.is_some() || arg.name.is_some() || !is_saved_index_branch_path(program, &arg.value)
    {
        return;
    }
    diagnostics.push(CheckDiagnostic {
        code: CHECK_COLLECTION_UNSUPPORTED,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: format!("`{name}` cannot materialize values from an index branch; use `keys`"),
        span,
    });
}

fn check_append_args(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [target, _value] = args else { return };
    let Some(node) = saved_layer_node(program, &target.value) else {
        return;
    };
    if matches!(node.kind, marrow_schema::NodeKind::Group) {
        diagnostics.push(call_diagnostic(
            file,
            span,
            "`append` target must be a keyed leaf layer, but this path names a group layer"
                .to_string(),
        ));
    }
}

fn saved_layer_node<'p>(
    program: &'p CheckedProgram,
    expr: &marrow_syntax::Expression,
) -> Option<&'p marrow_schema::Node> {
    let (root, layers) = saved_group_chain(expr)?;
    find_resource_schema(program, root)?.descend_layers(&layers)
}

fn check_conversion_arg(
    target_name: &str,
    target: ScalarType,
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [arg_type] = arg_types else { return };
    if conversion_source_supported(target_name, target, arg_type) {
        return;
    }
    diagnostics.push(call_diagnostic(
        file,
        span,
        format!(
            "`{target_name}` cannot convert `{}`; supported sources are {}",
            marrow_type_name(arg_type),
            conversion_supported_sources(target_name, target)
        ),
    ));
}

fn conversion_source_supported(target_name: &str, target: ScalarType, source: &MarrowType) -> bool {
    use ScalarType::{Bool, Bytes, Date, Decimal, Duration, Instant, Int, Str};
    match source {
        MarrowType::Unknown | MarrowType::Invalid => true,
        MarrowType::Primitive(source) => match target_name {
            "ErrorCode" => *source == Str,
            "bool" => matches!(source, Bool | Int),
            "string" => matches!(
                source,
                Str | Int | Decimal | Bool | Bytes | Date | Instant | Duration
            ),
            _ => match target {
                Int => matches!(source, Int | Str | Decimal),
                Decimal => matches!(source, Decimal | Int | Str),
                Bytes => matches!(source, Bytes | Str),
                Date => matches!(source, Date | Str),
                Instant => matches!(source, Instant | Str),
                Duration => matches!(source, Duration | Str),
                Str => *source == Str,
                Bool => matches!(source, Bool | Int),
            },
        },
        MarrowType::Enum { .. }
        | MarrowType::Error
        | MarrowType::GroupEntry { .. }
        | MarrowType::Identity(_)
        | MarrowType::LocalTree { .. }
        | MarrowType::Resource(_)
        | MarrowType::Sequence(_) => false,
    }
}

fn conversion_supported_sources(target_name: &str, target: ScalarType) -> &'static str {
    use ScalarType::{Bytes, Date, Decimal, Duration, Instant, Int};
    match target_name {
        "ErrorCode" => "`ErrorCode`, `string`, or `unknown`",
        "bool" => "`bool`, `int`, or `unknown`",
        "string" => {
            "`string`, `int`, `decimal`, `bool`, `bytes`, `date`, `instant`, `duration`, or `unknown`"
        }
        _ => match target {
            Int => "`int`, canonical integer `string`, integral `decimal`, or `unknown`",
            Decimal => "`decimal`, `int`, canonical decimal `string`, or `unknown`",
            Bytes => "`bytes`, UTF-8 `string`, or `unknown`",
            Date => "`date`, canonical date `string`, or `unknown`",
            Instant => "`instant`, canonical instant `string`, or `unknown`",
            Duration => "`duration`, canonical duration `string`, or `unknown`",
            _ => "`unknown`",
        },
    }
}

fn check_plain_call_modes(
    label: &str,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for arg in args {
        if let Some(mode) = arg.mode {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!(
                    "argument to `{label}` cannot be passed as {}",
                    arg_mode_name(mode)
                ),
            ));
        }
    }
}

fn check_call_mode(
    label: &str,
    arg: &marrow_syntax::Argument,
    param_mode: Option<marrow_syntax::ParamMode>,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if !call_modes_match(arg.mode, param_mode) {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "argument to `{label}` must be passed as {}",
                call_mode_expectation(param_mode)
            ),
        ));
    }
    if arg.mode.is_some() && !crate::rules::is_assignable(&arg.value) {
        diagnostics.push(CheckDiagnostic {
            code: crate::rules::CHECK_INVALID_ASSIGN_TARGET,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: "out/inout argument is not a writable place".to_string(),
            span: arg.value.span(),
        });
    }
}

fn arg_mode_name(mode: marrow_syntax::ArgMode) -> &'static str {
    match mode {
        marrow_syntax::ArgMode::Out => "`out`",
        marrow_syntax::ArgMode::InOut => "`inout`",
    }
}

fn call_modes_match(
    arg: Option<marrow_syntax::ArgMode>,
    param: Option<marrow_syntax::ParamMode>,
) -> bool {
    matches!(
        (arg, param),
        (None, None)
            | (
                Some(marrow_syntax::ArgMode::Out),
                Some(marrow_syntax::ParamMode::Out)
            )
            | (
                Some(marrow_syntax::ArgMode::InOut),
                Some(marrow_syntax::ParamMode::InOut)
            )
    )
}

fn call_mode_expectation(mode: Option<marrow_syntax::ParamMode>) -> &'static str {
    match mode {
        Some(marrow_syntax::ParamMode::Out) => "`out`",
        Some(marrow_syntax::ParamMode::InOut) => "`inout`",
        None => "a plain argument",
    }
}

fn reversed_type(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
) -> MarrowType {
    if let [arg] = args
        && arg.mode.is_none()
        && arg.name.is_none()
        && let Some((element, None)) = collection_loop_binding_types(program, false, &arg.value)
    {
        return MarrowType::Sequence(Box::new(element));
    }
    arg_types.first().cloned().unwrap_or(MarrowType::Unknown)
}

#[allow(clippy::too_many_arguments)]
fn check_resource_constructor_args(
    label: &str,
    resource: &ResourceSchema,
    names: TypeNames<'_>,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let fields: Vec<&marrow_schema::Node> = resource
        .members
        .iter()
        .filter(|node| node.is_plain_field())
        .collect();
    let mut supplied = vec![false; fields.len()];

    for (arg, arg_type) in args.iter().zip(arg_types) {
        let Some(name) = &arg.name else {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` constructor takes named fields"),
            ));
            continue;
        };
        let Some(index) = fields.iter().position(|field| &field.name == name) else {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` has no field `{name}`"),
            ));
            continue;
        };
        if supplied[index] {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("field `{name}` is supplied more than once"),
            ));
            continue;
        }
        supplied[index] = true;
        if let Some(ty) = fields[index].plain_field_type() {
            let expected = MarrowType::from_resolved(ty.clone(), names);
            check_one_arg(label, &expected, arg_type, span, file, diagnostics);
        }
    }

    for (field, supplied) in fields.iter().zip(supplied) {
        if !supplied
            && matches!(
                &field.kind,
                marrow_schema::NodeKind::Slot { required: true, .. }
            )
        {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("`{label}` requires `{}`", field.name),
            ));
        }
    }
}

/// Check one positional/named argument against the type its parameter expects: a
/// known-but-different type is a `check.call_argument`; an `Unknown` argument for a
/// concrete parameter is a `check.untyped_value` (strict typing — convert dynamic
/// data before typed use). Shared by the user-function and std argument loops;
/// `label` names the callee for the message. The expectation is a scalar for every
/// std slot except `std::log::error` (the checker-only `Error` value), and an enum
/// for a user parameter typed as one.
pub(crate) fn check_one_arg(
    label: &str,
    parameter: &MarrowType,
    arg_type: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match type_compatible(parameter, arg_type) {
        Some(true) => {}
        // A known type the parameter does not accept — a different scalar, a foreign
        // identity, a resource, a sequence, an enum, or an `Error` — is a real
        // argument mismatch. Two same-named enums from different modules are
        // qualified so the message distinguishes them.
        Some(false) => {
            let (expected, found) = mismatch_display(parameter, arg_type);
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!("argument to `{label}` expects `{expected}`, but found `{found}`"),
            ));
        }
        // The parameter places no constraint, or the argument is `unknown`. Only an
        // `unknown` argument against a convertible parameter is reported, under
        // strict typing: dynamic data must be converted before typed use.
        None if matches!(arg_type, MarrowType::Unknown) && expects_conversion(parameter) => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNTYPED_VALUE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "argument to `{label}` has no known type, but `{}` is expected; convert it first",
                    marrow_type_name(parameter),
                ),
                span,
            });
        }
        None => {}
    }
}

/// Check positional `args` against a fixed positional parameter list (the std
/// helper signatures): an arity mismatch is a `check.call_argument`, and each
/// argument with a known-required parameter type is checked by [`check_one_arg`].
/// A `None` parameter slot (e.g. a path argument) is left alone. Std helpers are
/// positional-only — named-argument matching stays user-function-only.
pub(crate) fn check_args_against(
    label: &str,
    params: &[Option<MarrowType>],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if arg_types.len() != params.len() {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "`{label}` expects {} argument(s), but {} were given",
                params.len(),
                arg_types.len(),
            ),
        ));
    }
    for (parameter, arg_type) in params.iter().zip(arg_types) {
        if let Some(parameter) = parameter {
            check_one_arg(label, parameter, arg_type, span, file, diagnostics);
        }
    }
}

/// Type `nextId(^root)` and gate it on a single-`int` saved root. A single-`int`
/// root types to `Resource::Id`; any other
/// identity shape reports `check.next_id_requires_single_int`. A non-`^root` or
/// wrong-arity argument is left to the runtime (matching how other builtins
/// behave), and an undeclared root is already reported elsewhere (a `^bogus` read
/// has no schema), so neither is double-reported here.
pub(crate) fn check_next_id(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    let [arg] = args else {
        return MarrowType::Unknown;
    };
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = &arg.value else {
        return MarrowType::Unknown;
    };
    let Some(resource) = find_resource_schema(program, root) else {
        return MarrowType::Unknown;
    };
    let Some(saved_root) = &resource.saved_root else {
        return MarrowType::Unknown;
    };
    if saved_root.single_int_root() {
        return MarrowType::Identity(resource.name.clone());
    }
    diagnostics.push(CheckDiagnostic {
        code: CHECK_NEXT_ID_REQUIRES_SINGLE_INT,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: format!(
            "`nextId` requires a resource with one `int` identity key, but `^{root}` \
             ({}) has no default allocation policy; composite and non-integer \
             identities are application-provided",
            saved_root.next_id_shape(),
        ),
        span,
    });
    MarrowType::Unknown
}

/// Type `next(<element>)` / `prev(<element>)`: the navigated neighbor's identity
/// type. A primary keyed root `^root` or a single-key record `^root(id)` navigates
/// among record identities, so the result is the owning resource's `Resource::Id` —
/// the type that makes `^root(next(^root(id))).field` check. A keyed child-layer
/// position `^root(id…).layer(k)` or a bare child layer `^root(id…).layer`
/// navigates among that layer's keys, so the result is the layer's single key type.
/// A composite-identity record and an index branch are statically unsupported — the
/// runtime would reject them with an uncatchable fault — so each is reported here as
/// a clear compile error. Any other shape is left `Unknown`; the runtime reports an
/// unsupported navigation, and a surrounding `??` still types the default. The edge
/// fault (stepping off the first/last) stays a runtime, `??`-catchable concern.
pub(crate) fn check_neighbor(
    program: &CheckedProgram,
    which: &str,
    args: &[marrow_syntax::Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::Expression;
    let [arg] = args else {
        return MarrowType::Unknown;
    };
    match &arg.value {
        // A bare primary keyed root `^root`: its first/last record is sought. A
        // composite identity has no single returned key value, so reject it before
        // the runtime can degrade the identity to one component.
        Expression::SavedRoot { name: root, .. } => {
            if composite_identity(program, root) {
                return neighbor_unsupported(
                    which,
                    "a composite-identity root (scope a single key level)",
                    span,
                    file,
                    diagnostics,
                );
            }
            record_identity_type(program, root)
        }
        Expression::Call { callee, .. } => match callee.as_ref() {
            // `^root(id…)`: a keyed record. `next`/`prev` anchor at one key level, so
            // a composite multi-key identity is out of scope — reject it statically.
            Expression::SavedRoot { name: root, .. } => {
                if composite_identity(program, root) {
                    return neighbor_unsupported(
                        which,
                        "a composite-identity record (scope a single key level)",
                        span,
                        file,
                        diagnostics,
                    );
                }
                record_identity_type(program, root)
            }
            // `^root.index(args…)`: an index branch (the callee's base is the root
            // itself). It inspects identities, with no single key position to seek.
            Expression::Field { base, .. }
                if matches!(base.as_ref(), Expression::SavedRoot { .. }) =>
            {
                neighbor_unsupported(which, "an index branch", span, file, diagnostics)
            }
            // `^root(id…).layer(k)`: a keyed layer position; its neighbor is a key.
            Expression::Field { .. } => layer_key_type(program, callee.as_ref()),
            _ => MarrowType::Unknown,
        },
        // A bare child layer `^root(id…).layer`: navigate among the layer's keys.
        Expression::Field { .. } => layer_key_type(program, &arg.value),
        _ if matches!(arg_types.first(), Some(MarrowType::Identity(_))) => neighbor_unsupported(
            which,
            "an identity value (use a saved place)",
            span,
            file,
            diagnostics,
        ),
        _ => MarrowType::Unknown,
    }
}

/// Check `append(layer, value)` against the statically declared layer key kind.
/// `append` allocates an integer position, so accepting a string- or bool-keyed
/// layer would create stored keys the schema cannot address.
pub(crate) fn check_append(
    program: &CheckedProgram,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let [target, _] = args else {
        return;
    };
    if target.mode.is_some() || target.name.is_some() {
        return;
    }
    let Some(key_type) = saved_path_key_type(program, &target.value) else {
        return;
    };
    if !matches!(as_primitive(&key_type), Some(ScalarType::Int)) {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "`append` requires an int-keyed layer, but this layer is keyed by `{}`",
                marrow_type_name(&key_type)
            ),
        ));
    }
}

/// Whether the resource at saved root `root` has a composite (multi-key) identity.
/// `next`/`prev` over a record anchor at one key level, so a composite identity is
/// out of scope. A non-keyed root or an unknown root is not composite.
pub(crate) fn composite_identity(program: &CheckedProgram, root: &str) -> bool {
    find_resource_schema(program, root)
        .and_then(|resource| resource.saved_root.as_ref())
        .is_some_and(|saved_root| saved_root.identity_keys.len() > 1)
}

/// Report a `check.neighbor_unsupported` error for a statically-unnavigable
/// `next`/`prev` shape and leave the result `Unknown`.
pub(crate) fn neighbor_unsupported(
    which: &str,
    shape: &str,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_NEIGHBOR_UNSUPPORTED,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: format!("`{which}` cannot navigate {shape}"),
        span,
    });
    MarrowType::Unknown
}

/// Report a `check.call_argument` arity diagnostic when a fixed-arity builtin is
/// called with the wrong number of arguments.
pub(crate) fn check_arity(
    name: &str,
    arity: usize,
    args: &[marrow_syntax::Argument],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if args.len() != arity {
        diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "`{name}` expects {arity} argument(s), but {} were given",
                args.len()
            ),
        ));
    }
}

/// A `check.call_argument` diagnostic located at a call's span.
pub(crate) fn call_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_CALL_ARGUMENT,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        span,
    }
}

/// Whether `file` contributes a module to the program — a library module or a
/// module-less script. Calls in such a file are resolution-checked; a file
/// excluded by a parse error is not.
pub(crate) fn file_in_program(program: &CheckedProgram, file: &Path) -> bool {
    program
        .modules
        .iter()
        .any(|module| module.source_file == file)
}
