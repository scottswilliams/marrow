//! Resolve and check a Marrow project's source.
//!
//! Discover the project's `.mw` files, parse each one, and report parse
//! diagnostics together with module/path resolution, type, and schema problems,
//! producing a resolved [`CheckedProgram`] alongside the diagnostics.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, ProjectConfig, discover_modules, discover_test_modules};
use marrow_schema::stdlib::{self, ParamType, ReturnType};
use marrow_store::value::ScalarType;
use marrow_syntax::{Severity, SourceSpan, parse_source};

pub mod binding;
pub mod program;
pub mod resolve;
mod rules;

pub use binding::{BindingIndex, RenameSafety, SymbolKind, SymbolRef, build_binding_index};
pub use program::{
    CheckedConst, CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, MarrowType,
};
pub use resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};

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
/// Under strict typing, dynamic data must be converted before typed use.
pub const CHECK_UNTYPED_VALUE: &str = "check.untyped_value";
/// A saved key or identity argument's type does not match the key it addresses: a
/// scalar of the wrong type in a keyed lookup, or an identity of a foreign
/// resource spliced into a keyspace. Saved keys are nominally typed, so a
/// key-compatible foreign identity is still rejected. The static counterpart of a
/// key-type fault at lowering.
pub const CHECK_KEY_TYPE: &str = "check.key_type";
/// A bare name used as a value does not resolve to any binding in scope (a
/// parameter, local, loop or catch binding, or module constant). Under strict
/// typing every value name must be defined.
pub const CHECK_UNRESOLVED_NAME: &str = "check.unresolved_name";
/// A call names a function that is neither a builtin nor a declared function. Only
/// reported for calls in library modules of a fully parsed project, so a
/// module-less script or a module excluded by a parse error never false-positives.
pub const CHECK_UNRESOLVED_CALL: &str = "check.unresolved_call";
/// A qualified call (`module::fn`) names a function that exists but is not `pub`,
/// so it is not callable from another module. Distinct from
/// [`CHECK_UNRESOLVED_CALL`]: the name resolves, the visibility does not.
pub const CHECK_PRIVATE_FUNCTION: &str = "check.private_function";
/// A bare call names a `pub` function reachable in two or more modules, so the
/// bare name cannot pick one — it must be qualified (`module::fn`). Distinct from
/// [`CHECK_UNRESOLVED_CALL`]: candidates exist, the bare spelling is ambiguous.
pub const CHECK_AMBIGUOUS_CALL: &str = "check.ambiguous_call";
/// `nextId(^root)` names a root with no default integer allocation policy: a
/// composite identity, a single non-integer identity key, or a keyless singleton.
/// The default per-root policy is only available for a resource with one `int`
/// identity key. The runtime backstops
/// this with `write.next_id_unsupported`; the checker catches it before a run.
pub const CHECK_NEXT_ID_REQUIRES_SINGLE_INT: &str = "check.next_id_requires_single_int";
/// `next`/`prev` is applied to a shape it cannot navigate: a composite
/// multi-key identity record (its identity spans several key levels, not the one
/// `next`/`prev` step over) or an index branch (it inspects identities, with no
/// single key position to seek). The runtime would reject these with an
/// uncatchable `run.unsupported` fault; the checker catches it before a run.
pub const CHECK_NEIGHBOR_UNSUPPORTED: &str = "check.neighbor_unsupported";
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
    pub span: SourceSpan,
}

impl marrow_syntax::Diagnose for CheckDiagnostic {
    fn code(&self) -> &str {
        self.code
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn severity(&self) -> Severity {
        self.severity
    }
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

/// An overlay of in-memory source text keyed by file path. Editor tooling fills
/// it with unsaved buffer contents so analysis sees what the user is typing; a
/// path with no overlay entry is read from disk as usual. An empty overlay means
/// "read everything from disk", which is exactly [`check_project`]'s behavior.
#[derive(Debug, Clone, Default)]
pub struct ProjectSources {
    overlays: HashMap<PathBuf, String>,
}

impl ProjectSources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Overlay `source` for `path`, replacing any existing entry, and return the
    /// overlay so calls can be chained when building one up.
    pub fn with(mut self, path: impl Into<PathBuf>, source: impl Into<String>) -> Self {
        self.insert(path, source);
        self
    }

    /// Overlay `source` for `path`, replacing any existing entry.
    pub fn insert(&mut self, path: impl Into<PathBuf>, source: impl Into<String>) {
        self.overlays.insert(path.into(), source.into());
    }

    /// The overlaid text for `path`, or `None` when the path should be read from
    /// disk.
    pub fn get(&self, path: &Path) -> Option<&str> {
        self.overlays.get(path).map(String::as_str)
    }
}

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
    pub parsed: marrow_syntax::ParsedSource,
}

/// Discover, read, and parse every `.mw` file in the project, collecting parse
/// diagnostics and module/path resolution problems. Fails only when a source
/// root cannot be walked; per-file read errors become diagnostics.
pub fn check_project(
    project_root: &Path,
    config: &ProjectConfig,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    check_project_with_sources(project_root, config, &ProjectSources::new())
}

/// Like [`check_project`], but analyzing `sources` over disk: any path with an
/// overlay entry is checked from that text instead of being re-read. An empty
/// overlay is identical to [`check_project`].
pub fn check_project_with_sources(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    analyze_project(project_root, config, sources)
        .map(|snapshot| (snapshot.report, snapshot.program))
}

/// The IDE-grade analysis core shared by [`check_project`]: discover, read (from
/// `sources` overlay or disk), parse, and check every `.mw` file, returning the
/// diagnostics and best-effort program plus every parsed file (error files
/// included). Fails only when a source root cannot be walked.
pub fn analyze_project(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
) -> Result<AnalysisSnapshot, DiscoverError> {
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
        // Editor buffer text wins over disk for an overlaid path; a path with no
        // overlay is read from disk, and a read failure drops the file (its
        // `io.read` diagnostic is recorded) exactly as before.
        let Some(source) = read_source(&file.path, sources, &mut report.diagnostics) else {
            continue;
        };
        let CheckedFile {
            parsed,
            resources,
            functions,
            constants,
        } = check_file_source(&file.path, &source, &mut report.diagnostics);

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
                        span: resource.span,
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
                        span,
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

    // Passes 2-3 plus unresolved-call suppression are shared with check_tests.
    check_resolved_files(
        files.len(),
        &parsed_files,
        &declared,
        &program,
        &project_resources,
        &mut report,
    );

    // Move every parse — error files included — into the snapshot now that the
    // shared tail has finished borrowing them. The path and module name are
    // copied from each `ModuleFile`; the parse itself moves.
    let analyzed = parsed_files
        .into_iter()
        .map(|(file, parsed)| AnalyzedFile {
            path: file.path.clone(),
            module_name: file.module_name.clone(),
            parsed,
        })
        .collect();

    Ok(AnalysisSnapshot {
        report,
        program,
        files: analyzed,
    })
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
    let mut scope = function_base_scope(program, function, &prelude.module_constants);
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
fn enclosing_function(
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
fn function_base_scope(
    program: &CheckedProgram,
    function: &marrow_syntax::FunctionDecl,
    module_constants: &HashMap<String, MarrowType>,
) -> Vec<HashMap<String, MarrowType>> {
    let mut base = module_constants.clone();
    for param in &function.params {
        base.insert(param.name.clone(), resolve_type(&param.ty, program));
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
fn walk_block_to_offset(
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
fn descend_target<'b>(
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
fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

/// The smallest expression in a function `body` whose span covers `offset`, the
/// expression the cursor sits on. Walks every expression the type pass would
/// visit, keeping the tightest span. Statement-level structure (conditions,
/// initializers, call arguments, nested blocks) is traversed so the cursor lands
/// on the leaf expression rather than an enclosing one.
fn smallest_expression_at(
    body: &marrow_syntax::Block,
    offset: usize,
) -> Option<&marrow_syntax::Expression> {
    let mut best: Option<&marrow_syntax::Expression> = None;
    collect_block_expression(body, offset, &mut best);
    best
}

fn collect_block_expression<'b>(
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
            Statement::Break { .. } | Statement::Continue { .. } => {}
        }
    }
}

/// Keep `expr` as the best match when its span covers `offset` and is no wider
/// than the current best, then recurse into its subexpressions so the tightest
/// covering leaf wins.
fn collect_expression<'e>(
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
fn check_resolved_files(
    files_len: usize,
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    resolvable: &HashMap<String, PathBuf>,
    program: &CheckedProgram,
    project_resources: &HashSet<String>,
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
struct FilePrelude {
    aliases: HashMap<String, Vec<String>>,
    module_constants: HashMap<String, MarrowType>,
}

/// Build a file's [`FilePrelude`]: the alias map from its imports and the typed
/// module constants, in source order so an earlier constant is in scope for a
/// later one. The type-check pass and the editor queries both start from this,
/// so the bindings a function body sees are derived in exactly one place.
fn file_prelude(
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
                (Some(ty), _) => resolve_type(ty, program),
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
/// [`check_tests`]) share this pass. `project_resources` is the project-wide set
/// of resource names used to recognize type annotations.
fn check_file_types(
    program: &CheckedProgram,
    project_resources: &HashSet<String>,
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
                        diagnostics,
                    );
                }
                if let Some(return_type) = &function.return_type {
                    check_type_annotation(
                        return_type,
                        function.span,
                        file,
                        project_resources,
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
                    check_type_annotation(ty, constant.span, file, project_resources, diagnostics);
                }
            }
            marrow_syntax::Declaration::Resource(_) => {}
        }
    }
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
    if !MarrowType::names_known_type(ty, resources) {
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
            _ => {}
        }
    }
}

/// Whether `block` definitely returns (or otherwise diverges) on every path —
/// a sound under-approximation of "every reachable path returns". It is
/// conservative: a function ending in a call or a loop may diverge, so it is not
/// flagged; only a clearly falling-through end is. This favors no false positives
/// over catching every genuine case.
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
/// an unresolved call — is never a false positive. The operator rules are:
/// matching numeric operands for `+ - * /`, `int` for
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
            ty, value, span, ..
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
                    &resolve_type(ty, program),
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
        Statement::Break { .. } | Statement::Continue { .. } => {}
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

/// The `(name, type)` a `const`/`var` statement introduces into its block,
/// computed exactly as [`check_statement_types`] computes it: the annotation when
/// written, otherwise the inferred initializer type, resolved against `scope`. Any
/// other statement introduces no block-frame binding and returns `None`. The
/// checker and the editor scope reconstruction share this so a binding's type is
/// derived in one place. Initializer diagnostics belong to the type-check pass, so
/// inference here discards them.
fn local_binding(
    program: &CheckedProgram,
    statement: &marrow_syntax::Statement,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<(String, MarrowType)> {
    use marrow_syntax::Statement;
    let mut sink = Vec::new();
    let (name, annotation, value_type) = match statement {
        Statement::Const {
            name, ty, value, ..
        } => (
            name,
            ty,
            infer_type(program, value, scope, aliases, file, &mut sink),
        ),
        Statement::Var {
            name, ty, value, ..
        } => {
            let value_type = match value {
                Some(value) => infer_type(program, value, scope, aliases, file, &mut sink),
                None => MarrowType::Unknown,
            };
            (name, ty, value_type)
        }
        _ => return None,
    };
    Some((
        name.clone(),
        binding_type(annotation.as_ref(), value_type, program),
    ))
}

/// The scope frame a `for` loop's body runs under, mirroring
/// [`check_statement_types`]: the loop binding(s) in scope for the body. Iterating
/// a sequence binds its single binding to the element type; other iterables
/// (ranges, index keys) and the key/value form stay unknown. The checker and the
/// editor scope reconstruction share this so a loop binding's type is derived in
/// one place. Inference here discards diagnostics; the type pass emits the
/// iterable's separately.
fn for_frame(
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
fn check_return_type(
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
fn check_assignment(
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
            MarrowType::Primitive(ScalarType::Str)
        }
        Expression::Name { segments, span } if segments.len() == 1 => {
            let name = &segments[0];
            lookup_opt(scope, name).unwrap_or_else(|| {
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNRESOLVED_NAME,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
                    message: format!("`{name}` is not defined"),
                    span: *span,
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
            let left_type = infer_type(program, left, scope, aliases, file, diagnostics);
            let right_type = infer_type(program, right, scope, aliases, file, diagnostics);
            // `??` only defaults an absent path read, so its left operand must be a
            // path read or `?.` chain — a present non-path value is never absent
            // and has nothing to default. The result is the leaf type of that read.
            if matches!(op, marrow_syntax::BinaryOp::Coalesce) {
                return check_coalesce(left, &left_type, &right_type, *span, file, diagnostics);
            }
            check_binary(*op, &left_type, &right_type, *span, file, diagnostics)
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
            // A saved access `^root(key…)` or `^root(key…).layer(key…)` carries key
            // arguments the function-call path does not type. Check them against the
            // root's identity keys or the layer's key parameters here, where the
            // saved-path helpers live.
            check_saved_key_args(program, callee, &arg_types, *span, file, diagnostics);
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
        // A plain field read and an optional (`?.`) field read resolve to the same
        // declared leaf type: the short-circuit only changes the read's runtime
        // behavior on absence, not the type of a populated leaf.
        Expression::Field { base, name, .. } | Expression::OptionalField { base, name, .. } => {
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
        // A multi-segment name or saved root has no known primitive type.
        Expression::Name { .. } | Expression::SavedRoot { .. } => MarrowType::Unknown,
    }
}

/// The declared type of a top-level saved field read: `base` is either a keyed
/// record access `^root(key…)` (a call whose callee is the saved root) or — for a
/// keyless singleton resource (`Settings at ^settings`) addressed by its root —
/// the saved root `^root` itself. Group-layer fields and keyed-leaf reads are not
/// resolved here.
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
    field_member_type(resource, &[field])
}

/// The schema of the resource that owns saved root `^root`, if any. Saved roots
/// are project-wide (a `^books` write addresses the one `books` resource from any
/// module), so this is a thin shim over the resolver's project-wide root lookup;
/// the runtime's `find_resource` routes through the same helper.
pub(crate) fn find_resource_schema<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> Option<&'p marrow_schema::ResourceSchema> {
    resolve::resolve_resource_by_root(program, root)
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

/// Type-check the key arguments of a saved access against the keys it addresses.
/// A record lookup `^root(key…)` is checked against the root's identity keys; a
/// keyed-layer access `^root(key…).layer(key…)` against that layer's key
/// parameters. A foreign identity spliced into a keyspace, or a scalar of the
/// wrong type, is a `check.key_type`. Non-saved callees (a function call, an index
/// lookup) and unresolved roots are left alone.
fn check_saved_key_args(
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
fn check_keys_against(
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
        let expected = MarrowType::from_resolved(key.ty.clone(), &[]);
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
fn key_type_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_KEY_TYPE,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        span,
    }
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
/// looked up in that resource's schema.
fn local_field_type(
    program: &CheckedProgram,
    base_type: &MarrowType,
    field: &str,
) -> Option<MarrowType> {
    let MarrowType::Resource(name) = base_type else {
        return None;
    };
    let resource = resolve::resolve_resource_by_name_any(program, name)?;
    field_member_type(resource, &[field])
}

/// The declared type of a group field read at any nesting depth, reached through
/// keyed layers (`^root(key…).layer(key…)….field`) or unkeyed groups
/// (`^root(key…).name.field`). `base` is the group entry — the part before the
/// leaf field.
fn saved_group_field_type(
    program: &CheckedProgram,
    base: &marrow_syntax::Expression,
    field: &str,
) -> Option<MarrowType> {
    let (root, mut chain) = saved_group_chain(base)?;
    let resource = find_resource_schema(program, root)?;
    chain.push(field);
    field_member_type(resource, &chain)
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
/// nesting depth. `callee` is the layer field `^root(key…)….layer`.
fn saved_leaf_type(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let (root, layers) = saved_layer_chain(callee)?;
    let resource = find_resource_schema(program, root)?;
    leaf_member_type(resource, &layers)
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

/// The checker type of a stored field read named by its saved-path chain — the
/// named segments after the identity, outermost first, terminating in a scalar
/// field. Resolves through the shared schema walk and lifts the result to the
/// checker's lattice. A saved leaf is always a scalar, sequence, or identity, so
/// it needs no module resource names to place a bare `Named`.
fn field_member_type(
    resource: &marrow_schema::ResourceSchema,
    chain: &[&str],
) -> Option<MarrowType> {
    resource
        .field_type(chain)
        .map(|ty| MarrowType::from_resolved(ty.clone(), &[]))
}

/// The checker type of a keyed-leaf layer read named by its chain of layer names,
/// outermost first. Resolves through the same shared schema walk as
/// [`field_member_type`], differing only in that the terminal name is a keyed-leaf
/// layer rather than a field.
fn leaf_member_type(
    resource: &marrow_schema::ResourceSchema,
    layers: &[&str],
) -> Option<MarrowType> {
    resource
        .leaf_type(layers)
        .map(|ty| MarrowType::from_resolved(ty.clone(), &[]))
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
            span,
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
        LiteralKind::Integer => ScalarType::Int,
        LiteralKind::Decimal => ScalarType::Decimal,
        LiteralKind::String => ScalarType::Str,
        LiteralKind::Bytes => ScalarType::Bytes,
        LiteralKind::Bool => ScalarType::Bool,
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
fn check_binary(
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
    // sequences have no equality; identities compare nominally, so same-resource
    // identities are equatable (`bool`) while a cross-resource pair, or an identity
    // against a scalar, is a category error. An `Unknown` operand defers to the
    // scalar path, where untyped values are handled.
    if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual)
        && let Some(result) = check_equality(op, left, right, span, file, diagnostics)
    {
        return result;
    }
    // No non-equality operator applies to a concrete non-scalar operand — an
    // identity, whole record, or sequence. Flag it as an operator misuse rather than
    // letting the scalar gate below drop it to `Unknown`. An `Unknown` operand still
    // defers there, where untyped values are handled.
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
/// and sequences are not equatable; identities compare nominally, so a
/// same-resource pair is `bool` and any other identity pairing is a category error.
/// An `Unknown` operand defers (the untyped-value path owns it); a scalar pair
/// defers to the ordinary scalar-equality check. A diagnostic is pushed on the
/// rejected cases, which still yield `bool` so a surrounding expression sees the
/// natural result type of a comparison.
fn check_equality(
    op: marrow_syntax::BinaryOp,
    left: &MarrowType,
    right: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<MarrowType> {
    let reject = |diagnostics: &mut Vec<CheckDiagnostic>| {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot compare `{}` and `{}`",
                binary_symbol(op),
                marrow_type_name(left),
                marrow_type_name(right),
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
        // An identity against a scalar or `Error` is a category error.
        (MarrowType::Identity(_), _) | (_, MarrowType::Identity(_)) => reject(diagnostics),
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
fn check_coalesce(
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
fn is_path_read(expr: &marrow_syntax::Expression) -> bool {
    use marrow_syntax::Expression;
    matches!(
        expr,
        Expression::Field { .. } | Expression::OptionalField { .. } | Expression::Call { .. }
    )
}

/// The scalar a type denotes, or `None` for any non-scalar (resource, identity,
/// sequence, the checker-only `Error`, or unknown) type. `Error` is concrete, not
/// untyped: each scalar-requiring site (operator, condition, return, assignment,
/// argument) handles a `None` from `Error` as a real mismatch, distinct from the
/// untyped-value path taken for `Unknown`.
fn as_primitive(ty: &MarrowType) -> Option<ScalarType> {
    match ty {
        MarrowType::Primitive(scalar) => Some(*scalar),
        _ => None,
    }
}

/// Whether a type is a concrete non-scalar value type: an identity, a whole
/// record, or a sequence. These compare nominally, so an operator that defaults or
/// equates them resolves by [`type_compatible`] rather than by scalar shape. The
/// checker-only `Error` and the untyped `Unknown` are excluded: `Error` has its own
/// operator handling and `Unknown` defers.
fn is_concrete_nonscalar(ty: &MarrowType) -> bool {
    matches!(
        ty,
        MarrowType::Identity(_) | MarrowType::Resource(_) | MarrowType::Sequence(_)
    )
}

/// Whether a value of type `actual` may stand where `expected` is required.
/// `Some(true)`/`Some(false)` is a verdict; `None` defers — the value's type is
/// `unknown` (the untyped-value path owns that case) or `expected` itself places
/// no constraint. Identities and resources compare nominally: same resource name
/// or nothing, so a key-compatible foreign identity is still a mismatch. A
/// cross-module identity the checker could not place is `Unknown` and defers,
/// permissive until the type IR is unified across modules.
fn type_compatible(expected: &MarrowType, actual: &MarrowType) -> Option<bool> {
    if matches!(actual, MarrowType::Unknown) {
        return None;
    }
    match expected {
        MarrowType::Primitive(p) => Some(matches!(actual, MarrowType::Primitive(q) if q == p)),
        MarrowType::Identity(resource) => {
            Some(matches!(actual, MarrowType::Identity(other) if other == resource))
        }
        MarrowType::Resource(resource) => {
            Some(matches!(actual, MarrowType::Resource(other) if other == resource))
        }
        MarrowType::Sequence(element) => match actual {
            MarrowType::Sequence(other) => type_compatible(element, other),
            _ => Some(false),
        },
        MarrowType::Error => Some(matches!(actual, MarrowType::Error)),
        MarrowType::Unknown => None,
    }
}

/// Whether an expected type has a value-conversion boundary, so an `unknown` value
/// against it is a `check.untyped_value` ("convert it first") rather than a
/// deferral. Scalars and the checker-only `Error` are reached by the conversion
/// builtins (`int(...)`, `ErrorCode(...)`, the `Error(...)` constructor); a
/// resource, identity, or sequence has no such conversion, so an `unknown` value
/// there is left to the runtime.
fn expects_conversion(ty: &MarrowType) -> bool {
    matches!(ty, MarrowType::Primitive(_) | MarrowType::Error)
}

fn is_numeric(scalar: ScalarType) -> bool {
    matches!(scalar, ScalarType::Int | ScalarType::Decimal)
}

fn is_ordered(scalar: ScalarType) -> bool {
    matches!(
        scalar,
        ScalarType::Int
            | ScalarType::Decimal
            | ScalarType::Str
            | ScalarType::Bytes
            | ScalarType::Date
            | ScalarType::Instant
            | ScalarType::Duration
    )
}

fn operator_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_OPERATOR_TYPE,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        span,
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
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::Coalesce => "??",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
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
    // qualifier and the target must be `pub`).
    let from_module = module_of_file(program, file);
    // A callee naming a declared resource is a constructor, not a function:
    // `Book(...)` builds the resource value and `Book::Id(...)` builds its
    // identity. Recognize it so a valid
    // constructor is not a false `check.unresolved_call`.
    if let Some(ty) = resource_constructor_type(program, from_module, segments) {
        return ty;
    }
    // Resolving as a `Function` can only ever Find a function, so the other arms
    // never carry a non-function item. Only library-module calls are reported: a
    // module-less script is not a call target, and a project that did not fully
    // parse has its unresolved calls suppressed in `check_project` (the missing
    // definition may live in an excluded module).
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
        // Every concrete parameter type — scalar, identity, resource, sequence, or
        // the checker-only `Error` — is checked nominally; an `unknown` parameter
        // places no constraint and is left to the runtime.
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
/// slot except `std::log::error`, which expects the checker-only `Error` value.
fn check_one_arg(
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
        // identity, a resource, a sequence, or an `Error` — is a real argument
        // mismatch.
        Some(false) => diagnostics.push(call_diagnostic(
            file,
            span,
            format!(
                "argument to `{label}` expects `{}`, but found `{}`",
                marrow_type_name(parameter),
                marrow_type_name(arg_type),
            ),
        )),
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

/// The source spelling of a type for a diagnostic message: a scalar by name, an
/// identity as `Resource::Id`, a resource by its name, a sequence as
/// `sequence[element]`, the checker-only `Error`, or `value` for a type with no
/// surface spelling.
fn marrow_type_name(ty: &MarrowType) -> String {
    match ty {
        MarrowType::Primitive(scalar) => scalar.name().to_string(),
        MarrowType::Error => "Error".to_string(),
        MarrowType::Identity(resource) => format!("{resource}::Id"),
        MarrowType::Resource(resource) => resource.clone(),
        MarrowType::Sequence(element) => format!("sequence[{}]", marrow_type_name(element)),
        MarrowType::Unknown => "value".to_string(),
    }
}

/// Check positional `args` against a fixed positional parameter list (the std
/// helper signatures): an arity mismatch is a `check.call_argument`, and each
/// argument with a known-required parameter type is checked by [`check_one_arg`].
/// A `None` parameter slot (e.g. a path argument) is left alone. Std helpers are
/// positional-only — named-argument matching stays user-function-only.
fn check_args_against(
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
fn check_neighbor(
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
fn composite_identity(program: &CheckedProgram, root: &str) -> bool {
    find_resource_schema(program, root)
        .and_then(|resource| resource.saved_root.as_ref())
        .is_some_and(|saved_root| saved_root.identity_keys.len() > 1)
}

/// Report a `check.neighbor_unsupported` error for a statically-unnavigable
/// `next`/`prev` shape and leave the result `Unknown`.
fn neighbor_unsupported(
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

/// The `Resource::Id` identity type of the resource at saved root `root`, or
/// `Unknown` when `root` names no keyed saved root.
fn record_identity_type(program: &CheckedProgram, root: &str) -> MarrowType {
    match find_resource_schema(program, root) {
        Some(resource) if resource.saved_root.is_some() => {
            MarrowType::Identity(resource.name.clone())
        }
        _ => MarrowType::Unknown,
    }
}

/// The single key type of the child layer a `^root(id…).layer` accessor names, or
/// `Unknown` when the layer is undeclared or not single-keyed. The neighbor of a
/// layer position is one of these keys, so `next`/`prev` over the layer type to it.
fn layer_key_type(program: &CheckedProgram, layer_field: &marrow_syntax::Expression) -> MarrowType {
    let Some((root, layers)) = saved_layer_chain(layer_field) else {
        return MarrowType::Unknown;
    };
    let Some(resource) = find_resource_schema(program, root) else {
        return MarrowType::Unknown;
    };
    match resource
        .descend_layers(&layers)
        .map(|node| node.key_params.as_slice())
    {
        Some([key]) => MarrowType::from_resolved(key.ty.clone(), &[]),
        _ => MarrowType::Unknown,
    }
}

/// Report a `check.call_argument` arity diagnostic when a fixed-arity builtin is
/// called with the wrong number of arguments.
fn check_arity(
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
fn call_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_CALL_ARGUMENT,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        span,
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

/// The name of the module `file` contributes, for resolving the calls inside it
/// from the right module. Empty for a file with no module in the program (a
/// module-less script), whose calls are suppressed anyway.
fn module_of_file<'p>(program: &'p CheckedProgram, file: &Path) -> &'p str {
    program
        .modules
        .iter()
        .find(|module| module.source_file == file)
        .map_or("", |module| module.name.as_str())
}

/// Resolve a call's `segments` to a function, also yielding the [`CheckedModule`]
/// that owns it so the binding index can locate the definition's source file. A
/// thin shim over the unified [`resolve`]: a bare name resolves in `from_module`,
/// a qualified name in the named module — so a bare cross-module call no longer
/// first-matches a foreign function. Used by the LSP binding index, which carries
/// the referencing module.
pub(crate) fn resolve_function_in_module<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    segments: &[String],
) -> Option<(&'p CheckedModule, &'p CheckedFunction)> {
    match resolve(program, from_module, segments, ResolvableKind::Function) {
        Resolution::Found(Def {
            module,
            item: DefItem::Function(function),
            ..
        }) => Some((module, function)),
        _ => None,
    }
}

/// The type produced by a resource constructor callee, if `segments` name a
/// resource visible to `from_module`: `Book(...)` constructs the resource value
/// (its [`MarrowType::Resource`]), and `Book::Id(...)` constructs its identity
/// ([`MarrowType::Identity`]). The resource *name* is module-scoped (own module
/// first), so two modules can each declare a `Book`. Any other callee returns
/// `None`, so a genuinely unresolved call is still reported.
fn resource_constructor_type(
    program: &CheckedProgram,
    from_module: &str,
    segments: &[String],
) -> Option<MarrowType> {
    match segments {
        [name] => match resolve(program, from_module, segments, ResolvableKind::Resource) {
            Resolution::Found(_) => Some(MarrowType::Resource(name.clone())),
            _ => None,
        },
        [name, id] if id == "Id" => {
            match resolve(
                program,
                from_module,
                segments,
                ResolvableKind::ResourceIdentity,
            ) {
                Resolution::Found(_) => Some(MarrowType::Identity(name.clone())),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether a name callee is a builtin, std helper, or the `Error` constructor —
/// each dispatched before user functions at runtime, so never a program function.
fn is_builtin_call(segments: &[String]) -> bool {
    match segments {
        // The single-name builtins, grouped by purpose. Each
        // dispatches before user-function resolution at runtime, so none is ever a
        // declared program function.
        [name] => {
            matches!(
                name.as_str(),
                // presence and reads
                "exists"
                // tree traversal
                | "keys" | "values" | "entries" | "count"
                // ordered navigation
                | "reversed" | "next" | "prev"
                // sequence updates and id allocation
                | "append" | "nextId"
                // write and print
                | "write" | "print"
                // error constructor
                | "Error" // conversions: the nine storable scalars, by canonical name.
            ) || ScalarType::from_scalar_name(name).is_some()
        }
        // A `std::module::op` builtin must name a real std module, mirroring
        // import resolution (`is_std_module`/`std_modules`); an unknown submodule
        // is not a builtin, so it is reported like a rejected `use std::bogus`.
        [first, module, _] => first == "std" && std_modules().contains(&module.as_str()),
        _ => false,
    }
}

/// The return type of a single-name data builtin: `exists(path): bool` and
/// `append(layer, value): int`. `nextId` is handled in [`check_next_id`], which
/// has the `^root` argument it needs to type the identity. The absence-default
/// `??` is an operator, not a builtin, and is typed in [`check_coalesce`].
fn builtin_return_type(segments: &[String], _arg_types: &[MarrowType]) -> Option<MarrowType> {
    let [name] = segments else {
        return None;
    };
    match name.as_str() {
        "exists" => Some(MarrowType::Primitive(ScalarType::Bool)),
        "append" => Some(MarrowType::Primitive(ScalarType::Int)),
        _ => None,
    }
}

/// The return type of a scalar conversion builtin (`int(x): int`, `string(x):
/// string`, …). The conversion builtins are exactly the nine storable scalars;
/// each validates a dynamically-typed value and yields its named type.
fn conversion_return_type(segments: &[String]) -> Option<MarrowType> {
    let [name] = segments else {
        return None;
    };
    ScalarType::from_scalar_name(name).map(MarrowType::Primitive)
}

/// The descriptor for a `std::module::op` helper, looked up in the shared table.
fn std_op(segments: &[String]) -> Option<&'static stdlib::StdOp> {
    let [first, module, op] = segments else {
        return None;
    };
    (first == "std")
        .then(|| stdlib::lookup(module, op))
        .flatten()
}

/// The declared return type of a value-returning `std::module::op` helper, derived
/// from its descriptor. Void helpers (`std::log`, `std::assert`,
/// `std::io::write*`) and single-name builtins return `None`, leaving the call
/// `Unknown` for the surrounding checks.
fn std_call_return_type(segments: &[String]) -> Option<MarrowType> {
    match std_op(segments)?.ret {
        ReturnType::Scalar(scalar) => Some(MarrowType::Primitive(scalar)),
        ReturnType::Sequence(element) => Some(MarrowType::Sequence(Box::new(
            MarrowType::Primitive(element),
        ))),
        ReturnType::Void => None,
    }
}

/// The positional parameter types of a `std::module::op` helper, in order, derived
/// from its descriptor. `None` for an unknown op, leaving its arguments to the
/// runtime; a `None` slot inside the list marks a non-checked path argument
/// (`assert::absent`).
fn std_call_params(segments: &[String]) -> Option<Vec<Option<MarrowType>>> {
    let op = std_op(segments)?;
    Some(
        op.params
            .iter()
            .map(|param| match param {
                ParamType::Scalar(scalar) => Some(MarrowType::Primitive(*scalar)),
                ParamType::Error => Some(MarrowType::Error),
                ParamType::Path => None,
            })
            .collect(),
    )
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

    // Run the same type-inference pass library files get, so a test file's std
    // argument/arity errors, `nextId` misuse, and ordinary type mismatches are
    // reported at check time rather than only at run time. Types resolve against a
    // program holding both the already-checked project modules and the clean test
    // modules — cross-module calls and resource constructors resolve, and the
    // `nextId` gate finds the project's resource schemas. Resource annotations are
    // recognized project-wide (project plus test resources).
    let combined = CheckedProgram {
        modules: project
            .modules
            .iter()
            .cloned()
            .chain(modules.iter().cloned())
            .collect(),
    };
    let project_resources: HashSet<String> = combined
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .map(|resource| resource.name.clone())
        .collect();

    // Passes 2-3 plus unresolved-call suppression are shared with check_project.
    // A read failure drops a file from `parsed_files` so a call into it would look
    // unresolved; the suppression there handles that exactly as check_project does.
    check_resolved_files(
        files.len(),
        &parsed_files,
        &resolvable,
        &combined,
        &project_resources,
        &mut report,
    );

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

/// Read one source file from disk, then parse and structurally check it. Returns
/// `None` if the file cannot be read (an `io.read` diagnostic is recorded). This
/// is the disk path used by [`check_tests`]; [`analyze_project`] reads through an
/// overlay instead.
fn check_file(file_path: &Path, diagnostics: &mut Vec<CheckDiagnostic>) -> Option<CheckedFile> {
    let source = read_source(file_path, &ProjectSources::new(), diagnostics)?;
    Some(check_file_source(file_path, &source, diagnostics))
}

/// Resolve a file's text: the overlay entry for `file_path` if `sources` carries
/// one, otherwise the on-disk contents. A read failure records an `io.read`
/// diagnostic and returns `None`, preserving the read-failure-drops-a-file
/// invariant the project and test checkers rely on.
fn read_source(
    file_path: &Path,
    sources: &ProjectSources,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<String> {
    if let Some(overlay) = sources.get(file_path) {
        return Some(overlay.to_string());
    }
    match std::fs::read_to_string(file_path) {
        Ok(source) => Some(source),
        Err(error) => {
            diagnostics.push(CheckDiagnostic {
                code: IO_READ,
                severity: Severity::Error,
                file: file_path.to_path_buf(),
                message: format!("failed to read source: {error}"),
                span: SourceSpan::default(),
            });
            None
        }
    }
}

/// Parse and structurally check one source file's `source`: record its parse,
/// duplicate-name, function-body, const-value, and resource-schema diagnostics,
/// and collect the declaration lists for a checked module. Always returns a
/// [`CheckedFile`] — a parse error yields one whose `parsed` carries the
/// diagnostics. Cross-file checks — module-path matching, saved-root ownership,
/// stable-id uniqueness, and import resolution — belong to the caller, since they
/// span files and differ between library modules and test scripts.
fn check_file_source(
    file_path: &Path,
    source: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> CheckedFile {
    let parsed = parse_source(source);
    for diagnostic in &parsed.diagnostics {
        diagnostics.push(CheckDiagnostic {
            code: diagnostic.code,
            severity: diagnostic.severity,
            file: file_path.to_path_buf(),
            message: diagnostic.message.clone(),
            span: diagnostic.span,
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
                        span: error.span,
                    });
                }
                resources.push(schema);
            }
            marrow_syntax::Declaration::Const(constant) => {
                if let Some(value) = &constant.value {
                    rules::check_const_value(file_path, value, diagnostics);
                }
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

    CheckedFile {
        parsed,
        resources,
        functions,
        constants,
    }
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
            condition.as_ref().is_some_and(expr_touches_saved_data)
                || block_touches_saved_data(then_block)
                || else_ifs.iter().any(|else_if| {
                    else_if
                        .condition
                        .as_ref()
                        .is_some_and(expr_touches_saved_data)
                        || block_touches_saved_data(&else_if.block)
                })
                || else_block.as_ref().is_some_and(block_touches_saved_data)
        }
        Statement::While {
            condition, body, ..
        } => {
            condition.as_ref().is_some_and(expr_touches_saved_data)
                || block_touches_saved_data(body)
        }
        Statement::For { iterable, body, .. } => {
            expr_touches_saved_data(iterable) || block_touches_saved_data(body)
        }
        Statement::Transaction { body, .. } => block_touches_saved_data(body),
        Statement::Lock { path, body, .. } => {
            path.as_ref().is_some_and(expr_touches_saved_data) || block_touches_saved_data(body)
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
        Statement::Break { .. } | Statement::Continue { .. } => false,
    }
}

fn expr_touches_saved_data(expr: &marrow_syntax::Expression) -> bool {
    use marrow_syntax::{Expression, InterpolationPart};
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Literal { .. } | Expression::Name { .. } => false,
        Expression::Call { callee, args, .. } => {
            expr_touches_saved_data(callee)
                || args.iter().any(|arg| expr_touches_saved_data(&arg.value))
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            expr_touches_saved_data(base)
        }
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

/// The standard-library module names, derived once from the shared stdlib table
/// ([`marrow_schema::stdlib::all`]) so import resolution and the op table share a
/// single source of truth — a new `std::<module>::op` row adds its module here
/// automatically, with no hardcoded list to drift. Host modules resolve at check
/// time even when a host would not provide them at run time.
fn std_modules() -> &'static std::collections::HashSet<&'static str> {
    static STD_MODULES: std::sync::OnceLock<std::collections::HashSet<&'static str>> =
        std::sync::OnceLock::new();
    STD_MODULES.get_or_init(|| stdlib::all().iter().map(|op| op.module).collect())
}

/// Is `name` a standard-library module path? Accepts `std::<module>` and any
/// deeper path under a valid `std` module.
fn is_std_module(name: &str) -> bool {
    name.strip_prefix("std::")
        .and_then(|rest| rest.split("::").next())
        .is_some_and(|module| std_modules().contains(module))
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
        span: module.span,
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
                span: *span,
            }),
            None => {
                first_seen.insert(name, *span);
            }
        }
    }
}
