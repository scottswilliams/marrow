//! Resolve and check a Marrow project's source.
//!
//! This is the start of the checked-program pipeline: discover the project's
//! `.mw` files, parse each one, and report parse diagnostics together with
//! module/path resolution problems. Type, effect, and schema facts build on
//! top of this in later work.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, ProjectConfig, discover_modules, discover_test_modules};
use marrow_syntax::{Severity, SourceSpan, parse_source};

pub mod program;
mod rules;

pub use program::{
    CheckedConst, CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, MarrowType,
    PrimitiveType,
};

/// A library file declares a module name that does not match its path.
pub const CHECK_MODULE_PATH: &str = "check.module_path";
/// Two library files declare the same module name.
pub const CHECK_DUPLICATE_MODULE: &str = "check.duplicate_module";
/// A name is declared or imported more than once within a single file.
pub const CHECK_DUPLICATE_DECLARATION: &str = "check.duplicate_declaration";
/// A `use` names a module that is neither a project module nor a standard
/// library module.
pub const CHECK_UNRESOLVED_IMPORT: &str = "check.unresolved_import";
/// A type annotation names a type the checker does not recognize.
pub const CHECK_UNKNOWN_TYPE: &str = "check.unknown_type";
/// A `return` carries a value in a function with no return type, or omits one in a
/// value-returning function.
pub const CHECK_RETURN_VALUE: &str = "check.return_value";
/// A value-returning function can reach the end of its body without returning.
pub const CHECK_MISSING_RETURN: &str = "check.missing_return";
/// An operator is applied to operands whose types it does not accept.
pub const CHECK_OPERATOR_TYPE: &str = "check.operator_type";
/// A condition (`if`/`while`) is not a `bool`.
pub const CHECK_CONDITION_TYPE: &str = "check.condition_type";
/// A call passes the wrong number of arguments, or names a parameter that does
/// not exist, for the function it resolves to.
pub const CHECK_CALL_ARGUMENT: &str = "check.call_argument";
/// A `return` value's type does not match the function's declared return type.
pub const CHECK_RETURN_TYPE: &str = "check.return_type";
/// A value's type does not match the binding or place it is stored into (a typed
/// `const`/`var` initializer, or an assignment target).
pub const CHECK_ASSIGNMENT_TYPE: &str = "check.assignment_type";
/// A value whose type cannot be resolved is stored into a concrete typed place.
/// Under strict typing, dynamic data must be converted before typed use; this is
/// the first position where an unresolved (`unknown`) value becomes an error
/// rather than being skipped.
pub const CHECK_UNTYPED_VALUE: &str = "check.untyped_value";
/// A bare name used as a value does not resolve to any binding in scope (a
/// parameter, local, loop or catch binding, or module constant). Under strict
/// typing every value name must be defined.
pub const CHECK_UNRESOLVED_NAME: &str = "check.unresolved_name";
/// A call names a function that is neither a builtin nor a declared function. Only
/// reported for calls in library modules of a fully parsed project, so a
/// module-less script or a module excluded by a parse error never false-positives.
pub const CHECK_UNRESOLVED_CALL: &str = "check.unresolved_call";
/// `nextId(^root)` names a root with no default integer allocation policy: a
/// composite identity, a single non-integer identity key, or a keyless singleton.
/// The default per-root policy is only available for a resource with one `int`
/// identity key (builtins.md:180-183, types.md:262-263). The runtime backstops
/// this with `write.next_id_unsupported`; the checker catches it before a run.
pub const CHECK_NEXT_ID_REQUIRES_SINGLE_INT: &str = "check.next_id_requires_single_int";
/// A numeric literal is provably outside its type's range: an integer literal
/// beyond `i64`, or a decimal literal outside the 34-significant-digit /
/// 34-fractional-place envelope. The runtime would reject it as `run.overflow`.
pub const CHECK_LITERAL_RANGE: &str = "check.literal_range";
/// A discovered source file could not be read.
pub const IO_READ: &str = "io.read";
/// Two resources in the project claim the same saved root. A saved root has one
/// managed owner. This is a schema-model rule, but it is cross-resource, so the
/// project checker reports it rather than per-resource schema compilation.
pub const SCHEMA_DUPLICATE_ROOT_OWNER: &str = "schema.duplicate_root_owner";

/// A problem found while checking a project, located in a specific file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckDiagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub file: PathBuf,
    pub message: String,
    pub line: u32,
    pub column: u32,
}

/// The result of checking a project: every diagnostic across its files, in
/// file then source order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckReport {
    pub diagnostics: Vec<CheckDiagnostic>,
}

impl CheckReport {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// Discover, read, and parse every `.mw` file in the project, collecting parse
/// diagnostics and module/path resolution problems. Fails only when a source
/// root cannot be walked; per-file read errors become diagnostics.
pub fn check_project(
    project_root: &Path,
    config: &ProjectConfig,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    let files = discover_modules(project_root, config)?;
    let mut report = CheckReport::default();
    let mut program = CheckedProgram::default();
    // The first valid library file (in path order) to declare each module owns
    // that name; later files declaring it are duplicates. This is also the set
    // of resolvable project module names for `use` resolution.
    let mut declared: HashMap<String, PathBuf> = HashMap::new();
    // The first resource (in file then source order) to claim each saved root
    // owns it; a later resource on the same root is a duplicate owner.
    let mut root_owners: HashMap<String, PathBuf> = HashMap::new();
    // The first resource to declare each stable ID owns it; the same ID in a
    // later resource is a project-wide duplicate.
    let mut stable_id_owners: HashMap<String, PathBuf> = HashMap::new();
    // Parsed sources kept from pass 1 so pass 2 can resolve imports against the
    // full project module set without re-reading files.
    let mut parsed_files: Vec<(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)> =
        Vec::new();

    // Pass 1: parse each file and collect per-file findings plus the project's
    // module set.
    for file in &files {
        let Some(CheckedFile {
            parsed,
            resources,
            functions,
            constants,
        }) = check_file(&file.path, &mut report.diagnostics)
        else {
            continue;
        };

        // Saved roots and stable IDs are owned project-wide. Walk the file's
        // resource declarations beside their compiled schemas (same order) to
        // enforce one owner per root and one declaration per stable id.
        let mut schemas = resources.iter();
        for declaration in &parsed.file.declarations {
            let marrow_syntax::Declaration::Resource(resource) = declaration else {
                continue;
            };
            let schema = schemas
                .next()
                .expect("one compiled schema per resource declaration");
            if let Some(saved) = &schema.saved_root {
                match root_owners.get(&saved.root) {
                    Some(first) => report.diagnostics.push(CheckDiagnostic {
                        code: SCHEMA_DUPLICATE_ROOT_OWNER,
                        severity: Severity::Error,
                        file: file.path.clone(),
                        message: format!(
                            "saved root `^{}` is already owned by a resource in `{}`",
                            saved.root,
                            first.display()
                        ),
                        line: resource.span.line,
                        column: resource.span.column,
                    }),
                    None => {
                        root_owners.insert(saved.root.clone(), file.path.clone());
                    }
                }
            }
            // Within-resource stable-id duplicates are reported by
            // compile_resource; this catches an id reused in another resource.
            let mut seen_here: Vec<String> = Vec::new();
            for (id, span) in marrow_schema::stable_ids(resource) {
                if seen_here.contains(&id) {
                    continue;
                }
                match stable_id_owners.get(&id) {
                    Some(first) => report.diagnostics.push(CheckDiagnostic {
                        code: marrow_schema::SCHEMA_DUPLICATE_STABLE_ID,
                        severity: Severity::Error,
                        file: file.path.clone(),
                        message: format!(
                            "stable id `{id}` is already declared in `{}`",
                            first.display()
                        ),
                        line: span.line,
                        column: span.column,
                    }),
                    None => {
                        stable_id_owners.insert(id.clone(), file.path.clone());
                    }
                }
                seen_here.push(id);
            }
        }

        // A library file (one that declares a `module`) must declare the name
        // its path implies. A module-less file is a script or entrypoint and is
        // not bound to a path.
        if let Some(module) = &parsed.file.module {
            match &file.module_name {
                // A valid library module: enforce uniqueness of the name.
                Some(expected) if expected == &module.name => {
                    if let Some(first) = declared.get(expected) {
                        report.diagnostics.push(CheckDiagnostic {
                            code: CHECK_DUPLICATE_MODULE,
                            severity: Severity::Error,
                            file: file.path.clone(),
                            message: format!(
                                "module `{expected}` is already declared by `{}`",
                                first.display()
                            ),
                            line: module.span.line,
                            column: module.span.column,
                        });
                    } else {
                        declared.insert(expected.clone(), file.path.clone());
                        // The artifact takes a clean, path-matched, first-seen
                        // library module. Skip any file that carries a parse
                        // error this slice.
                        if !parsed.has_errors() {
                            program.modules.push(CheckedModule {
                                name: module.name.clone(),
                                source_file: file.path.clone(),
                                span: module.span,
                                imports: parsed
                                    .file
                                    .uses
                                    .iter()
                                    .map(|use_decl| use_decl.name.clone())
                                    .collect(),
                                constants,
                                functions,
                                resources,
                            });
                        }
                    }
                }
                Some(expected) => report.diagnostics.push(module_path_error(
                    file,
                    module,
                    format!(
                        "module `{}` does not match its path; expected `{expected}`",
                        module.name
                    ),
                )),
                // `discover_modules` only yields `.mw` files with clean relative
                // paths, so it always carries an expected name; this arm is
                // defensive for any other source of `ModuleFile`.
                None => report.diagnostics.push(module_path_error(
                    file,
                    module,
                    format!(
                        "a file at this path cannot declare module `{}`",
                        module.name
                    ),
                )),
            }
        }

        parsed_files.push((file, parsed));
    }

    // Pass 2: every `use` must name a project module or a standard-library
    // module, now that the full project module set is known.
    for (file, parsed) in &parsed_files {
        for use_decl in &parsed.file.uses {
            if !is_resolved_import(&use_decl.name, &declared) {
                report.diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNRESOLVED_IMPORT,
                    severity: Severity::Error,
                    file: file.path.clone(),
                    message: format!("cannot resolve import `{}`", use_decl.name),
                    line: use_decl.span.line,
                    column: use_decl.span.column,
                });
            }
        }
    }

    // Pass 3: flag type annotations on functions and constants that name an
    // unknown type. Resource member types are validated by schema compilation.
    let project_resources: HashSet<String> = parsed_files
        .iter()
        .flat_map(|(_, parsed)| parsed.file.declarations.iter())
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Resource(resource) => Some(resource.name.clone()),
            _ => None,
        })
        .collect();
    for (file, parsed) in &parsed_files {
        // A module's top-level constants are in scope (bare) for its functions.
        // An annotated constant carries its annotation; an unannotated one's type
        // is inferred from its initializer (as a local `const` already is), so a
        // typed use like `var x: int = M` resolves rather than false-positiving
        // `check.untyped_value`. Built in source order so an earlier constant is
        // in scope for a later one. This pass only builds the scope map; an
        // initializer's own diagnostics (constant-expression validity and literal
        // range) come from `check_const_value`, so inference diagnostics are
        // discarded here.
        // Short→full import aliases for this file, used to expand short-form calls
        // (`clock::now()` → `std::clock::now`) before resolution. Built once per
        // file; the runtime rebuilds the same map from `CheckedModule::imports`.
        let aliases = build_alias_map(
            &parsed
                .file
                .uses
                .iter()
                .map(|use_decl| use_decl.name.clone())
                .collect::<Vec<_>>(),
        );
        let mut module_constants: HashMap<String, MarrowType> = HashMap::new();
        for declaration in &parsed.file.declarations {
            if let marrow_syntax::Declaration::Const(constant) = declaration {
                let ty = match &constant.ty {
                    Some(ty) => resolve_type(ty, &program),
                    None => infer_type(
                        &program,
                        &constant.value,
                        std::slice::from_ref(&module_constants),
                        &aliases,
                        &file.path,
                        &mut Vec::new(),
                    ),
                };
                module_constants.insert(constant.name.clone(), ty);
            }
        }
        for declaration in &parsed.file.declarations {
            match declaration {
                marrow_syntax::Declaration::Function(function) => {
                    for param in &function.params {
                        check_type_annotation(
                            &param.ty,
                            function.span,
                            &file.path,
                            &project_resources,
                            &mut report.diagnostics,
                        );
                    }
                    if let Some(return_type) = &function.return_type {
                        check_type_annotation(
                            return_type,
                            function.span,
                            &file.path,
                            &project_resources,
                            &mut report.diagnostics,
                        );
                    }
                    check_return_values(
                        &file.path,
                        &function.body,
                        function.return_type.is_some(),
                        &mut report.diagnostics,
                    );
                    check_function_types(
                        &program,
                        &file.path,
                        function,
                        &module_constants,
                        &aliases,
                        &mut report.diagnostics,
                    );
                    if function.return_type.is_some() && !block_returns(&function.body) {
                        report.diagnostics.push(CheckDiagnostic {
                            code: CHECK_MISSING_RETURN,
                            severity: Severity::Error,
                            file: file.path.clone(),
                            message: format!(
                                "function `{}` may reach its end without returning a value",
                                function.name
                            ),
                            line: function.span.line,
                            column: function.span.column,
                        });
                    }
                }
                marrow_syntax::Declaration::Const(constant) => {
                    if let Some(ty) = &constant.ty {
                        check_type_annotation(
                            ty,
                            constant.span,
                            &file.path,
                            &project_resources,
                            &mut report.diagnostics,
                        );
                    }
                }
                marrow_syntax::Declaration::Resource(_) => {}
            }
        }
    }

    // Unresolved-call reports are trustworthy only when the whole project parsed:
    // a file that failed to parse or read is excluded from the program, so a call
    // into it would look unresolved though its definition exists. Suppress them in
    // that case — the parse or read errors are the real problem to fix first.
    let fully_parsed = files.len() == parsed_files.len()
        && parsed_files.iter().all(|(_, parsed)| !parsed.has_errors());
    if !fully_parsed {
        report
            .diagnostics
            .retain(|diagnostic| diagnostic.code != CHECK_UNRESOLVED_CALL);
    }

    Ok((report, program))
}

/// Record a `check.unknown_type` diagnostic when `ty` names a type the checker
/// does not recognize. Located at `span` (the declaration), since a type
/// annotation carries no span of its own.
fn check_type_annotation(
    ty: &marrow_syntax::TypeRef,
    span: SourceSpan,
    file: &Path,
    resources: &HashSet<String>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if !MarrowType::names_known_type(&ty.text, resources) {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_UNKNOWN_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!("unknown type `{}`", ty.text.trim()),
            line: span.line,
            column: span.column,
        });
    }
}

/// Flag each `return` whose value presence does not match the function's declared
/// return type: a value-returning function must return a value, and a function
/// with no return type must not return one. Recurses into nested blocks; `finally`
/// is left to `check.finally_control_flow`, and the "every reachable path returns"
/// rule is a later slice.
fn check_return_values(
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
                    line: span.line,
                    column: span.column,
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
            _ => {}
        }
    }
}

/// Whether `block` definitely returns (or otherwise diverges) on every path —
/// a sound under-approximation of "every reachable path returns" (spec). It is
/// conservative: a function ending in a call or a loop may diverge, so it is not
/// flagged; only a clearly falling-through end is. Avoids false positives at the
/// cost of missing some genuine cases (a later slice can tighten it).
fn block_returns(block: &marrow_syntax::Block) -> bool {
    block.statements.last().is_some_and(statement_returns)
}

fn statement_returns(statement: &marrow_syntax::Statement) -> bool {
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
/// an unresolved call — is never a false positive. The operator rules mirror
/// `docs/language/syntax.md`: matching numeric operands for `+ - * /`, `int` for
/// `%`, `string` for `_`, ordered same-typed operands for comparisons, and `bool`
/// for `and`/`or`/`not`.
fn check_function_types(
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
        base.insert(param.name.clone(), resolve_type(&param.ty, program));
    }
    let mut scope: Vec<HashMap<String, MarrowType>> = vec![base];
    // The declared return type (unknown for a void function), used to check each
    // `return` expression's type as the walk reaches it.
    let return_type = function
        .return_type
        .as_ref()
        .map_or(MarrowType::Unknown, |ty| resolve_type(ty, program));
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
fn check_block_types(
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
fn check_statement_types(
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
            name,
            ty,
            value,
            span,
        } => {
            let value_type = infer_type(program, value, scope, aliases, file, diagnostics);
            if let Some(ty) = ty {
                check_assignment(
                    file,
                    *span,
                    &resolve_type(ty, program),
                    &value_type,
                    diagnostics,
                );
            }
            bind(scope, name, binding_type(ty.as_ref(), value_type, program));
        }
        Statement::Var {
            name,
            ty,
            value,
            span,
            ..
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
                    &resolve_type(ty, program),
                    &value_type,
                    diagnostics,
                );
            }
            bind(scope, name, binding_type(ty.as_ref(), value_type, program));
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
            check_condition(program, file, condition, scope, aliases, diagnostics);
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
                check_condition(
                    program,
                    file,
                    &else_if.condition,
                    scope,
                    aliases,
                    diagnostics,
                );
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
            check_condition(program, file, condition, scope, aliases, diagnostics);
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
            let iterable_type = infer_type(program, iterable, scope, aliases, file, diagnostics);
            // The loop binding(s) are in scope for the body. Iterating a sequence
            // binds its single binding to the element type; other iterables (ranges,
            // index keys) and the key/value form stay unknown for now.
            let first_type = match (&binding.second, &iterable_type) {
                (None, MarrowType::Sequence(element)) => (**element).clone(),
                _ => MarrowType::Unknown,
            };
            let mut frame = HashMap::new();
            frame.insert(binding.first.clone(), first_type);
            if let Some(second) = &binding.second {
                frame.insert(second.clone(), MarrowType::Unknown);
            }
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
            infer_type(program, path, scope, aliases, file, diagnostics);
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
                frame.insert(
                    clause.name.clone(),
                    MarrowType::Primitive(PrimitiveType::Error),
                );
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
        Statement::Break { .. } | Statement::Continue { .. } | Statement::Unparsed { .. } => {}
    }
}

/// The declared type of a binding: its annotation when written, otherwise the
/// inferred type of its initializer.
fn binding_type(
    annotation: Option<&marrow_syntax::TypeRef>,
    value_type: MarrowType,
    program: &CheckedProgram,
) -> MarrowType {
    match annotation {
        Some(ty) => resolve_type(ty, program),
        None => value_type,
    }
}

/// Record `name`'s type in the innermost scope frame.
fn bind(scope: &mut [HashMap<String, MarrowType>], name: &str, ty: MarrowType) {
    if let Some(frame) = scope.last_mut() {
        frame.insert(name.to_string(), ty);
    }
}

/// Type-check an `if`/`while` condition. Inferring it also operator-checks it;
/// then a condition whose type is a known primitive other than `bool` is flagged
/// (`docs/language/control-flow-and-effects.md`: "Conditions must be `bool`"). An
/// unknown type — an unresolved call such as `exists(...)`, a saved-data read — is
/// left alone, so the check never fires on an uncertain condition.
fn check_condition(
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
        Some(primitive) if primitive != PrimitiveType::Bool => diagnostics.push(CheckDiagnostic {
            code: CHECK_CONDITION_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "condition must be `bool`, found `{}`",
                primitive_name(primitive)
            ),
            line: span.line,
            column: span.column,
        }),
        // Strict typing: a condition whose type cannot be resolved cannot be shown
        // to be `bool`.
        None if matches!(condition_type, MarrowType::Unknown) => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNTYPED_VALUE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: "condition has no known type; it must be `bool`".to_string(),
                line: span.line,
                column: span.column,
            });
        }
        _ => {}
    }
}

/// Flag a `return` value whose type does not match the function's declared
/// return type. Fires only when both are known, incompatible primitives, so a
/// void function (unknown return type), a non-primitive return (a resource or
/// identity), or an unresolved returned value is left alone. Value presence is
/// checked separately by `check.return_value`.
fn check_return_type(
    file: &Path,
    span: SourceSpan,
    return_type: &MarrowType,
    value_type: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(expected) = as_primitive(return_type) else {
        return;
    };
    match as_primitive(value_type) {
        Some(actual) if actual != expected => diagnostics.push(CheckDiagnostic {
            code: CHECK_RETURN_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "function returns `{}`, but this value is `{}`",
                primitive_name(expected),
                primitive_name(actual),
            ),
            line: span.line,
            column: span.column,
        }),
        // Strict typing: a value with no known type returned where a concrete type
        // is declared must be converted first.
        None if matches!(value_type, MarrowType::Unknown) => diagnostics.push(CheckDiagnostic {
            code: CHECK_UNTYPED_VALUE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "this `return` value has no known type, but the function returns `{}`; convert it first",
                primitive_name(expected),
            ),
            line: span.line,
            column: span.column,
        }),
        _ => {}
    }
}

/// Flag a value stored into a concrete (primitive) place when its type is wrong
/// or cannot be resolved. A known-incompatible primitive is a
/// `check.assignment_type` mismatch; an `Unknown` value is a `check.untyped_value`
/// error (strict typing: dynamic data must be converted before typed use). An
/// untyped place (a local resource field, a whole resource, `unknown`) is left
/// alone, as is a known non-primitive value (deferred to a later strict slice).
fn check_assignment(
    file: &Path,
    span: SourceSpan,
    place: &MarrowType,
    value: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(place) = as_primitive(place) else {
        return;
    };
    match as_primitive(value) {
        Some(value) if value != place => diagnostics.push(CheckDiagnostic {
            code: CHECK_ASSIGNMENT_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "expected `{}`, but the value is `{}`",
                primitive_name(place),
                primitive_name(value),
            ),
            line: span.line,
            column: span.column,
        }),
        // A value the checker could not resolve, stored into a concrete place.
        None if matches!(value, MarrowType::Unknown) => diagnostics.push(CheckDiagnostic {
            code: CHECK_UNTYPED_VALUE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!(
                "the value stored into `{}` has no known type; convert it before typed use",
                primitive_name(place),
            ),
            line: span.line,
            column: span.column,
        }),
        _ => {}
    }
}

/// Infer an expression's type, recording a `check.operator_type` diagnostic for
/// any operator whose operands are known to be incompatible. Returns
/// [`MarrowType::Unknown`] whenever the type cannot be determined with certainty,
/// so a containing operator never fires on an uncertain operand.
fn infer_type(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::Expression;
    match expr {
        Expression::Literal { kind, text, span } => {
            check_literal_range(*kind, text, *span, file, diagnostics);
            literal_type(*kind)
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let marrow_syntax::InterpolationPart::Expr(expr) = part {
                    infer_type(program, expr, scope, aliases, file, diagnostics);
                }
            }
            MarrowType::Primitive(PrimitiveType::String)
        }
        Expression::Name { segments, span } if segments.len() == 1 => {
            let name = &segments[0];
            lookup_opt(scope, name).unwrap_or_else(|| {
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNRESOLVED_NAME,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
                    message: format!("`{name}` is not defined"),
                    line: span.line,
                    column: span.column,
                });
                MarrowType::Unknown
            })
        }
        Expression::Unary { op, operand, span } => {
            let operand = infer_type(program, operand, scope, aliases, file, diagnostics);
            check_unary(*op, &operand, *span, file, diagnostics)
        }
        Expression::Binary {
            op,
            left,
            right,
            span,
        } => {
            let left = infer_type(program, left, scope, aliases, file, diagnostics);
            let right = infer_type(program, right, scope, aliases, file, diagnostics);
            check_binary(*op, &left, &right, *span, file, diagnostics)
        }
        Expression::Call { callee, args, span } => {
            // Visit the callee subtree (it may hold nested calls, e.g. the
            // `^books(id)` inside `^books(id).tags(pos)`) and infer each argument
            // once. A bare single-segment callee names a function, not a value, so
            // it is left to `check_call` to resolve rather than flagged as an
            // unresolved value name. `check_call` validates the call and yields its
            // return type.
            if !is_bare_name(callee) {
                infer_type(program, callee, scope, aliases, file, diagnostics);
            }
            let arg_types: Vec<MarrowType> = args
                .iter()
                .map(|arg| infer_type(program, &arg.value, scope, aliases, file, diagnostics))
                .collect();
            let call_type = check_call(
                program,
                callee,
                args,
                &arg_types,
                aliases,
                *span,
                file,
                diagnostics,
            );
            // A keyed-leaf read `^root(key…).layer(key…)` is call-shaped but is not
            // a function call; it types to the layer's declared leaf type. A whole
            // record read `^root(key…)` types to its resource.
            if matches!(call_type, MarrowType::Unknown) {
                saved_leaf_type(program, callee)
                    .or_else(|| saved_index_identity_type(program, callee))
                    .or_else(|| saved_resource_type(program, callee))
                    .unwrap_or(MarrowType::Unknown)
            } else {
                call_type
            }
        }
        Expression::Field { base, name, .. } => {
            let base_type = infer_type(program, base, scope, aliases, file, diagnostics);
            // A saved field read resolves to its declared type: a top-level field
            // `^root(key…).field` or a group-layer field `^root(key…).layer(key…).field`.
            // A field off a resource-typed local (`book.title`) resolves through the
            // resource's schema.
            saved_field_type(program, base, name)
                .or_else(|| saved_group_field_type(program, base, name))
                .or_else(|| local_field_type(program, &base_type, name))
                .unwrap_or(MarrowType::Unknown)
        }
        // A multi-segment name, saved root, or undecomposed text has no known
        // primitive type for this slice.
        Expression::Name { .. } | Expression::SavedRoot { .. } | Expression::Unparsed { .. } => {
            MarrowType::Unknown
        }
    }
}

/// The declared type of a top-level saved field read: `base` is either a keyed
/// record access `^root(key…)` (a call whose callee is the saved root) or — for a
/// keyless singleton resource (`Settings at ^settings`) addressed by its root —
/// the saved root `^root` itself. Group-layer fields and keyed-leaf reads are not
/// resolved here. Mirrors the runtime's `resource_field_type`.
fn saved_field_type(
    program: &CheckedProgram,
    base: &marrow_syntax::Expression,
    field: &str,
) -> Option<MarrowType> {
    use marrow_syntax::Expression;
    let root = match base {
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::SavedRoot { name, .. } => name,
            _ => return None,
        },
        Expression::SavedRoot { name, .. } => name,
        _ => return None,
    };
    let resource = find_resource_schema(program, root)?;
    let field = resource
        .fields
        .iter()
        .find(|declared| declared.name == field)?;
    Some(MarrowType::resolve(&field.ty, &[]))
}

/// The schema of the resource that owns saved root `^root`, if any. Mirrors the
/// runtime's `find_resource`.
fn find_resource_schema<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> Option<&'p marrow_schema::ResourceSchema> {
    program
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .find(|resource| {
            resource
                .saved_root
                .as_ref()
                .is_some_and(|saved| saved.root == root)
        })
}

/// The resource type of a whole-record read `^root(key…)`: the call's callee is
/// the saved root, and the value is the owning resource (mirrors the runtime's
/// whole-resource read producing a `Value::Resource`). Lets field access off a
/// saved read stored in a local be typed.
fn saved_resource_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = callee else {
        return None;
    };
    let resource = find_resource_schema(program, root)?;
    Some(MarrowType::Resource(resource.name.clone()))
}

/// The identity type of a unique-index lookup `^root.uniqueIndex(args)`: the
/// owning resource's `Resource::Id`. A unique index stores one resource identity
/// at the lookup path, so reading it yields that identity (mirrors the runtime's
/// `eval_index_lookup`). A non-unique index has no single identity in value
/// position, so it is not typed here. `callee` is the `^root.index` field.
fn saved_index_identity_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let marrow_syntax::Expression::Field { base, name, .. } = callee else {
        return None;
    };
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = base.as_ref() else {
        return None;
    };
    let resource = find_resource_schema(program, root)?;
    let index = resource.indexes.iter().find(|index| &index.name == name)?;
    index
        .unique
        .then(|| MarrowType::Identity(resource.name.clone()))
}

/// Resolve a type annotation against the project's resource names, so a resource
/// type like `Book` resolves to `MarrowType::Resource("Book")` rather than
/// `Unknown` — which lets field reads off a resource-typed local be typed.
fn resolve_type(ty: &marrow_syntax::TypeRef, program: &CheckedProgram) -> MarrowType {
    let names: Vec<String> = program
        .modules
        .iter()
        .flat_map(|module| {
            module
                .resources
                .iter()
                .map(|resource| resource.name.clone())
        })
        .collect();
    MarrowType::resolve(ty, &names)
}

/// The declared type of a field read off a resource-typed value, e.g. `book.title`
/// where `book: Book`. `base_type` must be a known resource type; the field is
/// looked up in that resource's schema. Mirrors the runtime's local field read.
fn local_field_type(
    program: &CheckedProgram,
    base_type: &MarrowType,
    field: &str,
) -> Option<MarrowType> {
    let MarrowType::Resource(name) = base_type else {
        return None;
    };
    let resource = program
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .find(|resource| &resource.name == name)?;
    let field = resource
        .fields
        .iter()
        .find(|declared| declared.name == field)?;
    Some(MarrowType::resolve(&field.ty, &[]))
}

/// The declared type of a group field read at any nesting depth, reached through
/// keyed layers (`^root(key…).layer(key…)….field`) or unkeyed groups
/// (`^root(key…).name.field`). `base` is the group entry — the part before the
/// leaf field. Mirrors the runtime's `resource_nested_member_type`.
fn saved_group_field_type(
    program: &CheckedProgram,
    base: &marrow_syntax::Expression,
    field: &str,
) -> Option<MarrowType> {
    let (root, layers) = saved_group_chain(base)?;
    let resource = find_resource_schema(program, root)?;
    let layer = descend_layers(resource, &layers)?;
    let member = layer.members.iter().find_map(|member| match member {
        marrow_schema::LayerMember::Field(member) if member.name == field => Some(member),
        _ => None,
    })?;
    Some(MarrowType::resolve(&member.ty, &[]))
}

/// Extract `(root, [member…])` from a group entry — the base of a group field
/// read — peeling each level outermost-last: a keyed layer `.layer(key…)` (a call
/// whose callee is the layer field) or an unkeyed group hop `.name` (a field off a
/// deeper saved path). The innermost base is the keyed record `^root(key…)` or the
/// singleton root `^root`.
fn saved_group_chain(expr: &marrow_syntax::Expression) -> Option<(&str, Vec<&str>)> {
    use marrow_syntax::Expression;
    // A keyed layer entry `….layer(key…)`: a call whose callee is the layer field.
    if let Expression::Call { callee, .. } = expr {
        return saved_layer_chain(callee.as_ref());
    }
    // An unkeyed group hop `….name`: a field off the record or a deeper group.
    let Expression::Field { base, name, .. } = expr else {
        return None;
    };
    match base.as_ref() {
        // The record base: `^root(key…)` (a call on the saved root) or the
        // singleton root `^root`. This `.name` is the first group member.
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::SavedRoot { name: root, .. } => Some((root, vec![name])),
            // A keyed layer entry `Call{callee:Field}` is a deeper group; recurse.
            Expression::Field { .. } => {
                let (root, mut members) = saved_group_chain(base)?;
                members.push(name);
                Some((root, members))
            }
            _ => None,
        },
        Expression::SavedRoot { name: root, .. } => Some((root, vec![name])),
        // A deeper unkeyed group `Field`: recurse and append this member.
        Expression::Field { .. } => {
            let (root, mut members) = saved_group_chain(base)?;
            members.push(name);
            Some((root, members))
        }
        _ => None,
    }
}

/// The declared leaf type of a keyed-leaf read `^root(key…).layer(key…)…` at any
/// nesting depth. `callee` is the layer field `^root(key…)….layer`. Mirrors
/// `resource_layer_leaf_type`.
fn saved_leaf_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let (root, layers) = saved_layer_chain(callee)?;
    let resource = find_resource_schema(program, root)?;
    let layer = descend_layers(resource, &layers)?;
    Some(MarrowType::resolve(layer.leaf_type.as_ref()?, &[]))
}

/// Extract `(root, [layer…])` from a keyed layer accessor `^root(key…).layer` or a
/// nested one `^root(key…).layer(key…)….layer`, with the layer names ordered
/// outermost first. Each `Field` peels one layer; its base is either the keyed
/// record `^root(key…)` (a call on a saved root) or a deeper layer entry.
fn saved_layer_chain(expr: &marrow_syntax::Expression) -> Option<(&str, Vec<&str>)> {
    use marrow_syntax::Expression;
    let Expression::Field {
        base, name: layer, ..
    } = expr
    else {
        return None;
    };
    let Expression::Call { callee, .. } = base.as_ref() else {
        return None;
    };
    match callee.as_ref() {
        Expression::SavedRoot { name: root, .. } => Some((root, vec![layer])),
        Expression::Field { .. } => {
            let (root, mut layers) = saved_layer_chain(callee)?;
            layers.push(layer);
            Some((root, layers))
        }
        _ => None,
    }
}

/// Descend a non-empty chain of group layer names from a resource, following
/// nested layer members, and return the innermost layer's schema.
fn descend_layers<'a>(
    resource: &'a marrow_schema::ResourceSchema,
    layers: &[&str],
) -> Option<&'a marrow_schema::LayerSchema> {
    let (first, rest) = layers.split_first()?;
    let mut current = resource.layers.iter().find(|layer| &layer.name == first)?;
    for name in rest {
        current = current.members.iter().find_map(|member| match member {
            marrow_schema::LayerMember::Layer(layer) if &layer.name == name => Some(layer),
            _ => None,
        })?;
    }
    Some(current)
}

/// Look up a name's binding, innermost scope frame first; `None` when unbound.
/// A bound name may still be [`MarrowType::Unknown`] (an `unknown`-typed binding
/// or one whose type could not be inferred), which is distinct from being unbound.
fn lookup_opt(scope: &[HashMap<String, MarrowType>], name: &str) -> Option<MarrowType> {
    scope
        .iter()
        .rev()
        .find_map(|frame| frame.get(name))
        .cloned()
}

/// Whether an expression is a bare single-segment name (`foo`, not `a::b` or
/// `^books`). In callee position such a name is a function name resolved by
/// `check_call`, so it is not treated as an unresolved value reference.
fn is_bare_name(expr: &marrow_syntax::Expression) -> bool {
    matches!(expr, marrow_syntax::Expression::Name { segments, .. } if segments.len() == 1)
}

/// The decimal envelope, mirroring `marrow_store::decimal`: at most 34
/// significant digits and 34 fractional places.
const DECIMAL_MAX_DIGITS: usize = 34;

/// Flag a numeric literal whose magnitude is provably out of range, so it is
/// caught at check time rather than only at run time (`run.overflow`). The lexer
/// emits a number literal as bare ASCII digits (the sign is a separate unary
/// operator), so an integer is in range exactly when it parses as `i64`, and a
/// decimal `digits.digits` is in range only within the 34-significant-digit /
/// 34-fractional-place envelope.
pub(crate) fn check_literal_range(
    kind: marrow_syntax::LiteralKind,
    text: &str,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::LiteralKind;
    let out_of_range = match kind {
        LiteralKind::Integer => text.parse::<i64>().is_err(),
        LiteralKind::Decimal => decimal_out_of_envelope(text),
        LiteralKind::String | LiteralKind::Bytes | LiteralKind::Bool => false,
    };
    if out_of_range {
        let type_name = match kind {
            LiteralKind::Integer => "int",
            _ => "decimal",
        };
        diagnostics.push(CheckDiagnostic {
            code: CHECK_LITERAL_RANGE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!("{type_name} literal `{text}` is out of range"),
            line: span.line,
            column: span.column,
        });
    }
}

/// Whether a decimal literal `digits.digits` (or `digits`) provably falls outside
/// the 34-digit envelope. Mirrors `marrow_store::decimal`, which normalizes before
/// the envelope check: leading integer zeros and trailing fraction zeros drop out,
/// so they are stripped before counting. A literal is rejected only when its
/// canonical significant digits or fractional places exceed 34 — never a value the
/// runtime would normalize back into range.
fn decimal_out_of_envelope(text: &str) -> bool {
    let (integer, fraction) = text.split_once('.').unwrap_or((text, ""));
    let integer = integer.trim_start_matches('0');
    let fraction = fraction.trim_end_matches('0');
    // Significant digits run from the first to the last nonzero digit. With the
    // integer part empty (all zeros), leading fraction zeros are not significant.
    let significant = if integer.is_empty() {
        fraction.trim_start_matches('0').len()
    } else {
        integer.len() + fraction.len()
    };
    significant > DECIMAL_MAX_DIGITS || fraction.len() > DECIMAL_MAX_DIGITS
}

/// The type of a literal by its lexical kind.
fn literal_type(kind: marrow_syntax::LiteralKind) -> MarrowType {
    use marrow_syntax::LiteralKind;
    MarrowType::Primitive(match kind {
        LiteralKind::Integer => PrimitiveType::Int,
        LiteralKind::Decimal => PrimitiveType::Decimal,
        LiteralKind::String => PrimitiveType::String,
        LiteralKind::Bytes => PrimitiveType::Bytes,
        LiteralKind::Bool => PrimitiveType::Bool,
    })
}

/// Validate a unary operator against its operand type, returning the result type,
/// or [`MarrowType::Unknown`] when the operand is not a known primitive or the
/// operator is misused (which records a diagnostic).
fn check_unary(
    op: marrow_syntax::UnaryOp,
    operand: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::UnaryOp;
    let Some(operand) = as_primitive(operand) else {
        return MarrowType::Unknown;
    };
    let valid = match op {
        UnaryOp::Neg => is_numeric(operand),
        UnaryOp::Not => operand == PrimitiveType::Bool,
    };
    if !valid {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}`",
                unary_symbol(op),
                primitive_name(operand),
            ),
        ));
        return MarrowType::Unknown;
    }
    MarrowType::Primitive(operand)
}

/// Validate a binary operator against its operand types, returning the result
/// type, or [`MarrowType::Unknown`] when either operand is not a known primitive
/// or the operator is misused (which records a diagnostic).
fn check_binary(
    op: marrow_syntax::BinaryOp,
    left: &MarrowType,
    right: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::BinaryOp;
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
            MarrowType::Primitive(PrimitiveType::Decimal),
        ),
        BinaryOp::Remainder => (
            left == PrimitiveType::Int && right == PrimitiveType::Int,
            MarrowType::Primitive(PrimitiveType::Int),
        ),
        BinaryOp::Concat => (
            left == PrimitiveType::String && right == PrimitiveType::String,
            MarrowType::Primitive(PrimitiveType::String),
        ),
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => (
            is_ordered(left) && left == right,
            MarrowType::Primitive(PrimitiveType::Bool),
        ),
        BinaryOp::Equal | BinaryOp::NotEqual => {
            (left == right, MarrowType::Primitive(PrimitiveType::Bool))
        }
        BinaryOp::And | BinaryOp::Or => (
            left == PrimitiveType::Bool && right == PrimitiveType::Bool,
            MarrowType::Primitive(PrimitiveType::Bool),
        ),
        // A range is not a value an operator consumes; accept int endpoints.
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => (
            left == PrimitiveType::Int && right == PrimitiveType::Int,
            MarrowType::Unknown,
        ),
    };
    if !valid {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}` and `{}`",
                binary_symbol(op),
                primitive_name(left),
                primitive_name(right),
            ),
        ));
        return MarrowType::Unknown;
    }
    result
}

/// The primitive a type denotes, or `None` for any non-primitive (resource,
/// identity, sequence, or unknown) type that no operator reasons about.
fn as_primitive(ty: &MarrowType) -> Option<PrimitiveType> {
    match ty {
        MarrowType::Primitive(primitive) => Some(*primitive),
        _ => None,
    }
}

fn is_numeric(primitive: PrimitiveType) -> bool {
    matches!(primitive, PrimitiveType::Int | PrimitiveType::Decimal)
}

fn is_ordered(primitive: PrimitiveType) -> bool {
    matches!(
        primitive,
        PrimitiveType::Int
            | PrimitiveType::Decimal
            | PrimitiveType::String
            | PrimitiveType::Bytes
            | PrimitiveType::Date
            | PrimitiveType::Instant
            | PrimitiveType::Duration
    )
}

fn operator_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_OPERATOR_TYPE,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        line: span.line,
        column: span.column,
    }
}

fn primitive_name(primitive: PrimitiveType) -> &'static str {
    match primitive {
        PrimitiveType::Int => "int",
        PrimitiveType::Decimal => "decimal",
        PrimitiveType::Bool => "bool",
        PrimitiveType::String => "string",
        PrimitiveType::Bytes => "bytes",
        PrimitiveType::Date => "date",
        PrimitiveType::Instant => "instant",
        PrimitiveType::Duration => "duration",
        PrimitiveType::ErrorCode => "ErrorCode",
        PrimitiveType::Error => "Error",
    }
}

fn unary_symbol(op: marrow_syntax::UnaryOp) -> &'static str {
    use marrow_syntax::UnaryOp;
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "not",
    }
}

fn binary_symbol(op: marrow_syntax::BinaryOp) -> &'static str {
    use marrow_syntax::BinaryOp;
    match op {
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Remainder => "%",
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Concat => "_",
        BinaryOp::RangeExclusive => "..",
        BinaryOp::RangeInclusive => "..=",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::Equal => "=",
        BinaryOp::NotEqual => "!=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

/// Validate a call against the user function it resolves to and return that
/// function's declared return type (or [`MarrowType::Unknown`]). Only a plain
/// name call that resolves to a declared function is checked; a builtin, std
/// helper, `Error` constructor, out/inout call, key-lookup (non-name callee), or
/// unresolved name is left alone — mirroring the runtime's dispatch order — so the
/// check never fires on a non-function or a call this slice cannot resolve.
///
/// It flags the argument count (every parameter is required), a named argument
/// that names no parameter, and an argument whose type does not match its
/// parameter (only when both are known, incompatible primitives, like operators).
// Each argument is an independent input threaded through the type-check pipeline
// (program/aliases/file/diagnostics are the cross-cutting context every node
// carries, like `scope`); bundling them would not aid clarity here.
#[allow(clippy::too_many_arguments)]
fn check_call(
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
    // Builtins dispatch before user functions. For std helpers the signatures are
    // fixed (standard-library.md), so argument types and arity are checked here the
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
            .or_else(|| {
                (segments == ["Error"]).then_some(MarrowType::Primitive(PrimitiveType::Error))
            })
            .unwrap_or(MarrowType::Unknown);
    }
    // A callee naming a declared resource is a constructor, not a function:
    // `Book(...)` builds the resource value and `Book::Id(...)` builds its
    // identity (types.md:152-158, 276-297). Recognize it so a spec-valid
    // constructor is not a false `check.unresolved_call`.
    if let Some(ty) = resource_constructor_type(program, segments) {
        return ty;
    }
    let Some(function) = resolve_function(program, segments) else {
        // A non-builtin call that resolves to no declared function is unresolved.
        // Only report it for calls in a library module of the program: a
        // module-less script is not a call target, and a project that did not
        // fully parse has its unresolved calls suppressed in `check_project` (the
        // missing definition may live in an excluded module).
        if file_in_program(program, file) {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNRESOLVED_CALL,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!("function `{}` is not defined", segments.join("::")),
                line: span.line,
                column: span.column,
            });
        }
        return MarrowType::Unknown;
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
        if let Some(param) = param
            && let Some(parameter) = as_primitive(&param.ty)
        {
            check_one_arg(
                &segments.join("::"),
                parameter,
                arg_type,
                span,
                file,
                diagnostics,
            );
        }
    }
    function.return_type.clone().unwrap_or(MarrowType::Unknown)
}

/// Check one positional/named argument's type against its parameter's primitive
/// type: a known-but-different primitive is a `check.call_argument`; an `Unknown`
/// argument for a concrete parameter is a `check.untyped_value` (strict typing —
/// convert dynamic data before typed use). Shared by the user-function and std
/// argument loops; `label` names the callee for the message.
fn check_one_arg(
    label: &str,
    parameter: PrimitiveType,
    arg_type: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match as_primitive(arg_type) {
        Some(argument) if argument != parameter => {
            diagnostics.push(call_diagnostic(
                file,
                span,
                format!(
                    "argument to `{label}` expects `{}`, but found `{}`",
                    primitive_name(parameter),
                    primitive_name(argument),
                ),
            ));
        }
        None if matches!(arg_type, MarrowType::Unknown) => {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_UNTYPED_VALUE,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "argument to `{label}` has no known type, but `{}` is expected; convert it first",
                    primitive_name(parameter),
                ),
                line: span.line,
                column: span.column,
            });
        }
        _ => {}
    }
}

/// Check positional `args` against a fixed positional parameter list (the std
/// helper signatures): an arity mismatch is a `check.call_argument`, and each
/// argument with a known-required primitive parameter is checked by
/// [`check_one_arg`]. A `None` parameter slot (e.g. a path argument) is not a
/// primitive and is left alone. Std helpers are positional-only — named-argument
/// matching stays user-function-only.
fn check_args_against(
    label: &str,
    params: &[Option<PrimitiveType>],
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
            check_one_arg(label, *parameter, arg_type, span, file, diagnostics);
        }
    }
}

/// Type `nextId(^root)` and gate it on a single-`int` saved root. A single-`int`
/// root types to `Resource::Id` (types.md:251, builtins.md:171-175); any other
/// identity shape reports `check.next_id_requires_single_int`. A non-`^root` or
/// wrong-arity argument is left to the runtime (matching how other builtins
/// behave), and an undeclared root is already reported elsewhere (a `^bogus` read
/// has no schema), so neither is double-reported here.
fn check_next_id(
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
    if single_int_root(saved_root) {
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
            next_id_shape(saved_root),
        ),
        line: span.line,
        column: span.column,
    });
    MarrowType::Unknown
}

/// Does this saved root qualify for the default `nextId` policy? Exactly one `int`
/// identity key. This mirrors `marrow_write::single_int_root` — the one contract
/// the runtime gate and the checker must agree on — duplicated only because this
/// crate cannot depend on `marrow-write`; both key off `KeyDef.ty.text == "int"`.
fn single_int_root(root: &marrow_schema::SavedRootSchema) -> bool {
    matches!(root.identity_keys.as_slice(), [key] if key.ty.text.trim() == "int")
}

/// Name the identity shape that disqualifies a root from the default `nextId`
/// policy, for the rejection message (mirrors `marrow_write`'s helper of the same
/// name): a keyless singleton, a single non-`int` key, or a composite identity.
fn next_id_shape(root: &marrow_schema::SavedRootSchema) -> String {
    match root.identity_keys.as_slice() {
        [] => "a keyless singleton".into(),
        [key] => format!("a single `{}` key", key.ty.text.trim()),
        keys => format!("a composite identity of {} keys", keys.len()),
    }
}

/// A `check.call_argument` diagnostic located at a call's span.
fn call_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_CALL_ARGUMENT,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        line: span.line,
        column: span.column,
    }
}

/// Whether `file` is a library module included in the program. Calls in such a
/// file are resolution-checked; a module-less script (not a call target) is not.
fn file_in_program(program: &CheckedProgram, file: &Path) -> bool {
    program
        .modules
        .iter()
        .any(|module| module.source_file == file)
}

/// Resolve a call name to a declared function, mirroring the runtime: a bare name
/// matches the first function of that name in any module; a qualified `mod::fn`
/// name matches a function in exactly that module.
fn resolve_function<'p>(
    program: &'p CheckedProgram,
    segments: &[String],
) -> Option<&'p CheckedFunction> {
    let (name, module) = segments.split_last()?;
    if module.is_empty() {
        program
            .modules
            .iter()
            .flat_map(|module| &module.functions)
            .find(|function| &function.name == name)
    } else {
        let module_name = module.join("::");
        program
            .modules
            .iter()
            .find(|module| module.name == module_name)?
            .functions
            .iter()
            .find(|function| &function.name == name)
    }
}

/// The type produced by a resource constructor callee, if `segments` name a
/// declared resource: `Book(...)` constructs the resource value (its
/// [`MarrowType::Resource`]), and `Book::Id(...)` constructs its identity
/// ([`MarrowType::Identity`]). Any other callee returns `None`, so a genuinely
/// unresolved call is still reported.
fn resource_constructor_type(program: &CheckedProgram, segments: &[String]) -> Option<MarrowType> {
    let is_resource = |name: &str| {
        program
            .modules
            .iter()
            .flat_map(|module| &module.resources)
            .any(|resource| resource.name == name)
    };
    match segments {
        [name] if is_resource(name) => Some(MarrowType::Resource(name.clone())),
        [name, id] if id == "Id" && is_resource(name) => Some(MarrowType::Identity(name.clone())),
        _ => None,
    }
}

/// Whether a name callee is a builtin, std helper, or the `Error` constructor —
/// each dispatched before user functions at runtime, so never a program function.
fn is_builtin_call(segments: &[String]) -> bool {
    match segments {
        // The single-name builtins, grouped as in docs/language/builtins.md. Each
        // dispatches before user-function resolution at runtime, so none is ever a
        // declared program function.
        [name] => matches!(
            name.as_str(),
            // presence and reads
            "exists" | "get"
            // tree traversal
            | "keys" | "values" | "entries" | "count"
            // sequence updates and id allocation
            | "append" | "nextId"
            // write and print
            | "write" | "print"
            // error constructor
            | "Error"
            // conversions
            | "int" | "decimal" | "string" | "bool" | "bytes" | "ErrorCode"
            | "date" | "instant" | "duration"
        ),
        // A `std::module::op` builtin must name a real std module, mirroring
        // import resolution (`is_std_module`/STD_MODULES); an unknown submodule is
        // not a builtin, so it is reported like a rejected `use std::bogus`.
        [first, module, _] => first == "std" && STD_MODULES.contains(&module.as_str()),
        _ => false,
    }
}

/// The return type of a single-name data builtin: `exists(path): bool`,
/// `append(layer, value): int`, and `get(path, default)`, which yields the saved
/// leaf-or-default type (taken from the default argument). `nextId` is handled in
/// [`check_next_id`], which has the `^root` argument it needs to type the identity.
fn builtin_return_type(segments: &[String], arg_types: &[MarrowType]) -> Option<MarrowType> {
    let [name] = segments else {
        return None;
    };
    match name.as_str() {
        "exists" => Some(MarrowType::Primitive(PrimitiveType::Bool)),
        "append" => Some(MarrowType::Primitive(PrimitiveType::Int)),
        "get" => arg_types.get(1).cloned(),
        _ => None,
    }
}

/// The return type of a scalar conversion builtin (`int(x): int`, `string(x):
/// string`, …), per docs/language/builtins.md. The conversion validates a
/// dynamically-typed value and yields the named type.
fn conversion_return_type(segments: &[String]) -> Option<MarrowType> {
    use PrimitiveType::{Bool, Bytes, Date, Decimal, Duration, ErrorCode, Instant, Int, String};
    let [name] = segments else {
        return None;
    };
    let primitive = match name.as_str() {
        "int" => Int,
        "decimal" => Decimal,
        "string" => String,
        "bool" => Bool,
        "bytes" => Bytes,
        "ErrorCode" => ErrorCode,
        "date" => Date,
        "instant" => Instant,
        "duration" => Duration,
        _ => return None,
    };
    Some(MarrowType::Primitive(primitive))
}

/// The declared return type of a value-returning `std::module::op` helper, per
/// `docs/language/standard-library.md`. Void helpers (`std::log`, `std::assert`,
/// `std::io::write*`) and single-name builtins return `None`, leaving the call
/// `Unknown` for the surrounding checks. Argument typing for std helpers stays the
/// runtime's job; this only supplies the result type.
fn std_call_return_type(segments: &[String]) -> Option<MarrowType> {
    use PrimitiveType::{Bool, Bytes, Date, Decimal, Duration, Instant, Int, String};
    let [first, module, op] = segments else {
        return None;
    };
    if first != "std" {
        return None;
    }
    let primitive = |p| Some(MarrowType::Primitive(p));
    match (module.as_str(), op.as_str()) {
        ("text", "length") => primitive(Int),
        ("text", "trim") => primitive(String),
        ("text", "contains") => primitive(Bool),
        ("text", "split") => Some(MarrowType::Sequence(Box::new(MarrowType::Primitive(
            String,
        )))),
        ("bytes", "length") => primitive(Int),
        ("bytes", "base64Encode") => primitive(String),
        ("bytes", "base64Decode") => primitive(Bytes),
        ("math", "absInt") => primitive(Int),
        ("math", "absDecimal") => primitive(Decimal),
        ("math", "floor") => primitive(Int),
        ("math", "modulo") => primitive(Int),
        ("math", "remainder") => primitive(Int),
        ("clock", "now") => primitive(Instant),
        ("clock", "today") => primitive(Date),
        ("clock", "parseInstant") => primitive(Instant),
        ("clock", "parseDate") => primitive(Date),
        ("clock", "parseDuration") => primitive(Duration),
        ("clock", "formatInstant" | "formatDate" | "formatDuration") => primitive(String),
        ("clock", "add") => primitive(Instant),
        ("env", "exists") => primitive(Bool),
        ("env", "get" | "require") => primitive(String),
        ("io", "readText") => primitive(String),
        ("io", "readBytes") => primitive(Bytes),
        _ => None,
    }
}

/// The positional parameter primitive types of a `std::module::op` helper, in
/// order, per `docs/language/standard-library.md`. `Some(params)` for an
/// enumerated helper (including void ones, whose arguments still need checking —
/// hence a separate function from [`std_call_return_type`]); `None` for an unknown
/// op, leaving its arguments to the runtime. A `None` slot inside the list marks a
/// non-primitive argument (`assert::absent` takes a path), which is not checked.
fn std_call_params(segments: &[String]) -> Option<Vec<Option<PrimitiveType>>> {
    use PrimitiveType::{Bool, Bytes, Date, Decimal, Duration, Error, Instant, Int, String};
    let [first, module, op] = segments else {
        return None;
    };
    if first != "std" {
        return None;
    }
    // Each `T` is a concrete primitive parameter; `[]` is a no-argument helper.
    let p = |types: &[PrimitiveType]| Some(types.iter().map(|t| Some(*t)).collect());
    match (module.as_str(), op.as_str()) {
        ("text", "length" | "trim") => p(&[String]),
        ("text", "contains" | "split") => p(&[String, String]),
        ("bytes", "length" | "base64Encode") => p(&[Bytes]),
        ("bytes", "base64Decode") => p(&[String]),
        ("math", "absInt") => p(&[Int]),
        ("math", "absDecimal") => p(&[Decimal]),
        ("math", "floor") => p(&[Decimal]),
        ("math", "modulo" | "remainder") => p(&[Int, Int]),
        ("clock", "now" | "today") => p(&[]),
        ("clock", "parseInstant" | "parseDate" | "parseDuration") => p(&[String]),
        ("clock", "formatInstant") => p(&[Instant]),
        ("clock", "formatDate") => p(&[Date]),
        ("clock", "formatDuration") => p(&[Duration]),
        ("clock", "add") => p(&[Instant, Duration]),
        ("env", "exists" | "require") => p(&[String]),
        ("env", "get") => p(&[String, String]),
        ("io", "readText" | "readBytes") => p(&[String]),
        ("io", "writeText") => p(&[String, String]),
        ("io", "writeBytes") => p(&[String, Bytes]),
        ("assert", "isTrue" | "isFalse") => p(&[Bool]),
        // `absent(path)` takes a path expression, not a primitive — leave it
        // unchecked, matching how the checker leaves other path arguments alone.
        ("assert", "absent") => Some(vec![None]),
        ("assert", "fail") => p(&[String]),
        ("log", "info" | "warn") => p(&[String]),
        ("log", "error") => p(&[Error]),
        _ => None,
    }
}

/// Discover, read, parse, and check a project's test files (the `tests`
/// patterns), producing one checked module per clean test file plus any
/// diagnostics. Test files are scripts outside the source roots, so each is
/// checked module-less and named from its project-relative path
/// (`tests/books_test.mw` → `tests::books_test`). Imports resolve against the
/// already-checked `project` modules, the test modules, and the standard library.
/// Saved-root ownership is not re-checked here: test scripts exercise the
/// project's resources, they do not own saved roots.
pub fn check_tests(
    project_root: &Path,
    config: &ProjectConfig,
    project: &CheckedProgram,
) -> Result<(CheckReport, Vec<CheckedModule>), DiscoverError> {
    let files = discover_test_modules(project_root, config)?;
    let mut report = CheckReport::default();
    let mut modules = Vec::new();
    let mut parsed_files: Vec<(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)> =
        Vec::new();

    for file in &files {
        let Some(CheckedFile {
            parsed,
            resources,
            functions,
            constants,
        }) = check_file(&file.path, &mut report.diagnostics)
        else {
            continue;
        };

        // A test file is a script: it is named from its path, never from a
        // declared `module`, so it can never shadow or duplicate a project
        // module. Skip a file carrying a parse error.
        if !parsed.has_errors() {
            modules.push(CheckedModule {
                name: file.module_name.clone().unwrap_or_default(),
                source_file: file.path.clone(),
                span: SourceSpan::default(),
                imports: parsed
                    .file
                    .uses
                    .iter()
                    .map(|use_decl| use_decl.name.clone())
                    .collect(),
                constants,
                functions,
                resources,
            });
        }
        parsed_files.push((file, parsed));
    }

    // Imports in a test file resolve against the project's modules, the other
    // test modules, and the standard library.
    let mut resolvable: HashMap<String, PathBuf> = project
        .modules
        .iter()
        .map(|module| (module.name.clone(), module.source_file.clone()))
        .collect();
    for module in &modules {
        resolvable.insert(module.name.clone(), module.source_file.clone());
    }
    for (file, parsed) in &parsed_files {
        for use_decl in &parsed.file.uses {
            if !is_resolved_import(&use_decl.name, &resolvable) {
                report.diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNRESOLVED_IMPORT,
                    severity: Severity::Error,
                    file: file.path.clone(),
                    message: format!("cannot resolve import `{}`", use_decl.name),
                    line: use_decl.span.line,
                    column: use_decl.span.column,
                });
            }
        }
    }

    Ok((report, modules))
}

/// The per-file result of [`check_file`]: the parsed source and the declaration
/// lists collected for a [`CheckedModule`].
struct CheckedFile {
    parsed: marrow_syntax::ParsedSource,
    resources: Vec<marrow_schema::ResourceSchema>,
    functions: Vec<CheckedFunction>,
    constants: Vec<CheckedConst>,
}

/// Read, parse, and structurally check one source file: record its parse,
/// duplicate-name, function-body, const-value, and resource-schema diagnostics,
/// and collect the declaration lists for a checked module. Returns `None` if the
/// file cannot be read (an `io.read` diagnostic is recorded). Cross-file checks —
/// module-path matching, saved-root ownership, stable-id uniqueness, and import
/// resolution — belong to the caller, since they span files and differ between
/// library modules and test scripts.
fn check_file(file_path: &Path, diagnostics: &mut Vec<CheckDiagnostic>) -> Option<CheckedFile> {
    let source = match std::fs::read_to_string(file_path) {
        Ok(source) => source,
        Err(error) => {
            diagnostics.push(CheckDiagnostic {
                code: IO_READ,
                severity: Severity::Error,
                file: file_path.to_path_buf(),
                message: format!("failed to read source: {error}"),
                line: 0,
                column: 0,
            });
            return None;
        }
    };

    let parsed = parse_source(&source);
    for diagnostic in &parsed.diagnostics {
        diagnostics.push(CheckDiagnostic {
            code: diagnostic.code,
            severity: diagnostic.severity,
            file: file_path.to_path_buf(),
            message: diagnostic.message.clone(),
            line: diagnostic.line,
            column: diagnostic.column,
        });
    }

    check_duplicate_declarations(file_path, &parsed.file, diagnostics);

    // Resource names resolve first so a module's function and constant types can
    // refer to its own resources.
    let module_resources: Vec<String> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Resource(resource) => Some(resource.name.clone()),
            _ => None,
        })
        .collect();
    let mut resources = Vec::new();
    let mut functions = Vec::new();
    let mut constants = Vec::new();
    for declaration in &parsed.file.declarations {
        match declaration {
            marrow_syntax::Declaration::Function(function) => {
                rules::check_function_body(file_path, &function.body, diagnostics);
                functions.push(checked_function(function, &module_resources));
            }
            marrow_syntax::Declaration::Resource(resource) => {
                let (schema, errors) = marrow_schema::compile_resource(resource);
                for error in errors {
                    diagnostics.push(CheckDiagnostic {
                        code: error.code,
                        severity: Severity::Error,
                        file: file_path.to_path_buf(),
                        message: error.message,
                        line: error.span.line,
                        column: error.span.column,
                    });
                }
                resources.push(schema);
            }
            marrow_syntax::Declaration::Const(constant) => {
                rules::check_const_value(file_path, &constant.value, diagnostics);
                constants.push(CheckedConst {
                    name: constant.name.clone(),
                    ty: constant
                        .ty
                        .as_ref()
                        .map(|ty| MarrowType::resolve(ty, &module_resources)),
                    span: constant.span,
                });
            }
        }
    }

    Some(CheckedFile {
        parsed,
        resources,
        functions,
        constants,
    })
}

/// Resolve a function declaration for the checked-program artifact: its
/// signature (parameter and return types resolve against the module's own
/// resource names) plus its body, which the runtime evaluates.
fn checked_function(
    function: &marrow_syntax::FunctionDecl,
    module_resources: &[String],
) -> CheckedFunction {
    CheckedFunction {
        name: function.name.clone(),
        public: function.public,
        params: function
            .params
            .iter()
            .map(|param| CheckedParam {
                name: param.name.clone(),
                mode: param.mode,
                ty: MarrowType::resolve(&param.ty, module_resources),
            })
            .collect(),
        return_type: function
            .return_type
            .as_ref()
            .map(|ty| MarrowType::resolve(ty, module_resources)),
        span: function.span,
        touches_saved_data: block_touches_saved_data(&function.body),
        body: function.body.clone(),
    }
}

/// Does a block read or write any saved root (`^...`)? Walks every statement and
/// nested expression, recursing through nested blocks.
fn block_touches_saved_data(block: &marrow_syntax::Block) -> bool {
    block.statements.iter().any(statement_touches_saved_data)
}

fn statement_touches_saved_data(statement: &marrow_syntax::Statement) -> bool {
    use marrow_syntax::Statement;
    match statement {
        Statement::Const { value, .. } | Statement::Throw { value, .. } => {
            expr_touches_saved_data(value)
        }
        Statement::Var { value, .. } => value.as_ref().is_some_and(expr_touches_saved_data),
        Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
            expr_touches_saved_data(target) || expr_touches_saved_data(value)
        }
        Statement::Delete { path, .. } => expr_touches_saved_data(path),
        Statement::Return { value, .. } => value.as_ref().is_some_and(expr_touches_saved_data),
        Statement::Expr { value, .. } => expr_touches_saved_data(value),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            expr_touches_saved_data(condition)
                || block_touches_saved_data(then_block)
                || else_ifs.iter().any(|else_if| {
                    expr_touches_saved_data(&else_if.condition)
                        || block_touches_saved_data(&else_if.block)
                })
                || else_block.as_ref().is_some_and(block_touches_saved_data)
        }
        Statement::While {
            condition, body, ..
        } => expr_touches_saved_data(condition) || block_touches_saved_data(body),
        Statement::For { iterable, body, .. } => {
            expr_touches_saved_data(iterable) || block_touches_saved_data(body)
        }
        Statement::Transaction { body, .. } => block_touches_saved_data(body),
        Statement::Lock { path, body, .. } => {
            expr_touches_saved_data(path) || block_touches_saved_data(body)
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            block_touches_saved_data(body)
                || catch
                    .as_ref()
                    .is_some_and(|catch| block_touches_saved_data(&catch.block))
                || finally.as_ref().is_some_and(block_touches_saved_data)
        }
        Statement::Break { .. } | Statement::Continue { .. } | Statement::Unparsed { .. } => false,
    }
}

fn expr_touches_saved_data(expr: &marrow_syntax::Expression) -> bool {
    use marrow_syntax::{Expression, InterpolationPart};
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Literal { .. } | Expression::Name { .. } | Expression::Unparsed { .. } => false,
        Expression::Call { callee, args, .. } => {
            expr_touches_saved_data(callee)
                || args.iter().any(|arg| expr_touches_saved_data(&arg.value))
        }
        Expression::Field { base, .. } => expr_touches_saved_data(base),
        Expression::Unary { operand, .. } => expr_touches_saved_data(operand),
        Expression::Binary { left, right, .. } => {
            expr_touches_saved_data(left) || expr_touches_saved_data(right)
        }
        Expression::Interpolation { parts, .. } => parts.iter().any(|part| match part {
            InterpolationPart::Text { .. } => false,
            InterpolationPart::Expr(expr) => expr_touches_saved_data(expr),
        }),
    }
}

/// Build the per-file short→full alias map from a file's full import paths
/// (`["std::clock", "shelf::books"]`): key = the short trailing segment
/// (`"clock"`, `"books"`), value = the full path split into segments
/// (`["std","clock"]`). Same short-name derivation as
/// [`check_duplicate_declarations`], so the two stay consistent. Drives short-form
/// call resolution; the runtime builds the identical map from
/// `CheckedModule::imports`.
pub fn build_alias_map(import_paths: &[String]) -> std::collections::HashMap<String, Vec<String>> {
    import_paths
        .iter()
        .map(|path| {
            let short = path.rsplit("::").next().unwrap_or(path).to_string();
            let full = path.split("::").map(str::to_string).collect();
            (short, full)
        })
        .collect()
}

/// Expand a call/name's leading segment through the file's import aliases, applied
/// once up front before any builtin/std/function resolution. `clock::now` with
/// `{clock → [std,clock]}` becomes `[std,clock,now]`. A single-segment name is
/// never expanded (short-form requires the module qualifier), and a leading
/// segment that is not an alias is left untouched (so `std::clock::now` and a
/// project `mod::fn` pass through unchanged). This is the shared semantics both the
/// checker and the runtime apply, so short-form resolves symmetrically.
pub fn expand_alias(
    segments: &[String],
    aliases: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    if segments.len() >= 2
        && let Some(full) = aliases.get(&segments[0])
    {
        return full
            .iter()
            .cloned()
            .chain(segments[1..].iter().cloned())
            .collect();
    }
    segments.to_vec()
}

/// The standard-library modules. Host modules resolve at check time even when a
/// host would not provide them at run time.
const STD_MODULES: &[&str] = &[
    "clock", "io", "env", "text", "bytes", "math", "assert", "log",
];

/// Is `name` a standard-library module path? Accepts `std::<module>` and any
/// deeper path under a valid `std` module.
fn is_std_module(name: &str) -> bool {
    name.strip_prefix("std::")
        .and_then(|rest| rest.split("::").next())
        .is_some_and(|module| STD_MODULES.contains(&module))
}

/// An import resolves when it names a project module or a standard-library
/// module.
fn is_resolved_import(name: &str, project_modules: &HashMap<String, PathBuf>) -> bool {
    project_modules.contains_key(name) || is_std_module(name)
}

fn module_path_error(
    file: &marrow_project::ModuleFile,
    module: &marrow_syntax::ModuleDecl,
    message: String,
) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_MODULE_PATH,
        severity: Severity::Error,
        file: file.path.clone(),
        message,
        line: module.span.line,
        column: module.span.column,
    }
}

/// Top-level declaration names (const, resource, function) and imported short
/// module names share one namespace within a file. Flag any name introduced
/// more than once, reporting the later occurrence and referencing the first.
fn check_duplicate_declarations(
    file: &Path,
    source: &marrow_syntax::SourceFile,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Declaration;

    // Every name this file introduces, in source order. A `use shelf::books`
    // introduces the short name `books`.
    let mut introduced: Vec<(&str, SourceSpan, &'static str)> = Vec::new();
    for use_decl in &source.uses {
        let short = use_decl.name.rsplit("::").next().unwrap_or(&use_decl.name);
        introduced.push((short, use_decl.span, "import"));
    }
    for declaration in &source.declarations {
        let (name, span) = match declaration {
            Declaration::Const(decl) => (decl.name.as_str(), decl.span),
            Declaration::Resource(decl) => (decl.name.as_str(), decl.span),
            Declaration::Function(decl) => (decl.name.as_str(), decl.span),
        };
        introduced.push((name, span, "declaration"));
    }
    introduced.sort_by_key(|(_, span, _)| (span.line, span.start_byte));

    let mut first_seen: HashMap<&str, SourceSpan> = HashMap::new();
    for (name, span, _kind) in &introduced {
        // The parser leaves a failed declaration with an empty name; do not
        // treat those as colliding with each other.
        if name.is_empty() {
            continue;
        }
        match first_seen.get(name) {
            Some(first) => diagnostics.push(CheckDiagnostic {
                code: CHECK_DUPLICATE_DECLARATION,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!("`{name}` is already declared on line {}", first.line),
                line: span.line,
                column: span.column,
            }),
            None => {
                first_seen.insert(name, *span);
            }
        }
    }
}
