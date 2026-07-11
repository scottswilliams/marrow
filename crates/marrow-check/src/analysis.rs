//! The IDE-grade analysis pipeline: discover, read, parse, and check a project's
//! source into the snapshot editor tooling consumes. The cursor type and scope
//! lookups that read that snapshot live in [`cursor`].

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_project::{DiscoverError, ProjectConfig, Sha256Digest, StoreBackend, discover_modules};
use marrow_syntax::SourceSpan;

use crate::checks::{ModuleNamePolicy, ResolvedFileCheck, check_resolved_files};
use crate::enums::bind_signature_types;
use crate::{
    CHECK_DUPLICATE_MODULE, CHECK_READ_ONLY_EXPRESSION_CONTEXT, CheckDiagnostic, CheckReport,
    CheckedDebugExpression, CheckedFile, CheckedModule, CheckedProgram, DebugSourceIdentity,
    DefaultEntryProblem, DiagnosticAnchor, DiagnosticPayload, IO_READ, ProjectSources,
    SCHEMA_DUPLICATE_ROOT_OWNER, SurfaceActionFact, SurfaceActionOperationDescriptor,
    SurfaceCatalogStatus, SurfaceComputedReadFact, SurfaceComputedReadOperationDescriptor,
    SurfaceCreateOperationDescriptor, SurfaceDeleteFact, SurfaceDeleteOperationDescriptor,
    SurfaceFact, SurfaceReadOperationDescriptor, SurfaceReadOperationFact,
    SurfaceUpdateOperationDescriptor, TestResolutionSuppression, check_file_source,
    enum_visibility, module_path_error, read_source,
};

mod catalog_nav;
mod cursor;
mod internal_type_audit;

pub use catalog_nav::{CatalogDeclaration, UseSite, UseSiteKind};
pub(crate) use cursor::{
    ScopeCompletionBindingKind, debug_expression_scope_before, scope_completion_bindings_at,
    span_covers,
};
pub use cursor::{scope_at, type_at};

pub const ANALYSIS_GENERATION_PROFILE_VERSION: &str = "analysis.generation.v1";

/// Stable content identity for an analyzed source/config set.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnalysisIdentity(String);

impl AnalysisIdentity {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AnalysisIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable digest of the project configuration fields the analysis pipeline reads.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnalysisConfigDigest(String);

impl AnalysisConfigDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AnalysisConfigDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisCatalogGeneration {
    pub epoch: u64,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisGeneration {
    pub profile_version: &'static str,
    pub content_identity: AnalysisIdentity,
    pub config_digest: AnalysisConfigDigest,
    pub checked_source_digest: String,
    pub read_only_context_digest: String,
    pub accepted_catalog: Option<AnalysisCatalogGeneration>,
    pub proposal_catalog: Option<AnalysisCatalogGeneration>,
}

/// An IDE-grade view of a checked project: the diagnostics and best-effort
/// program [`check_project`] produces, plus every parsed file — including files
/// with parse errors, which contribute no [`CheckedModule`] but are retained
/// here so editor tooling can still work on them.
#[derive(Debug, Clone)]
pub struct AnalysisSnapshot {
    pub content_identity: AnalysisIdentity,
    config_digest: AnalysisConfigDigest,
    pub report: CheckReport,
    pub program: CheckedProgram,
    pub files: Vec<AnalyzedFile>,
    use_sites: Vec<UseSite>,
    catalog_declarations: Vec<CatalogDeclaration>,
}

impl AnalysisSnapshot {
    pub fn content_identity(&self) -> &AnalysisIdentity {
        &self.content_identity
    }

    pub fn generation(&self) -> AnalysisGeneration {
        AnalysisGeneration {
            profile_version: ANALYSIS_GENERATION_PROFILE_VERSION,
            content_identity: self.content_identity.clone(),
            config_digest: self.config_digest.clone(),
            checked_source_digest: self.program.source_digest(),
            read_only_context_digest: self.program.read_only_context_digest(),
            accepted_catalog: self.program.catalog.accepted_epoch.map(|epoch| {
                AnalysisCatalogGeneration {
                    epoch,
                    digest: self.program.catalog.accepted_digest.clone(),
                }
            }),
            proposal_catalog: self.program.catalog.proposal.as_ref().map(|proposal| {
                AnalysisCatalogGeneration {
                    epoch: proposal.epoch,
                    digest: Some(proposal.digest.clone()),
                }
            }),
        }
    }

    pub fn surface_read_operations(
        &self,
    ) -> impl Iterator<Item = SurfaceReadOperationAnalysis<'_>> {
        let modules = self.program.facts.modules();
        self.program
            .facts
            .surfaces()
            .iter()
            .flat_map(move |surface| {
                let file = modules
                    .get(surface.module.0 as usize)
                    .map(|module| module.source_file.as_path());
                debug_assert!(
                    file.is_some(),
                    "checked surface module id belongs to the analyzed facts"
                );
                surface.read_operations.iter().filter_map(move |operation| {
                    file.map(|file| SurfaceReadOperationAnalysis {
                        program: &self.program,
                        file,
                        surface,
                        operation,
                    })
                })
            })
    }

    pub fn surface_update_operations(
        &self,
    ) -> impl Iterator<Item = SurfaceUpdateOperationAnalysis<'_>> {
        let modules = self.program.facts.modules();
        self.program
            .facts
            .surfaces()
            .iter()
            .filter(|surface| !surface.update.is_empty())
            .filter_map(move |surface| {
                let file = modules
                    .get(surface.module.0 as usize)
                    .map(|module| module.source_file.as_path());
                debug_assert!(
                    file.is_some(),
                    "checked surface module id belongs to the analyzed facts"
                );
                file.map(|file| SurfaceUpdateOperationAnalysis {
                    program: &self.program,
                    file,
                    surface,
                })
            })
    }

    pub fn surface_create_operations(
        &self,
    ) -> impl Iterator<Item = SurfaceCreateOperationAnalysis<'_>> {
        let modules = self.program.facts.modules();
        self.program
            .facts
            .surfaces()
            .iter()
            .filter(|surface| !surface.create.is_empty())
            .filter_map(move |surface| {
                let file = modules
                    .get(surface.module.0 as usize)
                    .map(|module| module.source_file.as_path());
                debug_assert!(
                    file.is_some(),
                    "checked surface module id belongs to the analyzed facts"
                );
                file.map(|file| SurfaceCreateOperationAnalysis {
                    program: &self.program,
                    file,
                    surface,
                })
            })
    }

    pub fn surface_delete_operations(
        &self,
    ) -> impl Iterator<Item = SurfaceDeleteOperationAnalysis<'_>> {
        let modules = self.program.facts.modules();
        self.program
            .facts
            .surfaces()
            .iter()
            .filter_map(move |surface| {
                let delete = surface.delete.as_ref()?;
                let file = modules
                    .get(surface.module.0 as usize)
                    .map(|module| module.source_file.as_path());
                debug_assert!(
                    file.is_some(),
                    "checked surface module id belongs to the analyzed facts"
                );
                file.map(|file| SurfaceDeleteOperationAnalysis {
                    program: &self.program,
                    file,
                    surface,
                    delete,
                })
            })
    }

    pub fn surface_action_operations(
        &self,
    ) -> impl Iterator<Item = SurfaceActionOperationAnalysis<'_>> {
        let modules = self.program.facts.modules();
        self.program
            .facts
            .surfaces()
            .iter()
            .flat_map(move |surface| {
                let file = modules
                    .get(surface.module.0 as usize)
                    .map(|module| module.source_file.as_path());
                debug_assert!(
                    file.is_some(),
                    "checked surface module id belongs to the analyzed facts"
                );
                surface.actions.iter().filter_map(move |action| {
                    file.map(|file| SurfaceActionOperationAnalysis {
                        program: &self.program,
                        file,
                        surface,
                        action,
                    })
                })
            })
    }

    pub fn surface_computed_read_operations(
        &self,
    ) -> impl Iterator<Item = SurfaceComputedReadOperationAnalysis<'_>> {
        let modules = self.program.facts.modules();
        self.program
            .facts
            .surfaces()
            .iter()
            .flat_map(move |surface| {
                let file = modules
                    .get(surface.module.0 as usize)
                    .map(|module| module.source_file.as_path());
                debug_assert!(
                    file.is_some(),
                    "checked surface module id belongs to the analyzed facts"
                );
                surface
                    .computed_reads
                    .iter()
                    .filter_map(move |computed_read| {
                        file.map(|file| SurfaceComputedReadOperationAnalysis {
                            program: &self.program,
                            file,
                            surface,
                            computed_read,
                        })
                    })
            })
    }

    pub fn use_sites(&self) -> &[UseSite] {
        &self.use_sites
    }

    pub fn sites_for(&self, catalog_id: &str) -> Vec<UseSite> {
        self.use_sites
            .iter()
            .filter(|site| site.catalog_id == catalog_id)
            .cloned()
            .collect()
    }

    pub fn catalog_declarations(&self) -> &[CatalogDeclaration] {
        &self.catalog_declarations
    }

    pub fn catalog_declaration(&self, catalog_id: &str) -> Option<&CatalogDeclaration> {
        self.catalog_declarations
            .iter()
            .find(|declaration| declaration.catalog_id == catalog_id)
    }

    pub fn checked_debug_expression(
        &self,
        file: &Path,
        span: SourceSpan,
        source: &str,
    ) -> Result<CheckedDebugExpression, Vec<CheckDiagnostic>> {
        let Some(parsed) = self
            .files
            .iter()
            .find(|candidate| candidate.path == file)
            .map(|file| &file.parsed)
        else {
            return Err(vec![CheckDiagnostic::error(
                CHECK_READ_ONLY_EXPRESSION_CONTEXT,
                file,
                span,
                "source file is not present in the analysis snapshot",
            )]);
        };
        let Some(debug_source_identity) = self.program.debug_source_identity().cloned() else {
            return Err(vec![CheckDiagnostic::error(
                CHECK_READ_ONLY_EXPRESSION_CONTEXT,
                file,
                span,
                "source program debug identity is not present in the analysis snapshot",
            )]);
        };
        let scope = cursor::debug_expression_scope_before(&self.program, file, parsed, span);
        self.program.checked_debug_expression_in_scope(
            file,
            span,
            source,
            &scope,
            debug_source_identity,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceReadOperationAnalysis<'a> {
    program: &'a CheckedProgram,
    pub file: &'a Path,
    pub surface: &'a SurfaceFact,
    pub operation: &'a SurfaceReadOperationFact,
}

impl SurfaceReadOperationAnalysis<'_> {
    pub fn stable_descriptor(&self) -> Option<SurfaceReadOperationDescriptor> {
        match self.surface.catalog_status {
            SurfaceCatalogStatus::Stable => SurfaceReadOperationDescriptor::from_operation(
                self.program,
                self.surface,
                self.operation,
            ),
            SurfaceCatalogStatus::SourceOnly(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceUpdateOperationAnalysis<'a> {
    program: &'a CheckedProgram,
    pub file: &'a Path,
    pub surface: &'a SurfaceFact,
}

impl SurfaceUpdateOperationAnalysis<'_> {
    pub fn stable_descriptor(&self) -> Option<SurfaceUpdateOperationDescriptor> {
        match self.surface.catalog_status {
            SurfaceCatalogStatus::Stable => {
                SurfaceUpdateOperationDescriptor::from_surface(self.program, self.surface)
            }
            SurfaceCatalogStatus::SourceOnly(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceCreateOperationAnalysis<'a> {
    program: &'a CheckedProgram,
    pub file: &'a Path,
    pub surface: &'a SurfaceFact,
}

impl SurfaceCreateOperationAnalysis<'_> {
    pub fn stable_descriptor(&self) -> Option<SurfaceCreateOperationDescriptor> {
        match self.surface.catalog_status {
            SurfaceCatalogStatus::Stable => {
                SurfaceCreateOperationDescriptor::from_surface(self.program, self.surface)
            }
            SurfaceCatalogStatus::SourceOnly(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceDeleteOperationAnalysis<'a> {
    program: &'a CheckedProgram,
    pub file: &'a Path,
    pub surface: &'a SurfaceFact,
    pub delete: &'a SurfaceDeleteFact,
}

impl SurfaceDeleteOperationAnalysis<'_> {
    pub fn stable_descriptor(&self) -> Option<SurfaceDeleteOperationDescriptor> {
        match self.surface.catalog_status {
            SurfaceCatalogStatus::Stable => {
                SurfaceDeleteOperationDescriptor::from_surface(self.program, self.surface)
            }
            SurfaceCatalogStatus::SourceOnly(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceActionOperationAnalysis<'a> {
    program: &'a CheckedProgram,
    pub file: &'a Path,
    pub surface: &'a SurfaceFact,
    pub action: &'a SurfaceActionFact,
}

impl SurfaceActionOperationAnalysis<'_> {
    pub fn stable_descriptor(&self) -> Option<SurfaceActionOperationDescriptor> {
        match self.surface.catalog_status {
            SurfaceCatalogStatus::Stable => SurfaceActionOperationDescriptor::from_action(
                self.program,
                self.surface,
                self.action,
            ),
            SurfaceCatalogStatus::SourceOnly(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceComputedReadOperationAnalysis<'a> {
    program: &'a CheckedProgram,
    pub file: &'a Path,
    pub surface: &'a SurfaceFact,
    pub computed_read: &'a SurfaceComputedReadFact,
}

impl SurfaceComputedReadOperationAnalysis<'_> {
    pub fn stable_descriptor(&self) -> Option<SurfaceComputedReadOperationDescriptor> {
        match self.surface.catalog_status {
            SurfaceCatalogStatus::Stable => {
                SurfaceComputedReadOperationDescriptor::from_computed_read(
                    self.program,
                    self.surface,
                    self.computed_read,
                )
            }
            SurfaceCatalogStatus::SourceOnly(_) => None,
        }
    }
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
/// (error files included). The accepted catalog is a caller-supplied input —
/// `None` checks the source as a first run — against which durable identity binds.
/// The committed lock is the caller-supplied source-tree projection a first run with
/// no accepted catalog adopts its identity and epoch high-water from; `None` mints a
/// fresh baseline. Fails only when a configured source or test directory cannot be walked.
pub fn analyze_project(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<AnalysisSnapshot, DiscoverError> {
    analyze_project_inner(
        project_root,
        config,
        sources,
        accepted,
        lock,
        CompilerDevAudit::Disabled,
    )
}

/// Analyze a project and append compiler-maintainer diagnostics for unresolved
/// recovery types. This is an internal CLI seam, not an end-user analysis mode.
#[doc(hidden)]
pub fn analyze_project_with_compiler_dev_audit(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<AnalysisSnapshot, DiscoverError> {
    analyze_project_inner(
        project_root,
        config,
        sources,
        accepted,
        lock,
        CompilerDevAudit::UnknownType,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompilerDevAudit {
    Disabled,
    UnknownType,
}

/// Source-only state retained while configured tests temporarily extend the
/// checked program. The maintainer audit keeps the whole semantic snapshot so it
/// can run only after the complete report is known clean; ordinary analysis keeps
/// only the program that must be restored.
enum SourceAnalysisState {
    Program(Box<CheckedProgram>),
    CompilerDevSnapshot(Box<AnalysisSnapshot>),
}

fn analyze_project_inner(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
    lock: Option<&marrow_catalog::CatalogLock>,
    compiler_dev_audit: CompilerDevAudit,
) -> Result<AnalysisSnapshot, DiscoverError> {
    let mut snapshot = analyze_source_project(project_root, config, sources, accepted, lock)?;
    let source_state = match compiler_dev_audit {
        CompilerDevAudit::Disabled => {
            SourceAnalysisState::Program(Box::new(snapshot.program.clone()))
        }
        CompilerDevAudit::UnknownType => {
            SourceAnalysisState::CompilerDevSnapshot(Box::new(snapshot.clone()))
        }
    };
    let resolution_suppression = source_resolution_suppression(&snapshot, project_root, config);
    let test_sources = cached_project_sources(&snapshot, sources);
    let tests = crate::check_tests_with_sources_analysis(
        project_root,
        config,
        &mut snapshot.program,
        &test_sources,
        resolution_suppression,
    )?;
    let test_files = tests
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<HashSet<_>>();
    snapshot.use_sites.extend(catalog_nav::collect_use_sites(
        &snapshot.program,
        &tests.files,
    ));
    catalog_nav::normalize_use_sites(&mut snapshot.use_sites);
    snapshot.report.diagnostics.extend(tests.report.diagnostics);
    snapshot.files.extend(tests.files);
    snapshot.files.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot.files.dedup_by(|a, b| a.path == b.path);
    if !snapshot.report.has_errors()
        && let SourceAnalysisState::CompilerDevSnapshot(source_snapshot) = &source_state
    {
        let mut diagnostics = internal_type_audit::internal_type_issue_diagnostics(source_snapshot);
        if !test_files.is_empty() {
            diagnostics.extend(
                internal_type_audit::internal_type_issue_diagnostics_for_files(
                    &snapshot,
                    &test_files,
                ),
            );
        }
        if !diagnostics.is_empty() {
            snapshot.report.diagnostics.extend(diagnostics);
            snapshot.report.diagnostics.sort_by(|left, right| {
                (
                    &left.file,
                    left.span.start_byte,
                    left.span.end_byte,
                    left.code,
                )
                    .cmp(&(
                        &right.file,
                        right.span.start_byte,
                        right.span.end_byte,
                        right.code,
                    ))
            });
        }
    }
    snapshot.program = match source_state {
        SourceAnalysisState::Program(program) => *program,
        SourceAnalysisState::CompilerDevSnapshot(source_snapshot) => source_snapshot.program,
    };
    snapshot.content_identity = analysis_content_identity(project_root, config, &snapshot.files);
    snapshot.config_digest = analysis_config_digest(config);
    Ok(snapshot)
}

fn cached_project_sources(snapshot: &AnalysisSnapshot, sources: &ProjectSources) -> ProjectSources {
    let mut cached = ProjectSources::new();
    for file in &snapshot.files {
        cached.insert(&file.path, file.source.clone());
    }
    for path in sources.paths() {
        if cached.get(path).is_none()
            && let Some(source) = sources.get(path)
        {
            cached.insert(path, source.to_string());
        }
    }
    cached
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
/// this so configured test files do not block running the checked source program. The
/// accepted catalog is the caller-supplied snapshot durable identity binds against; the
/// committed lock is the first-run adoption source when no accepted catalog is present.
pub(crate) fn analyze_source_project(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
    lock: Option<&marrow_catalog::CatalogLock>,
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
    let mut rejected_surfaces = crate::surface::RejectedSurfaceDeclarations::default();
    let mut backing_invalidations = crate::backing_validity::PendingBackingInvalidations::default();
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
        // `io.read` diagnostic is recorded).
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
            rejected_surfaces: file_rejected_surfaces,
            backing_invalidations: file_backing_invalidations,
        } = check_file_source(&file.path, &source, &mut report.diagnostics);
        rejected_surfaces.extend(file_rejected_surfaces);
        backing_invalidations.extend(file_backing_invalidations);

        // Saved roots are owned project-wide by stores.
        for declaration in &parsed.file.declarations {
            let marrow_syntax::Declaration::Store(store) = declaration else {
                continue;
            };
            match root_owners.get(&store.root.root) {
                Some(first) => {
                    backing_invalidations.record_invalid_root(&store.root.root);
                    report.diagnostics.push(
                        CheckDiagnostic::error(
                            SCHEMA_DUPLICATE_ROOT_OWNER,
                            &file.path,
                            store.span,
                            format!(
                                "saved root `^{}` is already owned by a store in `{}`",
                                store.root.root,
                                first.display()
                            ),
                        )
                        .with_payload(
                            DiagnosticPayload::DuplicateRootOwner {
                                root: store.root.root.clone(),
                                first_owner: first.clone(),
                            },
                        ),
                    );
                }
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
                        report.diagnostics.push(
                            CheckDiagnostic::error(
                                CHECK_DUPLICATE_MODULE,
                                &file.path,
                                module.span,
                                format!(
                                    "module `{expected}` is already declared by `{}`",
                                    first.display()
                                ),
                            )
                            .with_payload(
                                DiagnosticPayload::DuplicateModule {
                                    name: expected.clone(),
                                    first_file: first.clone(),
                                },
                            ),
                        );
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
                    Some(expected.clone()),
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
                    None,
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
    // must declare a `module`. A project with exactly one script joins it normally.
    if scripts.len() <= 1 {
        program.modules.append(&mut scripts);
    } else {
        for script in &scripts {
            report.diagnostics.push(CheckDiagnostic::new(
                Code::CheckMultipleScripts,
                DiagnosticAnchor::whole_file(&script.source_file),
                DiagnosticPayload::None,
                &program.decl_ids(),
            ));
        }
    }

    // Assemble the declaration facts once the whole program is known, so the id
    // tables a nominal signature slot interns against exist before the bind pass
    // reads them. Ids are a function of declaration order, so this assembly and
    // every later rebuild agree by construction.
    program.rebuild_facts_with_sources(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );

    // Bind each named-type signature slot to its true owner, now that the whole
    // program is known, before any pass reads parameter types. Module build left
    // these slots `Unknown`; this is their only writer.
    bind_signature_types(&mut program, &parsed_files);
    crate::keyed_entries::normalize_resource_layers(
        &mut program,
        &parsed_files,
        Some(&mut backing_invalidations),
        &mut report.diagnostics,
    );
    program.rebuild_facts_with_sources(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );

    // Lower bodies before the type pass so its flow narrowing can read each call's
    // effect footprint (a field-writing call expires a narrowing). The later facts
    // rebuild clears these effects, so the downstream passes run on the same state
    // as before; the runtime lowering below re-derives them catalog-aware.
    program.lower_runtime_bodies(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );

    // Passes 2-3 plus unresolved-call suppression are shared with check_tests.
    let incomplete_modules = check_resolved_files(
        ResolvedFileCheck {
            files: &files,
            parsed_files: &parsed_files,
            module_name_policy: ModuleNamePolicy::DeclaredOrPath,
            resolvable: &declared,
            program: &program,
            backing_invalidations: Some(&mut backing_invalidations),
        },
        &mut report,
    );

    program.rebuild_facts_with_sources(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );

    let evolve_intents = crate::evolution::collect_evolve_intents(
        parsed_files.iter().filter_map(|(file, parsed)| {
            parsed_sources
                .get(&file.path)
                .map(|source| (file.path.as_path(), source.as_str(), parsed))
        }),
        &mut report.diagnostics,
        &program.decl_ids(),
    );
    crate::catalog::bind_catalog(
        accepted,
        lock,
        &mut program,
        &evolve_intents,
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
        &mut report.diagnostics,
    );
    if config.store.backend == StoreBackend::Memory {
        crate::catalog::require_durable_store(&program, &mut report.diagnostics);
    }
    let backing_validity = backing_invalidations.resolve(&program);
    crate::surface::check_surfaces(
        &mut program,
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
        &rejected_surfaces,
        &incomplete_modules,
        &backing_validity,
        &mut report.diagnostics,
    );
    crate::evolution::check_evolve_types(
        &program,
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
        &mut report.diagnostics,
    );
    program.lower_runtime_bodies(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );
    crate::evolution::check_transform_effects(&program, &mut report.diagnostics);
    crate::presence::check_next_id_collisions(&mut program, &mut report.diagnostics);
    crate::surface::check_computed_read_effects(&mut program, &mut report.diagnostics);
    let any_parse_errors = parsed_files.iter().any(|(_, parsed)| parsed.has_errors());
    check_default_entry(
        project_root,
        config,
        &program,
        any_parse_errors,
        &mut report.diagnostics,
    );

    // Move every parse — error files included — into the snapshot now that the
    // shared tail has finished borrowing them. The path and module name are
    // copied from each `ModuleFile`; the parse itself moves.
    let analyzed: Vec<AnalyzedFile> = parsed_files
        .into_iter()
        .map(|(file, parsed)| AnalyzedFile {
            path: file.path.clone(),
            module_name: file.module_name.clone(),
            source: parsed_sources.remove(&file.path).unwrap_or_default(),
            parsed,
        })
        .collect();

    let content_identity = analysis_content_identity(project_root, config, &analyzed);
    let config_digest = analysis_config_digest(config);
    program.set_debug_source_identity(source_program_debug_identity(
        project_root,
        config,
        &analyzed,
    ));
    let use_sites = catalog_nav::collect_use_sites(&program, &analyzed);
    let catalog_declarations = catalog_nav::collect_catalog_declarations(&program);

    Ok(AnalysisSnapshot {
        content_identity,
        config_digest,
        report,
        program,
        files: analyzed,
        use_sites,
        catalog_declarations,
    })
}

/// Reject a `run.defaultEntry` that cannot run argument-free. A default entry runs
/// with no arguments, so a missing, private, ambiguous, or parameterized target can
/// only fault at run time; the check fails it up front, spanned at `marrow.json`
/// where the entry is configured.
///
/// A module that failed to parse never enters the program, so a target it would have
/// defined reads as missing, private, or ambiguous purely because of the parse error.
/// Those verdicts are suppressed while any module has parse errors so the developer
/// fixes the parse error first; a `HasParameters` verdict comes from a function that
/// did parse and is reported regardless.
fn check_default_entry(
    project_root: &Path,
    config: &ProjectConfig,
    program: &CheckedProgram,
    any_parse_errors: bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(entry) = config.default_entry.as_deref() else {
        return;
    };
    let Some(problem) = program.default_entry_verdict(entry) else {
        return;
    };
    if any_parse_errors
        && matches!(
            problem,
            DefaultEntryProblem::Missing
                | DefaultEntryProblem::Private
                | DefaultEntryProblem::Ambiguous
        )
    {
        return;
    }
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckDefaultEntry,
        DiagnosticAnchor::whole_file(&project_root.join("marrow.json")),
        DiagnosticPayload::DefaultEntry {
            entry: entry.to_string(),
            problem,
        },
        &program.decl_ids(),
    ));
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

fn analysis_content_identity(
    project_root: &Path,
    config: &ProjectConfig,
    files: &[AnalyzedFile],
) -> AnalysisIdentity {
    let mut digest = Sha256Digest::new();
    digest.update(b"marrow.analysis.content.v1\0");
    hash_config(&mut digest, config);
    hash_analyzed_files(&mut digest, project_root, files);
    AnalysisIdentity(digest.finish())
}

fn analysis_config_digest(config: &ProjectConfig) -> AnalysisConfigDigest {
    let mut digest = Sha256Digest::new();
    digest.update(b"marrow.analysis.config.v1\0");
    hash_config(&mut digest, config);
    AnalysisConfigDigest(digest.finish())
}

fn source_program_debug_identity(
    project_root: &Path,
    config: &ProjectConfig,
    files: &[AnalyzedFile],
) -> DebugSourceIdentity {
    let mut digest = Sha256Digest::new();
    digest.update(b"marrow.debug.source.v1\0");
    hash_source_program_config(&mut digest, config);
    hash_analyzed_files(&mut digest, project_root, files);
    DebugSourceIdentity::from_digest(digest.finish())
}

fn hash_analyzed_files(digest: &mut Sha256Digest, project_root: &Path, files: &[AnalyzedFile]) {
    hash_str(digest, "files.len", &files.len().to_string());
    for file in files {
        let path = file
            .path
            .strip_prefix(project_root)
            .unwrap_or(file.path.as_path());
        hash_str(digest, "file.path", &path_token(path));
        hash_opt_str(digest, "file.module_name", file.module_name.as_deref());
        hash_str(digest, "file.source", &file.source);
    }
}

fn hash_config(digest: &mut Sha256Digest, config: &ProjectConfig) {
    hash_source_program_config(digest, config);
    hash_str_list(digest, "config.tests", &config.tests);
}

fn hash_source_program_config(digest: &mut Sha256Digest, config: &ProjectConfig) {
    hash_str_list(digest, "config.source_roots", &config.source_roots);
    hash_opt_str(
        digest,
        "config.default_entry",
        config.default_entry.as_deref(),
    );
    hash_str(
        digest,
        "config.store.backend",
        match config.store.backend {
            StoreBackend::Memory => "memory",
            StoreBackend::Native => "native",
        },
    );
    hash_opt_str(
        digest,
        "config.store.data_dir",
        config.store.data_dir.as_deref(),
    );
}

fn hash_str_list(digest: &mut Sha256Digest, label: &str, values: &[String]) {
    hash_str(digest, &format!("{label}.len"), &values.len().to_string());
    for value in values {
        hash_str(digest, label, value);
    }
}

fn hash_opt_str(digest: &mut Sha256Digest, label: &str, value: Option<&str>) {
    match value {
        Some(value) => {
            hash_str(digest, label, "some");
            hash_str(digest, label, value);
        }
        None => hash_str(digest, label, "none"),
    }
}

fn hash_str(digest: &mut Sha256Digest, label: &str, value: &str) {
    digest.update(label.as_bytes());
    digest.update(b"\0");
    digest.update(value.len().to_string().as_bytes());
    digest.update(b"\0");
    digest.update(value.as_bytes());
    digest.update(b"\0");
}

fn path_token(path: &Path) -> String {
    let mut token = String::new();
    for component in path.components() {
        if !token.is_empty() {
            token.push('/');
        }
        token.push_str(&component.as_os_str().to_string_lossy());
    }
    token
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    use marrow_project::{ProjectConfig, StoreBackend, StoreConfig};

    use super::analyze_project;
    use crate::ProjectSources;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "marrow-analysis-{name}-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn native_config(data_dir: &str) -> ProjectConfig {
        ProjectConfig {
            source_roots: vec!["src".to_string()],
            default_entry: None,
            store: StoreConfig {
                backend: StoreBackend::Native,
                data_dir: Some(data_dir.to_string()),
            },
            tests: Vec::new(),
            client: None,
        }
    }

    const DURABLE_PROJECT_SOURCE: &str = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        pub fn title(id: int): string\n    \
        return ^books(id).title ?? \"\"\n";

    fn write_durable_project(root: &TempDir) -> ProjectSources {
        let src = root.path().join("src");
        fs::create_dir_all(&src).expect("create src");
        let path = src.join("m.mw");
        fs::write(&path, DURABLE_PROJECT_SOURCE).expect("write source");
        ProjectSources::new().with(path, DURABLE_PROJECT_SOURCE)
    }

    #[test]
    fn analysis_generation_reports_source_config_catalog_and_context_identity() {
        let root = TempDir::new("generation-simple");
        let sources = write_durable_project(&root);
        let config = native_config(".data");

        let snapshot =
            analyze_project(root.path(), &config, &sources, None, None).expect("analyze");
        assert!(
            !snapshot.report.has_errors(),
            "{:#?}",
            snapshot.report.diagnostics
        );
        let generation = snapshot.generation();
        let proposal = snapshot
            .program
            .catalog
            .proposal
            .as_ref()
            .expect("first run proposes durable catalog identity");

        assert_eq!(generation.profile_version, "analysis.generation.v1");
        assert_eq!(
            generation.content_identity,
            snapshot.content_identity().clone()
        );
        assert!(
            generation.config_digest.as_str().starts_with("sha256:"),
            "{generation:#?}"
        );
        assert_eq!(
            generation.checked_source_digest,
            snapshot.program.source_digest()
        );
        assert_eq!(
            generation.read_only_context_digest,
            snapshot.program.read_only_context_digest()
        );
        assert_eq!(generation.accepted_catalog, None);
        assert_eq!(
            generation
                .proposal_catalog
                .as_ref()
                .map(|catalog| catalog.epoch),
            Some(proposal.epoch)
        );
        assert_eq!(
            generation
                .proposal_catalog
                .as_ref()
                .and_then(|catalog| catalog.digest.as_deref()),
            Some(proposal.digest.as_str())
        );
    }

    #[test]
    fn analysis_generation_config_digest_changes_without_changing_checked_source_digest() {
        let root = TempDir::new("generation-config");
        let sources = write_durable_project(&root);
        let first = analyze_project(root.path(), &native_config(".data-a"), &sources, None, None)
            .expect("analyze first config");
        let second = analyze_project(root.path(), &native_config(".data-b"), &sources, None, None)
            .expect("analyze second config");

        assert_ne!(
            first.generation().config_digest,
            second.generation().config_digest
        );
        assert_eq!(
            first.generation().checked_source_digest,
            second.generation().checked_source_digest
        );
    }

    #[test]
    fn analysis_generation_reflects_accepted_and_proposal_catalog_facts() {
        let root = TempDir::new("generation-catalog");
        let sources = write_durable_project(&root);
        let config = native_config(".data");
        let first =
            analyze_project(root.path(), &config, &sources, None, None).expect("analyze first run");
        let proposal = first
            .program
            .catalog
            .proposal
            .clone()
            .expect("first run proposal");

        let accepted = analyze_project(root.path(), &config, &sources, Some(&proposal), None)
            .expect("analyze against accepted catalog");
        let generation = accepted.generation();

        assert_eq!(generation.proposal_catalog, None);
        assert_eq!(
            generation
                .accepted_catalog
                .as_ref()
                .map(|catalog| catalog.epoch),
            Some(proposal.epoch)
        );
        assert_eq!(
            generation
                .accepted_catalog
                .as_ref()
                .and_then(|catalog| catalog.digest.as_deref()),
            Some(proposal.digest.as_str())
        );
    }

    #[test]
    fn source_and_test_analysis_reads_each_unique_disk_file_once() {
        let root = TempDir::new("source-test-cache");
        let src = root.path().join("src");
        let tests = root.path().join("tests");
        fs::create_dir_all(&src).expect("create src");
        fs::create_dir_all(&tests).expect("create tests");
        fs::write(
            src.join("m.mw"),
            "module m\npub fn smoke(): int\n    return 1\n",
        )
        .expect("write source");
        fs::write(
            src.join("extra.mw"),
            "module extra\npub fn value(): int\n    return 2\n",
        )
        .expect("write extra source");
        fs::write(tests.join("smoke.mw"), "fn smoke(): int\n    return 3\n").expect("write test");
        let config = ProjectConfig {
            source_roots: vec!["src".to_string()],
            default_entry: None,
            store: StoreConfig {
                backend: StoreBackend::Memory,
                data_dir: None,
            },
            tests: vec!["src".to_string(), "tests".to_string()],
            client: None,
        };

        crate::driver::reset_source_read_count();
        let snapshot = analyze_project(root.path(), &config, &ProjectSources::new(), None, None)
            .expect("analyze");

        // Overlapping `src` into `tests` re-scans each source module as a test,
        // where its test-path name differs from its declared source module, so the
        // only diagnostics here are those expected module-path mismatches. The
        // caching invariant is the disk read count below.
        assert!(
            snapshot
                .report
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code == "check.module_path"),
            "{:#?}",
            snapshot.report.diagnostics
        );
        assert_eq!(
            crate::driver::source_read_count(),
            3,
            "source and test analysis should read each unique disk file once"
        );
    }
}
