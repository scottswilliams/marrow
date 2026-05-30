//! The type-check driver passes over a parsed file: return placement, operator,
//! condition, assignment, call, and saved-key argument checks.

use super::*;

/// Resolve every `use` against `resolvable`, run the type pass over each parsed
/// file against `program` with `project_resources`, then suppress unresolved-call
/// reports when any file failed to parse or read. This is the shared tail of
/// check_project and check_tests: pass 1 (parse plus each caller's module and
/// ownership construction) differs and stays in the caller, but once the
/// resolvable module set, program, and resource set are known every step is
/// identical. `files_len` is the discovered-file count, passed in so the helper
/// does not depend on the two callers' differing discovery return shapes; the
/// `files_len == parsed_files.len()` check keeps the read-failure-drops-a-file
/// invariant both callers rely on.
pub(crate) fn check_resolved_files(
    files_len: usize,
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    resolvable: &HashMap<String, PathBuf>,
    program: &CheckedProgram,
    project_resources: &HashSet<String>,
    project_enums: &HashSet<String>,
    report: &mut CheckReport,
) {
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

    // Unresolved-call reports are trustworthy only when every file parsed and the
    // program is whole: a file that failed to parse or read is excluded from the
    // program, so a call into it would look unresolved though its definition
    // exists. Suppress them then — the parse or read errors are the real problem to
    // fix first. (A read failure drops a file from `parsed_files` without setting
    // has_errors, so the length check is needed alongside the has_errors check.)
    let fully_parsed = files_len == parsed_files.len()
        && parsed_files.iter().all(|(_, parsed)| !parsed.has_errors());
    if !fully_parsed {
        report
            .diagnostics
            .retain(|diagnostic| diagnostic.code != CHECK_UNRESOLVED_CALL);
    }
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
    let FilePrelude {
        aliases,
        module_constants,
    } = file_prelude(program, file, parsed);
    for declaration in &parsed.file.declarations {
        match declaration {
            marrow_syntax::Declaration::Function(function) => {
                for param in &function.params {
                    check_type_annotation(
                        &param.ty,
                        function.span,
                        file,
                        project_resources,
                        project_enums,
                        diagnostics,
                    );
                }
                if let Some(return_type) = &function.return_type {
                    check_type_annotation(
                        return_type,
                        function.span,
                        file,
                        project_resources,
                        project_enums,
                        diagnostics,
                    );
                }
                check_return_values(
                    file,
                    &function.body,
                    function.return_type.is_some(),
                    diagnostics,
                );
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
                    check_type_annotation(
                        ty,
                        constant.span,
                        file,
                        project_resources,
                        project_enums,
                        diagnostics,
                    );
                }
            }
            // Resource and enum member types are validated by schema compilation.
            marrow_syntax::Declaration::Resource(_) | marrow_syntax::Declaration::Enum(_) => {}
        }
    }
}

/// Record a `check.unknown_type` diagnostic when `ty` names a type the checker
/// does not recognize. Located at `span` (the declaration), since a type
/// annotation carries no span of its own.
pub(crate) fn check_type_annotation(
    ty: &marrow_syntax::TypeRef,
    span: SourceSpan,
    file: &Path,
    resources: &HashSet<String>,
    enums: &HashSet<String>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if !MarrowType::names_known_type(ty, resources, enums) {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_UNKNOWN_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!("unknown type `{}`", ty.text.trim()),
            span,
        });
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
                Some(value) => infer_type(program, value, scope, aliases, file, diagnostics),
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
        }
        | Statement::Merge {
            target,
            value,
            span,
        } => {
            // The target's type is known for a local variable or a saved field;
            // for other places (a local resource field, a whole resource) it is
            // unknown and the assignment is left alone.
            let target_type = infer_type(program, target, scope, aliases, file, diagnostics);
            let value_type = infer_type(program, value, scope, aliases, file, diagnostics);
            check_assignment(file, *span, &target_type, &value_type, diagnostics);
        }
        Statement::Delete { path, .. } => {
            infer_type(program, path, scope, aliases, file, diagnostics);
        }
        Statement::Return { value, span } => {
            if let Some(value) = value {
                let value_type = infer_type(program, value, scope, aliases, file, diagnostics);
                check_return_type(file, *span, return_type, &value_type, diagnostics);
            }
        }
        Statement::Throw { value, .. } | Statement::Expr { value, .. } => {
            infer_type(program, value, scope, aliases, file, diagnostics);
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
            body,
            ..
        } => {
            // Inferring the iterable here also operator-checks it; its diagnostics
            // belong to the type pass, so `for_frame` re-infers with a discard sink.
            infer_type(program, iterable, scope, aliases, file, diagnostics);
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
        Statement::Lock { path, body, .. } => {
            if let Some(path) = path {
                infer_type(program, path, scope, aliases, file, diagnostics);
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
/// [`check_statement_types`]: the loop binding(s) in scope for the body. Iterating
/// a sequence binds its single binding to the element type; other iterables
/// (ranges, index keys) and the key/value form stay unknown. The checker and the
/// editor scope reconstruction share this so a loop binding's type is derived in
/// one place. Inference here discards diagnostics; the type pass emits the
/// iterable's separately.
pub(crate) fn for_frame(
    program: &CheckedProgram,
    binding: &marrow_syntax::ForBinding,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> HashMap<String, MarrowType> {
    let iterable_type = infer_type(program, iterable, scope, aliases, file, &mut Vec::new());
    let first_type = match (&binding.second, &iterable_type) {
        (None, MarrowType::Sequence(element)) => (**element).clone(),
        _ => MarrowType::Unknown,
    };
    let mut frame = HashMap::new();
    frame.insert(binding.first.clone(), first_type);
    if let Some(second) = &binding.second {
        frame.insert(second.clone(), MarrowType::Unknown);
    }
    frame
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
        // or one returning a resource/identity/sequence (no conversion boundary),
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

/// Flag a value stored into a concrete (primitive) place when its type is wrong
/// or cannot be resolved. A known-incompatible primitive is a
/// `check.assignment_type` mismatch; an `Unknown` value is a `check.untyped_value`
/// error (strict typing: dynamic data must be converted before typed use). An
/// untyped place (a local resource field, a whole resource, `unknown`) is left
/// alone, as is a known non-primitive value.
pub(crate) fn check_assignment(
    file: &Path,
    span: SourceSpan,
    place: &MarrowType,
    value: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match type_compatible(place, value) {
        Some(true) => {}
        Some(false) => diagnostics.push(CheckDiagnostic {
            code: CHECK_ASSIGNMENT_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "expected `{}`, but the value is `{}`",
                marrow_type_name(place),
                marrow_type_name(value),
            ),
            span,
        }),
        // A value the checker could not resolve, stored into a convertible place. An
        // untyped place (a whole resource, an identity, a sequence, `unknown`) has no
        // conversion boundary and is left alone.
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
        return MarrowType::Unknown;
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
        return MarrowType::Unknown;
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
        return MarrowType::Unknown;
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
        // A range is not a value an operator consumes; accept int endpoints.
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => (
            left == ScalarType::Int && right == ScalarType::Int,
            MarrowType::Unknown,
        ),
        // `??` constrains its operands by the path's leaf type, not by scalar
        // shape alone, so it is typed in `check_coalesce` before reaching here.
        BinaryOp::Coalesce => (left == right, MarrowType::Primitive(left)),
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
        // An untyped operand defers: the scalar path handles untyped values.
        (MarrowType::Unknown, _) | (_, MarrowType::Unknown) => None,
        // Whole records and sequences have no equality at all.
        (MarrowType::Resource(_) | MarrowType::Sequence(_), _)
        | (_, MarrowType::Resource(_) | MarrowType::Sequence(_)) => reject(diagnostics),
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
/// helper, `Error` constructor, out/inout call, key-lookup (non-name callee), or
/// unresolved name is left alone — mirroring the runtime's dispatch order — so the
/// check never fires on a non-function or a call the checker cannot resolve.
///
/// It flags the argument count (every parameter is required), a named argument
/// that names no parameter, and an argument whose type does not match its
/// parameter (only when both are known, incompatible primitives, like operators).
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
        return MarrowType::Unknown;
    };
    // Expand a short-form leading segment through the file's imports once, up front,
    // so `clock::now()` resolves like `std::clock::now()` and a project `books::add`
    // like `shelf::books::add`. All downstream resolution uses the expanded form.
    let expanded = expand_alias(segments, aliases);
    let segments = expanded.as_slice();
    if args.iter().any(|arg| arg.mode.is_some()) {
        return MarrowType::Unknown;
    }
    // `nextId(^root)` needs the argument *expression* (the `^root`), not just its
    // type, to know which resource is allocated — so it is handled here, before the
    // generic builtin branch typed only from `arg_types`. It types to `Resource::Id`
    // for a single-`int` root and reports `check.next_id_requires_single_int` for
    // any other shape (composite/non-int/singleton).
    if let [name] = segments
        && name == "nextId"
    {
        return check_next_id(program, args, span, file, diagnostics);
    }
    // `reversed` is type-transparent: it yields the same elements in reverse order,
    // so its result is exactly its argument's type (a `sequence[T]` stays
    // `sequence[T]`; an untyped layer stays `Unknown`, never regressing loop-element
    // typing). `next`/`prev` need the argument *expression* — the `^path` — to find
    // the resource/layer they navigate, so they too resolve here before the generic
    // builtin branch typed only from `arg_types`.
    if let [name] = segments {
        match name.as_str() {
            "reversed" => {
                check_arity(name, 1, args, span, file, diagnostics);
                return arg_types.first().cloned().unwrap_or(MarrowType::Unknown);
            }
            "next" | "prev" => {
                check_arity(name, 1, args, span, file, diagnostics);
                return check_neighbor(program, name, args, span, file, diagnostics);
            }
            _ => {}
        }
    }
    // Builtins dispatch before user functions. For std helpers the signatures are
    // fixed, so argument types and arity are checked here the
    // same way user-function arguments are; other builtins leave their arguments to
    // the runtime. A std helper's return type feeds the surrounding type checks.
    if is_builtin_call(segments) {
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
    // A callee naming a declared resource is a constructor, not a function:
    // `Book(...)` builds the resource value and `Book::Id(...)` builds its
    // identity. Recognize it so a valid
    // constructor is not a false `check.unresolved_call`.
    if let Some(ty) = resource_constructor_type(program, from_module, segments) {
        return ty;
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
    // named argument that names no parameter and an argument whose known primitive
    // type differs from the parameter's.
    for (index, (arg, arg_type)) in args.iter().zip(arg_types).enumerate() {
        let param = match &arg.name {
            Some(name) => {
                let param = function.params.iter().find(|param| &param.name == name);
                if param.is_none() {
                    diagnostics.push(call_diagnostic(
                        file,
                        span,
                        format!(
                            "function `{}` has no parameter `{name}`",
                            segments.join("::")
                        ),
                    ));
                }
                param
            }
            None => function.params.get(index),
        };
        // Every concrete parameter type — scalar, identity, resource, sequence,
        // enum, or the checker-only `Error` — is checked nominally; an `unknown`
        // parameter places no constraint and is left to the runtime.
        if let Some(param) = param {
            check_one_arg(
                &segments.join("::"),
                &param.ty,
                arg_type,
                span,
                file,
                diagnostics,
            );
        }
    }
    function.return_type.clone().unwrap_or(MarrowType::Unknown)
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
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::Expression;
    let [arg] = args else {
        return MarrowType::Unknown;
    };
    match &arg.value {
        // A bare primary keyed root `^root`: its first/last record is sought, so a
        // composite identity is fine here (no single key level is anchored).
        Expression::SavedRoot { name: root, .. } => record_identity_type(program, root),
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
        _ => MarrowType::Unknown,
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
