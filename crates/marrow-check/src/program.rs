//! The checked-program artifact built alongside a project's diagnostics.
//!
//! [`check_project`](crate::check_project) builds [`CheckedProgram`] best-effort:
//! it includes a [`CheckedModule`] only for a library file that declared a module,
//! matched its path, is not a duplicate, and parsed without errors.
//! [`check_tests`](crate::check_tests) adds a module per clean test file, named
//! from its path (test files are scripts). Error-bearing files contribute no
//! module. The artifact never affects diagnostics; it is a structured view of the
//! same parse the checker already produced.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use marrow_schema::ReturnPresence;
use marrow_schema::{ScalarType, Type};
use marrow_store::cell::CatalogId;
use marrow_syntax::{Declaration, Diagnostic, Expression, ParsedSource, SourceSpan, TypeRef};

use crate::diagnostics::{
    CHECK_READ_ONLY_EXPRESSION_CONTEXT, CHECK_READ_ONLY_EXPRESSION_HOST_EFFECT,
    CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP, CHECK_READ_ONLY_EXPRESSION_WRITE, CheckDiagnostic,
    DiagnosticPayload,
};
use crate::executable::CheckedBodyVisitor;
use crate::executable::{
    CheckedBody, CheckedExecutableContext, CheckedExpr, CheckedFunctionRef,
    CheckedRuntimeValueType, CheckedStmt, checked_runtime_value_type, walk_checked_body,
    walk_checked_stmt,
};
use crate::facts::{
    CheckedFacts, EffectClosureFacts, EntryCostShapeFact, EntryFootprintFact, EntryStoreOpenMode,
    FunctionId, StoreId, StoreIndexId, WorkShapeClass,
};

/// Identifies one source file in a [`CheckedProgram`] by the index of the module
/// that came from it. A program's modules are 1:1 with their files, so the index
/// is the file's stable id and the program needs no separate file table. A
/// runtime fault stamps the id of the module it was raised in, and a renderer maps
/// it back to a path with [`CheckedProgram::file_path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedReadOnlyExpression {
    file_id: FileId,
    source_file: PathBuf,
    source_digest: String,
    read_only_context_digest: String,
    expression: CheckedExpr,
}

impl CheckedReadOnlyExpression {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn source_file(&self) -> &Path {
        &self.source_file
    }

    pub fn source_digest(&self) -> &str {
        &self.source_digest
    }

    pub fn read_only_context_digest(&self) -> &str {
        &self.read_only_context_digest
    }

    pub fn expression(&self) -> &CheckedExpr {
        &self.expression
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedDebugExpression {
    file_id: FileId,
    source_file: PathBuf,
    source_digest: String,
    read_only_context_digest: String,
    expression: CheckedExpr,
    ty: MarrowType,
}

impl CheckedDebugExpression {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn source_file(&self) -> &Path {
        &self.source_file
    }

    pub fn source_digest(&self) -> &str {
        &self.source_digest
    }

    pub fn read_only_context_digest(&self) -> &str {
        &self.read_only_context_digest
    }

    pub fn expression(&self) -> &CheckedExpr {
        &self.expression
    }

    pub fn ty(&self) -> &MarrowType {
        &self.ty
    }
}

/// A runtime statement span the evaluator can report through `StepHook`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStopPoint {
    pub file_id: FileId,
    pub span: SourceSpan,
}

/// The resolved shape of a checked project: every clean library module, in the
/// order their files were discovered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedProgram {
    pub modules: Vec<CheckedModule>,
    pub facts: CheckedFacts,
    pub catalog: ProgramCatalog,
}

impl CheckedProgram {
    /// Assemble a program directly from modules, rebuilding its facts but capturing
    /// no durable source renderings. Gated behind `test-support` so it never enters a
    /// normal or release build: it exists only to construct the deliberately
    /// uncaptured state the source-digest panic tests assert against, which the
    /// production checker never produces.
    #[cfg(feature = "test-support")]
    pub fn from_modules(modules: Vec<CheckedModule>) -> Self {
        let mut program = Self {
            modules,
            facts: CheckedFacts::default(),
            catalog: ProgramCatalog::default(),
        };
        program.rebuild_facts();
        program
    }

    pub fn checked_read_only_expression(
        &self,
        module: &str,
        source: &str,
    ) -> Result<CheckedReadOnlyExpression, Vec<CheckDiagnostic>> {
        let Some((module_index, module)) = self
            .modules
            .iter()
            .enumerate()
            .find(|(_, candidate)| candidate.name == module)
        else {
            return Err(vec![CheckDiagnostic::error(
                CHECK_READ_ONLY_EXPRESSION_CONTEXT,
                Path::new(""),
                SourceSpan::default(),
                format!("module `{module}` is not present in the checked program"),
            )]);
        };
        let scope = module_constant_scope(module);
        let checked = self.checked_expression_in_scope(module, source, &scope)?;

        Ok(CheckedReadOnlyExpression {
            file_id: FileId(module_index as u32),
            source_file: module.source_file.clone(),
            source_digest: self.source_digest(),
            read_only_context_digest: self.read_only_context_digest(),
            expression: checked.expression,
        })
    }

    pub(crate) fn checked_debug_expression_in_scope(
        &self,
        file: &Path,
        source: &str,
        scope: &[HashMap<String, MarrowType>],
    ) -> Result<CheckedDebugExpression, Vec<CheckDiagnostic>> {
        let Some((module_index, module)) = self
            .modules
            .iter()
            .enumerate()
            .find(|(_, module)| module.source_file == file)
        else {
            return Err(vec![CheckDiagnostic::error(
                CHECK_READ_ONLY_EXPRESSION_CONTEXT,
                file,
                SourceSpan::default(),
                "source file is not present in the checked program",
            )]);
        };
        let checked = self.checked_expression_in_scope(module, source, scope)?;
        Ok(CheckedDebugExpression {
            file_id: FileId(module_index as u32),
            source_file: module.source_file.clone(),
            source_digest: self.source_digest(),
            read_only_context_digest: self.read_only_context_digest(),
            expression: checked.expression,
            ty: checked.ty,
        })
    }

    #[cfg(feature = "test-support")]
    fn rebuild_facts(&mut self) {
        self.facts = CheckedFacts::from_modules(&self.modules, &HashMap::new());
    }

    pub(crate) fn rebuild_facts_with_sources<'a, I>(&mut self, sources: I)
    where
        I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
    {
        let sources: HashMap<PathBuf, &ParsedSource> = sources
            .into_iter()
            .map(|(path, parsed)| (path.to_path_buf(), parsed))
            .collect();
        self.facts = CheckedFacts::from_modules(&self.modules, &sources);
    }

    pub(crate) fn rebuild_facts_with_sources_preserving_current_prefix<'a, I>(&mut self, sources: I)
    where
        I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
    {
        let prefix = std::mem::take(&mut self.facts);
        self.rebuild_facts_with_sources(sources);
        self.facts.overwrite_prefix_from(&prefix);
    }

    pub(crate) fn rebuild_durable_digest_renderings<'a, I>(&mut self, sources: I)
    where
        I: IntoIterator<Item = (&'a Path, &'a str, &'a ParsedSource)>,
    {
        let (captured_modules, renderings) = self.durable_digest_renderings_from_sources(sources);
        self.facts
            .set_durable_digest_renderings(captured_modules, renderings);
    }

    pub(crate) fn extend_durable_digest_renderings<'a, I>(&mut self, sources: I)
    where
        I: IntoIterator<Item = (&'a Path, &'a str, &'a ParsedSource)>,
    {
        let (captured_modules, renderings) = self.durable_digest_renderings_from_sources(sources);
        self.facts
            .extend_durable_digest_renderings(captured_modules, renderings);
    }

    fn durable_digest_renderings_from_sources<'a, I>(
        &self,
        sources: I,
    ) -> (Vec<u32>, Vec<crate::catalog::DurableRendering>)
    where
        I: IntoIterator<Item = (&'a Path, &'a str, &'a ParsedSource)>,
    {
        let sources: HashMap<PathBuf, (&str, &ParsedSource)> = sources
            .into_iter()
            .map(|(path, source, parsed)| (path.to_path_buf(), (source, parsed)))
            .collect();
        let mut captured_modules = Vec::new();
        let mut renderings = Vec::new();
        for (module_index, module) in self.modules.iter().enumerate() {
            let Some(&(source, parsed)) = sources.get(&module.source_file) else {
                continue;
            };
            let module_index = module_index as u32;
            captured_modules.push(module_index);
            renderings.extend(crate::catalog::durable_renderings_for_source(
                module_index,
                &module.name,
                source,
                parsed,
            ));
        }
        (captured_modules, renderings)
    }

    /// The source file the given file id names, or `None` if the id is out of
    /// range (an id from a different program, or a fault with no project file).
    pub fn file_path(&self, id: FileId) -> Option<&Path> {
        self.modules
            .get(id.0 as usize)
            .map(|module| module.source_file.as_path())
    }

    /// The file id of `module`, identifying it by pointer within this program's
    /// own `modules`. Runs only on the cold path where a fault is leaving the
    /// frame that raised it, so the linear scan is off the hot path.
    pub fn file_id_of(&self, module: &CheckedModule) -> Option<FileId> {
        self.modules
            .iter()
            .position(|candidate| std::ptr::eq(candidate, module))
            .map(|index| FileId(index as u32))
    }

    /// The name of the module that declares the resource whose qualified type name is
    /// `resource`, or `None` when no module declares it. An `evolve transform` names the
    /// resource it reshapes by this qualified type name; both body lowering and apply
    /// resolve the owning module through here so the module is never re-derived by
    /// splitting the resource path.
    pub fn owning_module_name(&self, resource: &str) -> Option<&str> {
        self.modules
            .iter()
            .find(|module| {
                module
                    .resources
                    .iter()
                    .any(|res| crate::resource_type_name(&module.name, &res.name) == resource)
            })
            .map(|module| module.name.as_str())
    }

    pub fn runtime(&self) -> CheckedRuntimeProgram {
        CheckedRuntimeProgram::from_checked(self)
    }

    /// The schema-bearing digest of this program's durable shape, in the
    /// `sha256:<hex>` form the store commit metadata records. It binds member types,
    /// identity key types, index shape, enum members, and module constants, so a
    /// structurally different schema produces a different digest even at the same
    /// catalog epoch. It excludes the transient evolve block, so a consumed transition
    /// is deletable without reading as schema drift. The activation fence compares it
    /// against the digest the store recorded. Non-empty programs must be produced by
    /// the checker so their durable source renderings are captured from the in-memory
    /// parse before this digest is requested.
    pub fn source_digest(&self) -> String {
        crate::catalog::analyzed_source_digest(self)
    }

    /// The digest of this program's durable shape *and* its evolve decision surface, in
    /// the same `sha256:<hex>` form. The evolution witness records it so apply aborts
    /// when the source it activates no longer matches what was discharged, including a
    /// transform-body or evolve-default edit the shape digest cannot see. Non-empty
    /// programs must carry the checker-captured source renderings used by
    /// [`CheckedProgram::source_digest`].
    pub fn evolution_digest(&self) -> String {
        crate::catalog::evolution_digest(self)
    }

    pub fn read_only_context_digest(&self) -> String {
        let proposal_digest = self
            .catalog
            .proposal
            .as_ref()
            .map(|proposal| proposal.digest.as_str())
            .unwrap_or("");
        marrow_project::sha256_digest(
            format!(
                "read-only-expression-v1\0{}\0{}\0{}\0{}\0{}\0{}",
                self.source_digest(),
                self.evolution_digest(),
                self.catalog.accepted_epoch.unwrap_or_default(),
                self.catalog.accepted_digest.as_deref().unwrap_or(""),
                self.catalog
                    .proposal
                    .as_ref()
                    .map(|proposal| proposal.epoch)
                    .unwrap_or_default(),
                proposal_digest
            )
            .as_bytes(),
        )
    }

    pub fn effect_closure(&self, function_ref: CheckedFunctionRef) -> Option<EffectClosureFacts> {
        crate::presence::effect_closure(self, function_ref)
    }

    pub fn entry_footprints(&self) -> Vec<EntryFootprintFact> {
        self.public_entry_refs()
            .into_iter()
            .filter_map(|entry_ref| {
                let closure = self.effect_closure(entry_ref.function_ref)?;
                let work_shape = work_shape(&closure);
                Some(EntryFootprintFact {
                    function: entry_ref.function,
                    entry: entry_ref.entry,
                    write_effects_reachable: closure.write_effects_reachable,
                    stores_read: closure.stores_read,
                    stores_written: closure.stores_written,
                    indexes_touched: closure.indexes_touched,
                    work_shape,
                })
            })
            .collect()
    }

    pub fn entry_cost_shapes(&self) -> Vec<EntryCostShapeFact> {
        self.public_entry_refs()
            .into_iter()
            .filter_map(|entry_ref| {
                let closure = self.effect_closure(entry_ref.function_ref)?;
                let work_shape = work_shape(&closure);
                let writes = closure.saved_writes.len() + closure.stores_written.len();
                Some(EntryCostShapeFact {
                    function: entry_ref.function,
                    entry: entry_ref.entry,
                    work_shape,
                    point_reads: closure.saved_reads.len(),
                    range_scans: closure.saved_index_reads.len(),
                    writes,
                    index_entry_touches: if closure.write_effects_reachable {
                        closure.indexes_touched.len()
                    } else {
                        0
                    },
                    commit_points: commit_points(&closure, writes),
                })
            })
            .collect()
    }

    pub fn entry_store_open_mode(
        &self,
        function_ref: CheckedFunctionRef,
    ) -> Option<EntryStoreOpenMode> {
        let closure = self.effect_closure(function_ref)?;
        if self.catalog.accepted_epoch.is_none()
            || self.catalog.proposal.is_some()
            || closure.write_effects_reachable
            || closure.transactions
        {
            return Some(EntryStoreOpenMode::WriteCapable);
        }
        Some(EntryStoreOpenMode::ReadOnly)
    }

    pub fn store_catalog_id(&self, store_id: StoreId) -> Option<&str> {
        let store = self.facts.stores().get(store_id.0 as usize)?;
        if let Some(catalog_id) = store.catalog_id.as_deref() {
            return Some(catalog_id);
        }
        let module = self.facts.modules().get(store.module.0 as usize)?;
        let path = crate::catalog::store_path(&module.name, &store.root);
        self.proposal_catalog_id(marrow_catalog::CatalogEntryKind::Store, &path)
    }

    pub fn store_index_catalog_id(&self, index_id: StoreIndexId) -> Option<&str> {
        let index = self.facts.store_indexes().get(index_id.0 as usize)?;
        if let Some(catalog_id) = index.catalog_id.as_deref() {
            return Some(catalog_id);
        }
        let store = self.facts.store(index.store);
        let module = self.facts.modules().get(store.module.0 as usize)?;
        let path = crate::catalog::store_index_path(&module.name, &store.root, &index.name);
        self.proposal_catalog_id(marrow_catalog::CatalogEntryKind::StoreIndex, &path)
    }

    fn proposal_catalog_id(
        &self,
        kind: marrow_catalog::CatalogEntryKind,
        path: &str,
    ) -> Option<&str> {
        crate::catalog::active_program_proposal_id(self, kind, path)
    }

    pub(crate) fn lower_runtime_bodies<'a, I>(&mut self, sources: I)
    where
        I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
    {
        let sources: HashMap<PathBuf, &ParsedSource> = sources
            .into_iter()
            .map(|(path, parsed)| (path.to_path_buf(), parsed))
            .collect();
        let function_bodies = lower_function_bodies(self, &sources);
        for lowered in function_bodies {
            if let Some(function) = self
                .modules
                .get_mut(lowered.module_index)
                .and_then(|module| module.functions.get_mut(lowered.function_index))
            {
                function.runtime_body = lowered.body;
            }
        }
        let transform_bodies = lower_transform_bodies(self, &sources);
        for (transform, body) in self
            .catalog
            .evolve_transforms
            .iter_mut()
            .zip(transform_bodies)
        {
            transform.runtime_body = body;
        }
        self.facts.refresh_direct_effects(&self.modules);
    }
}

/// The names a module's constants bind in scope, mapped to their checked types.
/// The single owner of the constant-scope shape that body lowering and read-only
/// expression checking both build their scope stack from.
fn module_constant_map(module: &CheckedModule) -> HashMap<String, MarrowType> {
    module
        .constants
        .iter()
        .map(|constant| {
            (
                constant.name.clone(),
                constant.ty.clone().unwrap_or(MarrowType::Unknown),
            )
        })
        .collect()
}

fn module_constant_scope(module: &CheckedModule) -> Vec<HashMap<String, MarrowType>> {
    vec![module_constant_map(module)]
}

struct CheckedExpressionInScope {
    expression: CheckedExpr,
    ty: MarrowType,
}

impl CheckedProgram {
    fn checked_expression_in_scope(
        &self,
        module: &CheckedModule,
        source: &str,
        scope: &[HashMap<String, MarrowType>],
    ) -> Result<CheckedExpressionInScope, Vec<CheckDiagnostic>> {
        let (parsed, syntax_diagnostics) = marrow_syntax::parse_expression(source);
        let mut diagnostics: Vec<CheckDiagnostic> = syntax_diagnostics
            .into_iter()
            .map(|diagnostic| syntax_expression_diagnostic(&module.source_file, diagnostic))
            .collect();
        let Some(parsed) = parsed else {
            return Err(diagnostics);
        };

        let aliases = crate::build_alias_map(&module.imports);
        let ty = crate::infer::infer_type(
            self,
            &parsed,
            scope,
            &aliases,
            &module.source_file,
            &mut diagnostics,
        );
        crate::checks::check_entries_value_position(&module.source_file, &parsed, &mut diagnostics);
        let Some(expression) =
            crate::executable::lower_expr_for_file(self, &module.source_file, &parsed, scope)
        else {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_READ_ONLY_EXPRESSION_CONTEXT,
                &module.source_file,
                parsed.span(),
                "expression cannot be lowered in the checked program context",
            ));
            return Err(diagnostics);
        };

        let read_only_effects = crate::presence::read_only_expression_effects(self, &expression);
        diagnostics.extend(read_only_expression_diagnostics(
            &module.source_file,
            &expression,
            &read_only_effects,
        ));
        if diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.severity, marrow_syntax::Severity::Error))
        {
            return Err(diagnostics);
        }

        Ok(CheckedExpressionInScope { expression, ty })
    }
}

fn syntax_expression_diagnostic(file: &Path, diagnostic: Diagnostic) -> CheckDiagnostic {
    CheckDiagnostic {
        code: diagnostic.code,
        severity: diagnostic.severity,
        file: file.to_path_buf(),
        message: diagnostic.message,
        span: diagnostic.span,
        payload: DiagnosticPayload::None,
    }
}

fn read_only_expression_diagnostics(
    file: &Path,
    expression: &CheckedExpr,
    effects: &crate::presence::ReadOnlyExpressionEffects,
) -> Vec<CheckDiagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(span) = effects
        .saved_write_span
        .or_else(|| effects.writes_reachable().then_some(expression.span()))
    {
        diagnostics.push(CheckDiagnostic::error(
            CHECK_READ_ONLY_EXPRESSION_WRITE,
            file,
            span,
            "checked read-only expressions cannot write saved data, allocate saved identities, or open transactions",
        ));
    }
    if effects.host_effects_reachable() {
        diagnostics.push(CheckDiagnostic::error(
            CHECK_READ_ONLY_EXPRESSION_HOST_EFFECT,
            file,
            expression.span(),
            "checked read-only expressions cannot call host-effecting operations",
        ));
    }
    if let Some(span) = effects.unindexed_lookup_span.or_else(|| {
        effects
            .unindexed_collection_reads_reachable()
            .then_some(expression.span())
    }) {
        diagnostics.push(CheckDiagnostic::error(
            CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP,
            file,
            span,
            "checked read-only expressions cannot traverse saved collections without a declared index",
        ));
    }
    diagnostics
}

struct LoweredFunctionBody {
    module_index: usize,
    function_index: usize,
    body: Option<CheckedBody>,
}

fn lower_function_bodies(
    program: &CheckedProgram,
    sources: &HashMap<PathBuf, &ParsedSource>,
) -> Vec<LoweredFunctionBody> {
    let mut bodies = Vec::new();
    for (module_index, module) in program.modules.iter().enumerate() {
        let constants = module_constant_map(module);
        let Some(parsed) = sources.get(&module.source_file).copied() else {
            continue;
        };
        let context = CheckedExecutableContext::new(program, module_index);
        // The checked functions are built in source order, one per function
        // declaration, so they zip positionally with the parse's function
        // declarations. Matching by name would attach the wrong body to the
        // second duplicate-named function.
        let declarations: Vec<_> = parsed
            .file
            .declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::Function(function) => Some(function),
                _ => None,
            })
            .collect();
        assert_eq!(
            context.module_name(),
            module.name,
            "checked executable context/module alignment diverged"
        );
        assert_eq!(
            module.functions.len(),
            declarations.len(),
            "checked function/declaration count diverged for module {}",
            module.name
        );
        for ((function_index, function), declaration) in
            module.functions.iter().enumerate().zip(declarations)
        {
            let mut scope = vec![constants.clone()];
            scope.push(
                function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.ty.clone()))
                    .collect(),
            );
            assert_eq!(
                function.name, declaration.name,
                "checked function/declaration body alignment diverged"
            );
            bodies.push(LoweredFunctionBody {
                module_index,
                function_index,
                body: CheckedBody::lower(&declaration.body, &context, scope),
            });
        }
    }
    bodies
}

/// Lower each `evolve transform` body with `old` bound to the owning resource's
/// type, so apply can execute it per record through the runtime evaluator. The body
/// is read back from the parse (the checked program carries no syntax body) and
/// lowered against its owning module's executable context, with the module constants
/// and the single `old` resource binding in scope. The type pass in
/// `check_evolve_types` has already proven the body well-typed, so a body that fails
/// to lower stays `None` and discharge does not classify it applyable.
fn lower_transform_bodies(
    program: &CheckedProgram,
    sources: &HashMap<PathBuf, &ParsedSource>,
) -> Vec<Option<CheckedBody>> {
    program
        .catalog
        .evolve_transforms
        .iter()
        .map(|transform| lower_transform_body(program, sources, transform))
        .collect()
}

/// Lower one transform body against its owning module, with the module constants and
/// `old: <resource>` in scope. Returns `None` when the owning module cannot be located,
/// the body cannot be read back from the parse, or the body does not lower.
fn lower_transform_body(
    snapshot: &CheckedProgram,
    sources: &HashMap<PathBuf, &ParsedSource>,
    transform: &EvolveTransform,
) -> Option<CheckedBody> {
    let owning = snapshot.owning_module_name(&transform.resource)?;
    let module_index = snapshot
        .modules
        .iter()
        .position(|module| module.name == owning)?;
    let module = &snapshot.modules[module_index];
    let parsed = sources.get(&transform.file).copied()?;
    let body =
        crate::evolution::transform_body_in_source(parsed, &module.name, &transform.target_path)?;
    let constants = module_constant_map(module);
    let context = CheckedExecutableContext::new(snapshot, module_index);
    let old_scope = HashMap::from([(
        "old".to_string(),
        MarrowType::Resource(transform.resource.clone()),
    )]);
    CheckedBody::lower(body, &context, vec![constants, old_scope])
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgramCatalog {
    pub accepted_epoch: Option<u64>,
    pub accepted_digest: Option<String>,
    /// The accepted catalog entries the snapshot was computed against. Discharge
    /// diffs these against current source to find a source-dropped or retired entry
    /// the proposal alone, when unchanged, would not surface.
    pub accepted_entries: Vec<marrow_catalog::CatalogEntry>,
    /// The constant fill values an `evolve default` step supplies, keyed by the
    /// member's stable catalog id. Discharge evaluates each value to a typed fill so
    /// a newly-required member is defaultable rather than a fail-closed data
    /// attachment; the source digest binds the normalized value expressions.
    pub evolve_defaults: Vec<EvolveDefault>,
    /// The transform obligations an `evolve transform` step declares, keyed by the
    /// member's stable catalog id. Discharge classifies each as an applyable
    /// transform obligation once its body passed the checker, carrying the read
    /// members and the lowered body apply executes per record.
    pub evolve_transforms: Vec<EvolveTransform>,
    /// The current source's identity-key shape token for each store, keyed by stable catalog
    /// id. The token is the comma-joined key types in order, so discharge compares it against
    /// the accepted shape to detect a key arity or key-type change. Like the structural
    /// signatures it comes from source, so a re-key is detected even for an otherwise-unchanged
    /// program.
    pub declared_store_key_shapes: HashMap<String, String>,
    /// The current source's identity-aware structural signature for each resource member,
    /// keyed by stable catalog id. The signature records the member's kind, its key shape if
    /// keyed, and its leaf token if a leaf, so discharge compares it against the accepted
    /// signature to fail closed on any structural divergence — a keyed-layer re-key, a
    /// group<->keyed-group reshape, or an unforeseen transition — that no targeted classifier
    /// already covers. It is the in-memory baseline discharge reads through the signature's
    /// single decoder, and it comes from source, so the divergence is detected even for an
    /// otherwise-unchanged program that emits no proposal.
    pub declared_member_structs: HashMap<String, String>,
    pub(crate) ambiguous_source_keys: HashSet<crate::catalog::CatalogKey>,
    pub proposal: Option<marrow_catalog::CatalogMetadata>,
}

/// A bound `evolve default`: the member's stable catalog id and the constant value
/// expression to backfill old records with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolveDefault {
    pub catalog_id: String,
    pub value: marrow_syntax::Expression,
}

/// A bound `evolve transform`: the target member's stable catalog id, the read members
/// the body names via `old.<member>` (their stable catalog ids), the resource's
/// qualified type name (used to type the `old` binding and to locate the owning module
/// through [`CheckedProgram::owning_module_name`]), the source file and the target's
/// module-qualified catalog path (used to find the body in the parse), the body span
/// (used to report a purity violation), and the lowered body apply executes.
/// `runtime_body` is the body lowered with `old` in scope, filled in by
/// [`CheckedProgram::lower_runtime_bodies`] from the parse; like a function's runtime
/// body it is not a public syntax-construction bridge, so it is read through
/// [`EvolveTransform::runtime_body`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolveTransform {
    pub catalog_id: Option<String>,
    pub reads: Vec<CatalogId>,
    pub resource: String,
    pub file: PathBuf,
    pub target_path: String,
    pub body_span: SourceSpan,
    pub(crate) runtime_body: Option<CheckedBody>,
}

impl EvolveTransform {
    /// The lowered transform body, or `None` when it has not been lowered (the body did
    /// not lower, or no parse was available). Apply executes it per affected record.
    pub fn runtime_body(&self) -> Option<&CheckedBody> {
        self.runtime_body.as_ref()
    }
}

/// One library module: its qualified name, the file it came from, and the
/// declarations it contributes. Names within a module are kept in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedModule {
    /// The qualified module name, such as `shelf::books`.
    pub name: String,
    pub source_file: PathBuf,
    pub span: SourceSpan,
    /// Resolved `use` target names, in source order.
    pub imports: Vec<String>,
    pub constants: Vec<CheckedConst>,
    pub functions: Vec<CheckedFunction>,
    pub resources: Vec<marrow_schema::ResourceSchema>,
    pub stores: Vec<marrow_schema::StoreSchema>,
    pub enums: Vec<marrow_schema::EnumSchema>,
    pub enum_public: HashMap<String, bool>,
}

/// A module-level constant. Its type is the resolved annotation when one was
/// written; an unannotated constant leaves it `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedConst {
    pub name: String,
    pub ty: Option<MarrowType>,
    pub value: Option<Expression>,
    pub span: SourceSpan,
}

/// A checked function: its resolved signature and checked executable body for
/// runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedFunction {
    pub name: String,
    pub public: bool,
    pub params: Vec<CheckedParam>,
    pub return_presence: ReturnPresence,
    pub return_type: Option<MarrowType>,
    pub span: SourceSpan,
    pub(crate) runtime_body: Option<CheckedBody>,
}

impl CheckedFunction {
    pub fn runtime_body(&self) -> Option<&CheckedBody> {
        self.runtime_body.as_ref()
    }
}

/// One resolved function parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedParam {
    pub name: String,
    pub ty: MarrowType,
}

/// Syntax-free program artifact for production execution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedRuntimeProgram {
    modules: Vec<CheckedRuntimeModule>,
    entry_functions: HashMap<String, CheckedEntryFunction>,
    private_entry_functions: HashSet<String>,
    facts: CheckedFacts,
    accepted_catalog_ids: HashSet<String>,
    accepted_catalog_epoch: Option<u64>,
    source_digest: String,
    read_only_context_digest: String,
}

impl CheckedRuntimeProgram {
    pub fn from_checked(program: &CheckedProgram) -> Self {
        let modules: Vec<CheckedRuntimeModule> = program
            .modules
            .iter()
            .enumerate()
            .map(|(module_index, module)| {
                CheckedRuntimeModule::from_checked(program, module_index, module)
            })
            .collect();
        let (entry_functions, private_entry_functions) = runtime_entry_functions(&modules);
        Self {
            modules,
            entry_functions,
            private_entry_functions,
            facts: program.facts.clone(),
            accepted_catalog_ids: program
                .catalog
                .accepted_entries
                .iter()
                .map(|entry| entry.stable_id.clone())
                .collect(),
            accepted_catalog_epoch: program.catalog.accepted_epoch,
            source_digest: program.source_digest(),
            read_only_context_digest: program.read_only_context_digest(),
        }
    }

    pub fn entry_function_ref(&self, entry: &str) -> CheckedEntryFunction {
        self.entry_functions
            .get(entry)
            .copied()
            .or_else(|| {
                self.private_entry_functions
                    .contains(entry)
                    .then_some(CheckedEntryFunction::Private)
            })
            .unwrap_or(CheckedEntryFunction::Missing)
    }

    pub fn modules(&self) -> &[CheckedRuntimeModule] {
        &self.modules
    }

    pub fn facts(&self) -> &CheckedFacts {
        &self.facts
    }

    pub fn accepted_catalog_epoch(&self) -> Option<u64> {
        self.accepted_catalog_epoch
    }

    pub fn has_accepted_catalog_id(&self, catalog_id: &str) -> bool {
        self.accepted_catalog_ids.contains(catalog_id)
    }

    pub fn source_digest(&self) -> &str {
        &self.source_digest
    }

    pub fn read_only_context_digest(&self) -> &str {
        &self.read_only_context_digest
    }

    pub fn file_path(&self, id: FileId) -> Option<&Path> {
        self.modules
            .get(id.0 as usize)
            .map(|module| module.source_file.as_path())
    }

    pub fn file_id_of(&self, module: &CheckedRuntimeModule) -> Option<FileId> {
        self.modules
            .iter()
            .position(|candidate| std::ptr::eq(candidate, module))
            .map(|index| FileId(index as u32))
    }

    pub fn stop_points(&self) -> Vec<RuntimeStopPoint> {
        let mut points = Vec::new();
        for (module_index, module) in self.modules.iter().enumerate() {
            let file_id = FileId(module_index as u32);
            for function in module.functions() {
                if let Some(body) = function.body() {
                    let mut collector = RuntimeStopPointCollector {
                        file_id,
                        points: &mut points,
                    };
                    walk_checked_body(&mut collector, body);
                }
            }
        }
        points
    }
}

struct RuntimeStopPointCollector<'a> {
    file_id: FileId,
    points: &'a mut Vec<RuntimeStopPoint>,
}

impl CheckedBodyVisitor for RuntimeStopPointCollector<'_> {
    fn visit_stmt(&mut self, statement: &CheckedStmt) {
        self.points.push(RuntimeStopPoint {
            file_id: self.file_id,
            span: statement.span(),
        });
        walk_checked_stmt(self, statement);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedEntryFunction {
    Found(CheckedFunctionRef),
    Ambiguous,
    Private,
    Missing,
}

fn runtime_entry_functions(
    modules: &[CheckedRuntimeModule],
) -> (HashMap<String, CheckedEntryFunction>, HashSet<String>) {
    let mut entries = HashMap::new();
    let mut private_entries = HashSet::new();
    for (module_index, module) in modules.iter().enumerate() {
        for (function_index, function) in module.functions.iter().enumerate() {
            let function_ref = CheckedFunctionRef {
                module: module_index as u32,
                function: function_index as u32,
                presence: function.return_presence,
            };
            let qualified = runtime_entry_name(&module.name, &function.name);
            if !function.public {
                private_entries.insert(qualified);
                continue;
            }
            entries.insert(qualified.clone(), CheckedEntryFunction::Found(function_ref));
            if qualified != function.name {
                insert_bare_entry(&mut entries, &function.name, function_ref);
            }
        }
    }
    (entries, private_entries)
}

fn insert_bare_entry(
    entries: &mut HashMap<String, CheckedEntryFunction>,
    name: &str,
    function_ref: CheckedFunctionRef,
) {
    match entries.entry(name.to_string()) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(CheckedEntryFunction::Found(function_ref));
        }
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            if entry.get() != &CheckedEntryFunction::Found(function_ref) {
                entry.insert(CheckedEntryFunction::Ambiguous);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublicEntryRef {
    function: FunctionId,
    entry: String,
    function_ref: CheckedFunctionRef,
}

impl CheckedProgram {
    fn public_entry_refs(&self) -> Vec<PublicEntryRef> {
        self.facts
            .functions()
            .iter()
            .filter(|function| function.public)
            .filter_map(|function| {
                let source = self
                    .modules
                    .get(function.module.0 as usize)?
                    .functions
                    .get(function.source_index as usize)?;
                let module = self.facts.modules().get(function.module.0 as usize)?;
                Some(PublicEntryRef {
                    function: function.id,
                    entry: runtime_entry_name(&module.name, &function.name),
                    function_ref: CheckedFunctionRef {
                        module: function.module.0,
                        function: function.source_index,
                        presence: source.return_presence,
                    },
                })
            })
            .collect()
    }
}

fn runtime_entry_name(module: &str, function: &str) -> String {
    if module.is_empty() {
        return function.to_string();
    }
    format!("{module}::{function}")
}

fn work_shape(closure: &EffectClosureFacts) -> WorkShapeClass {
    if closure.write_effects_reachable {
        WorkShapeClass::WritesSavedData
    } else if !closure.stores_read.is_empty()
        || !closure.saved_index_reads.is_empty()
        || !closure.host_calls.is_empty()
    {
        WorkShapeClass::ReadOnly
    } else {
        WorkShapeClass::ComputeOnly
    }
}

fn commit_points(closure: &EffectClosureFacts, writes: usize) -> usize {
    if !closure.write_effects_reachable {
        0
    } else if closure.transactions {
        1
    } else {
        writes.max(1)
    }
}

impl From<&CheckedProgram> for CheckedRuntimeProgram {
    fn from(program: &CheckedProgram) -> Self {
        Self::from_checked(program)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedRuntimeModule {
    pub name: String,
    pub source_file: PathBuf,
    pub span: SourceSpan,
    pub constants: Vec<CheckedRuntimeConst>,
    functions: Vec<CheckedRuntimeFunction>,
}

impl CheckedRuntimeModule {
    fn from_checked(program: &CheckedProgram, module_index: usize, module: &CheckedModule) -> Self {
        let context = CheckedExecutableContext::new(program, module_index);
        Self {
            name: module.name.clone(),
            source_file: module.source_file.clone(),
            span: module.span,
            constants: module
                .constants
                .iter()
                .map(|constant| CheckedRuntimeConst::from_checked(constant, &context))
                .collect(),
            functions: module
                .functions
                .iter()
                .map(|function| CheckedRuntimeFunction::from_checked(program, function))
                .collect(),
        }
    }

    pub fn functions(&self) -> &[CheckedRuntimeFunction] {
        &self.functions
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedRuntimeConst {
    pub name: String,
    pub ty: Option<MarrowType>,
    pub value: Option<CheckedExpr>,
    pub span: SourceSpan,
}

impl CheckedRuntimeConst {
    fn from_checked(constant: &CheckedConst, context: &CheckedExecutableContext<'_>) -> Self {
        Self {
            name: constant.name.clone(),
            ty: constant.ty.clone(),
            value: constant.value.as_ref().and_then(|value| {
                let mut scope = Vec::new();
                CheckedExpr::lower(value, context, &mut scope)
            }),
            span: constant.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedRuntimeFunction {
    pub name: String,
    pub public: bool,
    pub params: Vec<CheckedParam>,
    entry_params: Vec<CheckedRuntimeParam>,
    entry_return_type: Option<CheckedRuntimeValueType>,
    pub return_presence: ReturnPresence,
    pub return_type: Option<MarrowType>,
    pub span: SourceSpan,
    body: Option<CheckedBody>,
}

impl CheckedRuntimeFunction {
    fn from_checked(program: &CheckedProgram, function: &CheckedFunction) -> Self {
        Self {
            name: function.name.clone(),
            public: function.public,
            params: function.params.clone(),
            entry_params: function
                .params
                .iter()
                .map(|param| CheckedRuntimeParam::from_checked(program, param))
                .collect(),
            entry_return_type: function
                .return_type
                .clone()
                .map(|ty| checked_runtime_value_type(program, ty)),
            return_presence: function.return_presence,
            return_type: function.return_type.clone(),
            span: function.span,
            body: function.runtime_body.clone(),
        }
    }

    pub fn body(&self) -> Option<&CheckedBody> {
        self.body.as_ref()
    }

    pub fn entry_params(&self) -> &[CheckedRuntimeParam] {
        &self.entry_params
    }

    pub fn entry_return_type(&self) -> Option<&CheckedRuntimeValueType> {
        self.entry_return_type.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedRuntimeParam {
    pub name: String,
    pub ty: CheckedRuntimeValueType,
}

impl CheckedRuntimeParam {
    fn from_checked(program: &CheckedProgram, param: &CheckedParam) -> Self {
        Self {
            name: param.name.clone(),
            ty: checked_runtime_value_type(program, param.ty.clone()),
        }
    }
}

/// A resolved Marrow type, best-effort. Anything the checker cannot resolve
/// (including cross-module resource references) is [`MarrowType::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarrowType {
    /// One of the storable scalar types.
    Primitive(ScalarType),
    /// The checker-only type of a caught or thrown error value (`catch e: Error`,
    /// `throw Error(...)`). It has no storage form and never resolves to a scalar.
    Error,
    /// A resource by canonical module-qualified name, or bare name for a
    /// module-less script.
    Resource(String),
    /// A saved keyed-group entry, identified by its owning resource and group
    /// layer chain.
    GroupEntry {
        resource: String,
        layers: Vec<String>,
    },
    /// A store identity such as `Id(^books)`, carrying the store root.
    Identity(String),
    /// An enum, identified by its owning module and bare name. Identity is
    /// module-qualified: a bare `Status` referenced in module `b` resolves to
    /// `b::Status` (same-module first), and two same-named enums in different
    /// modules never alias. Nominal: an enum value equals only a value of the
    /// same enum.
    Enum {
        module: String,
        name: String,
    },
    Sequence(Box<MarrowType>),
    LocalTree {
        keys: Vec<MarrowType>,
        value: Box<MarrowType>,
    },
    /// An expression whose own type check already produced a primary diagnostic.
    /// It suppresses secondary "untyped value" hints while still keeping unknown
    /// dynamic values distinct.
    Invalid,
    Unknown,
}

/// The module's enum names, used while resolving annotations with module-owned
/// enum identity. Resource names resolve through the checked module-aware
/// resolver instead.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TypeNames<'a> {
    /// The qualified name of the module these names belong to, so a bare enum
    /// annotation resolves to that module's enum (`module::name` identity). Empty
    /// for a module-less script, whose enums are project-unique by construction.
    pub module: &'a str,
    pub enums: &'a [String],
}

impl MarrowType {
    /// Resolve a [`TypeRef`] against the named types declared in the same module.
    /// Best-effort and total: it never errors, falling back to
    /// [`MarrowType::Unknown`] for anything it cannot place.
    pub(crate) fn resolve(ty: &TypeRef, names: TypeNames<'_>) -> Self {
        Self::from_resolved(Type::resolve(ty), names)
    }

    /// Promote a schema-resolved [`Type`] to the checker's lattice using the
    /// module's enum names. The structure (scalar, sequence, identity, `unknown`)
    /// is already decided; this layer only places a bare [`Type::Named`] as an enum
    /// reference, the checker-only `Error` type, or `Unknown`.
    pub(crate) fn from_resolved(ty: Type, names: TypeNames<'_>) -> Self {
        match ty {
            Type::Scalar(scalar) => Self::Primitive(scalar),
            Type::Sequence(element) => {
                Self::Sequence(Box::new(Self::from_resolved(*element, names)))
            }
            Type::Identity(root) => Self::Identity(root),
            Type::Unknown => Self::Unknown,
            // `Error` is the one checker-only type the store does not model, so it
            // never resolves to a scalar; recognize it here.
            Type::Named(name) if name == "Error" => Self::Error,
            // A bare enum annotation names the owning module's enum.
            Type::Named(name) if names.enums.contains(&name) => Self::Enum {
                module: names.module.to_string(),
                name,
            },
            Type::Named(_) => Self::Unknown,
        }
    }
}
