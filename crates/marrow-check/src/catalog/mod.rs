use std::collections::{HashMap, HashSet};
use std::path::Path;

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};
use marrow_store::cell::CatalogId;
use marrow_syntax::SourceSpan;

use crate::evolution::leaf_type;
use crate::evolution::{DefaultIntent, EvolveIntents, RenameIntent, RetireIntent, TransformIntent};
use crate::facts::{StoreIndexFact, StoreIndexKeySource, StoredValueMeaning};
use crate::program::{EvolveDefault, EvolveTransform};
use crate::{
    CHECK_CATALOG_INTENT, CHECK_EVOLVE_TARGET, CatalogIntentDiagnostic, CatalogIntentKind,
    CatalogPathCandidate, CheckDiagnostic, CheckedProgram, DiagnosticPayload,
};

mod source_digest;
mod stable_id;

pub(crate) use source_digest::{
    DurableRendering, analyzed_source_digest, durable_renderings_for_source, evolution_digest,
    source_and_evolution_digests,
};
use stable_id::StableIdAllocator;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CatalogKey {
    pub(crate) kind: CatalogEntryKind,
    pub(crate) path: String,
}

impl CatalogKey {
    pub(crate) fn new(kind: CatalogEntryKind, path: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.into(),
        }
    }
}

pub(crate) struct CatalogBinding {
    pub(crate) accepted_epoch: Option<u64>,
    pub(crate) accepted_digest: Option<String>,
    pub(crate) ids: HashMap<CatalogKey, String>,
    /// Resolves a member's referent enum or store to its identity-aware leaf token. Covers
    /// proposal-only ids the accepted-only `ids` omits, and is never bound onto live facts.
    pub(crate) leaf_token_ids: HashMap<CatalogKey, String>,
    pub(crate) proposal: Option<CatalogMetadata>,
}

pub(crate) fn bind_catalog(
    accepted: Option<&CatalogMetadata>,
    program: &mut CheckedProgram,
    evolve: &EvolveIntents,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let binding = catalog_binding(program, accepted, evolve, diagnostics);
    let declared_store_key_shapes = declared_store_key_shapes(program, &binding.leaf_token_ids);
    let declared_member_structs = declared_member_structs(program, &binding.leaf_token_ids);
    program
        .facts
        .bind_catalog_ids(&program.modules, &binding.ids);
    program.catalog.accepted_epoch = binding.accepted_epoch;
    program.catalog.accepted_digest = binding.accepted_digest;
    program.catalog.accepted_entries = accepted
        .map(|catalog| catalog.entries.clone())
        .unwrap_or_default();
    // Defaults and transforms bind through the proposal id map, not the accepted-only ids:
    // a default or transform may target a brand-new member current source adds, whose stable
    // id lives only in the proposal until it is accepted. Discharge keys that member's
    // obligation by the same proposal id, so the fill resolves to the obligation it covers.
    program.catalog.evolve_defaults = bound_defaults(&evolve.defaults, &binding.leaf_token_ids);
    program.catalog.evolve_transforms =
        bound_transforms(&evolve.transforms, &binding.leaf_token_ids);
    program.catalog.declared_store_key_shapes = declared_store_key_shapes;
    program.catalog.declared_member_structs = declared_member_structs;
    program.catalog.proposal = binding.proposal;
}

/// The single owner of each store's `(stable_id, identity-key shape token)` from source, for
/// every store whose identity is bound in `ids`; a store with no bound identity (pending
/// first-run identity) is omitted. The token records the key types in order, so a key arity or
/// key-type change drifts it even when the program is otherwise unchanged. Both the
/// fact-binding map and the proposal recorder read these pairs.
fn store_key_shapes(
    program: &CheckedProgram,
    ids: &HashMap<CatalogKey, String>,
) -> Vec<(String, String)> {
    program
        .modules
        .iter()
        .flat_map(|module| {
            module.stores.iter().filter_map(|store| {
                let catalog_id = ids.get(&CatalogKey::new(
                    CatalogEntryKind::Store,
                    store_path(&module.name, &store.root),
                ))?;
                let token = leaf_type::store_key_shape_token(&store.identity_keys);
                Some((catalog_id.clone(), token))
            })
        })
        .collect()
}

/// The single owner of each resource member's `(stable_id, structural signature token)` from
/// source, for every member whose identity is bound in `ids`. The signature records the
/// member's kind, its key shape if a keyed layer, and its leaf token if a leaf, so discharge
/// fails closed on a structural divergence the other classifiers leave unclaimed. The token is
/// `None` when a leaf member's value type cannot be tokenized yet (a pending first-run
/// referent); the recorder writes that `None` forward while the fact-binding map omits it.
fn member_structs(
    program: &CheckedProgram,
    ids: &HashMap<CatalogKey, String>,
) -> Vec<(String, Option<String>)> {
    source_catalog_entries(program)
        .into_iter()
        .filter(|source| source.kind == CatalogEntryKind::ResourceMember)
        .filter_map(|source| {
            let module = member_struct_module(&source);
            let leaf = source.leaf.as_ref().map(|leaf| &leaf.ty);
            let token =
                leaf_type::member_struct_token(program, module, leaf, &source.key_params, ids);
            let catalog_id = ids.get(&CatalogKey::new(source.kind, source.path))?;
            Some((catalog_id.clone(), token))
        })
        .collect()
}

/// The single owner of each store index's `(stable_id, declaration shape token)` from source,
/// for every index whose identity is bound in `ids`. The token records uniqueness and the
/// ordered key sources by durable identity, so a same-path index key or uniqueness edit advances
/// the proposal and discharges a rebuild under the preserved index id.
fn store_index_shapes(
    program: &CheckedProgram,
    ids: &HashMap<CatalogKey, String>,
) -> Vec<(String, Option<String>)> {
    program
        .facts
        .store_indexes()
        .iter()
        .filter_map(|index| {
            let store = program.facts.store(index.store);
            let module = &program.modules[store.module.0 as usize];
            let catalog_id = ids.get(&CatalogKey::new(
                CatalogEntryKind::StoreIndex,
                store_index_path(&module.name, &store.root, &index.name),
            ))?;
            Some((
                catalog_id.clone(),
                store_index_shape_token(program, index, ids),
            ))
        })
        .collect()
}

fn store_index_shape_token(
    program: &CheckedProgram,
    index: &StoreIndexFact,
    ids: &HashMap<CatalogKey, String>,
) -> Option<String> {
    let store = program.facts.store(index.store);
    let mut key_tokens = Vec::with_capacity(index.keys.len());
    for key in &index.keys {
        let meaning = stored_value_meaning_token(program, &key.value_meaning, ids)?;
        let source = match key.source {
            StoreIndexKeySource::IdentityKey => {
                let position = store
                    .identity_keys
                    .iter()
                    .position(|identity_key| identity_key.name == key.name)?;
                format!("identity:{position}:{meaning}")
            }
            StoreIndexKeySource::ResourceMember(member_id) => {
                let member_path = program.facts.resource_member_catalog_path(member_id)?;
                let member_id = ids.get(&CatalogKey::new(
                    CatalogEntryKind::ResourceMember,
                    member_path,
                ))?;
                format!("member:{member_id}:{meaning}")
            }
        };
        key_tokens.push(source);
    }
    Some(format!(
        "unique={};keys=[{}]",
        index.unique,
        key_tokens.join(",")
    ))
}

fn stored_value_meaning_token(
    program: &CheckedProgram,
    meaning: &StoredValueMeaning,
    ids: &HashMap<CatalogKey, String>,
) -> Option<String> {
    match meaning {
        StoredValueMeaning::Scalar(scalar) => Some(scalar.name().to_string()),
        StoredValueMeaning::Identity {
            store: store_id,
            arity,
            ..
        } => {
            let store = program.facts.store(*store_id);
            let module = &program.modules[store.module.0 as usize];
            let store_id = ids.get(&CatalogKey::new(
                CatalogEntryKind::Store,
                store_path(&module.name, &store.root),
            ))?;
            Some(format!("id:{store_id}:{arity}"))
        }
        StoredValueMeaning::Enum { enum_id, .. } => {
            let enum_fact = program.facts.enum_(*enum_id)?;
            let module = &program.modules[enum_fact.module.0 as usize];
            let enum_id = ids.get(&CatalogKey::new(
                CatalogEntryKind::Enum,
                enum_path(&module.name, &enum_fact.name),
            ))?;
            Some(format!("enum:{enum_id}"))
        }
    }
}

/// [`store_key_shapes`] keyed by stable catalog id for lookup.
fn declared_store_key_shapes(
    program: &CheckedProgram,
    ids: &HashMap<CatalogKey, String>,
) -> HashMap<String, String> {
    store_key_shapes(program, ids).into_iter().collect()
}

/// [`member_structs`] keyed by stable catalog id for lookup, dropping members with no bound
/// identity or an unresolved leaf referent.
fn declared_member_structs(
    program: &CheckedProgram,
    ids: &HashMap<CatalogKey, String>,
) -> HashMap<String, String> {
    member_structs(program, ids)
        .into_iter()
        .filter_map(|(id, token)| Some((id, token?)))
        .collect()
}

/// A leaf member's declaring module (where its referent resolves), or empty for a group.
fn member_struct_module(source: &SourceCatalogEntry) -> &str {
    source
        .leaf
        .as_ref()
        .map(|leaf| leaf.module.as_str())
        .unwrap_or("")
}

/// A catalog-intent error for a project-level failure not tied to one declaration, so it
/// carries no source span.
fn catalog_diagnostic(file: std::path::PathBuf, message: String) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_CATALOG_INTENT, &file, SourceSpan::default(), message)
}

fn catalog_error(file: std::path::PathBuf, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_CATALOG_INTENT, &file, span, message)
}

fn catalog_binding(
    program: &CheckedProgram,
    accepted: Option<&CatalogMetadata>,
    evolve: &EvolveIntents,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> CatalogBinding {
    let source_entries = source_catalog_entries(program);
    let mut ids = HashMap::new();
    let proposal = match accepted {
        Some(catalog) => bind_against_accepted(
            program,
            catalog,
            evolve,
            &source_entries,
            &mut ids,
            diagnostics,
        ),
        None => Some(bind_first_run(
            program,
            evolve,
            &source_entries,
            diagnostics,
        )),
    };

    // The proposal is the catalog the commit path freezes when the program runs or an
    // evolution applies, so it must satisfy the same identity invariants. Validating it
    // here makes an identity collision the binding logic produced fail closed at check
    // time rather than at apply.
    if let Some(proposal) = &proposal
        && let Err(error) = proposal.validate()
    {
        diagnostics.push(catalog_diagnostic(
            first_source_file(&source_entries),
            format!("proposed catalog metadata is not valid: {}", error.message),
        ));
    }

    // The leaf token resolves a member's referent enum or store to its stable id. When a
    // proposal exists its entries carry every referent's id, including freshly-minted ones
    // the accepted-only `ids` map omits; when nothing changed, all referents are accepted
    // and `ids` already has them. This map is for token resolution only and is never bound
    // onto live facts, so a proposal-only identity does not leak into the program's facts.
    let leaf_token_ids = match &proposal {
        Some(proposal) => proposal_id_map(&proposal.entries),
        None => ids.clone(),
    };

    CatalogBinding {
        accepted_epoch: accepted.map(|catalog| catalog.epoch),
        accepted_digest: accepted.map(|catalog| catalog.digest.clone()),
        ids,
        leaf_token_ids,
        proposal,
    }
}

/// Bind current source against an existing accepted catalog: carry accepted identity forward,
/// apply renames and retires, mint identity for new entities, and record signatures, binding
/// the resolved stable ids into `ids`. Returns the advanced proposal on any real change, or
/// `None` when the source matches the accepted catalog exactly.
fn bind_against_accepted(
    program: &CheckedProgram,
    catalog: &CatalogMetadata,
    evolve: &EvolveIntents,
    source_entries: &[SourceCatalogEntry],
    ids: &mut HashMap<CatalogKey, String>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<CatalogMetadata> {
    let accepted_index = AcceptedCatalog::new(catalog);
    let source_catalog = SourceCatalog::new(source_entries);
    let mut renames = resolve_renames(
        &accepted_index,
        &source_catalog,
        &evolve.renames,
        diagnostics,
    );
    let mut proposal_entries = catalog.entries.clone();
    let mut changed = bind_source_entries(
        &accepted_index,
        source_entries,
        &mut renames,
        ids,
        &mut proposal_entries,
        diagnostics,
    );
    report_unresolved_renames(&renames, diagnostics);
    if apply_retires(
        &mut proposal_entries,
        &evolve.retires,
        &accepted_index,
        &source_catalog,
        diagnostics,
    ) {
        changed = true;
    }
    if drop_absent_indexes(&mut proposal_entries, &source_catalog) {
        changed = true;
    }
    if record_signatures_into(program, &mut proposal_entries, Some(catalog)) {
        changed = true;
    }
    changed.then(|| CatalogMetadata::new(catalog.epoch + 1, proposal_entries))
}

/// Resolve each current source entry to its identity — carry an accepted active entry's id
/// forward, relocate a renamed one, or mint identity for a new entity — binding it into `ids`
/// and returning whether any entry is a real change. An accepted entry whose source
/// declaration has disappeared but is neither renamed nor retired stays active with no source
/// backing: dropping a sparse field is a legal no-op, so it is a discharge obligation rather
/// than a binding error.
fn bind_source_entries(
    accepted_index: &AcceptedCatalog<'_>,
    source_entries: &[SourceCatalogEntry],
    renames: &mut HashMap<String, ResolvedRename>,
    ids: &mut HashMap<CatalogKey, String>,
    proposal_entries: &mut Vec<CatalogEntry>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> bool {
    let mut allocator = StableIdAllocator::over(proposal_entries);
    let mut changed = false;
    for source in source_entries {
        let rename = renames.remove(&source.path);
        if let Some(binding) = accepted_index.active_entry(source.kind, &source.path) {
            // A rename onto a path that already names a live accepted entity cannot move
            // identity there; report the no-op intent instead of dropping it.
            if rename.is_some() {
                push_rename_target_live(source, diagnostics);
            }
            let stable_id = binding.entry.stable_id.clone();
            ids.insert(CatalogKey::new(source.kind, source.path.clone()), stable_id);
        } else if let Some(reserved) = accepted_index.reserved_entry(source.kind, &source.path) {
            push_reserved_reuse(source, reserved.entry, diagnostics);
            changed = true;
        } else if let Some(rename) = rename {
            apply_rename(proposal_entries, source, &rename.from_path, ids);
            changed = true;
        } else {
            push_pending_identity(source, diagnostics);
            prepare_proposal_path(proposal_entries, source.kind, &source.path);
            proposal_entries.push(proposed_catalog_entry(source, &mut allocator));
            changed = true;
        }
    }
    changed
}

/// Report every rename whose target the source never declares; it relocates nothing, so the
/// intent must not vanish silently.
fn report_unresolved_renames(
    renames: &HashMap<String, ResolvedRename>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for rename in renames.values() {
        report_unresolved_intent(&rename.file, rename.span, diagnostics);
    }
}

/// Record each store's identity-key shape and each member's structural signature into the
/// proposal. Runs after every referent's id is bound, so a renamed enum or store resolves to
/// its preserved identity through the proposal id map (which covers freshly-minted referents
/// and is never bound onto live facts). Returns whether any signature changed against
/// `accepted`.
fn record_signatures_into(
    program: &CheckedProgram,
    proposal_entries: &mut [CatalogEntry],
    accepted: Option<&CatalogMetadata>,
) -> bool {
    let leaf_token_ids = proposal_id_map(proposal_entries);
    let key_shapes_changed =
        record_store_key_shapes(program, proposal_entries, &leaf_token_ids, accepted);
    let index_shapes_changed =
        record_store_index_shapes(program, proposal_entries, &leaf_token_ids, accepted);
    let structs_changed =
        record_member_structs(program, proposal_entries, &leaf_token_ids, accepted);
    key_shapes_changed || index_shapes_changed || structs_changed
}

/// Bind current source with no accepted catalog: every entity mints fresh identity, and every
/// rename or retire is an unresolved intent (nothing to carry forward). The first-run proposal
/// is always real, so this returns it directly rather than as an `Option`.
fn bind_first_run(
    program: &CheckedProgram,
    evolve: &EvolveIntents,
    source_entries: &[SourceCatalogEntry],
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> CatalogMetadata {
    for rename in &evolve.renames {
        report_unresolved_intent(&rename.file, rename.span, diagnostics);
    }
    for retire in &evolve.retires {
        report_unresolved_intent(&retire.file, retire.span, diagnostics);
    }
    let mut allocator = StableIdAllocator::empty();
    let mut proposal_entries: Vec<CatalogEntry> = source_entries
        .iter()
        .map(|source| proposed_catalog_entry(source, &mut allocator))
        .collect();
    record_signatures_into(program, &mut proposal_entries, None);
    CatalogMetadata::new(1, proposal_entries)
}

/// The `(kind, path) -> stable id` map of a proposal's active entries. Unlike the accepted-only
/// binding map, this covers freshly-minted and renamed referents, so the leaf token can resolve
/// an enum or store the accepted catalog does not yet record.
fn proposal_id_map(entries: &[CatalogEntry]) -> HashMap<CatalogKey, String> {
    entries
        .iter()
        .filter(|entry| entry.lifecycle == CatalogLifecycle::Active)
        .map(|entry| {
            (
                CatalogKey::new(entry.kind, entry.path.clone()),
                entry.stable_id.clone(),
            )
        })
        .collect()
}

/// The proposal identity map for activation-only readers, exposed so executable places reuse
/// catalog binding's proposal identity semantics rather than rebuilding them.
pub(crate) fn active_proposal_id_map(program: &CheckedProgram) -> HashMap<CatalogKey, String> {
    program
        .catalog
        .proposal
        .as_ref()
        .map(|proposal| proposal_id_map(&proposal.entries))
        .unwrap_or_default()
}

/// The stable id a member-target evolve path binds to, or `None` when it names no resource
/// member (the type pass already reported it).
fn member_target_id(path: &str, ids: &HashMap<CatalogKey, String>) -> Option<String> {
    ids.get(&CatalogKey::new(
        CatalogEntryKind::ResourceMember,
        path.to_string(),
    ))
    .cloned()
}

/// Resolve each `evolve default` to the stable id its data cells use, carrying the
/// constant value forward for discharge and the source digest.
fn bound_defaults(
    defaults: &[DefaultIntent],
    ids: &HashMap<CatalogKey, String>,
) -> Vec<EvolveDefault> {
    defaults
        .iter()
        .filter_map(|default| {
            member_target_id(&default.path, ids).map(|catalog_id| EvolveDefault {
                catalog_id,
                value: default.value.clone(),
            })
        })
        .collect()
}

/// Record every `evolve transform` with the owning resource type name, the stable
/// accepted or proposal ids it addresses, and the body apply executes. A transform
/// whose target names no resource member is dropped: the type pass already reports it,
/// and it anchors no obligation.
fn bound_transforms(
    transforms: &[TransformIntent],
    ids: &HashMap<CatalogKey, String>,
) -> Vec<EvolveTransform> {
    transforms
        .iter()
        .filter_map(|transform| {
            let resource = transform
                .path
                .rsplit_once("::")
                .map(|(resource, _)| resource.to_string())?;
            let reads = transform
                .read_paths
                .iter()
                .filter_map(|path| member_target_id(path, ids))
                .filter_map(|id| CatalogId::new(id).ok())
                .collect();
            Some(EvolveTransform {
                catalog_id: member_target_id(&transform.path, ids),
                reads,
                resource,
                file: transform.file.clone(),
                target_path: transform.path.clone(),
                body_span: transform.body_span,
                runtime_body: None,
            })
        })
        .collect()
}

/// The first source file a catalog entry came from, used to attach a
/// proposal-level diagnostic that is not tied to a single declaration span.
fn first_source_file(source_entries: &[SourceCatalogEntry]) -> std::path::PathBuf {
    source_entries
        .first()
        .map(|entry| entry.file.clone())
        .unwrap_or_default()
}

/// One rename the binding will carry forward, keyed in the resolution map by its
/// new path. The kind is the one the source fixes for that new path, so the
/// accepted entry behind `from_path` is matched without relying on paths being
/// unique across kinds.
struct ResolvedRename {
    from_path: String,
    file: std::path::PathBuf,
    span: SourceSpan,
}

/// Resolve the rename intents into an injective partial map `to_path -> rename`.
/// A rename is dropped with a diagnostic when it cannot move identity soundly:
///
/// - its target or source collides with another rename (the map must be injective
///   on both ends, or one accepted entry would be orphaned);
/// - its source path is still a live source declaration (a rename removes the old
///   spelling, so a source that still declares it would alias one stable id onto
///   two members);
/// - no active accepted entry carries the source path's identity forward.
fn resolve_renames(
    accepted: &AcceptedCatalog<'_>,
    source_catalog: &SourceCatalog<'_>,
    renames: &[RenameIntent],
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> HashMap<String, ResolvedRename> {
    let mut resolved: HashMap<String, ResolvedRename> = HashMap::new();
    let mut from_paths: HashSet<String> = HashSet::new();
    for rename in renames {
        let kind = match source_catalog.path_kind(rename.to_path.as_str()) {
            SourcePathKind::Absent => {
                report_unresolved_intent(&rename.file, rename.span, diagnostics);
                continue;
            }
            SourcePathKind::Ambiguous => {
                push_intent_path_ambiguous(
                    &rename.file,
                    rename.span,
                    CatalogIntentKind::RenameTarget,
                    &rename.to_path,
                    accepted.path_candidates(&rename.to_path),
                    source_catalog.kinds_at_path(&rename.to_path),
                    diagnostics,
                );
                continue;
            }
            SourcePathKind::Unique(kind) => kind,
        };
        if accepted_source_path_ambiguous(accepted, source_catalog, &rename.to_path, kind) {
            push_intent_path_ambiguous(
                &rename.file,
                rename.span,
                CatalogIntentKind::RenameTarget,
                &rename.to_path,
                accepted.path_candidates(&rename.to_path),
                source_catalog.kinds_at_path(&rename.to_path),
                diagnostics,
            );
            continue;
        }
        if accepted.path_is_ambiguous(&rename.from_path) {
            push_intent_path_ambiguous(
                &rename.file,
                rename.span,
                CatalogIntentKind::RenameSource,
                &rename.from_path,
                accepted.path_candidates(&rename.from_path),
                source_catalog.kinds_at_path(&rename.from_path),
                diagnostics,
            );
            continue;
        }
        match source_catalog.path_kind(rename.from_path.as_str()) {
            SourcePathKind::Absent => {}
            SourcePathKind::Unique(source_kind) if source_kind == kind => {
                push_rename_source_declared(rename, diagnostics);
                continue;
            }
            SourcePathKind::Unique(_) | SourcePathKind::Ambiguous => {
                push_intent_path_ambiguous(
                    &rename.file,
                    rename.span,
                    CatalogIntentKind::RenameSource,
                    &rename.from_path,
                    accepted.path_candidates(&rename.from_path),
                    source_catalog.kinds_at_path(&rename.from_path),
                    diagnostics,
                );
                continue;
            }
        }
        if rename_already_recorded(accepted, kind, rename) {
            // The accepted catalog already carries this entity at `to_path` with
            // `from_path` recorded as an alias. The intent has no relocation work left
            // to perform and is not unresolved.
            continue;
        }
        let duplicate_target = resolved.contains_key(&rename.to_path);
        let duplicate_source = !from_paths.insert(rename.from_path.clone());
        if duplicate_target || duplicate_source {
            push_rename_conflict(rename, diagnostics);
            continue;
        }
        if accepted.active_entry(kind, &rename.from_path).is_none() {
            report_unresolved_intent(&rename.file, rename.span, diagnostics);
            continue;
        }
        resolved.insert(
            rename.to_path.clone(),
            ResolvedRename {
                from_path: rename.from_path.clone(),
                file: rename.file.clone(),
                span: rename.span,
            },
        );
    }
    resolved
}

fn accepted_source_path_ambiguous(
    accepted: &AcceptedCatalog<'_>,
    source_catalog: &SourceCatalog<'_>,
    path: &str,
    source_kind: CatalogEntryKind,
) -> bool {
    accepted.path_is_ambiguous(path)
        || accepted
            .path_candidates(path)
            .iter()
            .any(|candidate| candidate.kind != source_kind)
        || source_catalog
            .kinds_at_path(path)
            .iter()
            .any(|kind| *kind != source_kind)
}

/// Whether the accepted catalog already carries this identity at `to_path` while
/// preserving `from_path` as an alias, leaving no relocation work for the intent.
fn rename_already_recorded(
    accepted: &AcceptedCatalog<'_>,
    kind: CatalogEntryKind,
    rename: &RenameIntent,
) -> bool {
    accepted
        .active_entry(kind, &rename.to_path)
        .is_some_and(|binding| {
            binding
                .entry
                .aliases
                .iter()
                .any(|alias| alias == &rename.from_path)
        })
}

struct SourceCatalog<'a> {
    entries: HashSet<(CatalogEntryKind, &'a str)>,
    kinds_by_path: HashMap<&'a str, Vec<CatalogEntryKind>>,
}

#[derive(Clone, Copy)]
enum SourcePathKind {
    Absent,
    Unique(CatalogEntryKind),
    Ambiguous,
}

impl<'a> SourceCatalog<'a> {
    fn new(source_entries: &'a [SourceCatalogEntry]) -> Self {
        let mut entries = HashSet::new();
        let mut kinds_by_path: HashMap<&str, Vec<CatalogEntryKind>> = HashMap::new();
        for entry in source_entries {
            entries.insert((entry.kind, entry.path.as_str()));
            let kinds = kinds_by_path.entry(entry.path.as_str()).or_default();
            if !kinds.contains(&entry.kind) {
                kinds.push(entry.kind);
            }
        }
        Self {
            entries,
            kinds_by_path,
        }
    }

    fn contains(&self, kind: CatalogEntryKind, path: &str) -> bool {
        self.entries.contains(&(kind, path))
    }

    fn kinds_at_path(&self, path: &str) -> &[CatalogEntryKind] {
        self.kinds_by_path
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn path_kind(&self, path: &str) -> SourcePathKind {
        match self.kinds_at_path(path) {
            [] => SourcePathKind::Absent,
            [kind] => SourcePathKind::Unique(*kind),
            _ => SourcePathKind::Ambiguous,
        }
    }
}

/// Carry the accepted entry at `from_path` forward to its new path: relocate it,
/// record the old path as an alias, and bind the source fact to its preserved
/// stable id. The entry stays active — a rename is identity-preserving, not
/// destructive. The accepted entry is matched on the source-fixed kind, so a
/// like-spelled entry of another kind is never relocated.
fn apply_rename(
    entries: &mut [CatalogEntry],
    source: &SourceCatalogEntry,
    from_path: &str,
    ids: &mut HashMap<CatalogKey, String>,
) {
    let Some(entry) = entries.iter_mut().find(|entry| {
        entry.lifecycle == CatalogLifecycle::Active
            && entry.kind == source.kind
            && entry.path == from_path
    }) else {
        return;
    };
    if !entry.aliases.iter().any(|alias| alias == from_path) {
        entry.aliases.push(from_path.to_string());
    }
    entry.path = source.path.clone();
    ids.insert(
        CatalogKey::new(source.kind, source.path.clone()),
        entry.stable_id.clone(),
    );
}

/// Record each store's identity-key shape into its proposal entry, once its id is bound.
/// Returns whether any store's shape is a real change relative to the accepted snapshot, so
/// an otherwise-unchanged program that only re-keyed a store still advances the proposal.
fn record_store_key_shapes(
    program: &CheckedProgram,
    entries: &mut [CatalogEntry],
    ids: &HashMap<CatalogKey, String>,
    accepted: Option<&CatalogMetadata>,
) -> bool {
    let pairs = store_key_shapes(program, ids)
        .into_iter()
        .map(|(id, shape)| (id, Some(shape)));
    record_signatures(
        entries,
        pairs,
        accepted_field(accepted, CatalogEntryKind::Store, |entry| {
            &entry.accepted_key_shape
        }),
        |entry| &mut entry.accepted_key_shape,
    )
}

/// Record each store index's declaration shape into its proposal entry. Unlike store and member
/// signatures, an accepted same-path index with no recorded shape is treated as changed once:
/// old catalogs cannot prove their derived cells match the current declaration, so apply must
/// rebuild or probe before freezing the signature forward.
fn record_store_index_shapes(
    program: &CheckedProgram,
    entries: &mut [CatalogEntry],
    ids: &HashMap<CatalogKey, String>,
    accepted: Option<&CatalogMetadata>,
) -> bool {
    record_index_signatures(
        entries,
        store_index_shapes(program, ids),
        accepted_field(accepted, CatalogEntryKind::StoreIndex, |entry| {
            &entry.accepted_index_shape
        }),
        |entry| &mut entry.accepted_index_shape,
    )
}

/// Record each resource member's identity-aware structural signature into its proposal entry,
/// once every referent's id is bound. The signature covers leaf and group members alike, so a
/// keyed-layer re-key, a group<->keyed-group reshape, or any other structural transition reads
/// as a different signature. Returns whether any member's signature is a real change relative to
/// the accepted snapshot, so an otherwise-unchanged program that only reshaped a member still
/// advances the proposal.
fn record_member_structs(
    program: &CheckedProgram,
    entries: &mut [CatalogEntry],
    ids: &HashMap<CatalogKey, String>,
    accepted: Option<&CatalogMetadata>,
) -> bool {
    record_signatures(
        entries,
        member_structs(program, ids),
        accepted_field(accepted, CatalogEntryKind::ResourceMember, |entry| {
            &entry.accepted_struct
        }),
        |entry| &mut entry.accepted_struct,
    )
}

/// The accepted-snapshot signature field for every entry of `kind`, keyed by stable id. Empty
/// when there is no accepted snapshot (a first-run catalog), so every signature records without
/// flagging change.
fn accepted_field<'a>(
    accepted: Option<&'a CatalogMetadata>,
    kind: CatalogEntryKind,
    field: impl Fn(&'a CatalogEntry) -> &'a Option<String>,
) -> HashMap<&'a str, &'a Option<String>> {
    accepted
        .map(|catalog| {
            catalog
                .entries
                .iter()
                .filter(|entry| entry.kind == kind)
                .map(|entry| (entry.stable_id.as_str(), field(entry)))
                .collect()
        })
        .unwrap_or_default()
}

/// Record each `(stable_id, signature)` pair into the matching proposal entry's signature field
/// and report whether any is a real change. A signature differing from a *known* accepted one is
/// a real change; backfilling an entry with no recorded accepted signature (minted before
/// signatures, or fresh this cycle) is not, since its durable shape is unchanged. This is the one
/// implementation of the record-or-diff rule the store-key and member-structure recorders share.
fn record_signatures(
    entries: &mut [CatalogEntry],
    pairs: impl IntoIterator<Item = (String, Option<String>)>,
    accepted: HashMap<&str, &Option<String>>,
    field: impl Fn(&mut CatalogEntry) -> &mut Option<String>,
) -> bool {
    let index: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| (entry.stable_id.clone(), i))
        .collect();
    let mut changed = false;
    for (stable_id, signature) in pairs {
        let Some(&i) = index.get(stable_id.as_str()) else {
            continue;
        };
        let accepted_signature = accepted.get(stable_id.as_str()).copied();
        if let Some(Some(_)) = accepted_signature
            && accepted_signature != Some(&signature)
        {
            changed = true;
        }
        *field(&mut entries[i]) = signature;
    }
    changed
}

fn record_index_signatures(
    entries: &mut [CatalogEntry],
    pairs: impl IntoIterator<Item = (String, Option<String>)>,
    accepted: HashMap<&str, &Option<String>>,
    field: impl Fn(&mut CatalogEntry) -> &mut Option<String>,
) -> bool {
    let index: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| (entry.stable_id.clone(), i))
        .collect();
    let mut changed = false;
    for (stable_id, signature) in pairs {
        let Some(&i) = index.get(stable_id.as_str()) else {
            continue;
        };
        match accepted.get(stable_id.as_str()).copied() {
            Some(Some(accepted_signature))
                if signature.as_deref() != Some(accepted_signature.as_str()) =>
            {
                changed = true;
            }
            Some(None) if signature.is_some() => {
                changed = true;
            }
            _ => {}
        }
        *field(&mut entries[i]) = signature;
    }
    changed
}

/// Mark each retired entity reserved in the proposal, returning whether any entry changed. A
/// retire names a destructive intent over an accepted entry whose source declaration is gone;
/// a path matching no active accepted entry is a target diagnostic.
fn apply_retires(
    entries: &mut [CatalogEntry],
    retires: &[RetireIntent],
    accepted: &AcceptedCatalog<'_>,
    source_catalog: &SourceCatalog<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> bool {
    let mut changed = false;
    for retire in retires {
        match retire_target(entries, accepted, source_catalog, retire, diagnostics) {
            RetireTarget::Active(index) => {
                entries[index].lifecycle = CatalogLifecycle::Reserved;
                changed = true;
            }
            RetireTarget::Consumed | RetireTarget::Rejected => {}
        }
    }
    changed
}

enum RetireTarget {
    Active(usize),
    Consumed,
    Rejected,
}

fn retire_target(
    entries: &[CatalogEntry],
    accepted: &AcceptedCatalog<'_>,
    source_catalog: &SourceCatalog<'_>,
    retire: &RetireIntent,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> RetireTarget {
    let active = entry_indexes_with_lifecycle(entries, CatalogLifecycle::Active, &retire.path);
    let reserved = entry_indexes_with_lifecycle(entries, CatalogLifecycle::Reserved, &retire.path);
    let declared_kinds = source_catalog.kinds_at_path(&retire.path);
    if accepted.path_is_ambiguous(&retire.path)
        || matches!(
            source_catalog.path_kind(&retire.path),
            SourcePathKind::Ambiguous
        )
    {
        push_intent_path_ambiguous(
            &retire.file,
            retire.span,
            CatalogIntentKind::RetireTarget,
            &retire.path,
            accepted.path_candidates(&retire.path),
            declared_kinds,
            diagnostics,
        );
        return RetireTarget::Rejected;
    }
    match active.as_slice() {
        [index] => {
            let active_kind = entries[*index].kind;
            if source_catalog.contains(active_kind, &retire.path) {
                push_retire_source_declared(retire, diagnostics);
                RetireTarget::Rejected
            } else if !declared_kinds.is_empty() {
                push_intent_path_ambiguous(
                    &retire.file,
                    retire.span,
                    CatalogIntentKind::RetireTarget,
                    &retire.path,
                    accepted.path_candidates(&retire.path),
                    declared_kinds,
                    diagnostics,
                );
                RetireTarget::Rejected
            } else {
                RetireTarget::Active(*index)
            }
        }
        [] => {
            if declared_kinds.is_empty() {
                match reserved.as_slice() {
                    [] => {
                        report_unresolved_intent(&retire.file, retire.span, diagnostics);
                        RetireTarget::Rejected
                    }
                    [_] => RetireTarget::Consumed,
                    _ => {
                        push_intent_path_ambiguous(
                            &retire.file,
                            retire.span,
                            CatalogIntentKind::RetireTarget,
                            &retire.path,
                            accepted.path_candidates(&retire.path),
                            declared_kinds,
                            diagnostics,
                        );
                        RetireTarget::Rejected
                    }
                }
            } else {
                push_retire_source_declared(retire, diagnostics);
                RetireTarget::Rejected
            }
        }
        _ => {
            push_intent_path_ambiguous(
                &retire.file,
                retire.span,
                CatalogIntentKind::RetireTarget,
                &retire.path,
                accepted.path_candidates(&retire.path),
                declared_kinds,
                diagnostics,
            );
            RetireTarget::Rejected
        }
    }
}

fn entry_indexes_with_lifecycle(
    entries: &[CatalogEntry],
    lifecycle: CatalogLifecycle,
    path: &str,
) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            (entry.lifecycle == lifecycle && entry.path == path).then_some(index)
        })
        .collect()
}

/// Remove from the proposal each active store-index entry current source no longer declares,
/// returning whether any was dropped. An index is derived schema, not user data: its entries
/// rebuild from the records it covers, so a source drop removes its catalog entry outright
/// rather than reserving it. Dropping the entry advances the epoch and publishes a catalog
/// without the index; the discharge stages the deletion of its generated cells from the
/// accepted snapshot in the same activation. A re-declared index later mints fresh identity,
/// which is sound because the index holds no durable identity of its own.
fn drop_absent_indexes(
    entries: &mut Vec<CatalogEntry>,
    source_catalog: &SourceCatalog<'_>,
) -> bool {
    let before = entries.len();
    entries.retain(|entry| {
        !(entry.kind == CatalogEntryKind::StoreIndex
            && entry.lifecycle == CatalogLifecycle::Active
            && !source_catalog.contains(CatalogEntryKind::StoreIndex, entry.path.as_str()))
    });
    entries.len() != before
}

fn report_unresolved_intent(file: &Path, span: SourceSpan, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic::error(
        CHECK_EVOLVE_TARGET,
        file,
        span,
        "evolve target does not name an accepted catalog entry to carry forward",
    ));
}

struct AcceptedCatalog<'a> {
    entries: HashMap<(CatalogEntryKind, &'a str), AcceptedEntry<'a>>,
    reserved: HashMap<(CatalogEntryKind, &'a str), AcceptedEntry<'a>>,
    path_candidates: HashMap<&'a str, Vec<AcceptedPathCandidate<'a>>>,
}

#[derive(Clone, Copy)]
struct AcceptedEntry<'a> {
    entry: &'a CatalogEntry,
}

#[derive(Clone, Copy)]
struct AcceptedPathCandidate<'a> {
    kind: CatalogEntryKind,
    lifecycle: CatalogLifecycle,
    stable_id: &'a str,
}

impl<'a> AcceptedCatalog<'a> {
    fn new(catalog: &'a CatalogMetadata) -> Self {
        let mut entries = HashMap::new();
        let mut reserved = HashMap::new();
        let mut path_candidates = HashMap::new();
        for entry in &catalog.entries {
            let binding = AcceptedEntry { entry };
            match entry.lifecycle {
                CatalogLifecycle::Active => {
                    entries.insert((entry.kind, entry.path.as_str()), binding);
                    push_accepted_path_candidate(&mut path_candidates, entry.path.as_str(), entry);
                    for alias in &entry.aliases {
                        push_accepted_path_candidate(&mut path_candidates, alias.as_str(), entry);
                    }
                }
                CatalogLifecycle::Reserved => {
                    reserved.insert((entry.kind, entry.path.as_str()), binding);
                    push_accepted_path_candidate(&mut path_candidates, entry.path.as_str(), entry);
                    for alias in &entry.aliases {
                        reserved.insert((entry.kind, alias.as_str()), binding);
                        push_accepted_path_candidate(&mut path_candidates, alias.as_str(), entry);
                    }
                }
            }
        }
        Self {
            entries,
            reserved,
            path_candidates,
        }
    }

    fn active_entry(&self, kind: CatalogEntryKind, path: &str) -> Option<AcceptedEntry<'a>> {
        self.entries.get(&(kind, path)).copied()
    }

    fn reserved_entry(&self, kind: CatalogEntryKind, path: &str) -> Option<AcceptedEntry<'a>> {
        self.reserved.get(&(kind, path)).copied()
    }

    fn path_candidates(&self, path: &str) -> &[AcceptedPathCandidate<'a>] {
        self.path_candidates
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn path_is_ambiguous(&self, path: &str) -> bool {
        self.path_candidates(path).len() > 1
    }
}

fn push_accepted_path_candidate<'a>(
    path_candidates: &mut HashMap<&'a str, Vec<AcceptedPathCandidate<'a>>>,
    path: &'a str,
    entry: &'a CatalogEntry,
) {
    let candidate = AcceptedPathCandidate {
        kind: entry.kind,
        lifecycle: entry.lifecycle,
        stable_id: entry.stable_id.as_str(),
    };
    let candidates = path_candidates.entry(path).or_default();
    if !candidates.iter().any(|existing| {
        existing.kind == candidate.kind
            && existing.lifecycle == candidate.lifecycle
            && existing.stable_id == candidate.stable_id
    }) {
        candidates.push(candidate);
    }
}

/// The leaf-position facts of a resource member holding a single value cell (a plain field or a
/// keyed-leaf layer). The module resolves an unqualified enum referent, and the value type
/// yields the value half of the member's identity-aware leaf token.
#[derive(Debug)]
pub(crate) struct MemberLeaf {
    pub(crate) module: String,
    pub(crate) ty: marrow_schema::Type,
}

#[derive(Debug)]
pub(crate) struct SourceCatalogEntry {
    pub(crate) kind: CatalogEntryKind,
    pub(crate) path: String,
    pub(crate) file: std::path::PathBuf,
    pub(crate) span: SourceSpan,
    /// The leaf-position facts of a resource member, `None` for a group. With `key_params`,
    /// these feed the identity-aware leaf token, so a value-type or key-shape change is detected
    /// by identity rather than by source spelling.
    pub(crate) leaf: Option<MemberLeaf>,
    /// The member's key-param shape: empty for a plain field or unkeyed group, non-empty for a
    /// keyed group or keyed-leaf layer. The structural signature reads this so a
    /// group<->keyed-group reshape or a re-key is a different signature. A leaf member's own key
    /// shape lives in its `leaf` facts; this is the only place a group's key shape lives.
    pub(crate) key_params: Vec<marrow_schema::KeyDef>,
}

impl SourceCatalogEntry {
    /// A non-leaf source entry (resource, store, store index, enum, or enum member): one that
    /// holds no value cell and declares no key params.
    fn group(
        kind: CatalogEntryKind,
        path: String,
        module: &crate::CheckedModule,
        span: SourceSpan,
    ) -> Self {
        Self {
            kind,
            path,
            file: module.source_file.clone(),
            span,
            leaf: None,
            key_params: Vec::new(),
        }
    }
}

pub(crate) fn source_catalog_entries(program: &CheckedProgram) -> Vec<SourceCatalogEntry> {
    let mut entries = Vec::new();
    let spans = source_catalog_spans(program);
    for module in &program.modules {
        for resource in &module.resources {
            let path = resource_path(&module.name, &resource.name);
            let span = span_for(&spans, CatalogEntryKind::Resource, &path);
            entries.push(SourceCatalogEntry::group(
                CatalogEntryKind::Resource,
                path,
                module,
                span,
            ));
            collect_resource_members(
                &mut entries,
                module,
                &resource.name,
                &[],
                &resource.members,
                &spans,
            );
        }
        for store in &module.stores {
            let path = store_path(&module.name, &store.root);
            let span = span_for(&spans, CatalogEntryKind::Store, &path);
            entries.push(SourceCatalogEntry::group(
                CatalogEntryKind::Store,
                path,
                module,
                span,
            ));
            for index in &store.indexes {
                let path = store_index_path(&module.name, &store.root, &index.name);
                let span = span_for(&spans, CatalogEntryKind::StoreIndex, &path);
                entries.push(SourceCatalogEntry::group(
                    CatalogEntryKind::StoreIndex,
                    path,
                    module,
                    span,
                ));
            }
        }
        for enum_schema in &module.enums {
            let path = enum_path(&module.name, &enum_schema.name);
            let span = span_for(&spans, CatalogEntryKind::Enum, &path);
            entries.push(SourceCatalogEntry::group(
                CatalogEntryKind::Enum,
                path,
                module,
                span,
            ));
            for index in 0..enum_schema.members.len() {
                let path = enum_member_path(&module.name, &enum_schema.name, index, enum_schema);
                let span = span_for(&spans, CatalogEntryKind::EnumMember, &path);
                entries.push(SourceCatalogEntry::group(
                    CatalogEntryKind::EnumMember,
                    path,
                    module,
                    span,
                ));
            }
        }
    }
    entries
}

fn collect_resource_members(
    entries: &mut Vec<SourceCatalogEntry>,
    module: &crate::CheckedModule,
    resource: &str,
    parent_path: &[String],
    nodes: &[marrow_schema::Node],
    spans: &HashMap<CatalogKey, SourceSpan>,
) {
    for node in nodes {
        let mut path = parent_path.to_vec();
        path.push(node.name.clone());
        let catalog_path = resource_member_path(&module.name, resource, &path);
        entries.push(SourceCatalogEntry {
            kind: CatalogEntryKind::ResourceMember,
            span: span_for(spans, CatalogEntryKind::ResourceMember, &catalog_path),
            path: catalog_path,
            file: module.source_file.clone(),
            leaf: member_leaf(module, node),
            key_params: node.key_params.clone(),
        });
        collect_resource_members(entries, module, resource, &path, &node.members, spans);
    }
}

fn source_catalog_spans(program: &CheckedProgram) -> HashMap<CatalogKey, SourceSpan> {
    let mut spans = HashMap::new();
    for resource in program.facts.resources() {
        let module = &program.modules[resource.module.0 as usize];
        spans.insert(
            CatalogKey::new(
                CatalogEntryKind::Resource,
                resource_path(&module.name, &resource.name),
            ),
            resource.span,
        );
    }
    for store in program.facts.stores() {
        let module = &program.modules[store.module.0 as usize];
        spans.insert(
            CatalogKey::new(
                CatalogEntryKind::Store,
                store_path(&module.name, &store.root),
            ),
            store.span,
        );
    }
    for index in program.facts.store_indexes() {
        let store = program.facts.store(index.store);
        let module = &program.modules[store.module.0 as usize];
        spans.insert(
            CatalogKey::new(
                CatalogEntryKind::StoreIndex,
                store_index_path(&module.name, &store.root, &index.name),
            ),
            index.span,
        );
    }
    for member in program.facts.resource_members() {
        if let Some(path) = program.facts.resource_member_catalog_path(member.id) {
            spans.insert(
                CatalogKey::new(CatalogEntryKind::ResourceMember, path),
                member.span,
            );
        }
    }
    for enum_fact in program.facts.enums() {
        let module = &program.modules[enum_fact.module.0 as usize];
        spans.insert(
            CatalogKey::new(
                CatalogEntryKind::Enum,
                enum_path(&module.name, &enum_fact.name),
            ),
            enum_fact.span,
        );
    }
    for member in program.facts.enum_members() {
        if let Some(path) = program.facts.enum_member_catalog_path(member.id) {
            spans.insert(
                CatalogKey::new(CatalogEntryKind::EnumMember, path),
                member.span,
            );
        }
    }
    spans
}

fn span_for(
    spans: &HashMap<CatalogKey, SourceSpan>,
    kind: CatalogEntryKind,
    path: &str,
) -> SourceSpan {
    spans
        .get(&CatalogKey::new(kind, path.to_string()))
        .copied()
        .unwrap_or_default()
}

/// The declaring module and value type a resource member stores its durable bytes as, or `None`
/// for a group. A plain field records its own type; a keyed-leaf layer records its
/// value type V, since the map field is itself the leaf its entries' values are stored under.
fn member_leaf(module: &crate::CheckedModule, node: &marrow_schema::Node) -> Option<MemberLeaf> {
    node.leaf_value_type().map(|ty| MemberLeaf {
        module: module.name.clone(),
        ty: ty.clone(),
    })
}

/// A source entity the accepted catalog does not yet record has no durable identity until a
/// state-establishing flow commits one. That pending state is informational, not a failure, so
/// `check` stays read-only and exits clean.
fn push_pending_identity(source: &SourceCatalogEntry, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic::warning(
        CHECK_CATALOG_INTENT,
        &source.file,
        source.span,
        format!(
            "durable identity for `{}` is not yet recorded; running the program or applying an evolution will record it",
            source.path
        ),
    ));
}

fn push_rename_source_declared(rename: &RenameIntent, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(catalog_error(
        rename.file.clone(),
        rename.span,
        format!(
            "rename source `{}` is still declared; a rename must remove the old spelling",
            rename.from_path
        ),
    ));
}

fn push_retire_source_declared(retire: &RetireIntent, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(catalog_error(
        retire.file.clone(),
        retire.span,
        format!(
            "retire target `{}` is still declared by source; remove the declaration before retiring it",
            retire.path
        ),
    ));
}

fn push_intent_path_ambiguous(
    file: &Path,
    span: SourceSpan,
    intent: CatalogIntentKind,
    path: &str,
    accepted: &[AcceptedPathCandidate<'_>],
    declared_kinds: &[CatalogEntryKind],
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let accepted_payload = accepted_path_payload(accepted);
    diagnostics.push(
        catalog_error(
            file.to_path_buf(),
            span,
            format!(
                "{} `{path}` is ambiguous across catalog entry kinds; accepted entries: {}; source kinds: {}",
                catalog_intent_label(intent),
                format_accepted_path_candidates(&accepted_payload),
                format_catalog_kinds(declared_kinds)
            ),
        )
        .with_payload(DiagnosticPayload::CatalogIntent(
            CatalogIntentDiagnostic::AmbiguousPath {
                intent,
                path: path.to_string(),
                accepted: accepted_payload,
                source: declared_kinds.to_vec(),
            },
        )),
    );
}

fn catalog_intent_label(intent: CatalogIntentKind) -> &'static str {
    match intent {
        CatalogIntentKind::RetireTarget => "retire target",
        CatalogIntentKind::RenameSource => "rename source",
        CatalogIntentKind::RenameTarget => "rename target",
    }
}

fn accepted_path_payload(accepted: &[AcceptedPathCandidate<'_>]) -> Vec<CatalogPathCandidate> {
    accepted
        .iter()
        .map(|candidate| CatalogPathCandidate {
            kind: candidate.kind,
            lifecycle: candidate.lifecycle,
            stable_id: candidate.stable_id.to_string(),
        })
        .collect()
}

fn format_accepted_path_candidates(candidates: &[CatalogPathCandidate]) -> String {
    if candidates.is_empty() {
        return "none".to_string();
    }
    candidates
        .iter()
        .map(|candidate| {
            format!(
                "{:?}/{:?}/{}",
                candidate.kind, candidate.lifecycle, candidate.stable_id
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_catalog_kinds(kinds: &[CatalogEntryKind]) -> String {
    if kinds.is_empty() {
        return "none".to_string();
    }
    kinds
        .iter()
        .map(|kind| format!("{kind:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn push_rename_conflict(rename: &RenameIntent, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(catalog_error(
        rename.file.clone(),
        rename.span,
        format!(
            "rename `{}` -> `{}` conflicts with another rename of the same source or target",
            rename.from_path, rename.to_path
        ),
    ));
}

fn push_rename_target_live(source: &SourceCatalogEntry, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(catalog_error(
        source.file.clone(),
        source.span,
        format!(
            "rename target `{}` already names a live entity; identity cannot be moved onto it",
            source.path
        ),
    ));
}

fn push_reserved_reuse(
    source: &SourceCatalogEntry,
    reserved: &CatalogEntry,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_CATALOG_INTENT,
            &source.file,
            source.span,
            format!(
                "`{}` is reserved by catalog id `{}` and cannot be reused",
                source.path, reserved.stable_id
            ),
        )
        .with_payload(DiagnosticPayload::ReservedCatalogPathReuse {
            source_kind: source.kind,
            source_path: source.path.clone(),
            reserved_stable_id: reserved.stable_id.clone(),
        }),
    );
}

fn prepare_proposal_path(entries: &mut [CatalogEntry], kind: CatalogEntryKind, path: &str) {
    for entry in entries.iter_mut().filter(|entry| entry.kind == kind) {
        if entry.lifecycle != CatalogLifecycle::Reserved {
            entry.aliases.retain(|alias| alias != path);
        }
    }
}

pub(crate) fn resource_path(module: &str, resource: &str) -> String {
    qualified(module, resource)
}

pub(crate) fn store_path(module: &str, root: &str) -> String {
    qualified(module, &format!("^{root}"))
}

pub(crate) fn store_index_path(module: &str, root: &str, index: &str) -> String {
    format!("{}::{index}", store_path(module, root))
}

pub(crate) fn resource_member_path(module: &str, resource: &str, members: &[String]) -> String {
    format!(
        "{}::{}",
        resource_path(module, resource),
        members.join("::")
    )
}

pub(crate) fn enum_path(module: &str, enum_name: &str) -> String {
    qualified(module, enum_name)
}

pub(crate) fn enum_member_path(
    module: &str,
    enum_name: &str,
    ordinal: usize,
    schema: &marrow_schema::EnumSchema,
) -> String {
    let path = schema.member_path(ordinal);
    format!("{}::{}", enum_path(module, enum_name), path.join("::"))
}

fn qualified(module: &str, item: &str) -> String {
    if module.is_empty() {
        item.to_string()
    } else {
        format!("{module}::{item}")
    }
}

fn proposed_catalog_entry(
    source: &SourceCatalogEntry,
    allocator: &mut StableIdAllocator,
) -> CatalogEntry {
    CatalogEntry {
        kind: source.kind,
        path: source.path.clone(),
        stable_id: allocator.allocate(),
        aliases: Vec::new(),
        lifecycle: CatalogLifecycle::Active,
        // Source-derived shape signatures are recorded in post-passes once every referent's id
        // is bound; freshly minted entries start without them.
        accepted_key_shape: None,
        accepted_index_shape: None,
        accepted_struct: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_entry(kind: CatalogEntryKind, path: &str, stable_id: &str) -> CatalogEntry {
        CatalogEntry {
            kind,
            path: path.to_string(),
            stable_id: stable_id.to_string(),
            aliases: Vec::new(),
            lifecycle: CatalogLifecycle::Active,
            accepted_key_shape: None,
            accepted_index_shape: None,
            accepted_struct: None,
        }
    }

    fn source_entry(kind: CatalogEntryKind, path: &str) -> SourceCatalogEntry {
        SourceCatalogEntry {
            kind,
            path: path.to_string(),
            file: std::path::PathBuf::from("src/books.mw"),
            span: SourceSpan::default(),
            leaf: None,
            key_params: Vec::new(),
        }
    }

    fn retire(path: &str) -> RetireIntent {
        RetireIntent {
            path: path.to_string(),
            file: std::path::PathBuf::from("src/books.mw"),
            span: SourceSpan::default(),
        }
    }

    #[test]
    fn path_only_retire_fails_closed_when_source_kind_collides_with_accepted_kind() {
        let path = "books::Color::red";
        let stable_id = "cat_000000000000000000000000000000aa";
        let mut entries = vec![active_entry(
            CatalogEntryKind::ResourceMember,
            path,
            stable_id,
        )];
        let accepted = CatalogMetadata::new(1, entries.clone());
        let source = vec![
            source_entry(CatalogEntryKind::Enum, "books::Color"),
            source_entry(CatalogEntryKind::EnumMember, path),
        ];
        let mut diagnostics = Vec::new();

        let changed = apply_retires(
            &mut entries,
            &[retire(path)],
            &AcceptedCatalog::new(&accepted),
            &SourceCatalog::new(&source),
            &mut diagnostics,
        );

        assert!(
            !changed,
            "an ambiguous path-only retire must not reserve an entry"
        );
        assert_eq!(entries[0].lifecycle, CatalogLifecycle::Active);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, CHECK_CATALOG_INTENT);
        assert_eq!(
            diagnostics[0].payload,
            DiagnosticPayload::CatalogIntent(CatalogIntentDiagnostic::AmbiguousPath {
                intent: CatalogIntentKind::RetireTarget,
                path: path.to_string(),
                accepted: vec![CatalogPathCandidate {
                    kind: CatalogEntryKind::ResourceMember,
                    lifecycle: CatalogLifecycle::Active,
                    stable_id: stable_id.to_string(),
                }],
                source: vec![CatalogEntryKind::EnumMember],
            })
        );
    }
}
