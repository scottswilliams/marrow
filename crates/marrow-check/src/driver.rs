//! The project- and test-check driver: discover, read, parse, and check a
//! project's `.mw` files into a [`CheckedProgram`] and a [`CheckReport`]. Holds
//! the source overlay used by editor tooling, the per-file structural check, the
//! name/path resolution helpers shared with the type passes, builtin and stdlib
//! classification, and the duplicate-declaration rule.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, ProjectConfig, discover_test_modules};
use marrow_schema::ReturnPresence;
use marrow_schema::stdlib::{self, ParamType, ReturnType};
use marrow_syntax::{SourceSpan, parse_source};

use crate::analysis;
use crate::checks;
use crate::diagnostics::{
    CHECK_DUPLICATE_DECLARATION, CHECK_DUPLICATE_MODULE, CHECK_MODULE_PATH, CHECK_UNKNOWN_TYPE,
    CheckDiagnostic, CheckReport, ConversionTarget, DiagnosticPayload, IO_READ,
};
use crate::enums;
use crate::program::{
    CheckedConst, CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, TypeNames,
};
use crate::resolve::{self, Def, DefItem, Resolution, ResolvableKind, resolve};
use crate::rules;
use crate::{MarrowType, ScalarType};

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

    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.overlays.keys().map(PathBuf::as_path)
    }
}

/// Discover, read, and parse every `.mw` file in the project, binding durable
/// identity against no accepted catalog: the first-run shape, where every saved
/// surface proposes a fresh baseline. Callers that hold a committed catalog bind it
/// through [`check_project_with_catalog`]. Fails only when a source root cannot be
/// walked; per-file read errors become diagnostics.
pub fn check_project(
    project_root: &Path,
    config: &ProjectConfig,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    check_project_with_catalog(project_root, config, None)
}

/// Like [`check_project`], but binding durable identity against the caller-supplied
/// `accepted` snapshot. The CLI owns catalog file/store recovery and threads the
/// selected snapshot here, so the checker never reads durable identity from disk.
pub fn check_project_with_catalog(
    project_root: &Path,
    config: &ProjectConfig,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    analysis::analyze_source_project(project_root, config, &ProjectSources::new(), accepted)
        .map(|snapshot| (snapshot.report, snapshot.program))
}

/// The schema of the resource stored at saved root `^root`, if any. Saved roots
/// are project-wide (a `^books` write addresses the one `books` store from any
/// module), so this resolves through the store table and returns only the
/// resource shape for callers that do not need identity keys or indexes.
pub(crate) fn find_resource_schema<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> Option<&'p marrow_schema::ResourceSchema> {
    resolve::resolve_store_by_root(program, root).map(|store| store.resource)
}

pub(crate) fn identity_type_for_store(store: &marrow_schema::StoreSchema) -> MarrowType {
    MarrowType::Identity(store.root.clone())
}

pub(crate) fn resource_type_name(module: &str, resource: &str) -> String {
    if module.is_empty() {
        resource.to_string()
    } else {
        format!("{module}::{resource}")
    }
}

/// Resolve resource references inside a schema type through the module-aware
/// resolver, yielding the canonical checker type.
pub(crate) fn resolve_resource_schema_type(
    program: &CheckedProgram,
    from_module: &str,
    ty: &marrow_schema::Type,
) -> Option<MarrowType> {
    match ty {
        marrow_schema::Type::Sequence(element) => {
            resolve_resource_schema_type(program, from_module, element)
                .map(|element_type| MarrowType::Sequence(Box::new(element_type)))
        }
        marrow_schema::Type::Named(name) => {
            let segments = split_type_path(name);
            match resolve(program, from_module, &segments, ResolvableKind::Resource) {
                Resolution::Found(Def {
                    module,
                    item: DefItem::Resource(resource),
                    ..
                }) => Some(MarrowType::Resource(resource_type_name(
                    &module.name,
                    &resource.name,
                ))),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Resolve a canonical resource type name (`module::resource`, or a bare
/// `resource` for the script module) to its declaration and owning module name.
/// The name was produced by [`resource_type_name`] from an already-resolved
/// resource, so its module prefix names the owning module exactly: resolving from
/// that module reaches the resource through the one in-module, visibility-aware
/// resolver.
pub(crate) fn resolve_resource_type<'p>(
    program: &'p CheckedProgram,
    name: &str,
) -> Option<(&'p marrow_schema::ResourceSchema, &'p str)> {
    let (from_module, segments) = match name.rsplit_once("::") {
        Some((module_name, resource_name)) => (
            module_name,
            vec![module_name.to_string(), resource_name.to_string()],
        ),
        None => ("", vec![name.to_string()]),
    };
    match resolve(program, from_module, &segments, ResolvableKind::Resource) {
        Resolution::Found(Def {
            module,
            item: DefItem::Resource(resource),
            ..
        }) => Some((resource, module.name.as_str())),
        _ => None,
    }
}

pub(crate) fn enum_visibility(file: &marrow_syntax::SourceFile) -> HashMap<String, bool> {
    file.declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Enum(decl) => Some((decl.name.clone(), decl.public)),
            _ => None,
        })
        .collect()
}

/// The qualified name of the program module whose source is `file`, if any. The
/// referencing module for resolving a bare enum name in that file's expressions.
pub(crate) fn module_of_file<'p>(program: &'p CheckedProgram, file: &Path) -> Option<&'p str> {
    program
        .modules
        .iter()
        .find(|module| module.source_file == file)
        .map(|module| module.name.as_str())
}

/// Expand a dotted `module` path's leading segment through the file's import
/// aliases, so an enum spelling qualified by a short alias (`c` under
/// `use a::b::c`) names the imported module (`a::b::c`).
pub(crate) fn expand_module_alias(module: &str, aliases: &HashMap<String, Vec<String>>) -> String {
    expand_leading_alias(&split_type_path(module), aliases).join("::")
}

/// Resolve a call's `segments` to a function, also yielding the [`CheckedModule`]
/// that owns it so the binding index can locate the definition's source file. A
/// small wrapper over the unified [`resolve`]: a bare name resolves in `from_module`,
/// a qualified name in the named module — so a bare cross-module call no longer
/// first-matches a foreign function. Used by the binding index, which carries
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

/// Whether a name callee is a builtin, std helper, or the `Error` constructor —
/// each dispatched before user functions at runtime, so never a program function.
pub(crate) fn is_builtin_call(segments: &[String]) -> bool {
    match segments {
        [name] => is_builtin_name(name),
        [first, module, op] => first == "std" && stdlib::lookup(module, op).is_some(),
        _ => false,
    }
}

/// A `std::module::op` call with a known std module but no declared operation is
/// a stdlib spelling error, not a user-function call and not an open runtime hook.
pub(crate) fn is_unknown_std_operation(segments: &[String]) -> bool {
    let [first, module, op] = segments else {
        return false;
    };
    first == "std"
        && std_modules().contains(&module.as_str())
        && stdlib::lookup(module, op).is_none()
}

fn is_builtin_name(name: &str) -> bool {
    matches!(
        name,
        // presence and reads
        "exists"
        // tree traversal
        | "keys" | "values" | "entries" | "count"
        // ordered navigation
        | "reversed" | "next" | "prev"
        // sequence updates and id allocation
        | "append" | "nextId"
        // output
        | "print"
        // error constructor
        | "Error"
    ) || ConversionTarget::from_name(name).is_some()
}

/// The return type of a single-name data builtin: `exists(path): bool` and
/// `append(layer, value): int`. `nextId` is handled in [`check_next_id`], which
/// has the `^root` argument it needs to type the identity. The absence-default
/// `??` is an operator, not a builtin, and is typed in [`check_coalesce`].
pub(crate) fn builtin_return_type(segments: &[String]) -> Option<MarrowType> {
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
pub(crate) fn conversion_return_type(segments: &[String]) -> Option<MarrowType> {
    let [name] = segments else {
        return None;
    };
    ConversionTarget::from_name(name).map(ConversionTarget::return_type)
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
pub(crate) fn std_call_return_type(segments: &[String]) -> Option<MarrowType> {
    match std_op(segments)?.ret {
        ReturnType::Scalar(scalar) => Some(MarrowType::Primitive(scalar)),
        ReturnType::Sequence(element) => Some(MarrowType::Sequence(Box::new(
            MarrowType::Primitive(element),
        ))),
        ReturnType::Void => None,
    }
}

/// The positional parameter types of a `std::module::op` helper, in order, derived
/// from its descriptor. Known `std` modules reject unknown operations at check time;
/// recognized descriptor rows supply their argument checks here. A `None` slot
/// inside the list marks a non-checked path argument (`assert::absent`).
pub(crate) fn std_call_params(segments: &[String]) -> Option<Vec<Option<MarrowType>>> {
    let op = std_op(segments)?;
    Some(
        op.params
            .iter()
            .map(|param| match param {
                ParamType::Scalar(scalar) => Some(MarrowType::Primitive(*scalar)),
                ParamType::Sequence(element) => Some(MarrowType::Sequence(Box::new(
                    MarrowType::Primitive(*element),
                ))),
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
    project: CheckedProgram,
) -> Result<(CheckReport, Vec<CheckedModule>), DiscoverError> {
    check_tests_with_sources(project_root, config, project, &ProjectSources::new())
}

/// Like [`check_tests`], but uses overlaid source text for selected test files and
/// includes overlaid test files that match the configured `tests` patterns even
/// when they are not on disk yet.
pub fn check_tests_with_sources(
    project_root: &Path,
    config: &ProjectConfig,
    mut project: CheckedProgram,
    sources: &ProjectSources,
) -> Result<(CheckReport, Vec<CheckedModule>), DiscoverError> {
    let checked = check_tests_with_sources_analysis(
        project_root,
        config,
        &mut project,
        sources,
        TestResolutionSuppression::default(),
        TestProgramOutput::FinalizeCombined,
    )?;
    let test_modules = project.modules.split_off(checked.test_module_start);
    Ok((checked.report, test_modules))
}

/// Like [`check_tests`], but returns the full combined [`CheckedProgram`] (project
/// modules plus the clean test modules) instead of only the test modules.
pub fn check_tests_program(
    project_root: &Path,
    config: &ProjectConfig,
    project: CheckedProgram,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    check_tests_with_sources_program(project_root, config, project, &ProjectSources::new())
}

pub fn check_tests_with_sources_program(
    project_root: &Path,
    config: &ProjectConfig,
    mut project: CheckedProgram,
    sources: &ProjectSources,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    let checked = check_tests_with_sources_analysis(
        project_root,
        config,
        &mut project,
        sources,
        TestResolutionSuppression::default(),
        TestProgramOutput::FinalizeCombined,
    )?;
    Ok((checked.report, project))
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TestResolutionSuppression {
    hidden_modules: HashSet<String>,
    hidden_types: HashSet<String>,
    hidden_qualified_types: HashSet<String>,
}

impl TestResolutionSuppression {
    pub(crate) fn hide_module(&mut self, name: String) {
        self.hidden_modules.insert(name);
    }

    pub(crate) fn hide_declared_types(
        &mut self,
        parsed: &marrow_syntax::ParsedSource,
        modules: &[String],
    ) {
        for declaration in &parsed.file.declarations {
            let name = match declaration {
                marrow_syntax::Declaration::Resource(resource) => &resource.name,
                marrow_syntax::Declaration::Enum(enum_decl) => &enum_decl.name,
                _ => continue,
            };
            self.hidden_types.insert(name.clone());
            for module in modules {
                self.hidden_qualified_types
                    .insert(format!("{module}::{name}"));
            }
        }
    }

    pub(crate) fn reveal_complete_modules(&mut self, program: &CheckedProgram) {
        for module in &program.modules {
            self.hidden_modules.remove(&module.name);
        }
    }

    fn should_suppress(&self, diagnostic: &CheckDiagnostic) -> bool {
        match &diagnostic.payload {
            DiagnosticPayload::UnresolvedImport(name) => {
                self.references_hidden_module_exactly(name)
            }
            DiagnosticPayload::UnresolvedCall(name) => self.references_hidden_module_member(name),
            DiagnosticPayload::UnknownType(ty) => self.references_hidden_type(ty),
            DiagnosticPayload::Schema(_)
            | DiagnosticPayload::DuplicateDeclaration { .. }
            | DiagnosticPayload::DuplicateModule { .. }
            | DiagnosticPayload::ModulePath { .. }
            | DiagnosticPayload::DuplicateRootOwner { .. }
            | DiagnosticPayload::RejectedSurface(_)
            | DiagnosticPayload::Enum(_)
            | DiagnosticPayload::PrivateEnum(_)
            | DiagnosticPayload::DuplicateNamedArgument(_)
            | DiagnosticPayload::AppendTarget(_)
            | DiagnosticPayload::ConversionUnsupportedSource(_)
            | DiagnosticPayload::InterpolationUnsupportedSource { .. }
            | DiagnosticPayload::ReservedCatalogPathReuse { .. }
            | DiagnosticPayload::CatalogIntent(_)
            | DiagnosticPayload::TypeMismatch { .. }
            | DiagnosticPayload::None => false,
        }
    }

    fn references_hidden_module_exactly(&self, name: &str) -> bool {
        self.hidden_modules.contains(name)
    }

    fn references_hidden_module_member(&self, name: &str) -> bool {
        self.hidden_modules
            .iter()
            .any(|module| name.starts_with(&format!("{module}::")))
    }

    fn references_hidden_type(&self, ty: &marrow_schema::Type) -> bool {
        match ty {
            // A sequence is hidden when its element names a hidden type; the type
            // model already peeled the `sequence[...]` wrapper.
            marrow_schema::Type::Sequence(element) => self.references_hidden_type(element),
            // A qualified name carries `::`; only such names go in the qualified
            // set, so the segment shape decides which hidden set to consult.
            marrow_schema::Type::Named(name) if name.contains("::") => {
                self.hidden_qualified_types.contains(name)
            }
            marrow_schema::Type::Named(name) => self.hidden_types.contains(name),
            _ => false,
        }
    }
}

pub(crate) struct CheckedTests {
    pub(crate) report: CheckReport,
    pub(crate) test_module_start: usize,
    pub(crate) files: Vec<analysis::AnalyzedFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestProgramOutput {
    DiagnosticsOnly,
    FinalizeCombined,
}

pub(crate) fn check_tests_with_sources_analysis(
    project_root: &Path,
    config: &ProjectConfig,
    project: &mut CheckedProgram,
    sources: &ProjectSources,
    mut resolution_suppression: TestResolutionSuppression,
    output: TestProgramOutput,
) -> Result<CheckedTests, DiscoverError> {
    let mut files = discover_test_modules(project_root, config)?;
    for path in sources.paths() {
        if let Some(file) = marrow_project::test_module_file(project_root, config, path) {
            files.push(file);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    let mut report = CheckReport::default();
    let mut modules = Vec::new();
    let mut parsed_files: Vec<(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)> =
        Vec::new();
    let mut parsed_sources: HashMap<PathBuf, String> = HashMap::new();

    for file in &files {
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

        // A test file is a script: it is named from its path, never from a
        // declared `module`. Duplicate source/test module names are rejected
        // after the clean test module set is known.
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
                stores,
                enums,
                enum_public: enum_visibility(&parsed.file),
            });
        }
        parsed_sources.insert(file.path.clone(), source);
        parsed_files.push((file, parsed));
    }

    hide_types_from_broken_test_files(&mut resolution_suppression, &parsed_files);

    let (modules, duplicate_test_module_names) =
        split_duplicate_test_modules(project, modules, &mut report);
    let resolvable = test_resolvable_modules(project, &modules);

    // Run the same type-inference pass library files get, so a test file's std
    // argument/arity errors, `nextId` misuse, and ordinary type mismatches surface
    // at check time. The combined program holds both the already-checked project
    // modules and the clean test modules, so cross-module calls, resource
    // constructors, and the `nextId` gate's resource schemas all resolve.
    let project_count = project.modules.len();
    project.modules.extend(modules);
    resolution_suppression.reveal_complete_modules(project);
    // The duplicate diagnostic is the useful error; downstream references into
    // that invalid namespace should not also pretend the function is just absent.
    for module in &duplicate_test_module_names {
        resolution_suppression.hide_module(module.clone());
    }
    // Stamp cross-module named-type signature slots in the test modules with their
    // true owner — resolved against the combined program — before the type pass
    // reads them. The project modules carry the project's own normalized
    // signatures already.
    enums::normalize_program_named_types(project, &parsed_files);
    crate::keyed_entries::normalize_resource_layers(
        project,
        &parsed_files,
        &mut report.diagnostics,
    );
    // Passes 2-3 plus targeted unresolved-call suppression are shared with check_project.
    // A read failure drops a file from `parsed_files` so a call into it would look
    // unresolved; the shared suppression handles that qualified-call case.
    let combined = project;
    checks::check_resolved_files(
        checks::ResolvedFileCheck {
            files: &files,
            parsed_files: &parsed_files,
            module_name_policy: checks::ModuleNamePolicy::PathOnly,
            resolvable: &resolvable,
            program: combined,
        },
        &mut report,
    );
    // When the source program is partial, a configured test may name a module,
    // function, resource, or enum from a source file that could not join `project`.
    // Keep test-local parse/type diagnostics, but drop resolution noise whose truth
    // depends on the incomplete source module set.
    report
        .diagnostics
        .retain(|diagnostic| !resolution_suppression.should_suppress(diagnostic));

    if output == TestProgramOutput::FinalizeCombined {
        combined.rebuild_facts_with_sources_preserving_current_prefix(
            parsed_files
                .iter()
                .map(|(file, parsed)| (file.path.as_path(), parsed)),
        );
        combined.lower_runtime_bodies(
            parsed_files
                .iter()
                .map(|(file, parsed)| (file.path.as_path(), parsed)),
        );
    }
    let analyzed = parsed_files
        .into_iter()
        .map(|(file, parsed)| analysis::AnalyzedFile {
            path: file.path.clone(),
            module_name: file.module_name.clone(),
            source: parsed_sources.remove(&file.path).unwrap_or_default(),
            parsed,
        })
        .collect();

    Ok(CheckedTests {
        report,
        test_module_start: project_count,
        files: analyzed,
    })
}

/// Hide the resource and enum types declared in test files that failed to parse,
/// so a downstream reference into the broken file's namespace is suppressed rather
/// than reported as an unknown type.
fn hide_types_from_broken_test_files(
    resolution_suppression: &mut TestResolutionSuppression,
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
) {
    for (file, parsed) in parsed_files {
        if parsed.has_errors() {
            match &file.module_name {
                Some(module) => {
                    resolution_suppression.hide_declared_types(parsed, std::slice::from_ref(module))
                }
                None => resolution_suppression.hide_declared_types(parsed, &[]),
            }
        }
    }
}

/// Split clean test modules into the ones whose path-derived name is unique and
/// the ones that collide with a source module. A clean test file is named from its
/// project-root path; keeping a collision would make one qualified name point at
/// two files depending on which resolver table was used, so the colliding module is
/// reported as a duplicate and its name returned for resolution-suppression.
fn split_duplicate_test_modules(
    project: &CheckedProgram,
    modules: Vec<CheckedModule>,
    report: &mut CheckReport,
) -> (Vec<CheckedModule>, HashSet<String>) {
    let project_module_sources: HashMap<&str, &PathBuf> = project
        .modules
        .iter()
        .filter(|module| !module.name.is_empty())
        .map(|module| (module.name.as_str(), &module.source_file))
        .collect();
    let mut duplicates = HashSet::new();
    let mut unique = Vec::new();
    for module in modules {
        if let Some(first) = project_module_sources.get(module.name.as_str()) {
            report.diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_DUPLICATE_MODULE,
                    &module.source_file,
                    SourceSpan::default(),
                    format!(
                        "module `{}` is already declared by `{}`",
                        module.name,
                        first.display()
                    ),
                )
                .with_payload(DiagnosticPayload::DuplicateModule {
                    name: module.name.clone(),
                    first_file: (*first).clone(),
                }),
            );
            duplicates.insert(module.name);
        } else {
            unique.push(module);
        }
    }
    (unique, duplicates)
}

/// The modules a test file's imports resolve against: the project's named modules
/// and the unique test modules. The project's module-less script carries the empty
/// name; a `use ""` is not spellable, so it is filtered out — the import-safety
/// invariant (a script is never importable) is kept local to this map rather than
/// resting on a distant construction guard.
fn test_resolvable_modules(
    project: &CheckedProgram,
    test_modules: &[CheckedModule],
) -> HashMap<String, PathBuf> {
    let mut resolvable: HashMap<String, PathBuf> = project
        .modules
        .iter()
        .filter(|module| !module.name.is_empty())
        .map(|module| (module.name.clone(), module.source_file.clone()))
        .collect();
    for module in test_modules {
        resolvable.insert(module.name.clone(), module.source_file.clone());
    }
    resolvable
}

/// The per-file result of [`check_file`]: the parsed source and the declaration
/// lists collected for a [`CheckedModule`].
pub(crate) struct CheckedFile {
    pub(crate) parsed: marrow_syntax::ParsedSource,
    pub(crate) resources: Vec<marrow_schema::ResourceSchema>,
    pub(crate) stores: Vec<marrow_schema::StoreSchema>,
    pub(crate) enums: Vec<marrow_schema::EnumSchema>,
    pub(crate) functions: Vec<CheckedFunction>,
    pub(crate) constants: Vec<CheckedConst>,
}

/// Resolve a file's text: the overlay entry for `file_path` if `sources` carries
/// one, otherwise the on-disk contents. A read failure records an `io.read`
/// diagnostic and returns `None`, preserving the read-failure-drops-a-file
/// invariant the project and test checkers rely on.
pub(crate) fn read_source(
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
            diagnostics.push(CheckDiagnostic::error(
                IO_READ,
                file_path,
                SourceSpan::default(),
                format!("failed to read source: {error}"),
            ));
            None
        }
    }
}

/// Parse and structurally check one source file's `source`: record its parse,
/// duplicate-name, function-body, const-value, and resource-schema diagnostics,
/// and collect the declaration lists for a checked module. Always returns a
/// [`CheckedFile`] — a parse error yields one whose `parsed` carries the
/// diagnostics. Cross-file checks — module-path matching, saved-root ownership,
/// and import resolution — belong to the caller, since they span files and differ
/// between library modules and test scripts.
pub(crate) fn check_file_source(
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
            payload: DiagnosticPayload::None,
        });
    }

    check_duplicate_declarations(file_path, &parsed.file, diagnostics);

    let mut module_enums: Vec<String> = Vec::new();
    let mut module_stored_resources: HashSet<String> = HashSet::new();
    for declaration in &parsed.file.declarations {
        match declaration {
            marrow_syntax::Declaration::Enum(decl) => module_enums.push(decl.name.clone()),
            marrow_syntax::Declaration::Store(store) => {
                module_stored_resources.insert(store.resource.clone());
            }
            _ => {}
        }
    }
    // A bare enum annotation in a signature names this module's enum, so the
    // resolved type carries the module's qualified name as the enum's owner. A
    // module-less script has no declared name (its enums are project-unique).
    let module_name = parsed
        .file
        .module
        .as_ref()
        .map_or("", |module| module.name.as_str());
    let names = TypeNames {
        module: module_name,
        enums: &module_enums,
    };
    let resources = checked_resources(
        file_path,
        &parsed,
        &module_stored_resources,
        &module_enums,
        diagnostics,
    );
    let mut stores = Vec::new();
    let mut enums = Vec::new();
    let mut functions = Vec::new();
    let mut constants = Vec::new();
    for declaration in &parsed.file.declarations {
        match declaration {
            marrow_syntax::Declaration::Function(function) => {
                rules::check_function_body(file_path, function, diagnostics);
                functions.push(checked_function(function, names));
            }
            marrow_syntax::Declaration::Resource(_) => {}
            marrow_syntax::Declaration::Store(store) => {
                if let Some(resource) = resources
                    .iter()
                    .find(|resource| resource.name == store.resource)
                {
                    let (schema, errors) = marrow_schema::compile_store(store, resource);
                    for error in errors {
                        // A stored resource is compiled twice — once for its
                        // resource schema, once for the store — so the same schema
                        // error can surface from both passes.
                        let diagnostic = schema_diagnostic(file_path, error);
                        if !has_duplicate_error(diagnostics, &diagnostic) {
                            diagnostics.push(diagnostic);
                        }
                    }
                    stores.push(schema);
                } else if !store.resource.is_empty() && !store.root.root.is_empty() {
                    diagnostics.push(CheckDiagnostic::error(
                        CHECK_UNKNOWN_TYPE,
                        file_path,
                        store.span,
                        format!(
                            "unknown resource `{}` for store `^{}`",
                            store.resource, store.root.root
                        ),
                    ));
                }
            }
            marrow_syntax::Declaration::Enum(decl) => {
                let (schema, errors) = marrow_schema::compile_enum(decl);
                for error in errors {
                    push_schema_error(file_path, diagnostics, error);
                }
                enums.push(schema);
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
                        .map(|ty| MarrowType::resolve(ty, names)),
                    value: constant.value.clone(),
                    span: constant.span,
                });
            }
            // An evolve block compiles to no declaration; its catalog intent is
            // resolved later against the bound program.
            marrow_syntax::Declaration::Evolve(_) => {}
        }
    }

    CheckedFile {
        parsed,
        resources,
        stores,
        enums,
        functions,
        constants,
    }
}

fn checked_resources(
    file_path: &Path,
    parsed: &marrow_syntax::ParsedSource,
    module_stored_resources: &HashSet<String>,
    module_enums: &[String],
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Vec<marrow_schema::ResourceSchema> {
    parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Resource(resource) => Some(resource),
            _ => None,
        })
        .map(|resource| {
            let has_store = module_stored_resources.contains(&resource.name);
            let (schema, errors) = if has_store {
                marrow_schema::compile_stored_resource(resource)
            } else {
                marrow_schema::compile_resource(resource)
            };
            for error in errors {
                push_schema_error(file_path, diagnostics, error);
            }
            if has_store {
                for error in marrow_schema::check_saved_member_rules(&resource.members) {
                    push_schema_error(file_path, diagnostics, error);
                }
                for error in
                    marrow_schema::check_saved_named_member_fields(&resource.members, module_enums)
                {
                    push_schema_error(file_path, diagnostics, error);
                }
            }
            schema
        })
        .collect()
}

pub(crate) fn push_schema_error(
    file_path: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    error: marrow_schema::SchemaError,
) {
    diagnostics.push(schema_diagnostic(file_path, error));
}

/// Whether `diagnostic` repeats one already collected, by typed identity: same
/// code, file, span, and payload. Two distinct errors at one span differ in
/// payload, so this only collapses a genuine duplicate.
fn has_duplicate_error(diagnostics: &[CheckDiagnostic], diagnostic: &CheckDiagnostic) -> bool {
    diagnostics.iter().any(|existing| {
        existing.code == diagnostic.code
            && existing.file == diagnostic.file
            && existing.payload == diagnostic.payload
            && existing.span == diagnostic.span
    })
}

fn schema_diagnostic(file_path: &Path, error: marrow_schema::SchemaError) -> CheckDiagnostic {
    let marrow_schema::SchemaError {
        kind,
        code,
        message,
        span,
    } = error;
    CheckDiagnostic::error(code, file_path, span, message)
        .with_payload(DiagnosticPayload::Schema(kind))
}

/// Resolve a function declaration for the checked-program artifact: its
/// signature plus the checked executable body runtime evaluates.
fn checked_function(
    function: &marrow_syntax::FunctionDecl,
    names: TypeNames<'_>,
) -> CheckedFunction {
    CheckedFunction {
        name: function.name.clone(),
        public: function.public,
        params: function
            .params
            .iter()
            .map(|param| CheckedParam {
                name: param.name.clone(),
                ty: MarrowType::resolve(&param.ty, names),
            })
            .collect(),
        return_presence: match function.return_presence {
            marrow_syntax::FunctionReturnPresence::Always => ReturnPresence::Always,
            marrow_syntax::FunctionReturnPresence::MaybePresent => ReturnPresence::MaybePresent,
        },
        return_type: function
            .return_type
            .as_ref()
            .map(|ty| MarrowType::resolve(ty, names)),
        span: function.span,
        runtime_body: None,
    }
}

/// Build the per-file short→full alias map from a file's full import paths
/// (`["std::clock", "shelf::books"]`): key = the short trailing segment
/// (`"clock"`, `"books"`), value = the full path split into segments
/// (`["std","clock"]`). Same short-name derivation as
/// [`check_duplicate_declarations`], so the two stay consistent. Drives short-form
/// call resolution during checking and executable lowering.
pub(crate) fn build_alias_map(
    import_paths: &[String],
) -> std::collections::HashMap<String, Vec<String>> {
    import_paths
        .iter()
        .map(|path| {
            let short = short_name(path).to_string();
            (short, split_type_path(path))
        })
        .collect()
}

/// Split a `::`-separated type or name path into its owned segments.
pub(crate) fn split_type_path(path: &str) -> Vec<String> {
    path.split("::").map(str::to_string).collect()
}

/// The unqualified last segment of a `::`-separated path (`shelf::books` → `books`).
fn short_name(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

/// Expand a call/name's leading segment through the file's import aliases, applied
/// once up front before any builtin/std/function resolution. `clock::now` with
/// `{clock → [std,clock]}` becomes `[std,clock,now]`. A single-segment name is
/// never expanded (short-form requires the module qualifier), and a leading
/// segment that is not an alias is left untouched (so `std::clock::now` and a
/// project `mod::fn` pass through unchanged).
pub(crate) fn expand_alias(
    segments: &[String],
    aliases: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    if segments.len() >= 2 {
        return expand_leading_alias(segments, aliases);
    }
    segments.to_vec()
}

fn expand_leading_alias(
    segments: &[String],
    aliases: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    if let Some(first) = segments.first()
        && let Some(full) = aliases.get(first)
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
pub(crate) fn is_resolved_import(name: &str, project_modules: &HashMap<String, PathBuf>) -> bool {
    project_modules.contains_key(name) || is_std_module(name)
}

pub(crate) fn module_path_error(
    file: &marrow_project::ModuleFile,
    module: &marrow_syntax::ModuleDecl,
    message: String,
    expected: Option<String>,
) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_MODULE_PATH, &file.path, module.span, message).with_payload(
        DiagnosticPayload::ModulePath {
            declared: module.name.clone(),
            expected,
        },
    )
}

/// How a name enters a file's top-level namespace. Only declarations may
/// collide with a builtin name; an imported short name binds the import
/// regardless. The variant also names the kind in the duplicate diagnostic.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NameKind {
    Import,
    Declaration,
}

impl NameKind {
    fn label(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Declaration => "declaration",
        }
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

    // A `use shelf::books` introduces the short name `books`.
    let mut introduced: Vec<(&str, SourceSpan, NameKind)> = Vec::new();
    for use_decl in &source.uses {
        let short = short_name(&use_decl.name);
        introduced.push((short, use_decl.span, NameKind::Import));
    }
    for declaration in &source.declarations {
        let (name, span) = match declaration {
            Declaration::Const(decl) => (decl.name.as_str(), decl.span),
            Declaration::Resource(decl) => (decl.name.as_str(), decl.span),
            Declaration::Store(_) | Declaration::Evolve(_) => continue,
            Declaration::Function(decl) => (decl.name.as_str(), decl.span),
            Declaration::Enum(decl) => (decl.name.as_str(), decl.span),
        };
        introduced.push((name, span, NameKind::Declaration));
    }
    introduced.sort_by_key(|(_, span, _)| (span.line, span.start_byte));

    let mut first_seen: HashMap<&str, SourceSpan> = HashMap::new();
    for (name, span, kind) in &introduced {
        // The parser leaves a failed declaration with an empty name; do not
        // treat those as colliding with each other.
        if name.is_empty() {
            continue;
        }
        if *kind == NameKind::Declaration && is_builtin_name(name) {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_DUPLICATE_DECLARATION,
                file,
                *span,
                format!(
                    "`{name}` is a builtin name and cannot be used as a module-level {}",
                    kind.label()
                ),
            ));
            continue;
        }
        match first_seen.get(name) {
            Some(first) => diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_DUPLICATE_DECLARATION,
                    file,
                    *span,
                    format!("`{name}` is already declared on line {}", first.line),
                )
                .with_payload(DiagnosticPayload::DuplicateDeclaration {
                    name: (*name).to_string(),
                    first_span: *first,
                }),
            ),
            None => {
                first_seen.insert(name, *span);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ConversionTarget;

    /// The conversion spelling table must hold every variant so the two
    /// directions stay in lockstep: `spelling` finds each variant, and
    /// `from_name` maps that spelling back to the same variant.
    #[test]
    fn conversion_spelling_round_trips_every_variant() {
        use ConversionTarget::{
            Bool, Bytes, Date, Decimal, Duration, ErrorCode, Instant, Int, Str,
        };
        for target in [
            Bool, Int, Str, ErrorCode, Bytes, Date, Instant, Duration, Decimal,
        ] {
            assert_eq!(ConversionTarget::from_name(target.spelling()), Some(target));
        }
    }
}
