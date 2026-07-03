use std::collections::HashSet;
use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_syntax::{
    Declaration, ParsedSource, SourceSpan, SurfaceDecl, SurfaceItem, SurfaceTarget,
};

use crate::backing_validity::BackingValidity;
use crate::diagnostics::{
    SurfaceActionDiagnostic, SurfaceComputedReadDiagnostic, SurfaceFieldDiagnostic,
    SurfaceFieldList, SurfaceFieldProblem, SurfaceRootOrigin, SurfaceTargetDiagnostic,
};
use crate::entry_abi::{
    ComputedReadSignatureUnsupported, EntrySignatureUnsupported,
    function_ref_has_accepted_computed_read_catalog_ids,
    function_ref_has_accepted_entry_catalog_ids,
    function_ref_has_supported_computed_read_signature, function_ref_has_supported_entry_signature,
};
use crate::executable::CheckedFunctionRef;
use crate::facts::{
    ModuleId, ResourceMemberFact, ResourceMemberId, ResourceMemberKind, StoreFact, StoreIndexFact,
    StoreIndexId, StoreIndexKeyFact, StoreIndexKeySource, StoredValueMeaning, SurfaceActionFact,
    SurfaceCatalogBlocker, SurfaceCatalogStatus, SurfaceCollectionFact, SurfaceCollectionTarget,
    SurfaceComputedReadFact, SurfaceDeleteFact, SurfaceFact, SurfaceFieldFact, SurfaceId,
    SurfaceIndexRangeCollection, SurfaceReadFootprint, SurfaceReadOperationFact,
    SurfaceReadOperationKind,
};
use crate::presence::transitive_unindexed_lookup_span;
use crate::surface_abi::surface_read_operation_tag;
use crate::{
    CheckDiagnostic, CheckedProgram, Def, DefItem, DiagnosticAnchor, DiagnosticPayload, Resolution,
    ResolvableKind, build_alias_map, expand_alias, resolve,
};

/// Surface declarations suppressed before surface checking because an earlier
/// declaration or item collision already made the generated API invalid.
#[derive(Debug, Clone, Default)]
pub(crate) struct RejectedSurfaceDeclarations {
    entries: Vec<RejectedSurfaceDeclaration>,
}

#[derive(Debug, Clone)]
struct RejectedSurfaceDeclaration {
    file: PathBuf,
    span: SourceSpan,
}

impl RejectedSurfaceDeclarations {
    pub(crate) fn reject(&mut self, file: &Path, span: SourceSpan) {
        if self.contains(file, span) {
            return;
        }
        self.entries.push(RejectedSurfaceDeclaration {
            file: file.to_path_buf(),
            span,
        });
    }

    pub(crate) fn extend(&mut self, other: Self) {
        for entry in other.entries {
            self.reject(&entry.file, entry.span);
        }
    }

    fn contains(&self, file: &Path, span: SourceSpan) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.file == file && entry.span == span)
    }
}

pub(crate) fn check_surfaces<'a, I>(
    program: &mut CheckedProgram,
    sources: I,
    rejected_surfaces: &RejectedSurfaceDeclarations,
    incomplete_modules: &HashSet<String>,
    backing_validity: &BackingValidity,
    diagnostics: &mut Vec<CheckDiagnostic>,
) where
    I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
{
    let surface_facts = {
        let mut checker = SurfaceChecker {
            program,
            rejected_surfaces,
            incomplete_modules,
            backing_validity,
            diagnostics,
            surface_facts: Vec::new(),
        };
        checker.check_sources(sources);
        checker.surface_facts
    };
    program.facts.set_surfaces(surface_facts);
}

pub(crate) fn check_computed_read_effects(
    program: &mut CheckedProgram,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let mut retained = Vec::new();
    for surface in program.facts.surfaces().to_vec() {
        let file = program
            .facts
            .modules()
            .get(surface.module.0 as usize)
            .map(|module| module.source_file.clone());
        let mut rejected = false;
        for read in &surface.computed_reads {
            if let Some(payload) =
                computed_read_effect_diagnostic(program, read.function, &read.path)
            {
                rejected = true;
                if let Some(file) = file.as_deref() {
                    let span = computed_read_effect_span(program, read, &payload);
                    push_surface_computed_read_diagnostic(file, span, payload, diagnostics);
                }
            }
        }
        if !rejected {
            retained.push(surface);
        }
    }
    for (index, surface) in retained.iter_mut().enumerate() {
        surface.id = SurfaceId(index as u32);
    }
    program.facts.set_surfaces(retained);
}

struct SurfaceChecker<'a> {
    program: &'a CheckedProgram,
    rejected_surfaces: &'a RejectedSurfaceDeclarations,
    incomplete_modules: &'a HashSet<String>,
    backing_validity: &'a BackingValidity,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
    surface_facts: Vec<SurfaceFact>,
}

impl<'a> SurfaceChecker<'a> {
    fn check_sources<'s, I>(&mut self, sources: I)
    where
        I: IntoIterator<Item = (&'s Path, &'s ParsedSource)>,
    {
        for (file, parsed) in sources {
            self.check_file(file, parsed);
        }
    }

    fn check_file(&mut self, file: &Path, parsed: &ParsedSource) {
        let Some(module) = module_for_file(self.program, file) else {
            return;
        };
        for surface in surface_declarations(parsed) {
            self.check_surface_decl(module, file, surface);
        }
    }

    fn check_surface_decl(&mut self, module: ModuleId, file: &Path, surface: &SurfaceDecl) {
        if self.rejected_surfaces.contains(file, surface.span) {
            return;
        }
        let diagnostic_start = self.diagnostics.len();
        let Some(store) = resolve_backing_store(
            self.program,
            file,
            surface,
            self.backing_validity,
            self.diagnostics,
        ) else {
            return;
        };

        let (fields, create, update) = self.resolve_fields(file, store, surface);
        let delete = resolve_delete(surface);
        let collections = resolve_collections(
            self.program,
            file,
            store,
            surface,
            self.backing_validity,
            self.diagnostics,
        );
        let mut suppressed_function_target = false;
        let Some(action_module) = checked_module_for_id(self.program, module) else {
            return;
        };
        let function_context = SurfaceFunctionContext {
            program: self.program,
            file,
            module_name: &action_module.name,
            imports: &action_module.imports,
            incomplete_modules: self.incomplete_modules,
        };
        let actions = resolve_actions(
            function_context,
            surface,
            &mut suppressed_function_target,
            self.diagnostics,
        );
        let computed_reads = resolve_computed_reads(
            function_context,
            surface,
            &mut suppressed_function_target,
            self.diagnostics,
        );
        self.reject_invalid_backing_resource(file, surface, store, diagnostic_start);
        if suppressed_function_target || self.diagnostics.len() != diagnostic_start {
            return;
        }

        let id = SurfaceId(self.surface_facts.len() as u32);
        let catalog_status = catalog_status(
            self.program,
            SurfaceCatalogInputs {
                store,
                fields: &fields,
                create: &create,
                update: &update,
                collections: &collections,
                actions: &actions,
                computed_reads: &computed_reads,
            },
        );
        let read_operations = read_operations(
            self.program,
            store,
            surface.span,
            &fields,
            &collections,
            &catalog_status,
        );
        self.surface_facts.push(SurfaceFact {
            id,
            module,
            name: surface.name.clone(),
            store: store.id,
            fields,
            create,
            update,
            delete,
            collections,
            actions,
            computed_reads,
            read_operations,
            catalog_status,
            span: surface.span,
        });
    }

    fn resolve_fields(
        &mut self,
        file: &Path,
        store: &StoreFact,
        surface: &SurfaceDecl,
    ) -> (
        Vec<SurfaceFieldFact>,
        Vec<SurfaceFieldFact>,
        Vec<SurfaceFieldFact>,
    ) {
        let field_context = SurfaceFieldContext {
            program: self.program,
            file,
            store,
            backing_validity: self.backing_validity,
        };
        let fields = resolve_field_list(
            field_context,
            surface,
            SurfaceFieldList::Fields,
            self.diagnostics,
        );
        let projected: HashSet<ResourceMemberId> =
            fields.iter().map(|field| field.member).collect();
        let create = resolve_input_field_list(
            field_context,
            surface,
            SurfaceFieldList::Create,
            &projected,
            self.diagnostics,
        );
        let update = resolve_input_field_list(
            field_context,
            surface,
            SurfaceFieldList::Update,
            &projected,
            self.diagnostics,
        );
        validate_create_completeness(field_context, surface, &create, self.diagnostics);
        (fields, create, update)
    }

    fn reject_invalid_backing_resource(
        &mut self,
        file: &Path,
        surface: &SurfaceDecl,
        store: &StoreFact,
        diagnostic_start: usize,
    ) {
        if self.diagnostics.len() == diagnostic_start
            && self
                .backing_validity
                .resource_is_invalid(self.program, store.resource)
        {
            push_invalid_store_resource_diagnostic(
                file,
                surface.store.span,
                &surface.name,
                store,
                self.program,
                self.diagnostics,
            );
        }
    }
}

fn checked_module_for_id(
    program: &CheckedProgram,
    module: ModuleId,
) -> Option<&crate::CheckedModule> {
    let module_name = &program.facts.modules().get(module.0 as usize)?.name;
    program.module_by_name(module_name)
}

fn resolve_actions(
    context: SurfaceFunctionContext<'_>,
    surface: &SurfaceDecl,
    suppressed_target: &mut bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Vec<SurfaceActionFact> {
    surface
        .items
        .iter()
        .filter_map(|item| match item {
            SurfaceItem::Action {
                function,
                function_span,
                alias,
                span,
            } => resolve_action(
                context,
                function,
                alias,
                *span,
                *function_span,
                suppressed_target,
                diagnostics,
            ),
            _ => None,
        })
        .collect()
}

fn resolve_computed_reads(
    context: SurfaceFunctionContext<'_>,
    surface: &SurfaceDecl,
    suppressed_target: &mut bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Vec<SurfaceComputedReadFact> {
    surface
        .items
        .iter()
        .filter_map(|item| match item {
            SurfaceItem::Read {
                function,
                function_span,
                alias,
                span,
            } => resolve_computed_read(
                context,
                function,
                alias,
                *span,
                *function_span,
                suppressed_target,
                diagnostics,
            ),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy)]
struct SurfaceFunctionContext<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    module_name: &'a str,
    imports: &'a [String],
    incomplete_modules: &'a HashSet<String>,
}

#[derive(Clone, Copy)]
enum SurfaceFunctionProfile {
    Action,
    ComputedRead,
}

struct ResolvedSurfaceFunction {
    path: String,
    function: CheckedFunctionRef,
}

fn resolve_action(
    context: SurfaceFunctionContext<'_>,
    function_path: &[String],
    alias: &str,
    span: SourceSpan,
    target_span: SourceSpan,
    suppressed_target: &mut bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<SurfaceActionFact> {
    let target = resolve_surface_function(
        context,
        SurfaceFunctionProfile::Action,
        function_path,
        target_span,
        suppressed_target,
        diagnostics,
    )?;

    if let Err(issue) = function_ref_has_supported_entry_signature(context.program, target.function)
    {
        let payload = match issue {
            EntrySignatureUnsupported::Parameter { name } => {
                SurfaceActionDiagnostic::UnsupportedParameter {
                    path: target.path.clone(),
                    parameter: name.clone(),
                }
            }
            EntrySignatureUnsupported::ReturnValue => SurfaceActionDiagnostic::UnsupportedReturn {
                path: target.path.clone(),
            },
        };
        push_surface_action_diagnostic(context.file, target_span, payload, diagnostics);
        return None;
    }

    Some(SurfaceActionFact {
        alias: alias.to_string(),
        function: target.function,
        span,
    })
}

fn resolve_computed_read(
    context: SurfaceFunctionContext<'_>,
    function_path: &[String],
    alias: &str,
    span: SourceSpan,
    target_span: SourceSpan,
    suppressed_target: &mut bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<SurfaceComputedReadFact> {
    let target = resolve_surface_function(
        context,
        SurfaceFunctionProfile::ComputedRead,
        function_path,
        target_span,
        suppressed_target,
        diagnostics,
    )?;

    if let Err(issue) =
        function_ref_has_supported_computed_read_signature(context.program, target.function)
    {
        let payload = match issue {
            ComputedReadSignatureUnsupported::Parameter { name } => {
                SurfaceComputedReadDiagnostic::UnsupportedParameter {
                    path: target.path.clone(),
                    parameter: name.clone(),
                }
            }
            ComputedReadSignatureUnsupported::ReturnValue => {
                SurfaceComputedReadDiagnostic::UnsupportedReturn {
                    path: target.path.clone(),
                }
            }
        };
        push_surface_computed_read_diagnostic(context.file, target_span, payload, diagnostics);
        return None;
    }

    Some(SurfaceComputedReadFact {
        alias: alias.to_string(),
        path: target.path,
        function: target.function,
        span,
        target_span,
    })
}

fn resolve_surface_function(
    context: SurfaceFunctionContext<'_>,
    profile: SurfaceFunctionProfile,
    function_path: &[String],
    span: SourceSpan,
    suppressed_target: &mut bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<ResolvedSurfaceFunction> {
    let expanded_path = expand_surface_function_path(context.imports, function_path);
    let path = expanded_path.join("::");
    match resolve(
        context.program,
        context.module_name,
        function_path,
        ResolvableKind::Function,
    ) {
        Resolution::Found(Def {
            module,
            module_index,
            item: DefItem::Function(function),
            ..
        }) => {
            if !function.public {
                push_surface_function_diagnostic(
                    profile,
                    context.file,
                    span,
                    SurfaceFunctionTargetDiagnostic::Private { path: path.clone() },
                    diagnostics,
                );
                return None;
            }
            let function = crate::executable::function_ref(module_index, module, function)
                .or_else(|| {
                    *suppressed_target = true;
                    None
                })?;
            Some(ResolvedSurfaceFunction { path, function })
        }
        Resolution::NotVisible(name) => {
            push_surface_function_diagnostic(
                profile,
                context.file,
                span,
                SurfaceFunctionTargetDiagnostic::Private { path: name.clone() },
                diagnostics,
            );
            None
        }
        Resolution::Ambiguous(_) => {
            push_surface_function_diagnostic(
                profile,
                context.file,
                span,
                SurfaceFunctionTargetDiagnostic::Ambiguous { path },
                diagnostics,
            );
            None
        }
        Resolution::Found(_) | Resolution::Unresolved => {
            if references_incomplete_function_module(&expanded_path, context.incomplete_modules) {
                *suppressed_target = true;
                return None;
            }
            push_surface_function_diagnostic(
                profile,
                context.file,
                span,
                SurfaceFunctionTargetDiagnostic::Unknown { path },
                diagnostics,
            );
            None
        }
    }
}

enum SurfaceFunctionTargetDiagnostic {
    Private { path: String },
    Ambiguous { path: String },
    Unknown { path: String },
}

fn push_surface_function_diagnostic(
    profile: SurfaceFunctionProfile,
    file: &Path,
    span: SourceSpan,
    issue: SurfaceFunctionTargetDiagnostic,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match profile {
        SurfaceFunctionProfile::Action => {
            let payload = match issue {
                SurfaceFunctionTargetDiagnostic::Private { path } => {
                    SurfaceActionDiagnostic::PrivateFunction { path }
                }
                SurfaceFunctionTargetDiagnostic::Ambiguous { path } => {
                    SurfaceActionDiagnostic::AmbiguousFunction { path }
                }
                SurfaceFunctionTargetDiagnostic::Unknown { path } => {
                    SurfaceActionDiagnostic::UnknownFunction { path }
                }
            };
            push_surface_action_diagnostic(file, span, payload, diagnostics);
        }
        SurfaceFunctionProfile::ComputedRead => {
            let payload = match issue {
                SurfaceFunctionTargetDiagnostic::Private { path } => {
                    SurfaceComputedReadDiagnostic::PrivateFunction { path }
                }
                SurfaceFunctionTargetDiagnostic::Ambiguous { path } => {
                    SurfaceComputedReadDiagnostic::AmbiguousFunction { path }
                }
                SurfaceFunctionTargetDiagnostic::Unknown { path } => {
                    SurfaceComputedReadDiagnostic::UnknownFunction { path }
                }
            };
            push_surface_computed_read_diagnostic(file, span, payload, diagnostics);
        }
    }
}

/// The span a computed-read effect diagnostic points at. An unindexed-collection rejection points
/// at the offending traversal site inside the read's function body (or a transitive callee), so
/// the developer fixes the loop rather than the surface declaration that names the read. Every
/// other rejection anchors at the read's target token, matching the resolution and signature
/// variants.
fn computed_read_effect_span(
    program: &CheckedProgram,
    read: &SurfaceComputedReadFact,
    payload: &SurfaceComputedReadDiagnostic,
) -> SourceSpan {
    match payload {
        SurfaceComputedReadDiagnostic::UnindexedCollectionRead { .. } => {
            transitive_unindexed_lookup_span(program, read.function).unwrap_or(read.target_span)
        }
        _ => read.target_span,
    }
}

fn computed_read_effect_diagnostic(
    program: &CheckedProgram,
    function: CheckedFunctionRef,
    path: &str,
) -> Option<SurfaceComputedReadDiagnostic> {
    let closure = program.effect_closure(function)?;
    if closure.write_effects_reachable {
        return Some(SurfaceComputedReadDiagnostic::Writes {
            path: path.to_string(),
        });
    }
    if closure.transactions {
        return Some(SurfaceComputedReadDiagnostic::Transactions {
            path: path.to_string(),
        });
    }
    if !closure.host_calls.is_empty() {
        return Some(SurfaceComputedReadDiagnostic::HostEffects {
            path: path.to_string(),
        });
    }
    if closure.throws {
        return Some(SurfaceComputedReadDiagnostic::Throws {
            path: path.to_string(),
        });
    }
    if closure.unindexed_collection_reads {
        return Some(SurfaceComputedReadDiagnostic::UnindexedCollectionRead {
            path: path.to_string(),
        });
    }
    None
}

fn expand_surface_function_path(imports: &[String], function_path: &[String]) -> Vec<String> {
    let aliases = build_alias_map(imports);
    expand_alias(function_path, &aliases)
}

fn references_incomplete_function_module(
    function_path: &[String],
    incomplete_modules: &HashSet<String>,
) -> bool {
    if function_path.len() < 2 {
        return false;
    }
    let path = function_path.join("::");
    incomplete_modules.iter().any(|module| {
        path.strip_prefix(module)
            .is_some_and(|rest| rest.starts_with("::"))
    })
}

fn push_surface_action_diagnostic(
    file: &Path,
    span: SourceSpan,
    payload: SurfaceActionDiagnostic,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceAction,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceAction(payload),
    ));
}

fn push_surface_computed_read_diagnostic(
    file: &Path,
    span: SourceSpan,
    payload: SurfaceComputedReadDiagnostic,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceComputedRead,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceComputedRead(payload),
    ));
}

fn read_operations(
    program: &CheckedProgram,
    store: &StoreFact,
    surface_span: SourceSpan,
    fields: &[SurfaceFieldFact],
    collections: &[SurfaceCollectionFact],
    catalog_status: &SurfaceCatalogStatus,
) -> Vec<SurfaceReadOperationFact> {
    let projection = fields.iter().map(|field| field.member).collect::<Vec<_>>();
    let footprint = SurfaceReadFootprint::FullRecord {
        resource: store.resource,
    };
    let stable_tags = matches!(catalog_status, SurfaceCatalogStatus::Stable);
    let mut operations = Vec::with_capacity(collections.len() + 1);
    let backing_kind = backing_read_operation_kind(store);
    operations.push(surface_read_operation_fact(
        "get".to_string(),
        backing_kind,
        footprint,
        projection.clone(),
        stable_read_operation_tag(
            program,
            store,
            backing_kind,
            footprint,
            &projection,
            stable_tags,
        ),
        surface_span,
    ));
    operations.extend(collections.iter().map(|collection| {
        let kind = collection_read_operation_kind(program, collection);
        surface_read_operation_fact(
            collection.alias.clone(),
            kind,
            footprint,
            projection.clone(),
            stable_read_operation_tag(program, store, kind, footprint, &projection, stable_tags),
            collection.span,
        )
    }));
    operations
}

fn surface_read_operation_fact(
    alias: String,
    kind: SurfaceReadOperationKind,
    footprint: SurfaceReadFootprint,
    projection: Vec<ResourceMemberId>,
    operation_tag: Option<String>,
    span: SourceSpan,
) -> SurfaceReadOperationFact {
    SurfaceReadOperationFact {
        alias,
        kind,
        footprint,
        operation_tag,
        projection,
        span,
    }
}

fn stable_read_operation_tag(
    program: &CheckedProgram,
    store: &StoreFact,
    kind: SurfaceReadOperationKind,
    footprint: SurfaceReadFootprint,
    projection: &[ResourceMemberId],
    stable_tags: bool,
) -> Option<String> {
    stable_tags
        .then(|| surface_read_operation_tag(program, store, kind, footprint, projection))
        .flatten()
}

fn backing_read_operation_kind(store: &StoreFact) -> SurfaceReadOperationKind {
    if store.identity_keys.is_empty() {
        SurfaceReadOperationKind::SingletonRead { store: store.id }
    } else {
        SurfaceReadOperationKind::PointRead { store: store.id }
    }
}

fn collection_read_operation_kind(
    program: &CheckedProgram,
    collection: &SurfaceCollectionFact,
) -> SurfaceReadOperationKind {
    match collection.target {
        SurfaceCollectionTarget::StoreRoot(store) => {
            SurfaceReadOperationKind::PagedRootCollection { store }
        }
        SurfaceCollectionTarget::StoreIndex(index) => {
            let fact = program.facts.store_index(index);
            if fact.unique {
                SurfaceReadOperationKind::UniqueIndexLookup {
                    index,
                    key_count: fact.keys.len(),
                }
            } else {
                let store = program.facts.store(fact.store);
                // Schema validation already rejects non-unique indexes that do
                // not end with the complete store identity; this keeps the fact
                // derivation fail-closed if an invalid fact reaches this layer.
                let identity_key_count = full_identity_suffix_count(store, fact);
                SurfaceReadOperationKind::PagedIndexCollection {
                    index,
                    exact_key_count: fact.keys.len().saturating_sub(identity_key_count),
                    identity_key_count,
                }
            }
        }
        SurfaceCollectionTarget::StoreIndexRange(range) => {
            SurfaceReadOperationKind::PagedIndexRangeCollection {
                index: range.index,
                exact_key_count: range.exact_key_count,
                range_key_index: range.range_key_index,
                identity_key_count: range.identity_key_count,
            }
        }
    }
}

fn full_identity_suffix_count(store: &StoreFact, index: &StoreIndexFact) -> usize {
    let identity_len = store.identity_keys.len();
    if index.keys.len() < identity_len {
        return 0;
    }
    let Some(suffix) = index
        .keys
        .get(index.keys.len().saturating_sub(identity_len)..)
    else {
        return 0;
    };
    let matches_store_identity =
        suffix
            .iter()
            .zip(&store.identity_keys)
            .all(|(index_key, identity_key)| {
                index_key.source == StoreIndexKeySource::IdentityKey
                    && index_key.name == identity_key.name
            });
    if matches_store_identity {
        identity_len
    } else {
        0
    }
}

fn surface_declarations(parsed: &ParsedSource) -> impl Iterator<Item = &SurfaceDecl> {
    parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Surface(surface) => Some(surface),
            _ => None,
        })
}

fn module_for_file(program: &CheckedProgram, file: &Path) -> Option<ModuleId> {
    program
        .module_index_by_file(file)
        .map(|index| ModuleId(index as u32))
}

enum StoreRootResolution<'p> {
    Missing,
    Unique(&'p StoreFact),
    Ambiguous,
}

fn resolve_unique_store_root<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> StoreRootResolution<'p> {
    let mut matches = program
        .facts
        .stores()
        .iter()
        .filter(|store| store.root == root);
    let Some(store) = matches.next() else {
        return StoreRootResolution::Missing;
    };
    if matches.next().is_some() {
        return StoreRootResolution::Ambiguous;
    }
    StoreRootResolution::Unique(store)
}

fn resolve_surface_store_root<'p>(
    program: &'p CheckedProgram,
    file: &Path,
    span: SourceSpan,
    root: &str,
    backing_validity: &BackingValidity,
    origin: SurfaceRootOrigin,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<&'p StoreFact> {
    match resolve_unique_store_root(program, root) {
        StoreRootResolution::Unique(store) => {
            if backing_validity.store_has_duplicate_root(store) {
                push_ambiguous_store_root_diagnostic(file, span, origin, root, diagnostics);
                None
            } else {
                Some(store)
            }
        }
        StoreRootResolution::Missing => {
            push_unknown_store_root_diagnostic(file, span, origin, root, diagnostics);
            None
        }
        StoreRootResolution::Ambiguous => {
            push_ambiguous_store_root_diagnostic(file, span, origin, root, diagnostics);
            None
        }
    }
}

fn push_unknown_store_root_diagnostic(
    file: &Path,
    span: SourceSpan,
    origin: SurfaceRootOrigin,
    root: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceTarget,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::UnknownStore {
            origin,
            root: root.to_string(),
        }),
    ));
}

fn push_ambiguous_store_root_diagnostic(
    file: &Path,
    span: SourceSpan,
    origin: SurfaceRootOrigin,
    root: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceTarget,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::AmbiguousStore {
            origin,
            root: root.to_string(),
        }),
    ));
}

fn resolve_backing_store<'p>(
    program: &'p CheckedProgram,
    file: &Path,
    surface: &SurfaceDecl,
    backing_validity: &BackingValidity,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<&'p StoreFact> {
    let store = resolve_surface_store_root(
        program,
        file,
        surface.store.span,
        &surface.store.root,
        backing_validity,
        SurfaceRootOrigin::Surface {
            name: surface.name.clone(),
        },
        diagnostics,
    )?;

    if backing_validity.store_is_invalid(store) {
        push_invalid_store_diagnostic(
            file,
            surface.store.span,
            &surface.name,
            &surface.store.root,
            diagnostics,
        );
        return None;
    }

    if store_resource_is_ambiguous(program, store) {
        let resource = program.facts.resource(store.resource);
        diagnostics.push(CheckDiagnostic::new(
            Code::CheckSurfaceTarget,
            DiagnosticAnchor::at(file, surface.store.span),
            DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::AmbiguousStoreResource {
                surface: surface.name.clone(),
                root: surface.store.root.clone(),
                resource: resource.name.clone(),
            }),
        ));
        None
    } else {
        Some(store)
    }
}

fn store_resource_is_ambiguous(program: &CheckedProgram, store: &StoreFact) -> bool {
    let resource = program.facts.resource(store.resource);
    program
        .facts
        .resources()
        .iter()
        .filter(|candidate| candidate.module == resource.module && candidate.name == resource.name)
        .nth(1)
        .is_some()
}

fn push_invalid_store_diagnostic(
    file: &Path,
    span: SourceSpan,
    surface_name: &str,
    root: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceTarget,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::InvalidStore {
            surface: surface_name.to_string(),
            root: root.to_string(),
        }),
    ));
}

fn push_invalid_store_resource_diagnostic(
    file: &Path,
    span: SourceSpan,
    surface_name: &str,
    store: &StoreFact,
    program: &CheckedProgram,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let resource = program.facts.resource(store.resource);
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceTarget,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::InvalidStoreResource {
            surface: surface_name.to_string(),
            root: store.root.clone(),
            resource: resource.name.clone(),
        }),
    ));
}

#[derive(Clone, Copy)]
struct SurfaceFieldContext<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    store: &'a StoreFact,
    backing_validity: &'a BackingValidity,
}

fn resolve_field_list(
    context: SurfaceFieldContext<'_>,
    surface: &SurfaceDecl,
    list: SurfaceFieldList,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Vec<SurfaceFieldFact> {
    let mut fields = Vec::new();
    for item in &surface.items {
        let Some((names, name_spans)) = (match (list, item) {
            (
                SurfaceFieldList::Fields,
                SurfaceItem::Fields {
                    names, name_spans, ..
                },
            )
            | (
                SurfaceFieldList::Create,
                SurfaceItem::Create {
                    names, name_spans, ..
                },
            )
            | (
                SurfaceFieldList::Update,
                SurfaceItem::Update {
                    names, name_spans, ..
                },
            ) => Some((names, name_spans)),
            _ => None,
        }) else {
            continue;
        };
        for (name, span) in names.iter().zip(name_spans) {
            if let Some(field) = resolve_surface_field(context, list, name, *span, diagnostics) {
                fields.push(field);
            }
        }
    }
    fields
}

fn resolve_delete(surface: &SurfaceDecl) -> Option<SurfaceDeleteFact> {
    surface.items.iter().find_map(|item| match item {
        SurfaceItem::Delete { span } => Some(SurfaceDeleteFact { span: *span }),
        _ => None,
    })
}

fn validate_create_completeness(
    context: SurfaceFieldContext<'_>,
    surface: &SurfaceDecl,
    create: &[SurfaceFieldFact],
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if create.is_empty() {
        return;
    }
    let create_members = create
        .iter()
        .map(|field| field.member)
        .collect::<HashSet<_>>();
    let span = create
        .first()
        .map(|field| field.span)
        .unwrap_or(surface.span);
    for member in context.program.facts.resource_members() {
        if member.resource != context.store.resource || member.plain_field_required != Some(true) {
            continue;
        }
        if create_members.contains(&member.id) {
            continue;
        }
        if member.parent.is_none() {
            push_field_diagnostic(
                context.file,
                span,
                SurfaceFieldList::Create,
                &member.name,
                SurfaceFieldProblem::RequiredNotCreateAddressable,
                diagnostics,
            );
            continue;
        }
        if path_is_inside_unkeyed_record(context.program, member.id) {
            let name = member_path_label(context.program, member.id);
            push_field_diagnostic(
                context.file,
                span,
                SurfaceFieldList::Create,
                &name,
                SurfaceFieldProblem::RequiredNotCreateAddressable,
                diagnostics,
            );
        }
    }
}

fn path_is_inside_unkeyed_record(program: &CheckedProgram, member_id: ResourceMemberId) -> bool {
    let mut current = Some(member_id);
    while let Some(id) = current {
        let Some(member) = program.facts.resource_members().get(id.0 as usize) else {
            return false;
        };
        if member.key_count != 0 {
            return false;
        }
        current = member.parent;
    }
    true
}

fn member_path_label(program: &CheckedProgram, member_id: ResourceMemberId) -> String {
    let mut names = Vec::new();
    let mut current = Some(member_id);
    while let Some(id) = current {
        let Some(member) = program.facts.resource_members().get(id.0 as usize) else {
            break;
        };
        names.push(member.name.as_str());
        current = member.parent;
    }
    names.reverse();
    names.join(".")
}

fn resolve_input_field_list(
    context: SurfaceFieldContext<'_>,
    surface: &SurfaceDecl,
    list: SurfaceFieldList,
    projected: &HashSet<ResourceMemberId>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Vec<SurfaceFieldFact> {
    resolve_field_list(context, surface, list, diagnostics)
        .into_iter()
        .filter(|field| {
            if projected.contains(&field.member) {
                return true;
            }
            push_field_diagnostic(
                context.file,
                field.span,
                list,
                &field.name,
                SurfaceFieldProblem::NotProjected,
                diagnostics,
            );
            false
        })
        .collect()
}

fn resolve_surface_field(
    context: SurfaceFieldContext<'_>,
    list: SurfaceFieldList,
    name: &str,
    span: SourceSpan,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<SurfaceFieldFact> {
    match plain_top_level_field(context, name) {
        Ok(member) => Some(SurfaceFieldFact {
            name: name.to_string(),
            member,
            span,
        }),
        Err(problem) => {
            push_field_diagnostic(context.file, span, list, name, problem, diagnostics);
            None
        }
    }
}

fn plain_top_level_field(
    context: SurfaceFieldContext<'_>,
    name: &str,
) -> Result<ResourceMemberId, SurfaceFieldProblem> {
    let member = unique_top_level_member(context.program, context.store, name)?;
    if member.kind != ResourceMemberKind::Field || member.plain_field_required.is_none() {
        return Err(SurfaceFieldProblem::Unsupported);
    }
    if member.value_meaning.is_none()
        || context
            .backing_validity
            .field_is_invalid(context.program, member)
    {
        return Err(SurfaceFieldProblem::Invalid);
    }
    Ok(member.id)
}

fn unique_top_level_member<'p>(
    program: &'p CheckedProgram,
    store: &StoreFact,
    name: &str,
) -> Result<&'p ResourceMemberFact, SurfaceFieldProblem> {
    let mut matches = program.facts.resource_members().iter().filter(|member| {
        member.resource == store.resource && member.parent.is_none() && member.name == name
    });
    let Some(member) = matches.next() else {
        if store.identity_keys.iter().any(|key| key.name == name) {
            return Err(SurfaceFieldProblem::IdentityKey);
        }
        return Err(SurfaceFieldProblem::Unknown);
    };
    if matches.next().is_some() {
        return Err(SurfaceFieldProblem::Ambiguous);
    }
    Ok(member)
}

fn resolve_collections(
    program: &CheckedProgram,
    file: &Path,
    store: &StoreFact,
    surface: &SurfaceDecl,
    backing_validity: &BackingValidity,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Vec<SurfaceCollectionFact> {
    surface
        .items
        .iter()
        .filter_map(|item| match item {
            SurfaceItem::Collection {
                target,
                alias,
                span,
            } => resolve_collection(
                program,
                file,
                store,
                CollectionItem {
                    target,
                    alias,
                    span: *span,
                },
                backing_validity,
                diagnostics,
            ),
            _ => None,
        })
        .collect()
}

struct CollectionItem<'a> {
    target: &'a SurfaceTarget,
    alias: &'a str,
    span: SourceSpan,
}

fn resolve_collection(
    program: &CheckedProgram,
    file: &Path,
    store: &StoreFact,
    item: CollectionItem<'_>,
    backing_validity: &BackingValidity,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<SurfaceCollectionFact> {
    let target_span = item.target.span();
    let target = match item.target {
        SurfaceTarget::Root { root, .. } => resolve_root_collection(
            program,
            file,
            store,
            root,
            target_span,
            backing_validity,
            diagnostics,
        )?,
        SurfaceTarget::Index { root, index, .. } => resolve_index_collection(
            CollectionResolveContext {
                program,
                file,
                store,
                backing_validity,
                diagnostics,
            },
            root,
            index,
            target_span,
        )?,
        SurfaceTarget::IndexRange { root, index, .. } => resolve_index_range_collection(
            CollectionResolveContext {
                program,
                file,
                store,
                backing_validity,
                diagnostics,
            },
            root,
            index,
            target_span,
        )?,
    };
    Some(SurfaceCollectionFact {
        alias: item.alias.to_string(),
        target,
        span: item.span,
    })
}

fn resolve_root_collection(
    program: &CheckedProgram,
    file: &Path,
    store: &StoreFact,
    root: &str,
    span: SourceSpan,
    backing_validity: &BackingValidity,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<SurfaceCollectionTarget> {
    if root != store.root {
        push_foreign_unknown_or_ambiguous_root(
            program,
            file,
            span,
            &store.root,
            root,
            backing_validity,
            diagnostics,
        );
        return None;
    }
    if store.identity_keys.is_empty() {
        push_keyless_collection_root_diagnostic(file, span, root, diagnostics);
        return None;
    }
    Some(SurfaceCollectionTarget::StoreRoot(store.id))
}

struct CollectionResolveContext<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    store: &'a StoreFact,
    backing_validity: &'a BackingValidity,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
}

fn resolve_index_collection(
    mut context: CollectionResolveContext<'_>,
    root: &str,
    index: &str,
    span: SourceSpan,
) -> Option<SurfaceCollectionTarget> {
    let index_id = resolve_collection_index_id(&mut context, root, index, span)?;
    validate_collection_index_backing(&mut context, root, index, span, index_id)?;
    Some(SurfaceCollectionTarget::StoreIndex(index_id))
}

fn resolve_index_range_collection(
    mut context: CollectionResolveContext<'_>,
    root: &str,
    index: &str,
    span: SourceSpan,
) -> Option<SurfaceCollectionTarget> {
    let index_id = resolve_collection_index_id(&mut context, root, index, span)?;
    let index_fact = context.program.facts.store_index(index_id);
    let range = range_collection_shape(context.store, index_fact).map_err(|problem| {
        push_invalid_range_collection_index_diagnostic(
            context.file,
            span,
            root,
            index,
            index_fact
                .keys
                .get(problem.key_index())
                .map(|key| key.name.as_str()),
            problem,
            context.diagnostics,
        );
    });
    if range.is_ok() {
        validate_collection_index_backing(&mut context, root, index, span, index_id)?;
    }
    Some(SurfaceCollectionTarget::StoreIndexRange(range.ok()?))
}

fn resolve_collection_index_id(
    context: &mut CollectionResolveContext<'_>,
    root: &str,
    index: &str,
    span: SourceSpan,
) -> Option<StoreIndexId> {
    if root != context.store.root {
        push_foreign_unknown_or_ambiguous_root(
            context.program,
            context.file,
            span,
            &context.store.root,
            root,
            context.backing_validity,
            context.diagnostics,
        );
        return None;
    }
    let index_id = match unique_store_index(context.program, context.store, index) {
        StoreIndexResolution::Unique(index_id) => index_id,
        StoreIndexResolution::Missing => {
            push_unknown_collection_index_diagnostic(
                context.file,
                span,
                root,
                index,
                context.diagnostics,
            );
            return None;
        }
        StoreIndexResolution::Ambiguous => {
            push_ambiguous_collection_index_diagnostic(
                context.file,
                span,
                root,
                index,
                context.diagnostics,
            );
            return None;
        }
    };
    Some(index_id)
}

fn validate_collection_index_backing(
    context: &mut CollectionResolveContext<'_>,
    root: &str,
    index: &str,
    span: SourceSpan,
    index_id: StoreIndexId,
) -> Option<()> {
    if context
        .backing_validity
        .index_is_invalid(context.program, index_id)
    {
        push_invalid_collection_index_diagnostic(
            context.file,
            span,
            root,
            index,
            context.diagnostics,
        );
        return None;
    }
    Some(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeCollectionIndexProblem {
    Unique,
    MissingIdentitySuffix,
    MissingRangeKey,
    UnsupportedRangeKey { key_index: usize },
}

impl RangeCollectionIndexProblem {
    fn key_index(self) -> usize {
        match self {
            Self::UnsupportedRangeKey { key_index } => key_index,
            Self::Unique | Self::MissingIdentitySuffix | Self::MissingRangeKey => usize::MAX,
        }
    }
}

fn range_collection_shape(
    store: &StoreFact,
    index: &StoreIndexFact,
) -> Result<SurfaceIndexRangeCollection, RangeCollectionIndexProblem> {
    if index.unique {
        return Err(RangeCollectionIndexProblem::Unique);
    }
    let identity_key_count = full_identity_suffix_count(store, index);
    if !store.identity_keys.is_empty() && identity_key_count != store.identity_keys.len() {
        return Err(RangeCollectionIndexProblem::MissingIdentitySuffix);
    }
    if index.keys.len() <= identity_key_count {
        return Err(RangeCollectionIndexProblem::MissingRangeKey);
    }
    let range_key_index = index.keys.len() - identity_key_count - 1;
    if !matches!(
        index.keys[range_key_index].value_meaning,
        StoredValueMeaning::Scalar(_)
    ) {
        return Err(RangeCollectionIndexProblem::UnsupportedRangeKey {
            key_index: range_key_index,
        });
    }
    Ok(SurfaceIndexRangeCollection {
        index: index.id,
        exact_key_count: range_key_index,
        range_key_index,
        identity_key_count,
    })
}

enum StoreIndexResolution {
    Missing,
    Unique(StoreIndexId),
    Ambiguous,
}

fn unique_store_index(
    program: &CheckedProgram,
    store: &StoreFact,
    index: &str,
) -> StoreIndexResolution {
    let mut matches = program
        .facts
        .store_indexes()
        .iter()
        .filter(|candidate| candidate.store == store.id && candidate.name == index)
        .map(|candidate| candidate.id);
    let Some(index_id) = matches.next() else {
        return StoreIndexResolution::Missing;
    };
    if matches.next().is_some() {
        return StoreIndexResolution::Ambiguous;
    }
    StoreIndexResolution::Unique(index_id)
}

fn push_unknown_collection_index_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    index: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceTarget,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::UnknownCollectionIndex {
            root: root.to_string(),
            index: index.to_string(),
        }),
    ));
}

fn push_ambiguous_collection_index_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    index: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceTarget,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::AmbiguousCollectionIndex {
            root: root.to_string(),
            index: index.to_string(),
        }),
    ));
}

fn push_invalid_collection_index_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    index: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceTarget,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::InvalidCollectionIndex {
            root: root.to_string(),
            index: index.to_string(),
        }),
    ));
}

fn push_invalid_range_collection_index_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    index: &str,
    key: Option<&str>,
    problem: RangeCollectionIndexProblem,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let payload = match problem {
        RangeCollectionIndexProblem::Unique => {
            SurfaceTargetDiagnostic::RangeCollectionUniqueIndex {
                root: root.to_string(),
                index: index.to_string(),
            }
        }
        RangeCollectionIndexProblem::MissingIdentitySuffix => {
            SurfaceTargetDiagnostic::RangeCollectionMissingIdentitySuffix {
                root: root.to_string(),
                index: index.to_string(),
            }
        }
        RangeCollectionIndexProblem::MissingRangeKey => {
            SurfaceTargetDiagnostic::RangeCollectionMissingRangeKey {
                root: root.to_string(),
                index: index.to_string(),
            }
        }
        RangeCollectionIndexProblem::UnsupportedRangeKey { .. } => {
            SurfaceTargetDiagnostic::RangeCollectionUnsupportedRangeKey {
                root: root.to_string(),
                index: index.to_string(),
                key: key.unwrap_or("<unknown>").to_string(),
            }
        }
    };
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceTarget,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceTarget(payload),
    ));
}

fn push_keyless_collection_root_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceTarget,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::KeylessCollectionRoot {
            root: root.to_string(),
        }),
    ));
}

fn push_foreign_unknown_or_ambiguous_root(
    program: &CheckedProgram,
    file: &Path,
    span: SourceSpan,
    surface_root: &str,
    target_root: &str,
    backing_validity: &BackingValidity,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if resolve_surface_store_root(
        program,
        file,
        span,
        target_root,
        backing_validity,
        SurfaceRootOrigin::Collection,
        diagnostics,
    )
    .is_some()
    {
        diagnostics.push(CheckDiagnostic::new(
            Code::CheckSurfaceTarget,
            DiagnosticAnchor::at(file, span),
            DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::ForeignCollectionRoot {
                surface_root: surface_root.to_string(),
                target_root: target_root.to_string(),
            }),
        ));
    }
}

fn push_field_diagnostic(
    file: &Path,
    span: SourceSpan,
    list: SurfaceFieldList,
    name: &str,
    problem: SurfaceFieldProblem,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic::new(
        Code::CheckSurfaceField,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
            list,
            name: name.to_string(),
            problem,
        }),
    ));
}

#[derive(Clone, Copy)]
struct SurfaceCatalogInputs<'a> {
    store: &'a StoreFact,
    fields: &'a [SurfaceFieldFact],
    create: &'a [SurfaceFieldFact],
    update: &'a [SurfaceFieldFact],
    collections: &'a [SurfaceCollectionFact],
    actions: &'a [SurfaceActionFact],
    computed_reads: &'a [SurfaceComputedReadFact],
}

fn catalog_status(
    program: &CheckedProgram,
    inputs: SurfaceCatalogInputs<'_>,
) -> SurfaceCatalogStatus {
    let mut blockers = Vec::new();
    if program.catalog.proposal.is_some() {
        blockers.push(SurfaceCatalogBlocker::PendingCatalogProposal);
    }
    if !surface_has_catalog_ids(program, inputs) {
        blockers.push(SurfaceCatalogBlocker::MissingAcceptedCatalogIds);
    }
    if blockers.is_empty() {
        SurfaceCatalogStatus::Stable
    } else {
        SurfaceCatalogStatus::SourceOnly(blockers)
    }
}

fn surface_has_catalog_ids(program: &CheckedProgram, inputs: SurfaceCatalogInputs<'_>) -> bool {
    inputs.store.catalog_id.is_some()
        && program
            .facts
            .resource(inputs.store.resource)
            .catalog_id
            .is_some()
        && fields_have_catalog_ids(program, inputs.fields)
        && fields_have_catalog_ids(program, inputs.create)
        && fields_have_catalog_ids(program, inputs.update)
        && collections_have_catalog_ids(program, inputs.collections)
        && actions_have_catalog_ids(program, inputs.actions)
        && computed_reads_have_catalog_ids(program, inputs.computed_reads)
}

fn actions_have_catalog_ids(program: &CheckedProgram, actions: &[SurfaceActionFact]) -> bool {
    actions
        .iter()
        .all(|action| function_ref_has_accepted_entry_catalog_ids(program, action.function))
}

fn computed_reads_have_catalog_ids(
    program: &CheckedProgram,
    computed_reads: &[SurfaceComputedReadFact],
) -> bool {
    computed_reads
        .iter()
        .all(|read| function_ref_has_accepted_computed_read_catalog_ids(program, read.function))
}

fn collections_have_catalog_ids(
    program: &CheckedProgram,
    collections: &[SurfaceCollectionFact],
) -> bool {
    collections
        .iter()
        .all(|collection| collection_has_catalog_ids(program, collection))
}

fn collection_has_catalog_ids(
    program: &CheckedProgram,
    collection: &SurfaceCollectionFact,
) -> bool {
    match collection.target {
        SurfaceCollectionTarget::StoreRoot(_) => true,
        SurfaceCollectionTarget::StoreIndex(index) => {
            collection_index_has_catalog_ids(program, index)
        }
        SurfaceCollectionTarget::StoreIndexRange(range) => {
            collection_index_has_catalog_ids(program, range.index)
        }
    }
}

fn collection_index_has_catalog_ids(program: &CheckedProgram, index: StoreIndexId) -> bool {
    let index = program.facts.store_index(index);
    index.catalog_id.is_some()
        && index
            .keys
            .iter()
            .all(|key| index_key_has_catalog_ids(program, key))
}

fn index_key_has_catalog_ids(program: &CheckedProgram, key: &StoreIndexKeyFact) -> bool {
    match key.source {
        StoreIndexKeySource::IdentityKey => {
            stored_value_meaning_has_catalog_ids(program, Some(&key.value_meaning))
        }
        StoreIndexKeySource::ResourceMember(member) => field_has_catalog_ids(program, member),
    }
}

fn fields_have_catalog_ids(program: &CheckedProgram, fields: &[SurfaceFieldFact]) -> bool {
    fields
        .iter()
        .all(|field| field_has_catalog_ids(program, field.member))
}

fn field_has_catalog_ids(program: &CheckedProgram, member: ResourceMemberId) -> bool {
    let member = &program.facts.resource_members()[member.0 as usize];
    member.catalog_id.is_some()
        && stored_value_meaning_has_catalog_ids(program, member.value_meaning.as_ref())
}

fn stored_value_meaning_has_catalog_ids(
    program: &CheckedProgram,
    meaning: Option<&StoredValueMeaning>,
) -> bool {
    match meaning {
        None | Some(StoredValueMeaning::Scalar(_)) => true,
        Some(StoredValueMeaning::Identity {
            store,
            store_catalog_id,
            ..
        }) => store_catalog_id.is_some() && program.facts.store(*store).catalog_id.is_some(),
        Some(StoredValueMeaning::Enum { enum_id, members }) => {
            program
                .facts
                .enum_(*enum_id)
                .is_some_and(|enum_| enum_.catalog_id.is_some())
                && members.iter().all(|member| {
                    program
                        .facts
                        .enum_member(*member)
                        .is_some_and(|member| member.catalog_id.is_some())
                })
        }
    }
}
