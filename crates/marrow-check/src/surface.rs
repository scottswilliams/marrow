use std::collections::HashSet;
use std::path::{Path, PathBuf};

use marrow_syntax::{
    Declaration, ParsedSource, SourceSpan, SurfaceDecl, SurfaceItem, SurfaceTarget,
};

use crate::backing_validity::BackingValidity;
use crate::diagnostics::{
    CHECK_SURFACE_ACTION, CHECK_SURFACE_FIELD, CHECK_SURFACE_TARGET, SurfaceActionDiagnostic,
    SurfaceFieldDiagnostic, SurfaceFieldList, SurfaceFieldProblem, SurfaceTargetDiagnostic,
};
use crate::entry_abi::{
    EntrySignatureUnsupported, function_ref_has_accepted_entry_catalog_ids,
    function_ref_has_supported_entry_signature,
};
use crate::executable::CheckedFunctionRef;
use crate::facts::{
    ModuleId, ResourceMemberFact, ResourceMemberId, ResourceMemberKind, StoreFact, StoreIndexFact,
    StoreIndexId, StoreIndexKeyFact, StoreIndexKeySource, StoredValueMeaning, SurfaceActionFact,
    SurfaceCatalogBlocker, SurfaceCatalogStatus, SurfaceCollectionFact, SurfaceCollectionTarget,
    SurfaceFact, SurfaceFieldFact, SurfaceId, SurfaceReadFootprint, SurfaceReadOperationFact,
    SurfaceReadOperationKind,
};
use crate::surface_abi::surface_read_operation_tag;
use crate::{
    CheckDiagnostic, CheckedProgram, Def, DefItem, DiagnosticPayload, Resolution, ResolvableKind,
    build_alias_map, expand_alias, resolve,
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
        let collections = resolve_collections(
            self.program,
            file,
            store,
            surface,
            self.backing_validity,
            self.diagnostics,
        );
        let mut suppressed_action_target = false;
        let Some(action_module) = checked_module_for_id(self.program, module) else {
            return;
        };
        let actions = resolve_actions(
            SurfaceActionContext {
                program: self.program,
                file,
                module_name: &action_module.name,
                imports: &action_module.imports,
                incomplete_modules: self.incomplete_modules,
            },
            surface,
            &mut suppressed_action_target,
            self.diagnostics,
        );
        self.reject_invalid_backing_resource(file, surface, store, diagnostic_start);
        if suppressed_action_target || self.diagnostics.len() != diagnostic_start {
            return;
        }

        let id = SurfaceId(self.surface_facts.len() as u32);
        let catalog_status = catalog_status(
            self.program,
            store,
            &fields,
            &update,
            &collections,
            &actions,
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
            collections,
            actions,
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
                surface.span,
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
    program
        .modules
        .iter()
        .find(|candidate| candidate.name == *module_name)
}

fn resolve_actions(
    context: SurfaceActionContext<'_>,
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
                alias,
                span,
            } => resolve_action(
                context,
                function,
                alias,
                *span,
                suppressed_target,
                diagnostics,
            ),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy)]
struct SurfaceActionContext<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    module_name: &'a str,
    imports: &'a [String],
    incomplete_modules: &'a HashSet<String>,
}

fn resolve_action(
    context: SurfaceActionContext<'_>,
    function_path: &[String],
    alias: &str,
    span: SourceSpan,
    suppressed_target: &mut bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<SurfaceActionFact> {
    let expanded_path = expand_action_path(context, function_path);
    let path = expanded_path.join("::");
    let function = match resolve(
        context.program,
        context.module_name,
        function_path,
        ResolvableKind::Function,
    ) {
        Resolution::Found(Def {
            module,
            item: DefItem::Function(function),
            ..
        }) => {
            if !function.public {
                push_surface_action_diagnostic(
                    context.file,
                    span,
                    SurfaceActionDiagnostic::PrivateFunction { path },
                    format!(
                        "surface action targets private function `{}`",
                        expanded_path.join("::")
                    ),
                    diagnostics,
                );
                return None;
            }
            function_ref_from_resolved(context.program, module, function).or_else(|| {
                *suppressed_target = true;
                None
            })?
        }
        Resolution::NotVisible(name) => {
            push_surface_action_diagnostic(
                context.file,
                span,
                SurfaceActionDiagnostic::PrivateFunction { path: name.clone() },
                format!("surface action targets private function `{name}`"),
                diagnostics,
            );
            return None;
        }
        Resolution::Ambiguous(_) => {
            push_surface_action_diagnostic(
                context.file,
                span,
                SurfaceActionDiagnostic::AmbiguousFunction { path: path.clone() },
                format!("surface action targets ambiguous function `{path}`"),
                diagnostics,
            );
            return None;
        }
        Resolution::Found(_) | Resolution::Unresolved => {
            if references_incomplete_action_module(&expanded_path, context.incomplete_modules) {
                *suppressed_target = true;
                return None;
            }
            push_surface_action_diagnostic(
                context.file,
                span,
                SurfaceActionDiagnostic::UnknownFunction { path: path.clone() },
                format!("surface action targets unknown function `{path}`"),
                diagnostics,
            );
            return None;
        }
    };

    if let Err(issue) = function_ref_has_supported_entry_signature(context.program, function) {
        let (payload, message) = match issue {
            EntrySignatureUnsupported::Parameter { name } => (
                SurfaceActionDiagnostic::UnsupportedParameter {
                    path: path.clone(),
                    parameter: name.clone(),
                },
                format!(
                    "surface action `{path}` parameter `{name}` has a type outside the action JSON surface"
                ),
            ),
            EntrySignatureUnsupported::ReturnValue => (
                SurfaceActionDiagnostic::UnsupportedReturn { path: path.clone() },
                format!("surface action `{path}` return type is outside the action JSON surface"),
            ),
        };
        push_surface_action_diagnostic(context.file, span, payload, message, diagnostics);
        return None;
    }

    Some(SurfaceActionFact {
        alias: alias.to_string(),
        function,
        span,
    })
}

fn expand_action_path(context: SurfaceActionContext<'_>, function_path: &[String]) -> Vec<String> {
    let aliases = build_alias_map(context.imports);
    expand_alias(function_path, &aliases)
}

fn function_ref_from_resolved(
    program: &CheckedProgram,
    module: &crate::CheckedModule,
    function: &crate::CheckedFunction,
) -> Option<CheckedFunctionRef> {
    let module_id = program.facts.module_id(&module.name)?;
    let source_index = u32::try_from(
        module
            .functions
            .iter()
            .position(|candidate| std::ptr::eq(candidate, function))?,
    )
    .ok()?;
    let function =
        program.facts.functions().iter().find(|function| {
            function.module == module_id && function.source_index == source_index
        })?;
    Some(CheckedFunctionRef {
        module: function.module.0,
        function: function.source_index,
        presence: function.return_presence,
    })
}

fn references_incomplete_action_module(
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
    message: String,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(
        CheckDiagnostic::error(CHECK_SURFACE_ACTION, file, span, message)
            .with_payload(DiagnosticPayload::SurfaceAction(payload)),
    );
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
        .modules
        .iter()
        .position(|module| module.source_file == file)
        .map(|index| ModuleId(index as u32))
}

enum StoreRootResolution<'p> {
    Missing,
    Unique(&'p StoreFact),
    Ambiguous,
}

enum RootDiagnosticContext<'a> {
    Surface { name: &'a str },
    Collection,
}

impl RootDiagnosticContext<'_> {
    fn unknown_message(&self, root: &str) -> String {
        match self {
            Self::Surface { name } => format!("surface `{name}` targets unknown store `^{root}`"),
            Self::Collection => format!("surface collection targets unknown store `^{root}`"),
        }
    }

    fn ambiguous_message(&self, root: &str) -> String {
        match self {
            Self::Surface { name } => {
                format!("surface `{name}` targets ambiguous store root `^{root}`")
            }
            Self::Collection => {
                format!("surface collection targets ambiguous store root `^{root}`")
            }
        }
    }
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
    context: RootDiagnosticContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<&'p StoreFact> {
    match resolve_unique_store_root(program, root) {
        StoreRootResolution::Unique(store) => {
            if backing_validity.store_has_duplicate_root(store) {
                push_ambiguous_store_root_diagnostic(
                    file,
                    span,
                    root,
                    context.ambiguous_message(root),
                    diagnostics,
                );
                None
            } else {
                Some(store)
            }
        }
        StoreRootResolution::Missing => {
            push_unknown_store_root_diagnostic(
                file,
                span,
                root,
                context.unknown_message(root),
                diagnostics,
            );
            None
        }
        StoreRootResolution::Ambiguous => {
            push_ambiguous_store_root_diagnostic(
                file,
                span,
                root,
                context.ambiguous_message(root),
                diagnostics,
            );
            None
        }
    }
}

fn push_unknown_store_root_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    message: String,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(
        CheckDiagnostic::error(CHECK_SURFACE_TARGET, file, span, message).with_payload(
            DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::UnknownStore {
                root: root.to_string(),
            }),
        ),
    );
}

fn push_ambiguous_store_root_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    message: String,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(
        CheckDiagnostic::error(CHECK_SURFACE_TARGET, file, span, message).with_payload(
            DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::AmbiguousStore {
                root: root.to_string(),
            }),
        ),
    );
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
        surface.span,
        &surface.store.root,
        backing_validity,
        RootDiagnosticContext::Surface {
            name: &surface.name,
        },
        diagnostics,
    )?;

    if backing_validity.store_is_invalid(store) {
        push_invalid_store_diagnostic(
            file,
            surface.span,
            &surface.name,
            &surface.store.root,
            diagnostics,
        );
        return None;
    }

    if store_resource_is_ambiguous(program, store) {
        let resource = program.facts.resource(store.resource);
        diagnostics.push(
            CheckDiagnostic::error(
                CHECK_SURFACE_TARGET,
                file,
                surface.span,
                format!(
                    "surface `{}` targets store `^{}` with ambiguous resource `{}`",
                    surface.name, surface.store.root, resource.name
                ),
            )
            .with_payload(DiagnosticPayload::SurfaceTarget(
                SurfaceTargetDiagnostic::AmbiguousStoreResource {
                    root: surface.store.root.clone(),
                    resource: resource.name.clone(),
                },
            )),
        );
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
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_SURFACE_TARGET,
            file,
            span,
            format!("surface `{surface_name}` targets invalid backing store `^{root}`"),
        )
        .with_payload(DiagnosticPayload::SurfaceTarget(
            SurfaceTargetDiagnostic::InvalidStore {
                root: root.to_string(),
            },
        )),
    );
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
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_SURFACE_TARGET,
            file,
            span,
            format!(
                "surface `{surface_name}` targets store `^{}` with invalid resource `{}`",
                store.root, resource.name
            ),
        )
        .with_payload(DiagnosticPayload::SurfaceTarget(
            SurfaceTargetDiagnostic::InvalidStoreResource {
                root: store.root.clone(),
                resource: resource.name.clone(),
            },
        )),
    );
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
        let Some((names, span)) = (match (list, item) {
            (SurfaceFieldList::Fields, SurfaceItem::Fields { names, span })
            | (SurfaceFieldList::Create, SurfaceItem::Create { names, span })
            | (SurfaceFieldList::Update, SurfaceItem::Update { names, span }) => {
                Some((names, *span))
            }
            _ => None,
        }) else {
            continue;
        };
        for name in names {
            if let Some(field) = resolve_surface_field(context, list, name, span, diagnostics) {
                fields.push(field);
            }
        }
    }
    fields
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
    let target = match item.target {
        SurfaceTarget::Root { root } => resolve_root_collection(
            program,
            file,
            store,
            root,
            item.span,
            backing_validity,
            diagnostics,
        )?,
        SurfaceTarget::Index { root, index } => resolve_index_collection(
            CollectionResolveContext {
                program,
                file,
                store,
                backing_validity,
                diagnostics,
            },
            root,
            index,
            item.span,
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
    context: CollectionResolveContext<'_>,
    root: &str,
    index: &str,
    span: SourceSpan,
) -> Option<SurfaceCollectionTarget> {
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
    Some(SurfaceCollectionTarget::StoreIndex(index_id))
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
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_SURFACE_TARGET,
            file,
            span,
            format!("surface collection names no index `{index}` on `^{root}`"),
        )
        .with_payload(DiagnosticPayload::SurfaceTarget(
            SurfaceTargetDiagnostic::UnknownCollectionIndex {
                root: root.to_string(),
                index: index.to_string(),
            },
        )),
    );
}

fn push_ambiguous_collection_index_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    index: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_SURFACE_TARGET,
            file,
            span,
            format!("surface collection names ambiguous index `{index}` on `^{root}`"),
        )
        .with_payload(DiagnosticPayload::SurfaceTarget(
            SurfaceTargetDiagnostic::AmbiguousCollectionIndex {
                root: root.to_string(),
                index: index.to_string(),
            },
        )),
    );
}

fn push_invalid_collection_index_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    index: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_SURFACE_TARGET,
            file,
            span,
            format!("surface collection names schema-invalid index `{index}` on `^{root}`"),
        )
        .with_payload(DiagnosticPayload::SurfaceTarget(
            SurfaceTargetDiagnostic::InvalidCollectionIndex {
                root: root.to_string(),
                index: index.to_string(),
            },
        )),
    );
}

fn push_keyless_collection_root_diagnostic(
    file: &Path,
    span: SourceSpan,
    root: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_SURFACE_TARGET,
            file,
            span,
            format!("surface collection targets keyless singleton root `^{root}`"),
        )
        .with_payload(DiagnosticPayload::SurfaceTarget(
            SurfaceTargetDiagnostic::KeylessCollectionRoot {
                root: root.to_string(),
            },
        )),
    );
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
        RootDiagnosticContext::Collection,
        diagnostics,
    )
    .is_some()
    {
        diagnostics.push(
            CheckDiagnostic::error(
                CHECK_SURFACE_TARGET,
                file,
                span,
                format!(
                    "surface collection target `^{target_root}` is not backing store `^{surface_root}`"
                ),
            )
            .with_payload(DiagnosticPayload::SurfaceTarget(
                SurfaceTargetDiagnostic::ForeignCollectionRoot {
                    surface_root: surface_root.to_string(),
                    target_root: target_root.to_string(),
                },
            )),
        );
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
    let message = match problem {
        SurfaceFieldProblem::Unknown => {
            format!(
                "surface {} item `{name}` is not a top-level backing field",
                list.label()
            )
        }
        SurfaceFieldProblem::Unsupported => {
            format!(
                "surface {} item `{name}` is not a plain top-level field",
                list.label()
            )
        }
        SurfaceFieldProblem::Invalid => {
            format!(
                "surface {} item `{name}` names a schema-invalid backing field",
                list.label()
            )
        }
        SurfaceFieldProblem::Ambiguous => {
            format!(
                "surface {} item `{name}` names an ambiguous backing field",
                list.label()
            )
        }
        SurfaceFieldProblem::NotProjected => {
            format!(
                "surface {} item `{name}` must also appear in `fields`",
                list.label()
            )
        }
    };
    diagnostics.push(
        CheckDiagnostic::error(CHECK_SURFACE_FIELD, file, span, message).with_payload(
            DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
                list,
                name: name.to_string(),
                problem,
            }),
        ),
    );
}

fn catalog_status(
    program: &CheckedProgram,
    store: &StoreFact,
    fields: &[SurfaceFieldFact],
    update: &[SurfaceFieldFact],
    collections: &[SurfaceCollectionFact],
    actions: &[SurfaceActionFact],
) -> SurfaceCatalogStatus {
    let mut blockers = Vec::new();
    if program.catalog.proposal.is_some() {
        blockers.push(SurfaceCatalogBlocker::PendingCatalogProposal);
    }
    if !surface_has_catalog_ids(program, store, fields, update, collections, actions) {
        blockers.push(SurfaceCatalogBlocker::MissingAcceptedCatalogIds);
    }
    if blockers.is_empty() {
        SurfaceCatalogStatus::Stable
    } else {
        SurfaceCatalogStatus::SourceOnly(blockers)
    }
}

fn surface_has_catalog_ids(
    program: &CheckedProgram,
    store: &StoreFact,
    fields: &[SurfaceFieldFact],
    update: &[SurfaceFieldFact],
    collections: &[SurfaceCollectionFact],
    actions: &[SurfaceActionFact],
) -> bool {
    store.catalog_id.is_some()
        && program.facts.resource(store.resource).catalog_id.is_some()
        && fields_have_catalog_ids(program, fields)
        && fields_have_catalog_ids(program, update)
        && collections_have_catalog_ids(program, collections)
        && actions_have_catalog_ids(program, actions)
}

fn actions_have_catalog_ids(program: &CheckedProgram, actions: &[SurfaceActionFact]) -> bool {
    actions
        .iter()
        .all(|action| function_ref_has_accepted_entry_catalog_ids(program, action.function))
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
            let index = program.facts.store_index(index);
            index.catalog_id.is_some()
                && index
                    .keys
                    .iter()
                    .all(|key| index_key_has_catalog_ids(program, key))
        }
    }
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
