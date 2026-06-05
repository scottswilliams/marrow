use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use marrow_project::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};
use marrow_store::cell::CatalogId;
use marrow_syntax::{Severity, SourceSpan};

use crate::evolution::leaf_type;
use crate::evolution::{DefaultIntent, EvolveIntents, RenameIntent, RetireIntent, TransformIntent};
use crate::program::{EvolveDefault, EvolveTransform};
use crate::{CHECK_CATALOG_INTENT, CHECK_EVOLVE_TARGET, CheckDiagnostic, CheckedProgram};

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
    /// The `(kind, path) -> stable id` map for resolving a member's referent enum or store
    /// to its identity-aware leaf token. It covers proposal-only ids the accepted-only `ids`
    /// omits, and is never bound onto live facts.
    pub(crate) leaf_token_ids: HashMap<CatalogKey, String>,
    pub(crate) proposal: Option<CatalogMetadata>,
}

pub(crate) fn bind_catalog(
    project_root: &Path,
    config: &marrow_project::ProjectConfig,
    program: &mut CheckedProgram,
    evolve: &EvolveIntents,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let accepted = read_accepted_catalog(project_root, config, diagnostics);
    let binding = catalog_binding(program, accepted.as_ref(), evolve, diagnostics);
    let declared_store_key_shapes = declared_store_key_shapes(program, &binding.leaf_token_ids);
    let declared_member_structs = declared_member_structs(program, &binding.leaf_token_ids);
    program
        .facts
        .bind_catalog_ids(&program.modules, &binding.ids);
    program.catalog.accepted_epoch = binding.accepted_epoch;
    program.catalog.accepted_digest = binding.accepted_digest;
    program.catalog.accepted_entries = accepted.map(|catalog| catalog.entries).unwrap_or_default();
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

/// The current source's identity-key shape token for each store, keyed by its bound stable
/// catalog id. The token is the comma-joined key types in order, so discharge can detect a
/// key arity or key-type change even when the program is otherwise unchanged and emits no
/// proposal. A store with no bound identity (pending first-run identity) is omitted, the same
/// way a member with no bound identity is.
fn declared_store_key_shapes(
    program: &CheckedProgram,
    ids: &HashMap<CatalogKey, String>,
) -> HashMap<String, String> {
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

/// The current source's identity-aware structural signature for each resource member, keyed
/// by its bound stable catalog id. The signature records the member's kind, its key shape if a
/// keyed layer, and its leaf token if a leaf, so discharge compares it against the accepted
/// signature and fails closed on a structural divergence the leaf-token, reshape, rename,
/// default, transform, and retire classifiers all leave unclaimed. Read from source so a
/// divergence is detected even when the program is otherwise unchanged and emits no proposal. A
/// member with no bound identity (a pending first-run identity) or an unresolved leaf referent
/// is omitted, the same way the store key shapes are.
fn declared_member_structs(
    program: &CheckedProgram,
    ids: &HashMap<CatalogKey, String>,
) -> HashMap<String, String> {
    source_catalog_entries(program)
        .into_iter()
        .filter(|source| source.kind == CatalogEntryKind::ResourceMember)
        .filter_map(|source| {
            let module = member_struct_module(&source);
            let leaf = source.leaf.as_ref().map(|leaf| &leaf.ty);
            let token =
                leaf_type::member_struct_token(program, module, leaf, &source.key_params, ids)?;
            let catalog_id = ids.get(&CatalogKey::new(source.kind, source.path))?;
            Some((catalog_id.clone(), token))
        })
        .collect()
}

/// The declaring module a member's structural signature resolves its leaf referent under: a
/// leaf member's own declaring module, or the empty string for a group (whose signature needs
/// no referent resolution).
fn member_struct_module(source: &SourceCatalogEntry) -> &str {
    source
        .leaf
        .as_ref()
        .map(|leaf| leaf.module.as_str())
        .unwrap_or("")
}

fn read_accepted_catalog(
    project_root: &Path,
    config: &marrow_project::ProjectConfig,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<CatalogMetadata> {
    let path = project_root.join(&config.accepted_catalog);
    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            diagnostics.push(catalog_diagnostic(
                path,
                format!("could not read accepted catalog metadata: {error}"),
            ));
            return None;
        }
    };
    match CatalogMetadata::from_json(&json) {
        Ok(catalog) => Some(catalog),
        Err(error) => {
            diagnostics.push(catalog_diagnostic(
                path,
                format!("invalid accepted catalog metadata: {}", error.message),
            ));
            None
        }
    }
}

fn catalog_diagnostic(file: std::path::PathBuf, message: String) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_CATALOG_INTENT,
        severity: Severity::Error,
        file,
        message,
        span: SourceSpan::default(),
    }
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
        Some(catalog) => {
            let accepted_index = AcceptedCatalog::new(catalog);
            // Each current source catalog path mapped to the kind the source declares
            // there, computed once and shared by rename resolution and retire
            // admission so both read the same source view.
            let source_kinds = source_kinds(&source_entries);
            // A rename declares that the entity now at `to_path` is the accepted
            // entry formerly at `from_path`. Resolution is keyed by the new path so
            // the source loop can find the matching intent, and is an injective
            // partial map: a duplicate source or target, a source still declared, or
            // a target with no accepted identity to carry is a closed-by-default
            // error rather than a silent relocation.
            let mut renames =
                resolve_renames(&accepted_index, &source_kinds, &evolve.renames, diagnostics);
            let mut proposal_entries = catalog.entries.clone();
            // An accepted Active entry whose source declaration has disappeared but
            // is neither renamed nor retired stays Active here with no source
            // backing. Dropping a sparse field is a legal no-op (its data simply
            // lingers), so this is not a check-time error; classifying such an entry
            // (deprecate it, or require a retire intent when an index, invariant, or
            // alias still depends on it) is a discharge obligation, not catalog
            // binding's concern.
            let mut allocator = StableIdAllocator::over(&proposal_entries);
            let mut changed = false;
            for source in &source_entries {
                let rename = renames.remove(&source.path);
                if let Some(binding) = accepted_index.active_entry(source.kind, &source.path) {
                    // A rename onto a path that already names a live accepted entity
                    // cannot move identity there; the declared intent is a no-op the
                    // author must resolve, so report it instead of dropping it.
                    if rename.is_some() {
                        push_rename_target_live(source, diagnostics);
                    }
                    let stable_id = binding.entry.stable_id.clone();
                    ids.insert(CatalogKey::new(source.kind, source.path.clone()), stable_id);
                } else if let Some(reserved) =
                    accepted_index.reserved_entry(source.kind, &source.path)
                {
                    push_reserved_reuse(source, reserved.entry, diagnostics);
                    changed = true;
                } else if let Some(rename) = rename {
                    apply_rename(&mut proposal_entries, source, &rename.from_path, &mut ids);
                    changed = true;
                } else {
                    push_pending_identity(source, diagnostics);
                    prepare_proposal_path(&mut proposal_entries, source.kind, &source.path);
                    proposal_entries.push(proposed_catalog_entry(source, &mut allocator));
                    changed = true;
                }
            }
            // A rename whose target the source never declares relocates nothing; the
            // declared intent must not vanish silently.
            for rename in renames.values() {
                report_unresolved_intent(&rename.file, rename.span, diagnostics);
            }
            if apply_retires(
                &mut proposal_entries,
                &evolve.retires,
                &source_kinds,
                diagnostics,
            ) {
                changed = true;
            }
            // Record each store's identity-key shape and each member's structural signature into
            // the proposal once every referent's id is bound, so a renamed enum or store resolves
            // to its preserved identity. The proposal id map covers freshly-minted referents the
            // accepted-only `ids` map does not, without binding proposal ids onto live facts. A
            // signature that differs from the accepted snapshot is a real change that advances the
            // proposal; backfilling an unknown one is not.
            let leaf_token_ids = proposal_id_map(&proposal_entries);
            if record_store_key_shapes(
                program,
                &mut proposal_entries,
                &leaf_token_ids,
                Some(catalog),
            ) {
                changed = true;
            }
            if record_member_structs(
                program,
                &mut proposal_entries,
                &leaf_token_ids,
                Some(catalog),
            ) {
                changed = true;
            }
            changed.then(|| CatalogMetadata::new(catalog.epoch + 1, proposal_entries))
        }
        None => {
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
            let leaf_token_ids = proposal_id_map(&proposal_entries);
            record_store_key_shapes(program, &mut proposal_entries, &leaf_token_ids, None);
            record_member_structs(program, &mut proposal_entries, &leaf_token_ids, None);
            Some(CatalogMetadata::new(1, proposal_entries))
        }
    };

    // The proposal is the catalog the commit path freezes when the program runs or an
    // evolution applies, so it must satisfy the same identity invariants. Validating it
    // here makes an identity collision the binding logic produced fail closed at check
    // time rather than at apply.
    if let Some(proposal) = &proposal
        && let Err(error) = proposal.validate()
    {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_CATALOG_INTENT,
            severity: Severity::Error,
            file: first_source_file(&source_entries),
            message: format!("proposed catalog metadata is not valid: {}", error.message),
            span: SourceSpan::default(),
        });
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

/// The `(kind, path) -> stable id` map of a proposal's entries, keyed by each entry's
/// current path. Unlike the accepted-only binding map, this covers freshly-minted and
/// renamed referents, so the identity-aware leaf token can resolve an enum or store the
/// accepted catalog does not yet record.
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

/// The active proposal identity map for activation-only readers. It is the same map
/// catalog binding uses for proposal-only referents, kept here so executable places
/// do not rebuild proposal identity semantics.
pub(crate) fn active_proposal_id_map(program: &CheckedProgram) -> HashMap<CatalogKey, String> {
    program
        .catalog
        .proposal
        .as_ref()
        .map(|proposal| proposal_id_map(&proposal.entries))
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationResumeRebindError {
    MissingProposal,
    ProposalIdCountMismatch,
}

pub(crate) fn rebind_activation_resume_program(
    program: &CheckedProgram,
    proposal_ids: &[CatalogId],
) -> Result<CheckedProgram, ActivationResumeRebindError> {
    let current_proposal = program
        .catalog
        .proposal
        .clone()
        .ok_or(ActivationResumeRebindError::MissingProposal)?;
    let current_entries = current_proposal.entries.clone();
    let accepted_ids: HashSet<_> = program
        .catalog
        .accepted_entries
        .iter()
        .map(|entry| entry.stable_id.as_str())
        .collect();
    let mut entries = current_proposal.entries;
    let mut proposal_ids = proposal_ids.iter();
    for entry in &mut entries {
        if !accepted_ids.contains(entry.stable_id.as_str()) {
            entry.stable_id = proposal_ids
                .next()
                .ok_or(ActivationResumeRebindError::ProposalIdCountMismatch)?
                .as_str()
                .to_string();
        }
    }
    if proposal_ids.next().is_some() {
        return Err(ActivationResumeRebindError::ProposalIdCountMismatch);
    }

    let replacement_ids = proposal_id_map(&entries);
    let accepted = CatalogMetadata::new(
        program.catalog.accepted_epoch.unwrap_or_default(),
        program.catalog.accepted_entries.clone(),
    );
    record_store_key_shapes(program, &mut entries, &replacement_ids, Some(&accepted));
    record_member_structs(program, &mut entries, &replacement_ids, Some(&accepted));
    let proposal = CatalogMetadata::new(current_proposal.epoch, entries);
    let replacement_ids = proposal_id_map(&proposal.entries);
    let current_paths = active_id_paths(&current_entries);

    let mut rebound = program.clone();
    rebound.catalog.evolve_defaults = rebound
        .catalog
        .evolve_defaults
        .iter()
        .map(|default| {
            let mut rebound_default = default.clone();
            rebound_default.catalog_id = rebind_catalog_id(
                CatalogEntryKind::ResourceMember,
                &default.catalog_id,
                &current_paths,
                &replacement_ids,
            );
            rebound_default
        })
        .collect();
    rebound.catalog.evolve_transforms = rebound
        .catalog
        .evolve_transforms
        .iter()
        .map(|transform| {
            let mut rebound_transform = transform.clone();
            rebound_transform.catalog_id =
                member_target_id(&transform.target_path, &replacement_ids).or_else(|| {
                    transform.catalog_id.as_deref().map(|catalog_id| {
                        rebind_catalog_id(
                            CatalogEntryKind::ResourceMember,
                            catalog_id,
                            &current_paths,
                            &replacement_ids,
                        )
                    })
                });
            rebound_transform.reads = transform
                .reads
                .iter()
                .map(|read| {
                    rebind_catalog_id(
                        CatalogEntryKind::ResourceMember,
                        read,
                        &current_paths,
                        &replacement_ids,
                    )
                })
                .collect();
            rebound_transform
        })
        .collect();
    rebound.catalog.proposal = Some(proposal);
    rebound.catalog.declared_store_key_shapes =
        declared_store_key_shapes(&rebound, &replacement_ids);
    rebound.catalog.declared_member_structs = declared_member_structs(&rebound, &replacement_ids);
    Ok(rebound)
}

fn active_id_paths(entries: &[CatalogEntry]) -> HashMap<(CatalogEntryKind, String), String> {
    entries
        .iter()
        .filter(|entry| entry.lifecycle == CatalogLifecycle::Active)
        .map(|entry| ((entry.kind, entry.stable_id.clone()), entry.path.clone()))
        .collect()
}

fn rebind_catalog_id(
    kind: CatalogEntryKind,
    stable_id: &str,
    current_paths: &HashMap<(CatalogEntryKind, String), String>,
    replacement_ids: &HashMap<CatalogKey, String>,
) -> String {
    current_paths
        .get(&(kind, stable_id.to_string()))
        .and_then(|path| replacement_ids.get(&CatalogKey::new(kind, path.clone())))
        .cloned()
        .unwrap_or_else(|| stable_id.to_string())
}

/// The stable id a member-target evolve path binds to, or `None` when it names no
/// resource member (the type pass already reported it). A default or transform
/// targets a resource member, so it is keyed by `ResourceMember`.
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
    source_kinds: &HashMap<&str, CatalogEntryKind>,
    renames: &[RenameIntent],
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> HashMap<String, ResolvedRename> {
    let mut resolved: HashMap<String, ResolvedRename> = HashMap::new();
    let mut from_paths: HashSet<String> = HashSet::new();
    for rename in renames {
        let Some(&kind) = source_kinds.get(rename.to_path.as_str()) else {
            // The new spelling names no current source entity, so there is nothing
            // to carry identity onto; report it rather than relocate blindly.
            report_unresolved_intent(&rename.file, rename.span, diagnostics);
            continue;
        };
        if rename_already_recorded(accepted, kind, rename) {
            // The accepted catalog already carries this entity at `to_path` with
            // `from_path` recorded as an alias, so a prior apply consumed this rename.
            // The block is a transient transition the author may keep or delete; the
            // identity is already moved, so there is nothing to relocate and no error.
            continue;
        }
        let duplicate_target = resolved.contains_key(&rename.to_path);
        let duplicate_source = !from_paths.insert(rename.from_path.clone());
        if duplicate_target || duplicate_source {
            push_rename_conflict(rename, diagnostics);
            continue;
        }
        if source_kinds.get(rename.from_path.as_str()) == Some(&kind) {
            push_rename_source_declared(rename, diagnostics);
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

/// Whether a prior apply already carried this rename into the accepted catalog: the
/// live entry now sits at `to_path` and records `from_path` among its aliases. A
/// consumed rename block is a transient transition the author may keep or delete, so
/// it relocates nothing and is not an unresolved intent.
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

/// Map each current source catalog path to the kind the source declares there.
fn source_kinds(source_entries: &[SourceCatalogEntry]) -> HashMap<&str, CatalogEntryKind> {
    source_entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.kind))
        .collect()
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
///
/// A store accepted before key shapes were recorded carries no accepted shape; filling it
/// from current source is not a re-key, since the durable keys are unchanged. With no accepted
/// snapshot (a first-run catalog) every shape is recorded without flagging change, because the
/// whole catalog is new. The discharge fail-closes on a real re-key; this only freezes the
/// shape so a later change has a baseline to compare against.
fn record_store_key_shapes(
    program: &CheckedProgram,
    entries: &mut [CatalogEntry],
    ids: &HashMap<CatalogKey, String>,
    accepted: Option<&CatalogMetadata>,
) -> bool {
    let accepted_shapes: HashMap<&str, &Option<String>> = accepted
        .map(|catalog| {
            catalog
                .entries
                .iter()
                .filter(|entry| entry.kind == CatalogEntryKind::Store)
                .map(|entry| (entry.stable_id.as_str(), &entry.accepted_key_shape))
                .collect()
        })
        .unwrap_or_default();
    let mut changed = false;
    for module in &program.modules {
        for store in &module.stores {
            let key = CatalogKey::new(
                CatalogEntryKind::Store,
                store_path(&module.name, &store.root),
            );
            let Some(stable_id) = ids.get(&key) else {
                continue;
            };
            let Some(entry) = entries
                .iter_mut()
                .find(|entry| &entry.stable_id == stable_id)
            else {
                continue;
            };
            let shape = Some(leaf_type::store_key_shape_token(&store.identity_keys));
            let accepted_shape = accepted_shapes.get(stable_id.as_str()).copied();
            if let Some(Some(_)) = accepted_shape
                && accepted_shape != Some(&shape)
            {
                changed = true;
            }
            entry.accepted_key_shape = shape;
        }
    }
    changed
}

/// Record each resource member's identity-aware structural signature into its proposal entry,
/// once every referent's id is bound. The signature covers leaf and group members alike, so a
/// keyed-layer re-key, a group<->keyed-group reshape, or any other structural transition reads
/// as a different signature. Returns whether any member's signature is a real change relative to
/// the accepted snapshot, so an otherwise-unchanged program that only reshaped a member still
/// advances the proposal.
///
/// A member accepted before signatures were recorded carries none; filling it from current
/// source is not a change, since the durable shape is unchanged. With no accepted snapshot (a
/// first-run catalog) every signature is recorded without flagging change, because the whole
/// catalog is new. The discharge fail-closes on a real divergence; this only freezes the
/// signature so a later change has a baseline to compare against.
fn record_member_structs(
    program: &CheckedProgram,
    entries: &mut [CatalogEntry],
    ids: &HashMap<CatalogKey, String>,
    accepted: Option<&CatalogMetadata>,
) -> bool {
    let accepted_structs: HashMap<&str, &Option<String>> = accepted
        .map(|catalog| {
            catalog
                .entries
                .iter()
                .filter(|entry| entry.kind == CatalogEntryKind::ResourceMember)
                .map(|entry| (entry.stable_id.as_str(), &entry.accepted_struct))
                .collect()
        })
        .unwrap_or_default();
    let mut changed = false;
    for source in source_catalog_entries(program) {
        if source.kind != CatalogEntryKind::ResourceMember {
            continue;
        }
        let module = member_struct_module(&source);
        let leaf = source.leaf.as_ref().map(|leaf| &leaf.ty);
        let token = leaf_type::member_struct_token(program, module, leaf, &source.key_params, ids);
        let Some(stable_id) = ids.get(&CatalogKey::new(source.kind, source.path.clone())) else {
            continue;
        };
        let Some(entry) = entries
            .iter_mut()
            .find(|entry| &entry.stable_id == stable_id)
        else {
            continue;
        };
        let accepted_struct = accepted_structs.get(stable_id.as_str()).copied();
        // A signature differing from a known accepted one is a real structural change. A member
        // with no recorded accepted signature (minted before signatures, or a fresh entry this
        // cycle) has an unchanged durable shape, so recording its signature forward is not a
        // change.
        if let Some(Some(_)) = accepted_struct
            && accepted_struct != Some(&token)
        {
            changed = true;
        }
        entry.accepted_struct = token;
    }
    changed
}

/// Mark each retired entity reserved in the proposal. A retire names a destructive
/// intent over an accepted entry whose source declaration is gone; a path that
/// matches no active accepted entry is a target diagnostic. A retire of an entry
/// the source still declares is rejected: reserving it would silently drop data
/// the running program still reads and writes, so the destructive intent only
/// applies once the source declaration is actually gone. Returns whether any entry
/// changed.
fn apply_retires(
    entries: &mut [CatalogEntry],
    retires: &[RetireIntent],
    source_kinds: &HashMap<&str, CatalogEntryKind>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> bool {
    let mut changed = false;
    for retire in retires {
        // A retire carries no kind; its path names destructive intent over an
        // accepted entry whose source declaration is gone. Fail closed whenever the
        // path is still declared by source under any kind, rather than comparing
        // against whichever same-path entry was found first: reserving a still-declared
        // entry would drop data the running program still reads and writes.
        // Once no source entry declares the path, the lone active accepted entry
        // there is genuinely orphaned and safe to remove.
        if source_kinds.contains_key(retire.path.as_str()) {
            push_retire_source_declared(retire, diagnostics);
            continue;
        }
        // A prior apply that already reserved this path leaves a transient retire
        // block the author may keep or delete: the entry is gone, so there is nothing
        // left to retire and no error. A path with no entry of any lifecycle names
        // nothing and stays an unresolved intent.
        let already_recorded = retire_already_recorded(entries, &retire.path);
        match entries
            .iter_mut()
            .find(|entry| entry.lifecycle == CatalogLifecycle::Active && entry.path == retire.path)
        {
            Some(entry) => {
                entry.lifecycle = CatalogLifecycle::Reserved;
                changed = true;
            }
            None if already_recorded => {}
            None => report_unresolved_intent(&retire.file, retire.span, diagnostics),
        }
    }
    changed
}

/// Whether a prior apply already reserved this path, so a retire block left in
/// source is a consumed transition rather than an unresolved intent.
fn retire_already_recorded(entries: &[CatalogEntry], path: &str) -> bool {
    entries
        .iter()
        .any(|entry| entry.lifecycle == CatalogLifecycle::Reserved && entry.path == path)
}

fn report_unresolved_intent(file: &Path, span: SourceSpan, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_EVOLVE_TARGET,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: "evolve target does not name an accepted catalog entry to carry forward"
            .to_string(),
        span,
    });
}

struct AcceptedCatalog<'a> {
    entries: HashMap<(CatalogEntryKind, &'a str), AcceptedEntry<'a>>,
    reserved: HashMap<(CatalogEntryKind, &'a str), AcceptedEntry<'a>>,
}

#[derive(Clone, Copy)]
struct AcceptedEntry<'a> {
    entry: &'a CatalogEntry,
}

impl<'a> AcceptedCatalog<'a> {
    fn new(catalog: &'a CatalogMetadata) -> Self {
        let mut entries = HashMap::new();
        let mut reserved = HashMap::new();
        for entry in &catalog.entries {
            let binding = AcceptedEntry { entry };
            match entry.lifecycle {
                CatalogLifecycle::Active => {
                    entries.insert((entry.kind, entry.path.as_str()), binding);
                }
                CatalogLifecycle::Reserved => {
                    reserved.insert((entry.kind, entry.path.as_str()), binding);
                    for alias in &entry.aliases {
                        reserved.insert((entry.kind, alias.as_str()), binding);
                    }
                }
                CatalogLifecycle::Deprecated => {}
            }
        }
        Self { entries, reserved }
    }

    fn active_entry(&self, kind: CatalogEntryKind, path: &str) -> Option<AcceptedEntry<'a>> {
        self.entries.get(&(kind, path)).copied()
    }

    fn reserved_entry(&self, kind: CatalogEntryKind, path: &str) -> Option<AcceptedEntry<'a>> {
        self.reserved.get(&(kind, path)).copied()
    }
}

/// The leaf-position facts of a resource member that holds a single value cell: a plain
/// field or a keyed-leaf layer. The declaring module resolves an unqualified enum referent,
/// and the value type yields the value half of the member's identity-aware leaf token. The
/// member's key-param shape lives on the [`SourceCatalogEntry`], which the structural
/// signature reads, so a value-type change and a key-shape change are both detected by
/// identity.
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
    /// The leaf-position facts of a resource member, `None` for a group (which holds no
    /// single value cell). A plain field and a keyed-leaf layer (a desugared `sequence`/`map`,
    /// whose value cell is the member itself) both record their declaring module and value
    /// type. The module resolves an unqualified enum referent; the value type and the
    /// member's `key_params` together feed the identity-aware leaf token, so a later change of
    /// value type OR of key shape (a plain field becoming a keyed leaf, or its key arity/types
    /// changing) is detected by identity rather than by source spelling.
    pub(crate) leaf: Option<MemberLeaf>,
    /// The member's key-param shape: empty for a plain field or an unkeyed group, non-empty
    /// for a keyed group or a keyed-leaf layer. The structural signature reads this to tell a
    /// keyed group from a plain one (and to record its key shape), so a group<->keyed-group
    /// reshape or a keyed-group re-key is a different signature. A non-member entry leaves it
    /// empty. A leaf member's key shape is already inside its `leaf` facts; this is the only
    /// place a group's key shape lives.
    pub(crate) key_params: Vec<marrow_schema::KeyDef>,
}

pub(crate) fn source_catalog_entries(program: &CheckedProgram) -> Vec<SourceCatalogEntry> {
    let mut entries = Vec::new();
    for module in &program.modules {
        for resource in &module.resources {
            entries.push(SourceCatalogEntry {
                kind: CatalogEntryKind::Resource,
                path: resource_path(&module.name, &resource.name),
                file: module.source_file.clone(),
                span: SourceSpan::default(),
                leaf: None,
                key_params: Vec::new(),
            });
            collect_resource_members(&mut entries, module, &resource.name, &[], &resource.members);
        }
        for store in &module.stores {
            entries.push(SourceCatalogEntry {
                kind: CatalogEntryKind::Store,
                path: store_path(&module.name, &store.root),
                file: module.source_file.clone(),
                span: SourceSpan::default(),
                leaf: None,
                key_params: Vec::new(),
            });
            for index in &store.indexes {
                entries.push(SourceCatalogEntry {
                    kind: CatalogEntryKind::StoreIndex,
                    path: store_index_path(&module.name, &store.root, &index.name),
                    file: module.source_file.clone(),
                    span: SourceSpan::default(),
                    leaf: None,
                    key_params: Vec::new(),
                });
            }
        }
        for enum_schema in &module.enums {
            entries.push(SourceCatalogEntry {
                kind: CatalogEntryKind::Enum,
                path: enum_path(&module.name, &enum_schema.name),
                file: module.source_file.clone(),
                span: SourceSpan::default(),
                leaf: None,
                key_params: Vec::new(),
            });
            for index in 0..enum_schema.members.len() {
                entries.push(SourceCatalogEntry {
                    kind: CatalogEntryKind::EnumMember,
                    path: enum_member_path(&module.name, &enum_schema.name, index, enum_schema),
                    file: module.source_file.clone(),
                    span: SourceSpan::default(),
                    leaf: None,
                    key_params: Vec::new(),
                });
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
) {
    for node in nodes {
        let mut path = parent_path.to_vec();
        path.push(node.name.clone());
        entries.push(SourceCatalogEntry {
            kind: CatalogEntryKind::ResourceMember,
            path: resource_member_path(&module.name, resource, &path),
            file: module.source_file.clone(),
            span: SourceSpan::default(),
            leaf: member_leaf(module, node),
            key_params: node.key_params.clone(),
        });
        collect_resource_members(entries, module, resource, &path, &node.members);
    }
}

/// The declaring module and declared type a resource member carries its durable bytes as,
/// or `None` for a group (which holds no single leaf cell). A plain field records its own
/// type; a keyed-leaf-layer (`map[K, V]`) member records its value type V, since the map
/// field is itself the leaf its entries' values are stored under. The module resolves an
/// unqualified enum referent; both feed the identity-aware leaf token that detects a value
/// type change by referent identity across leaf kinds, so a map value retype is caught the
/// same way a plain-field retype is.
fn member_leaf(module: &crate::CheckedModule, node: &marrow_schema::Node) -> Option<MemberLeaf> {
    node.leaf_value_type().map(|ty| MemberLeaf {
        module: module.name.clone(),
        ty: ty.clone(),
    })
}

/// A source entity the accepted catalog does not yet record has no durable
/// identity until a state-establishing flow commits one. That pending state is
/// informational, not a failure: `check` stays read-only and exits clean while
/// telling the author durable identity for the entity is not yet frozen.
fn push_pending_identity(source: &SourceCatalogEntry, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_CATALOG_INTENT,
        severity: Severity::Warning,
        file: source.file.clone(),
        message: format!(
            "durable identity for `{}` is not yet recorded; running the program or applying an evolution will record it",
            source.path
        ),
        span: source.span,
    });
}

fn push_rename_source_declared(rename: &RenameIntent, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_CATALOG_INTENT,
        severity: Severity::Error,
        file: rename.file.clone(),
        message: format!(
            "rename source `{}` is still declared; a rename must remove the old spelling",
            rename.from_path
        ),
        span: rename.span,
    });
}

fn push_retire_source_declared(retire: &RetireIntent, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_CATALOG_INTENT,
        severity: Severity::Error,
        file: retire.file.clone(),
        message: format!(
            "retire target `{}` is still declared by source; remove the declaration before retiring it",
            retire.path
        ),
        span: retire.span,
    });
}

fn push_rename_conflict(rename: &RenameIntent, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_CATALOG_INTENT,
        severity: Severity::Error,
        file: rename.file.clone(),
        message: format!(
            "rename `{}` -> `{}` conflicts with another rename of the same source or target",
            rename.from_path, rename.to_path
        ),
        span: rename.span,
    });
}

fn push_rename_target_live(source: &SourceCatalogEntry, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_CATALOG_INTENT,
        severity: Severity::Error,
        file: source.file.clone(),
        message: format!(
            "rename target `{}` already names a live entity; identity cannot be moved onto it",
            source.path
        ),
        span: source.span,
    });
}

fn push_reserved_reuse(
    source: &SourceCatalogEntry,
    reserved: &CatalogEntry,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_CATALOG_INTENT,
        severity: Severity::Error,
        file: source.file.clone(),
        message: format!(
            "`{}` is reserved by catalog id `{}` and cannot be reused",
            source.path, reserved.stable_id
        ),
        span: source.span,
    });
}

fn prepare_proposal_path(entries: &mut Vec<CatalogEntry>, kind: CatalogEntryKind, path: &str) {
    entries.retain(|entry| {
        !(entry.kind == kind
            && entry.path == path
            && entry.lifecycle != CatalogLifecycle::Active
            && entry.lifecycle != CatalogLifecycle::Reserved)
    });
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
        // A store's identity-key shape and a member's structural signature are recorded in
        // post-passes once every referent's id is bound; a freshly minted entry starts without
        // either, and a leaf member's accepted leaf token is read back off its signature.
        accepted_key_shape: None,
        accepted_struct: None,
    }
}

/// Hands out catalog ids in the `cat_<32 lowercase hex>` shape as random opaque
/// 128-bit values, re-rolling against the ids already in use. Allocation is
/// independent of the entity's source path, so an id never changes when a path
/// changes, and it is random rather than a monotonic counter so two project
/// branches that each allocate identity for different entities cannot collide on
/// one id when they merge — a monotonic sequence is only safe with a single
/// coordinator, which branch-parallel work has none of. An id is frozen the moment
/// the catalog is committed and never recomputed afterward. The vanishingly rare
/// random clash (or a hand-edited or badly merged catalog) is not silently
/// tolerated: `CatalogMetadata::validate()` rejects two entries sharing a stable id,
/// and the proposal is validated at check, so a duplicate fails closed there.
struct StableIdAllocator<E = OsCatalogIdEntropy> {
    used: HashSet<String>,
    entropy: E,
}

impl StableIdAllocator<OsCatalogIdEntropy> {
    fn empty() -> Self {
        Self {
            used: HashSet::new(),
            entropy: OsCatalogIdEntropy,
        }
    }

    /// Seed the in-use set from every recorded entry regardless of lifecycle, so a
    /// retired or deprecated id is never handed back out to a new entity.
    fn over(entries: &[CatalogEntry]) -> Self {
        Self {
            used: entries
                .iter()
                .map(|entry| entry.stable_id.clone())
                .collect(),
            entropy: OsCatalogIdEntropy,
        }
    }
}

impl<E: CatalogIdEntropy> StableIdAllocator<E> {
    #[cfg(test)]
    fn with_entropy(used: HashSet<String>, entropy: E) -> Self {
        Self { used, entropy }
    }

    fn allocate(&mut self) -> String {
        loop {
            let id = catalog_id_from_bytes(self.entropy.next_id_bytes());
            if self.used.insert(id.clone()) {
                return id;
            }
        }
    }
}

trait CatalogIdEntropy {
    fn next_id_bytes(&mut self) -> [u8; 16];
}

struct OsCatalogIdEntropy;

impl CatalogIdEntropy for OsCatalogIdEntropy {
    fn next_id_bytes(&mut self) -> [u8; 16] {
        let mut bytes = [0; 16];
        fill_os_entropy(&mut bytes);
        bytes
    }
}

#[cfg(unix)]
fn fill_os_entropy(bytes: &mut [u8; 16]) {
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(bytes))
        .expect("catalog id allocation requires OS entropy");
}

#[cfg(not(unix))]
fn fill_os_entropy(_bytes: &mut [u8; 16]) {
    panic!("catalog id allocation requires an approved OS entropy source on this platform");
}

fn catalog_id_from_bytes(bytes: [u8; 16]) -> String {
    let mut id = String::with_capacity("cat_".len() + 32);
    id.push_str("cat_");
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut id, "{byte:02x}").expect("writing to a string cannot fail");
    }
    id
}

#[cfg(test)]
mod tests {
    use std::collections::{HashSet, VecDeque};

    use super::{CatalogIdEntropy, StableIdAllocator, catalog_id_from_bytes};

    struct ScriptedEntropy {
        ids: VecDeque<[u8; 16]>,
    }

    impl ScriptedEntropy {
        fn new(ids: impl IntoIterator<Item = [u8; 16]>) -> Self {
            Self {
                ids: ids.into_iter().collect(),
            }
        }
    }

    impl CatalogIdEntropy for ScriptedEntropy {
        fn next_id_bytes(&mut self) -> [u8; 16] {
            self.ids.pop_front().expect("scripted entropy exhausted")
        }
    }

    #[test]
    fn stable_id_allocator_retries_forced_entropy_collisions() {
        let collision = [0x11; 16];
        let unique = [0x22; 16];
        let mut used = HashSet::new();
        used.insert(catalog_id_from_bytes(collision));
        let mut allocator =
            StableIdAllocator::with_entropy(used, ScriptedEntropy::new([collision, unique]));

        assert_eq!(catalog_id_from_bytes(unique), allocator.allocate());
    }
}

/// A stable digest of the analyzed program's durable shape, in the same
/// `sha256:<hex>` form the catalog digest uses. This is the digest the store stamps
/// at commit and the activation-window fence enforces, so it binds exactly the facts a
/// stored snapshot must satisfy: each `resource`, `store`, `enum`, and module `const`.
///
/// It excludes the `evolve` block. A consumed block describes work already recorded in
/// the accepted catalog, so hashing it would read its deletion as schema drift; the fence
/// tracks the durable shape, not the transition that produced it.
///
/// The digest hashes the canonical formatter's rendering of those declarations rather than
/// enumerating their fields, so any shape change drifts it while a whitespace reformat does
/// not. The formatter is therefore a frozen anchor: a golden over its output pins the text,
/// so a formatter change that moved it for an unchanged shape must be handled as a
/// store-format decision rather than silently re-reading every committed snapshot as drift.
pub(crate) fn analyzed_source_digest(program: &CheckedProgram) -> String {
    digest_of(render_declarations(program, DigestScope::Shape))
}

/// A stable digest of the analyzed shape *and* the evolve decision surface, in the same
/// `sha256:<hex>` form. It binds everything [`analyzed_source_digest`] binds plus each
/// `evolve` block, so a changed evolve default value or transform body drifts it.
///
/// The evolution witness records this digest, not the shape digest, so apply aborts
/// when the source it activates no longer matches what was discharged — including a
/// transform-body edit the shape digest cannot see. The two digests divide the work:
/// the store fences on shape so a consumed block is deletable, and the witness fences on
/// shape-plus-intent so the preview-to-apply transition cannot silently change.
pub(crate) fn evolution_digest(program: &CheckedProgram) -> String {
    digest_of(render_declarations(program, DigestScope::ShapeAndEvolve))
}

/// Which declarations a digest binds. The shape digest the store stamps excludes the
/// evolve block; the evolution digest the witness records includes it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DigestScope {
    Shape,
    ShapeAndEvolve,
}

impl DigestScope {
    /// Whether a declaration of `kind` contributes to a digest at this scope.
    fn binds(self, kind: DurableKind) -> bool {
        match self {
            DigestScope::Shape => kind != DurableKind::Evolve,
            DigestScope::ShapeAndEvolve => true,
        }
    }
}

/// Render the digest-bound declarations at `scope` into the deterministically ordered
/// renderings a digest hashes.
///
/// The rendering reads each module's source file because the formatter operates on the
/// syntax tree, which the checked program drops. A source file that no longer reads or
/// parses (a checked-program invariant violation) contributes a path-tagged marker so
/// the digest stays deterministic and never silently collides with a clean rendering.
fn render_declarations(program: &CheckedProgram, scope: DigestScope) -> Vec<DurableRendering> {
    let mut entries: Vec<DurableRendering> = Vec::new();
    for module in &program.modules {
        let source = std::fs::read_to_string(&module.source_file).ok();
        let parsed = source.as_deref().map(marrow_syntax::parse_source);
        match (&source, &parsed) {
            (Some(source), Some(parsed)) => {
                for declaration in &parsed.file.declarations {
                    let Some(kind) = durable_kind(declaration).filter(|&kind| scope.binds(kind))
                    else {
                        continue;
                    };
                    entries.push(DurableRendering {
                        module: module.name.clone(),
                        kind,
                        name: declaration_name(declaration),
                        text: marrow_syntax::format_declaration_normalized(source, declaration),
                    });
                }
            }
            _ => entries.push(DurableRendering {
                module: module.name.clone(),
                kind: DurableKind::Unreadable,
                name: module.source_file.display().to_string(),
                text: String::new(),
            }),
        }
    }
    entries.sort_by(|a, b| {
        (&a.module, a.kind as u8, &a.name).cmp(&(&b.module, b.kind as u8, &b.name))
    });
    entries
}

/// Hash the ordered renderings into the canonical `sha256:<hex>` digest.
fn digest_of(entries: Vec<DurableRendering>) -> String {
    let payload = entries
        .iter()
        .map(|entry| {
            format!(
                "{}\0{}\0{}\0{}",
                entry.module, entry.kind as u8, entry.name, entry.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\0\n");
    marrow_project::sha256_digest(payload.as_bytes())
}

/// One digest-bound declaration's normalized rendering, with the keys that order it
/// deterministically: its module, declaration kind, and declaration name.
struct DurableRendering {
    module: String,
    kind: DurableKind,
    name: String,
    text: String,
}

/// The declaration kinds whose shape or transform-visible value a stored snapshot
/// must satisfy. The discriminant orders renderings deterministically within a module;
/// an evolve block carries no name, so its kind alone keeps it last.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DurableKind {
    Resource = 0,
    Store = 1,
    Enum = 2,
    Const = 3,
    Evolve = 4,
    Unreadable = 5,
}

/// The digest kind of a declaration, or `None` for a function. Transform bodies cannot
/// call user functions, but they can read module constants.
fn durable_kind(declaration: &marrow_syntax::Declaration) -> Option<DurableKind> {
    match declaration {
        marrow_syntax::Declaration::Resource(_) => Some(DurableKind::Resource),
        marrow_syntax::Declaration::Store(_) => Some(DurableKind::Store),
        marrow_syntax::Declaration::Enum(_) => Some(DurableKind::Enum),
        marrow_syntax::Declaration::Const(_) => Some(DurableKind::Const),
        marrow_syntax::Declaration::Evolve(_) => Some(DurableKind::Evolve),
        marrow_syntax::Declaration::Function(_) => None,
    }
}

/// The ordering name for a durable declaration: its declared name, the store root, or
/// the empty string for a nameless evolve block. The normalized text disambiguates
/// equal names, so this only needs a stable within-module sort key.
fn declaration_name(declaration: &marrow_syntax::Declaration) -> String {
    match declaration {
        marrow_syntax::Declaration::Resource(decl) => decl.name.clone(),
        marrow_syntax::Declaration::Store(decl) => decl.root.root.clone(),
        marrow_syntax::Declaration::Enum(decl) => decl.name.clone(),
        marrow_syntax::Declaration::Const(decl) => decl.name.clone(),
        marrow_syntax::Declaration::Evolve(_) | marrow_syntax::Declaration::Function(_) => {
            String::new()
        }
    }
}
