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
use marrow_syntax::{
    Block, Declaration, EvolveStep, FunctionDecl, SourceFile, SourceSpan, Statement, parse_source,
};

use crate::analysis;
use crate::checks;
use crate::diagnostics::{
    CHECK_BUILTIN_COLLISION, CHECK_DUPLICATE_DECLARATION, CHECK_DUPLICATE_MODULE,
    CHECK_MODULE_PATH, CHECK_SURFACE_COLLISION, CHECK_UNKNOWN_TYPE, CheckDiagnostic, CheckReport,
    ConversionTarget, DiagnosticPayload, IO_READ, SurfaceCollisionNameKind,
};
use crate::enums;
use crate::program::{
    CheckedConst, CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, TypeNames,
};
use crate::resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};
use crate::rules;
use crate::{MarrowType, ScalarType};

#[cfg(test)]
static SOURCE_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn reset_source_read_count() {
    SOURCE_READS.store(0, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn source_read_count() -> usize {
    SOURCE_READS.load(std::sync::atomic::Ordering::SeqCst)
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
    analysis::analyze_source_project(project_root, config, &ProjectSources::new(), accepted, None)
        .map(|snapshot| (snapshot.report, snapshot.program))
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

/// The qualified name of the program module whose source is `file`; callers use it
/// as the referencing module when resolving a bare enum name in that file's
/// expressions.
pub(crate) fn module_of_file<'p>(program: &'p CheckedProgram, file: &Path) -> Option<&'p str> {
    program
        .module_by_file(file)
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

/// Whether `name` is a builtin name: a pure builtin call (`exists`, `keys`,
/// conversions, …) per the typed [`CheckedBuiltinCall`] owner, or the `Error`
/// constructor, which is dispatched as a call target rather than a builtin call.
fn is_builtin_name(name: &str) -> bool {
    crate::executable::CheckedBuiltinCall::from_name(name).is_some() || name == "Error"
}

/// The return type of a single-name data builtin: `exists(path): bool`,
/// `append(layer, value): int`, and `count(collection): int` over any saved or
/// local collection (an unusable argument is rejected before this point and types
/// `invalid`). `nextId` is handled in [`check_next_id`], which has the `^root`
/// argument it needs to type the identity. The absence-default `??` is an
/// operator, not a builtin, and is typed in [`check_coalesce`].
pub(crate) fn builtin_return_type(segments: &[String]) -> Option<MarrowType> {
    let [name] = segments else {
        return None;
    };
    match name.as_str() {
        "exists" => Some(MarrowType::Primitive(ScalarType::Bool)),
        "append" | "count" => Some(MarrowType::Primitive(ScalarType::Int)),
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
/// inside the list marks an argument checked by a call-specific rule instead.
pub(crate) fn std_call_params(segments: &[String]) -> Option<Vec<Option<MarrowType>>> {
    let op = std_op(segments)?;
    Some(
        op.params
            .iter()
            .map(|param| match param {
                ParamType::Scalar(scalar) => Some(MarrowType::Primitive(*scalar)),
                ParamType::ScalarAny => None,
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
/// paths), producing one checked module per clean test file plus any
/// diagnostics. Test files are scripts outside the source roots, so each is
/// checked module-less and named from its project-relative path
/// (`tests/books_test.mw` → `tests::books_test`). Imports resolve against the
/// already-checked `project` modules, the test modules, and the standard library.
/// Saved-root ownership is not re-checked here: test scripts exercise the
/// project's resources, they do not own saved roots.
pub fn check_tests(
    project_root: &Path,
    config: &ProjectConfig,
    mut project: CheckedProgram,
) -> Result<(CheckReport, Vec<CheckedModule>), DiscoverError> {
    let checked = check_tests_with_sources_analysis(
        project_root,
        config,
        &mut project,
        &ProjectSources::new(),
        TestResolutionSuppression::default(),
    )?;
    let test_modules = project.modules.split_off(checked.test_module_start);
    Ok((checked.report, test_modules))
}

/// Like [`check_tests`], but returns the full combined [`CheckedProgram`] (project
/// modules plus the clean test modules) instead of only the test modules.
pub fn check_tests_program(
    project_root: &Path,
    config: &ProjectConfig,
    mut project: CheckedProgram,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    let checked = check_tests_with_sources_analysis(
        project_root,
        config,
        &mut project,
        &ProjectSources::new(),
        TestResolutionSuppression::default(),
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
            DiagnosticPayload::UnknownType(ty) | DiagnosticPayload::AmbiguousType { ty, .. } => {
                self.references_hidden_type(ty)
            }
            DiagnosticPayload::Schema(_)
            | DiagnosticPayload::DuplicateDeclaration { .. }
            | DiagnosticPayload::SurfaceCollision { .. }
            | DiagnosticPayload::SurfaceTarget(_)
            | DiagnosticPayload::SurfaceField(_)
            | DiagnosticPayload::SurfaceAction(_)
            | DiagnosticPayload::SurfaceComputedRead(_)
            | DiagnosticPayload::DuplicateModule { .. }
            | DiagnosticPayload::ModulePath { .. }
            | DiagnosticPayload::DefaultEntry { .. }
            | DiagnosticPayload::ReservedTestModulePathSegment { .. }
            | DiagnosticPayload::DuplicateRootOwner { .. }
            | DiagnosticPayload::RejectedSurface(_)
            | DiagnosticPayload::Enum(_)
            | DiagnosticPayload::PrivateEnum(_)
            | DiagnosticPayload::ExposedPrivateEnum { .. }
            | DiagnosticPayload::DuplicateNamedArgument(_)
            | DiagnosticPayload::AppendTarget(_)
            | DiagnosticPayload::ConversionUnsupportedSource(_)
            | DiagnosticPayload::RenderUnsupportedSource { .. }
            | DiagnosticPayload::ReservedCatalogPathReuse { .. }
            | DiagnosticPayload::CatalogIntent(_)
            | DiagnosticPayload::SuggestedIndex { .. }
            | DiagnosticPayload::UnresolvedName { .. }
            | DiagnosticPayload::RequiredAbsent { .. }
            | DiagnosticPayload::TypeMismatch { .. }
            | DiagnosticPayload::SavedCollectionByValue { .. }
            | DiagnosticPayload::LayerNotValue { .. }
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

pub(crate) fn check_tests_with_sources_analysis(
    project_root: &Path,
    config: &ProjectConfig,
    project: &mut CheckedProgram,
    sources: &ProjectSources,
    mut resolution_suppression: TestResolutionSuppression,
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
            rejected_surfaces: _,
            backing_invalidations: _,
        } = check_file_source(&file.path, &source, &mut report.diagnostics);
        let reserved_module = file
            .module_name
            .as_deref()
            .and_then(|name| reserved_module_segment(name).map(|segment| (name, segment)));
        if let Some((module_name, segment)) = reserved_module {
            report
                .diagnostics
                .push(test_module_path_error(file, module_name, segment));
            resolution_suppression.hide_module(module_name.to_string());
            resolution_suppression.hide_declared_types(&parsed, &[module_name.to_string()]);
        } else if let Some(declared) = &parsed.file.module {
            // A test file is named from its path. A declared `module` is optional,
            // but when present it must spell that path-derived name, mirroring the
            // source-file rule, so a test cannot masquerade under another name.
            if file.module_name.as_deref() != Some(declared.name.as_str()) {
                report.diagnostics.push(module_path_error(
                    file,
                    declared,
                    format!(
                        "module `{}` does not match its path; expected `{}`",
                        declared.name,
                        file.module_name.as_deref().unwrap_or_default()
                    ),
                    file.module_name.clone(),
                ));
            }
        }

        // A test file is a script: it is named from its path, never from a
        // declared `module`. Duplicate source/test module names are rejected
        // after the clean test module set is known.
        if !parsed.has_errors() && reserved_module.is_none() {
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
        None,
        &mut report.diagnostics,
    );
    project.rebuild_facts_with_sources_preserving_current_prefix(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );
    // Passes 2-3 plus targeted unresolved-call suppression are shared with check_project.
    // A read failure drops a file from `parsed_files` so a call into it would look
    // unresolved; the shared suppression handles that qualified-call case.
    checks::check_resolved_files(
        checks::ResolvedFileCheck {
            files: &files,
            parsed_files: &parsed_files,
            module_name_policy: checks::ModuleNamePolicy::PathOnly,
            resolvable: &resolvable,
            program: project,
            backing_invalidations: None,
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

    project.rebuild_facts_with_sources_preserving_current_prefix(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );
    let source_evolve_transforms = project.catalog.evolve_transforms.clone();
    project.lower_runtime_bodies(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );
    project.catalog.evolve_transforms = source_evolve_transforms;
    project.extend_durable_digest_renderings(parsed_files.iter().filter_map(|(file, parsed)| {
        parsed_sources
            .get(&file.path)
            .map(|source| (file.path.as_path(), source.as_str(), parsed))
    }));
    let analyzed = parsed_files
        .into_iter()
        .map(|(file, parsed)| analysis::AnalyzedFile {
            path: file.path.clone(),
            module_name: clean_path_module_name(&file.module_name).cloned(),
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

fn clean_path_module_name(module_name: &Option<String>) -> Option<&String> {
    module_name
        .as_ref()
        .filter(|name| reserved_module_segment(name).is_none())
}

fn reserved_module_segment(module_name: &str) -> Option<&str> {
    module_name
        .split("::")
        .find(|segment| marrow_syntax::is_reserved_word(segment))
}

fn test_module_path_error(
    file: &marrow_project::ModuleFile,
    module_name: &str,
    segment: &str,
) -> CheckDiagnostic {
    CheckDiagnostic::error(
        CHECK_MODULE_PATH,
        &file.path,
        SourceSpan::default(),
        format!(
            "test module path-derived name `{module_name}` contains reserved segment `{segment}`"
        ),
    )
    .with_payload(DiagnosticPayload::ReservedTestModulePathSegment {
        module_name: module_name.to_string(),
        reserved_segment: segment.to_string(),
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
    pub(crate) rejected_surfaces: crate::surface::RejectedSurfaceDeclarations,
    pub(crate) backing_invalidations: crate::backing_validity::PendingBackingInvalidations,
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
    #[cfg(test)]
    SOURCE_READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

    let mut backing_invalidations = crate::backing_validity::PendingBackingInvalidations::default();
    let rejected_surfaces = check_duplicate_declarations(
        file_path,
        &parsed.file,
        &mut backing_invalidations,
        diagnostics,
    );

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
        &mut backing_invalidations,
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
                check_local_key_types(file_path, function, diagnostics);
                functions.push(checked_function(function, names));
            }
            marrow_syntax::Declaration::Resource(_) => {}
            marrow_syntax::Declaration::Surface(_) => {}
            marrow_syntax::Declaration::Store(store) => {
                if let Some(resource) = resources
                    .iter()
                    .find(|resource| resource.name == store.resource)
                {
                    let (schema, errors) = marrow_schema::compile_store(store, resource);
                    for error in errors {
                        backing_invalidations.record_store_error(file_path, store, &error);
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
                    backing_invalidations.record_invalid_enum(file_path, &decl.name);
                    push_schema_error(file_path, diagnostics, error);
                }
                enums.push(schema);
            }
            marrow_syntax::Declaration::Const(constant) => {
                if let Some(value) = &constant.value {
                    // Earlier module constants in declaration order are already folded
                    // in `constants`, so a `const` defined over a preceding one resolves
                    // its overflow at check rather than faulting at run.
                    rules::check_const_value(
                        file_path,
                        value,
                        &checks::module_const_int_scope(&constants),
                        diagnostics,
                    );
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
            // resolved against the bound program.
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
        rejected_surfaces,
        backing_invalidations,
    }
}

fn checked_resources(
    file_path: &Path,
    parsed: &marrow_syntax::ParsedSource,
    module_stored_resources: &HashSet<String>,
    backing_invalidations: &mut crate::backing_validity::PendingBackingInvalidations,
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
            let (schema, errors) = marrow_schema::compile_resource(resource);
            for error in errors {
                backing_invalidations.record_resource_error(file_path, &resource.name);
                push_schema_error(file_path, diagnostics, error);
            }
            if has_store {
                for error in marrow_schema::check_saved_member_rules(&resource.members) {
                    backing_invalidations.record_resource_error(file_path, &resource.name);
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

/// Validate the key types of a function's local keyed collections — its keyed
/// parameters and the keyed `var` declarations in its body — against the same
/// orderable-scalar allowlist a saved keyed layer obeys. A local keyed tree holds no
/// saved data, but its key still projects from an orderable scalar, so an `Id`, an
/// enum, a resource, a sequence, or a `decimal` key is rejected here as it would be on
/// a saved layer, before it reaches the runtime.
fn check_local_key_types(
    file_path: &Path,
    function: &marrow_syntax::FunctionDecl,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for param in &function.params {
        check_key_param_types(file_path, &param.keys, diagnostics);
    }
    check_block_local_key_types(file_path, &function.body, diagnostics);
}

fn check_block_local_key_types(
    file_path: &Path,
    block: &marrow_syntax::Block,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Statement;
    for statement in &block.statements {
        match statement {
            Statement::Var { keys, .. } => check_key_param_types(file_path, keys, diagnostics),
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            }
            | Statement::IfConst {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                check_block_local_key_types(file_path, then_block, diagnostics);
                for else_if in else_ifs {
                    check_block_local_key_types(file_path, &else_if.block, diagnostics);
                }
                if let Some(block) = else_block {
                    check_block_local_key_types(file_path, block, diagnostics);
                }
            }
            Statement::While { body, .. }
            | Statement::For { body, .. }
            | Statement::Transaction { body, .. } => {
                check_block_local_key_types(file_path, body, diagnostics);
            }
            Statement::Try { body, catch, .. } => {
                check_block_local_key_types(file_path, body, diagnostics);
                if let Some(catch) = catch {
                    check_block_local_key_types(file_path, &catch.block, diagnostics);
                }
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    check_block_local_key_types(file_path, &arm.block, diagnostics);
                }
            }
            Statement::Const { .. }
            | Statement::Assign { .. }
            | Statement::CompoundAssign { .. }
            | Statement::Delete { .. }
            | Statement::Return { .. }
            | Statement::ReturnAbsent { .. }
            | Statement::Break { .. }
            | Statement::Continue { .. }
            | Statement::Throw { .. }
            | Statement::Expr { .. } => {}
        }
    }
}

fn check_key_param_types(
    file_path: &Path,
    keys: &[marrow_syntax::KeyParam],
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for key in keys {
        let ty = marrow_schema::Type::resolve(&key.ty);
        if let Some(error) = marrow_schema::local_key_type_error(&key.name, &ty, key.ty.span) {
            push_schema_error(file_path, diagnostics, error);
        }
    }
}

/// Whether `diagnostic` repeats one already collected, by typed identity: same
/// code, file, span, and payload. Two distinct errors at one span differ in
/// payload, so this only collapses a genuine duplicate.
pub(crate) fn has_duplicate_error(
    diagnostics: &[CheckDiagnostic],
    diagnostic: &CheckDiagnostic,
) -> bool {
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
                ty: MarrowType::keyed(
                    param
                        .keys
                        .iter()
                        .map(|key| MarrowType::resolve(&key.ty, names)),
                    MarrowType::resolve(&param.ty, names),
                ),
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
pub(crate) fn short_name(path: &str) -> &str {
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

pub(crate) fn expand_unique_import_alias(
    source: &marrow_syntax::SourceFile,
    segments: &[String],
) -> Result<Vec<String>, AmbiguousImportAlias> {
    if segments.len() < 2 {
        return Ok(segments.to_vec());
    }
    let Some(head) = segments.first() else {
        return Ok(segments.to_vec());
    };
    let Some(alias) = unique_import_module_alias_path(source, head)? else {
        return Ok(segments.to_vec());
    };
    Ok(alias
        .into_iter()
        .chain(segments[1..].iter().cloned())
        .collect())
}

pub(crate) fn unique_import_module_alias_path(
    source: &marrow_syntax::SourceFile,
    head: &str,
) -> Result<Option<Vec<String>>, AmbiguousImportAlias> {
    let mut import_matches = source
        .uses
        .iter()
        .filter(|use_decl| short_name(&use_decl.name) == head);
    let Some(import) = import_matches.next() else {
        return Ok(None);
    };
    if import_matches.next().is_some() || source_declares_top_level_name(source, head) {
        return Err(AmbiguousImportAlias);
    }
    Ok(Some(split_type_path(&import.name)))
}

pub(crate) fn unique_import_alias_for_module(
    source: &marrow_syntax::SourceFile,
    module_name: &str,
) -> Result<Option<String>, AmbiguousImportAlias> {
    let alias = short_name(module_name);
    Ok(unique_import_module_alias_path(source, alias)?
        .is_some_and(|path| path.join("::") == module_name)
        .then(|| alias.to_string()))
}

pub(crate) struct AmbiguousImportAlias;

pub(crate) fn import_alias_head_is_file_shadowed(source: &SourceFile, name: &str) -> bool {
    source_declares_top_level_name(source, name) || source_declares_body_local_name(source, name)
}

pub(crate) fn source_declares_top_level_name(source: &SourceFile, name: &str) -> bool {
    source
        .declarations
        .iter()
        .filter_map(declaration_introduced_name)
        .any(|intro| intro.name == name)
}

fn source_declares_body_local_name(source_file: &SourceFile, name: &str) -> bool {
    source_file
        .declarations
        .iter()
        .any(|declaration| match declaration {
            Declaration::Function(function) => function_declares_local_name(function, name),
            Declaration::Evolve(evolve) => evolve.steps.iter().any(|step| {
                matches!(
                    step,
                    EvolveStep::Transform { body, .. } if block_declares_local_name(body, name)
                )
            }),
            Declaration::Const(_)
            | Declaration::Resource(_)
            | Declaration::Store(_)
            | Declaration::Surface(_)
            | Declaration::Enum(_) => false,
        })
}

fn function_declares_local_name(function: &FunctionDecl, name: &str) -> bool {
    function.params.iter().any(|param| param.name == name)
        || block_declares_local_name(&function.body, name)
}

fn block_declares_local_name(block: &Block, name: &str) -> bool {
    block
        .statements
        .iter()
        .any(|statement| statement_declares_local_name(statement, name))
}

fn statement_declares_local_name(statement: &Statement, name: &str) -> bool {
    match statement {
        Statement::Const {
            name: local_name, ..
        }
        | Statement::Var {
            name: local_name, ..
        }
        | Statement::IfConst {
            name: local_name, ..
        } => local_name == name || statement_child_blocks_declare_local_name(statement, name),
        Statement::For { binding, body, .. } => {
            binding.first == name
                || binding.second.as_deref() == Some(name)
                || block_declares_local_name(body, name)
        }
        Statement::Try { body, catch, .. } => {
            block_declares_local_name(body, name)
                || catch.as_ref().is_some_and(|catch| {
                    catch.name == name || block_declares_local_name(&catch.block, name)
                })
        }
        _ => statement_child_blocks_declare_local_name(statement, name),
    }
}

fn statement_child_blocks_declare_local_name(statement: &Statement, name: &str) -> bool {
    match statement {
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        }
        | Statement::IfConst {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            block_declares_local_name(then_block, name)
                || else_ifs
                    .iter()
                    .any(|else_if| block_declares_local_name(&else_if.block, name))
                || else_block
                    .as_ref()
                    .is_some_and(|block| block_declares_local_name(block, name))
        }
        Statement::While { body, .. } | Statement::Transaction { body, .. } => {
            block_declares_local_name(body, name)
        }
        Statement::Match { arms, .. } => arms
            .iter()
            .any(|arm| block_declares_local_name(&arm.block, name)),
        Statement::For { body, .. } => block_declares_local_name(body, name),
        Statement::Try { body, catch, .. } => {
            block_declares_local_name(body, name)
                || catch
                    .as_ref()
                    .is_some_and(|catch| block_declares_local_name(&catch.block, name))
        }
        Statement::Const { .. }
        | Statement::Var { .. }
        | Statement::Assign { .. }
        | Statement::CompoundAssign { .. }
        | Statement::Delete { .. }
        | Statement::Return { .. }
        | Statement::ReturnAbsent { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Throw { .. }
        | Statement::Expr { .. } => false,
    }
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
/// regardless. Surface names need their exact kind so surface-involved collisions
/// can carry the dedicated diagnostic code and payload.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NameKind {
    Import,
    Const,
    Resource,
    Function,
    Enum,
    Surface,
}

impl NameKind {
    fn is_declaration(self) -> bool {
        self != Self::Import
    }

    fn is_backing_owner(self) -> bool {
        matches!(self, Self::Resource | Self::Enum)
    }

    fn is_surface(self) -> bool {
        self == Self::Surface
    }

    fn surface_collision_kind(self) -> SurfaceCollisionNameKind {
        match self {
            Self::Import => SurfaceCollisionNameKind::Import,
            Self::Const => SurfaceCollisionNameKind::Const,
            Self::Resource => SurfaceCollisionNameKind::Resource,
            Self::Function => SurfaceCollisionNameKind::Function,
            Self::Enum => SurfaceCollisionNameKind::Enum,
            Self::Surface => SurfaceCollisionNameKind::Surface,
        }
    }
}

#[derive(Clone, Copy)]
struct IntroducedName<'a> {
    name: &'a str,
    span: SourceSpan,
    kind: NameKind,
}

#[derive(Clone, Copy)]
struct NameOwner {
    span: SourceSpan,
    kind: NameKind,
    is_builtin_declaration: bool,
}

struct TopLevelNameOwners {
    first: NameOwner,
    first_surface: Option<NameOwner>,
    first_backing_owner: Option<NameOwner>,
    has_builtin_declaration: bool,
}

impl TopLevelNameOwners {
    fn new(first: NameOwner) -> Self {
        Self {
            first,
            first_surface: first.kind.is_surface().then_some(first),
            first_backing_owner: first.kind.is_backing_owner().then_some(first),
            has_builtin_declaration: first.is_builtin_declaration,
        }
    }

    fn surface_collision_owner(&self, duplicate_kind: NameKind) -> Option<NameOwner> {
        if duplicate_kind.is_surface() {
            Some(self.first)
        } else {
            self.first_surface
        }
    }

    fn record(&mut self, owner: NameOwner) {
        if owner.kind.is_surface() && self.first_surface.is_none() {
            self.first_surface = Some(owner);
        }
        if owner.kind.is_backing_owner() && self.first_backing_owner.is_none() {
            self.first_backing_owner = Some(owner);
        }
        self.has_builtin_declaration |= owner.is_builtin_declaration;
    }
}

/// Module-level source names and imported short module names share one
/// namespace within a file. Surface-local names have their own checked
/// namespaces, but use the same diagnostic payload shape.
fn check_duplicate_declarations(
    file: &Path,
    source: &marrow_syntax::SourceFile,
    backing_invalidations: &mut crate::backing_validity::PendingBackingInvalidations,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> crate::surface::RejectedSurfaceDeclarations {
    use marrow_syntax::Declaration;

    let mut rejected_surfaces = crate::surface::RejectedSurfaceDeclarations::default();

    let introduced = source_top_level_names(source);

    let mut first_seen: HashMap<&str, TopLevelNameOwners> = HashMap::new();
    for intro in &introduced {
        // The parser leaves a failed declaration with an empty name; do not
        // treat those as colliding with each other.
        if intro.name.is_empty() {
            continue;
        }

        let is_builtin_declaration = intro.kind.is_declaration() && is_builtin_name(intro.name);
        let owner = NameOwner {
            span: intro.span,
            kind: intro.kind,
            is_builtin_declaration,
        };
        if is_builtin_declaration && intro.kind.is_surface() {
            diagnostics.push(surface_collision_diagnostic(
                file,
                intro.name,
                intro.span,
                intro.span,
                SurfaceCollisionNameKind::Builtin,
                SurfaceCollisionNameKind::Surface,
            ));
            reject_surface_owner(file, intro.kind, intro.span, &mut rejected_surfaces);
            record_top_level_owner(&mut first_seen, intro.name, owner);
            continue;
        }
        if is_builtin_declaration {
            record_invalid_duplicate_backing_owner(
                file,
                intro.name,
                intro.kind,
                backing_invalidations,
            );
            diagnostics.push(builtin_name_diagnostic(file, intro.name, intro.span));
            record_top_level_owner(&mut first_seen, intro.name, owner);
            continue;
        }

        match first_seen.get_mut(intro.name) {
            Some(owners) => {
                if let Some(first_surface) = owners.surface_collision_owner(intro.kind) {
                    if intro.kind.is_backing_owner()
                        && let Some(first_backing_owner) = owners.first_backing_owner
                    {
                        diagnostics.push(duplicate_declaration_diagnostic(
                            file,
                            intro.name,
                            intro.span,
                            first_backing_owner.span,
                        ));
                        record_invalid_duplicate_backing_owner(
                            file,
                            intro.name,
                            first_backing_owner.kind,
                            backing_invalidations,
                        );
                        record_invalid_duplicate_backing_owner(
                            file,
                            intro.name,
                            intro.kind,
                            backing_invalidations,
                        );
                    }
                    diagnostics.push(surface_collision_diagnostic(
                        file,
                        intro.name,
                        intro.span,
                        first_surface.span,
                        first_surface.kind.surface_collision_kind(),
                        intro.kind.surface_collision_kind(),
                    ));
                    reject_surface_owner(
                        file,
                        first_surface.kind,
                        first_surface.span,
                        &mut rejected_surfaces,
                    );
                    reject_surface_owner(file, intro.kind, intro.span, &mut rejected_surfaces);
                } else if owners.has_builtin_declaration {
                    owners.record(owner);
                    continue;
                } else {
                    record_invalid_duplicate_backing_owner(
                        file,
                        intro.name,
                        owners.first.kind,
                        backing_invalidations,
                    );
                    record_invalid_duplicate_backing_owner(
                        file,
                        intro.name,
                        intro.kind,
                        backing_invalidations,
                    );
                    diagnostics.push(duplicate_declaration_diagnostic(
                        file,
                        intro.name,
                        intro.span,
                        owners.first.span,
                    ));
                }
                owners.record(owner);
            }
            None => {
                first_seen.insert(intro.name, TopLevelNameOwners::new(owner));
            }
        }
    }

    for declaration in &source.declarations {
        match declaration {
            Declaration::Surface(surface) => {
                if check_surface_local_namespace(file, surface, diagnostics) {
                    rejected_surfaces.reject(file, surface.span);
                }
            }
            Declaration::Const(_)
            | Declaration::Resource(_)
            | Declaration::Store(_)
            | Declaration::Function(_)
            | Declaration::Enum(_)
            | Declaration::Evolve(_) => {}
        }
    }

    rejected_surfaces
}

fn source_top_level_names(source: &marrow_syntax::SourceFile) -> Vec<IntroducedName<'_>> {
    let mut introduced: Vec<IntroducedName<'_>> = source
        .uses
        .iter()
        .map(import_introduced_name)
        .chain(
            source
                .declarations
                .iter()
                .filter_map(declaration_introduced_name),
        )
        .collect();
    introduced.sort_by_key(|intro| (intro.span.line, intro.span.start_byte));
    introduced
}

fn import_introduced_name(use_decl: &marrow_syntax::UseDecl) -> IntroducedName<'_> {
    IntroducedName {
        name: short_name(&use_decl.name),
        span: use_decl.span,
        kind: NameKind::Import,
    }
}

fn declaration_introduced_name(
    declaration: &marrow_syntax::Declaration,
) -> Option<IntroducedName<'_>> {
    use marrow_syntax::Declaration;

    match declaration {
        Declaration::Const(decl) => Some(IntroducedName {
            name: decl.name.as_str(),
            span: decl.span,
            kind: NameKind::Const,
        }),
        Declaration::Resource(decl) => Some(IntroducedName {
            name: decl.name.as_str(),
            span: decl.span,
            kind: NameKind::Resource,
        }),
        Declaration::Store(_) => None,
        Declaration::Surface(decl) => Some(IntroducedName {
            name: decl.name.as_str(),
            span: decl.span,
            kind: NameKind::Surface,
        }),
        Declaration::Function(decl) => Some(IntroducedName {
            name: decl.name.as_str(),
            span: decl.span,
            kind: NameKind::Function,
        }),
        Declaration::Enum(decl) => Some(IntroducedName {
            name: decl.name.as_str(),
            span: decl.span,
            kind: NameKind::Enum,
        }),
        Declaration::Evolve(_) => None,
    }
}

/// The `check.duplicate_declaration` diagnostic, shared by module-scope detection
/// here and same-block redeclaration detection in [`crate::rules`].
pub(crate) fn duplicate_declaration_diagnostic(
    file: &Path,
    name: &str,
    span: SourceSpan,
    first_span: SourceSpan,
) -> CheckDiagnostic {
    CheckDiagnostic::error(
        CHECK_DUPLICATE_DECLARATION,
        file,
        span,
        format!("`{name}` is already declared on line {}", first_span.line),
    )
    .with_payload(DiagnosticPayload::DuplicateDeclaration {
        name: name.to_string(),
        first_span,
    })
}

fn record_invalid_duplicate_backing_owner(
    file: &Path,
    name: &str,
    kind: NameKind,
    backing_invalidations: &mut crate::backing_validity::PendingBackingInvalidations,
) {
    match kind {
        NameKind::Resource => backing_invalidations.record_invalid_resource(file, name),
        NameKind::Enum => backing_invalidations.record_invalid_enum(file, name),
        NameKind::Import | NameKind::Const | NameKind::Function | NameKind::Surface => {}
    }
}

fn reject_surface_owner(
    file: &Path,
    kind: NameKind,
    span: SourceSpan,
    rejected_surfaces: &mut crate::surface::RejectedSurfaceDeclarations,
) {
    if kind.is_surface() {
        rejected_surfaces.reject(file, span);
    }
}

fn record_top_level_owner<'a>(
    first_seen: &mut HashMap<&'a str, TopLevelNameOwners>,
    name: &'a str,
    owner: NameOwner,
) {
    match first_seen.get_mut(name) {
        Some(owners) => owners.record(owner),
        None => {
            first_seen.insert(name, TopLevelNameOwners::new(owner));
        }
    }
}

fn builtin_name_diagnostic(file: &Path, name: &str, span: SourceSpan) -> CheckDiagnostic {
    CheckDiagnostic::error(
        CHECK_BUILTIN_COLLISION,
        file,
        span,
        format!("`{name}` is a builtin name and cannot be used as a module-level declaration"),
    )
}

const GENERATED_SURFACE_OPERATION_NAMES: &[&str] = &["id", "get", "create", "update", "delete"];

fn check_surface_local_namespace(
    file: &Path,
    surface: &marrow_syntax::SurfaceDecl,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> bool {
    use marrow_syntax::SurfaceItem;

    let mut collided = false;
    let mut operations: HashMap<&str, (SourceSpan, SurfaceCollisionNameKind)> = HashMap::new();
    for name in GENERATED_SURFACE_OPERATION_NAMES {
        collided |= introduce_surface_local_name(
            file,
            diagnostics,
            &mut operations,
            name,
            surface.span,
            SurfaceCollisionNameKind::GeneratedOperation,
        );
    }

    let mut fields: HashMap<&str, (SourceSpan, SurfaceCollisionNameKind)> = HashMap::new();
    let mut create: HashMap<&str, (SourceSpan, SurfaceCollisionNameKind)> = HashMap::new();
    let mut update: HashMap<&str, (SourceSpan, SurfaceCollisionNameKind)> = HashMap::new();
    let mut delete: HashMap<&str, (SourceSpan, SurfaceCollisionNameKind)> = HashMap::new();
    for item in &surface.items {
        match item {
            SurfaceItem::Fields {
                names, name_spans, ..
            } => {
                collided |= introduce_surface_payload_names(
                    file,
                    diagnostics,
                    &mut fields,
                    names,
                    name_spans,
                    SurfaceCollisionNameKind::FieldItem,
                );
            }
            SurfaceItem::Collection { alias, span, .. } => {
                collided |= introduce_surface_local_name(
                    file,
                    diagnostics,
                    &mut operations,
                    alias,
                    *span,
                    SurfaceCollisionNameKind::CollectionAlias,
                );
            }
            SurfaceItem::Action { alias, span, .. } => {
                collided |= introduce_surface_local_name(
                    file,
                    diagnostics,
                    &mut operations,
                    alias,
                    *span,
                    SurfaceCollisionNameKind::ActionAlias,
                );
            }
            SurfaceItem::Read { alias, span, .. } => {
                collided |= introduce_surface_local_name(
                    file,
                    diagnostics,
                    &mut operations,
                    alias,
                    *span,
                    SurfaceCollisionNameKind::ComputedReadAlias,
                );
            }
            SurfaceItem::Create {
                names, name_spans, ..
            } => {
                collided |= introduce_surface_payload_names(
                    file,
                    diagnostics,
                    &mut create,
                    names,
                    name_spans,
                    SurfaceCollisionNameKind::CreateItem,
                );
            }
            SurfaceItem::Update {
                names, name_spans, ..
            } => {
                collided |= introduce_surface_payload_names(
                    file,
                    diagnostics,
                    &mut update,
                    names,
                    name_spans,
                    SurfaceCollisionNameKind::UpdateItem,
                );
            }
            SurfaceItem::Delete { span } => {
                collided |= introduce_surface_local_name(
                    file,
                    diagnostics,
                    &mut delete,
                    "delete",
                    *span,
                    SurfaceCollisionNameKind::DeleteItem,
                );
            }
        }
    }

    collided
}

fn introduce_surface_payload_names<'a>(
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    first_seen: &mut HashMap<&'a str, (SourceSpan, SurfaceCollisionNameKind)>,
    names: &'a [String],
    name_spans: &[SourceSpan],
    kind: SurfaceCollisionNameKind,
) -> bool {
    let mut collided = false;
    for (name, span) in names.iter().zip(name_spans) {
        collided |= introduce_surface_local_name(file, diagnostics, first_seen, name, *span, kind);
    }
    collided
}

fn introduce_surface_local_name<'a>(
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    first_seen: &mut HashMap<&'a str, (SourceSpan, SurfaceCollisionNameKind)>,
    name: &'a str,
    span: SourceSpan,
    kind: SurfaceCollisionNameKind,
) -> bool {
    if name.is_empty() {
        return false;
    }
    match first_seen.get(name).copied() {
        Some((first_span, first_kind)) => {
            diagnostics.push(surface_collision_diagnostic(
                file, name, span, first_span, first_kind, kind,
            ));
            true
        }
        None => {
            first_seen.insert(name, (span, kind));
            false
        }
    }
}

fn surface_collision_diagnostic(
    file: &Path,
    name: &str,
    span: SourceSpan,
    first_span: SourceSpan,
    first_kind: SurfaceCollisionNameKind,
    duplicate_kind: SurfaceCollisionNameKind,
) -> CheckDiagnostic {
    CheckDiagnostic::error(
        CHECK_SURFACE_COLLISION,
        file,
        span,
        format!(
            "surface name `{name}` from {} collides with {} on line {}",
            duplicate_kind.label(),
            first_kind.label(),
            first_span.line
        ),
    )
    .with_payload(DiagnosticPayload::SurfaceCollision {
        name: name.to_string(),
        first_kind,
        first_span,
        duplicate_kind,
    })
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
