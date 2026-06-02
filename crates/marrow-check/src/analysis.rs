//! The IDE-grade analysis surface: project analysis plus cursor type and
//! scope queries. This is the stable path editor tooling consumes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, ProjectConfig, discover_modules};
use marrow_syntax::{Severity, SourceSpan};

use crate::checks::{
    ModuleNamePolicy, ResolvedFileCheck, check_resolved_files, file_prelude, for_frame,
};
use crate::enums::{collect_enum_names, normalize_program_named_types, resolve_type};
use crate::infer::{bind, infer_type, local_binding};
use crate::{
    CHECK_DUPLICATE_MODULE, CHECK_MULTIPLE_SCRIPTS, CheckDiagnostic, CheckReport, CheckedFile,
    CheckedModule, CheckedProgram, IO_READ, MarrowType, ProjectSources,
    SCHEMA_DUPLICATE_ROOT_OWNER, TestResolutionSuppression, check_file_source, enum_visibility,
    module_path_error, read_source, resolve_match_enums,
};

/// An IDE-grade view of a checked project: the diagnostics and best-effort
/// program [`check_project`] produces, plus every parsed file — including files
/// with parse errors, which contribute no [`CheckedModule`] but are retained
/// here so editor tooling can still work on them.
#[derive(Debug, Clone)]
pub struct AnalysisSnapshot {
    pub report: CheckReport,
    pub program: CheckedProgram,
    pub files: Vec<AnalyzedFile>,
}

/// One parsed file in an [`AnalysisSnapshot`]: its path, the module name its
/// path implies (`None` for a path that cannot name a module), and the parse —
/// retained whether or not it carries errors.
#[derive(Debug, Clone)]
pub struct AnalyzedFile {
    pub path: PathBuf,
    pub module_name: Option<String>,
    pub source: String,
    pub parsed: marrow_syntax::ParsedSource,
}

/// The IDE-grade analysis core: discover, read (from `sources` overlay or disk),
/// parse, and check source-root files plus configured test files, returning the
/// diagnostics and best-effort source program plus every parsed source file
/// (error files included). Fails only when a configured source or test directory
/// cannot be walked.
pub fn analyze_project(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
) -> Result<AnalysisSnapshot, DiscoverError> {
    let mut snapshot = analyze_source_project(project_root, config, sources)?;
    let resolution_suppression = source_resolution_suppression(&snapshot, project_root, config);
    let tests = crate::check_tests_with_sources_analysis(
        project_root,
        config,
        &snapshot.program,
        sources,
        resolution_suppression,
    )?;
    snapshot.report.diagnostics.extend(tests.report.diagnostics);
    snapshot.files.extend(tests.files);
    snapshot.files.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot.files.dedup_by(|a, b| a.path == b.path);
    Ok(snapshot)
}

fn source_resolution_suppression(
    snapshot: &AnalysisSnapshot,
    project_root: &Path,
    config: &ProjectConfig,
) -> TestResolutionSuppression {
    let mut suppression = TestResolutionSuppression::default();
    let mut declared_modules: HashMap<String, usize> = HashMap::new();
    for file in &snapshot.files {
        if let Some(module) = &file.parsed.file.module {
            *declared_modules.entry(module.name.clone()).or_default() += 1;
        }
    }

    for file in &snapshot.files {
        let declared = file.parsed.file.module.as_ref().map(|module| &module.name);
        let path_matches = match (declared, file.module_name.as_ref()) {
            (Some(declared), Some(expected)) => declared == expected,
            (Some(_), None) => false,
            _ => true,
        };
        let duplicate_module = declared.is_some_and(|name| declared_modules[name] > 1);
        if file.parsed.has_errors() || !path_matches || duplicate_module {
            let mut hidden_module_names = Vec::new();
            if let Some(name) = declared {
                hidden_module_names.push(name.clone());
            }
            if let Some(name) = &file.module_name
                && !hidden_module_names.iter().any(|module| module == name)
            {
                hidden_module_names.push(name.clone());
            }
            for name in &hidden_module_names {
                suppression.hide_module(name.clone());
            }
            suppression.hide_declared_types(&file.parsed, &hidden_module_names);
        }
    }

    for diagnostic in &snapshot.report.diagnostics {
        if diagnostic.code == IO_READ
            && let Some(file) = overlay_module_file(project_root, config, &diagnostic.file)
            && let Some(name) = file.module_name
        {
            suppression.hide_module(name);
        }
    }

    suppression
}

/// Source-root-only analysis shared by [`check_project`]. Runtime entry points use
/// this so configured test files do not block running the checked source program.
pub(crate) fn analyze_source_project(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
) -> Result<AnalysisSnapshot, DiscoverError> {
    let mut files = discover_modules(project_root, config)?;
    for path in sources.paths() {
        if let Some(file) = overlay_module_file(project_root, config, path) {
            files.push(file);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    let mut report = CheckReport::default();
    let mut program = CheckedProgram::default();
    // The first valid library file (in path order) to declare each module owns
    // that name; later files declaring it are duplicates. This is also the set
    // of resolvable project module names for `use` resolution.
    let mut declared: HashMap<String, PathBuf> = HashMap::new();
    // The first store (in file then source order) to declare each saved root owns
    // it; a later store on the same root is a duplicate owner.
    let mut root_owners: HashMap<String, PathBuf> = HashMap::new();
    // Parsed sources kept from pass 1 so pass 2 can resolve imports against the
    // full project module set without re-reading files.
    let mut parsed_files: Vec<(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)> =
        Vec::new();
    let mut parsed_sources: HashMap<PathBuf, String> = HashMap::new();
    // Module-less parse-clean files, deferred until the whole project is seen. A
    // project may hold at most one such single-file script; it joins `program`
    // under the empty module name. Two or more would share that name, so a bare
    // reference in one could alias another's declarations — that is rejected, not
    // assembled. Holding the candidates until the loop ends lets the decision rest
    // on the project-wide count rather than first-seen order.
    let mut scripts: Vec<CheckedModule> = Vec::new();

    // Pass 1: parse each file and collect per-file findings plus the project's
    // module set.
    for file in &files {
        // Editor buffer text wins over disk for an overlaid path; a path with no
        // overlay is read from disk, and a read failure drops the file (its
        // `io.read` diagnostic is recorded) exactly as before.
        let Some(source) = read_source(&file.path, sources, &mut report.diagnostics) else {
            continue;
        };
        let CheckedFile {
            parsed,
            resources,
            stores,
            enums,
            functions,
            constants,
        } = check_file_source(&file.path, &source, &mut report.diagnostics);

        // Saved roots are owned project-wide by stores.
        for declaration in &parsed.file.declarations {
            let marrow_syntax::Declaration::Store(store) = declaration else {
                continue;
            };
            match root_owners.get(&store.root.root) {
                Some(first) => report.diagnostics.push(CheckDiagnostic {
                    code: SCHEMA_DUPLICATE_ROOT_OWNER,
                    severity: Severity::Error,
                    file: file.path.clone(),
                    message: format!(
                        "saved root `^{}` is already owned by a store in `{}`",
                        store.root.root,
                        first.display()
                    ),
                    span: store.span,
                }),
                None => {
                    root_owners.insert(store.root.root.clone(), file.path.clone());
                }
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
                            span: module.span,
                        });
                    } else {
                        declared.insert(expected.clone(), file.path.clone());
                        // The artifact takes a clean, path-matched, first-seen
                        // library module; a file carrying a parse error contributes none.
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
                                stores,
                                enums,
                                enum_public: enum_visibility(&parsed.file),
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
        } else if !parsed.has_errors() {
            // A module-less file is a single-file script: its declarations are
            // nominally self-resolvable within the file, but it is never bound to
            // a path and no other module can `use` it. A single script joins
            // `program` under the empty module name it always carries — so its own
            // resource, enum, and function references resolve and are checked — but
            // never `declared`, the import-resolvable map, so it stays
            // un-importable. The empty-name module is deferred until the project's
            // script count is known. A file carrying a parse error contributes none.
            scripts.push(CheckedModule {
                name: String::new(),
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
                stores,
                enums,
                enum_public: enum_visibility(&parsed.file),
            });
        }

        parsed_sources.insert(file.path.clone(), source);
        parsed_files.push((file, parsed));
    }

    // A project may have at most one module-less file: its single entrypoint
    // script. Exactly one joins the program under the empty module name. Two or
    // more share that name, so the empty-named module would be ambiguous — a bare
    // reference in one script could resolve against another's declarations, a
    // `var o: Order` could bind to a foreign resource of the same name, and an
    // entry in any but the first would be unreachable at run time. Rather than
    // assemble that aliasing module, reject every script: a project's library files
    // must declare a `module`. The single-file `check`/`run` paths see one script
    // and are unaffected.
    if scripts.len() <= 1 {
        program.modules.append(&mut scripts);
    } else {
        for script in &scripts {
            report.diagnostics.push(CheckDiagnostic {
                code: CHECK_MULTIPLE_SCRIPTS,
                severity: Severity::Error,
                file: script.source_file.clone(),
                message: "a project may have at most one file without a `module` \
                    declaration (its single-file script); declare a `module` for this file"
                    .to_string(),
                span: SourceSpan::default(),
            });
        }
    }

    // Pass 3: flag type annotations on functions and constants that name an
    // unknown type. Resource and enum member types are validated by schema
    // compilation. Both name sets are gathered from every parsed file (not from
    // `program`) so a type referencing a name in an error-bearing file is not
    // false-flagged unknown.
    let project_resources: HashSet<String> = parsed_files
        .iter()
        .flat_map(|(_, parsed)| parsed.file.declarations.iter())
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Resource(resource) => Some(resource.name.clone()),
            _ => None,
        })
        .collect();
    let project_enums = collect_enum_names(&parsed_files);

    // Stamp each cross-module named-type signature slot with its true owner, now
    // that the whole program is known, before any pass reads parameter types.
    normalize_program_named_types(&mut program, &parsed_files);

    // Passes 2-3 plus unresolved-call suppression are shared with check_tests.
    check_resolved_files(
        ResolvedFileCheck {
            files: &files,
            parsed_files: &parsed_files,
            module_name_policy: ModuleNamePolicy::DeclaredOrPath,
            resolvable: &declared,
            program: &program,
            project_resources: &project_resources,
            project_enums: &project_enums,
        },
        &mut report,
    );

    // Record each `match`'s resolved scrutinee enum on the artifact's bodies, so the
    // runtime dispatches by ordinals rather than guessing the enum from the arms.
    let snapshot = program.clone();
    resolve_match_enums(&mut program, &snapshot);
    program.rebuild_facts_with_sources(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );

    // Move every parse — error files included — into the snapshot now that the
    // shared tail has finished borrowing them. The path and module name are
    // copied from each `ModuleFile`; the parse itself moves.
    let analyzed = parsed_files
        .into_iter()
        .map(|(file, parsed)| AnalyzedFile {
            path: file.path.clone(),
            module_name: file.module_name.clone(),
            source: parsed_sources.remove(&file.path).unwrap_or_default(),
            parsed,
        })
        .collect();

    Ok(AnalysisSnapshot {
        report,
        program,
        files: analyzed,
    })
}

fn overlay_module_file(
    project_root: &Path,
    config: &ProjectConfig,
    path: &Path,
) -> Option<marrow_project::ModuleFile> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("mw") {
        return None;
    }
    for source_root in &config.source_roots {
        let root = project_root.join(source_root);
        let Ok(relative_path) = path.strip_prefix(&root) else {
            continue;
        };
        let relative_path = relative_path.to_path_buf();
        return Some(marrow_project::ModuleFile {
            path: path.to_path_buf(),
            module_name: marrow_project::expected_module_name(&relative_path),
            relative_path,
        });
    }
    None
}

/// The type of the expression at byte `offset` in `parsed` (a file of `program`),
/// or `None` when no expression covers the offset. Editor tooling uses this for
/// hover and type-aware actions. It reconstructs the cursor's lexical scope
/// exactly as the checker does — module constants and imports, the enclosing
/// function's parameters, the `const`/`var` bindings that precede the cursor, and
/// any loop or catch binding whose body the cursor sits in — then infers the
/// smallest expression covering the offset. It records no diagnostics.
pub fn type_at(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Option<MarrowType> {
    let prelude = file_prelude(program, file, parsed);
    let function = enclosing_function(parsed, offset)?;
    let mut scope = function_base_scope(
        program,
        function,
        &prelude.module_constants,
        &prelude.aliases,
        file,
    );
    walk_block_to_offset(
        program,
        &function.body,
        offset,
        &prelude.aliases,
        file,
        &mut scope,
    );
    let expr = smallest_expression_at(&function.body, offset)?;
    Some(infer_type(
        program,
        expr,
        &scope,
        &prelude.aliases,
        file,
        &mut Vec::new(),
    ))
}

/// The bindings visible at byte `offset` in `parsed` (a file of `program`), as
/// `(name, type)` pairs, for completion. The reconstructed scope is the same one
/// [`type_at`] infers against: module constants and imports, then — when the
/// offset is inside a function — that function's parameters, the `const`/`var`
/// locals declared before the cursor, and any loop or catch binding in scope.
/// Import aliases are surfaced with [`MarrowType::Unknown`] (they name modules,
/// not values). Inner bindings shadow outer ones. It records no diagnostics.
pub fn scope_at(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Vec<(String, MarrowType)> {
    let prelude = file_prelude(program, file, parsed);
    // Imports and module constants are the outermost frame; a later frame's
    // binding shadows them. Imports name modules, so they carry no value type.
    let mut scope: Vec<HashMap<String, MarrowType>> = vec![
        prelude
            .aliases
            .keys()
            .map(|alias| (alias.clone(), MarrowType::Unknown))
            .collect(),
        prelude.module_constants.clone(),
    ];
    if let Some(function) = enclosing_function(parsed, offset) {
        scope.extend(function_base_scope(
            program,
            function,
            &prelude.module_constants,
            &prelude.aliases,
            file,
        ));
        walk_block_to_offset(
            program,
            &function.body,
            offset,
            &prelude.aliases,
            file,
            &mut scope,
        );
    }
    // Flatten outermost-first so an inner binding overwrites a shadowed outer one,
    // leaving each visible name once with the type that actually applies.
    let mut visible: HashMap<String, MarrowType> = HashMap::new();
    for frame in scope {
        visible.extend(frame);
    }
    let mut bindings: Vec<(String, MarrowType)> = visible.into_iter().collect();
    bindings.sort_by(|a, b| a.0.cmp(&b.0));
    bindings
}

/// The function declaration whose body span covers `offset`, if any. A cursor in a
/// function signature or at module level has no enclosing body and yields `None`.
pub(crate) fn enclosing_function(
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Option<&marrow_syntax::FunctionDecl> {
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            marrow_syntax::Declaration::Function(function)
                if span_covers(function.body.span, offset) =>
            {
                Some(function)
            }
            _ => None,
        })
}

/// The base scope frame for a function body: the module's constants overlaid with
/// the parameter list, mirroring [`check_function_types`] (a parameter shadows a
/// like-named constant).
pub(crate) fn function_base_scope(
    program: &CheckedProgram,
    function: &marrow_syntax::FunctionDecl,
    module_constants: &HashMap<String, MarrowType>,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Vec<HashMap<String, MarrowType>> {
    let mut base = module_constants.clone();
    for param in &function.params {
        base.insert(
            param.name.clone(),
            resolve_type(&param.ty, program, aliases, file),
        );
    }
    vec![base]
}

/// Replay the binding behavior of [`check_block_types`]/[`check_statement_types`]
/// up to `offset`: push a frame for `block`, record each `const`/`var` binding the
/// block introduces before the cursor, and descend into the one nested block (and
/// its loop or catch frame) that covers the cursor. Statements after the cursor
/// are not visible and are skipped. This shares the checker's binding primitives
/// (`local_binding`, the loop/catch frames) so the reconstructed scope cannot
/// drift from the one the checker builds.
pub(crate) fn walk_block_to_offset(
    program: &CheckedProgram,
    block: &marrow_syntax::Block,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    scope: &mut Vec<HashMap<String, MarrowType>>,
) {
    scope.push(HashMap::new());
    for statement in &block.statements {
        // A binding declared at or after the cursor is not yet in scope. Compared
        // against the statement's start so the cursor on a `const`'s own line does
        // not see that `const` (its initializer cannot reference itself).
        if statement.span().start_byte >= offset {
            break;
        }
        // Record the binding this statement introduces, exactly as the checker
        // does, before deciding whether to descend into it.
        if let Some((name, ty)) = local_binding(program, statement, scope, aliases, file) {
            bind(scope, &name, ty);
        }
        // Descend into the nested block (and its loop/catch frame) that the cursor
        // sits in. Only one statement can cover the cursor, so the walk stops here.
        if span_covers(statement.span(), offset)
            && let Some(body) = descend_target(program, statement, offset, aliases, file, scope)
        {
            walk_block_to_offset(program, body, offset, aliases, file, scope);
            return;
        }
    }
}

/// The nested block of `statement` that covers `offset`, pushing the loop or catch
/// frame that block runs under (a `for` binding, a `catch` error value) onto
/// `scope` first, mirroring [`check_statement_types`]. Returns `None` when the
/// cursor is in the statement but not in one of its bodies (for example in an `if`
/// condition), leaving `scope` untouched.
pub(crate) fn descend_target<'b>(
    program: &CheckedProgram,
    statement: &'b marrow_syntax::Statement,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    scope: &mut Vec<HashMap<String, MarrowType>>,
) -> Option<&'b marrow_syntax::Block> {
    use marrow_syntax::Statement;
    match statement {
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if span_covers(then_block.span, offset) {
                return Some(then_block);
            }
            for else_if in else_ifs {
                if span_covers(else_if.block.span, offset) {
                    return Some(&else_if.block);
                }
            }
            else_block
                .as_ref()
                .filter(|block| span_covers(block.span, offset))
        }
        Statement::While { body, .. }
        | Statement::Transaction { body, .. }
        | Statement::Lock { body, .. } => span_covers(body.span, offset).then_some(body),
        Statement::For {
            binding,
            iterable,
            body,
            ..
        } => {
            if !span_covers(body.span, offset) {
                return None;
            }
            let frame = for_frame(program, binding, iterable, scope, aliases, file);
            scope.push(frame);
            Some(body)
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            if span_covers(body.span, offset) {
                return Some(body);
            }
            if let Some(clause) = catch
                && span_covers(clause.block.span, offset)
            {
                let mut frame = HashMap::new();
                frame.insert(clause.name.clone(), MarrowType::Error);
                scope.push(frame);
                return Some(&clause.block);
            }
            finally
                .as_ref()
                .filter(|block| span_covers(block.span, offset))
        }
        _ => None,
    }
}

/// Whether `span` covers `offset`, inclusive of the end byte so a cursor at the
/// closing edge of an expression still resolves.
pub(crate) fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

/// The smallest expression in a function `body` whose span covers `offset`, the
/// expression the cursor sits on. Walks every expression the type pass would
/// visit, keeping the tightest span. Statement-level structure (conditions,
/// initializers, call arguments, nested blocks) is traversed so the cursor lands
/// on the leaf expression rather than an enclosing one.
pub(crate) fn smallest_expression_at(
    body: &marrow_syntax::Block,
    offset: usize,
) -> Option<&marrow_syntax::Expression> {
    let mut best: Option<&marrow_syntax::Expression> = None;
    collect_block_expression(body, offset, &mut best);
    best
}

pub(crate) fn collect_block_expression<'b>(
    block: &'b marrow_syntax::Block,
    offset: usize,
    best: &mut Option<&'b marrow_syntax::Expression>,
) {
    use marrow_syntax::Statement;
    for statement in &block.statements {
        match statement {
            Statement::Const { value, .. } | Statement::Throw { value, .. } => {
                collect_expression(value, offset, best);
            }
            Statement::Expr { value, .. } => collect_expression(value, offset, best),
            Statement::Var { value, .. } => {
                if let Some(value) = value {
                    collect_expression(value, offset, best);
                }
            }
            Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
                collect_expression(target, offset, best);
                collect_expression(value, offset, best);
            }
            Statement::Delete { path, .. } => collect_expression(path, offset, best),
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    collect_expression(value, offset, best);
                }
            }
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                if let Some(condition) = condition {
                    collect_expression(condition, offset, best);
                }
                collect_block_expression(then_block, offset, best);
                for else_if in else_ifs {
                    if let Some(condition) = &else_if.condition {
                        collect_expression(condition, offset, best);
                    }
                    collect_block_expression(&else_if.block, offset, best);
                }
                if let Some(block) = else_block {
                    collect_block_expression(block, offset, best);
                }
            }
            Statement::While {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_expression(condition, offset, best);
                }
                collect_block_expression(body, offset, best);
            }
            Statement::For { iterable, body, .. } => {
                collect_expression(iterable, offset, best);
                collect_block_expression(body, offset, best);
            }
            Statement::Transaction { body, .. } => collect_block_expression(body, offset, best),
            Statement::Lock { path, body, .. } => {
                if let Some(path) = path {
                    collect_expression(path, offset, best);
                }
                collect_block_expression(body, offset, best);
            }
            Statement::Try {
                body,
                catch,
                finally,
                ..
            } => {
                collect_block_expression(body, offset, best);
                if let Some(clause) = catch {
                    collect_block_expression(&clause.block, offset, best);
                }
                if let Some(finally) = finally {
                    collect_block_expression(finally, offset, best);
                }
            }
            Statement::Match {
                scrutinee, arms, ..
            } => {
                if let Some(scrutinee) = scrutinee {
                    collect_expression(scrutinee, offset, best);
                }
                for arm in arms {
                    collect_block_expression(&arm.block, offset, best);
                }
            }
            Statement::Break { .. } | Statement::Continue { .. } => {}
        }
    }
}

/// Keep `expr` as the best match when its span covers `offset` and is no wider
/// than the current best, then recurse into its subexpressions so the tightest
/// covering leaf wins.
pub(crate) fn collect_expression<'e>(
    expr: &'e marrow_syntax::Expression,
    offset: usize,
    best: &mut Option<&'e marrow_syntax::Expression>,
) {
    use marrow_syntax::Expression;
    let span = expr.span();
    if !span_covers(span, offset) {
        return;
    }
    let width = span.end_byte.saturating_sub(span.start_byte);
    let replace = best.is_none_or(|current| {
        let current = current.span();
        width <= current.end_byte.saturating_sub(current.start_byte)
    });
    if replace {
        *best = Some(expr);
    }
    match expr {
        Expression::Unary { operand, .. } => collect_expression(operand, offset, best),
        Expression::Binary { left, right, .. } => {
            collect_expression(left, offset, best);
            collect_expression(right, offset, best);
        }
        Expression::Call { callee, args, .. } => {
            collect_expression(callee, offset, best);
            for arg in args {
                collect_expression(&arg.value, offset, best);
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            collect_expression(base, offset, best);
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let marrow_syntax::InterpolationPart::Expr(inner) = part {
                    collect_expression(inner, offset, best);
                }
            }
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {}
    }
}
