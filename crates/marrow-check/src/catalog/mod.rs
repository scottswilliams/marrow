use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

use marrow_catalog::{
    CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogLock, CatalogMetadata, LockEntry,
    LockLedgerTombstone,
};
use marrow_store::cell::CatalogId;
use marrow_syntax::{Declaration, EnumMember, ParsedSource, SourceSpan};

use crate::evolution::leaf_type;
use crate::evolution::{DefaultIntent, EvolveIntents, RenameIntent, RetireIntent, TransformIntent};
use crate::facts::{StoreIndexFact, StoreIndexKeySource, StoredValueMeaning};
use crate::program::{EvolveDefault, EvolveTransform};
use crate::{
    CHECK_CATALOG_INTENT, CHECK_DURABLE_STORE_REQUIRED, CHECK_EVOLVE_TARGET, CHECK_LOCK_CORRUPT,
    CatalogIntentDiagnostic, CatalogIntentKind, CatalogPathCandidate, CheckDiagnostic,
    CheckedProgram, DiagnosticPayload, PendingRecord,
};

mod source_digest;
mod stable_id;

pub(crate) use source_digest::{
    DurableRendering, analyzed_source_digest, durable_renderings_for_source, evolution_digest,
    source_and_evolution_digests, transform_identity,
};
use stable_id::{CatalogIdEntropy, StableIdAllocator};

enum CatalogProposalError {
    Allocation(io::Error),
    Catalog(marrow_catalog::CatalogError),
}

/// The result of first-run binding when no accepted store catalog is present. A committed lock
/// that adopts the current source cleanly is the accepted reference itself (`Accepted`); a fresh
/// mint or a lock the source has drifted from is a pending change (`Proposal`); a corrupt lock
/// refuses adoption (`Refused`), having already pushed the typed [`CHECK_LOCK_CORRUPT`].
///
/// The accepted reference is the same [`CatalogMetadata`] a live store snapshot would carry: its
/// epoch, digest, and entries are derived through the one catalog owner, so a fresh checkout that
/// adopts the lock binds the identical accepted digest a present store binds. A read-only context
/// digest folds in that accepted digest, so deriving it here keeps the surface ABI stable whether
/// a checkout reads its identity from the committed lock or from a present (or momentarily locked)
/// store.
enum FirstRunOutcome {
    Accepted(CatalogMetadata),
    Proposal(CatalogMetadata),
    Refused,
}

impl From<io::Error> for CatalogProposalError {
    fn from(error: io::Error) -> Self {
        Self::Allocation(error)
    }
}

impl From<marrow_catalog::CatalogError> for CatalogProposalError {
    fn from(error: marrow_catalog::CatalogError) -> Self {
        Self::Catalog(error)
    }
}

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
    /// The accepted catalog's entries the surface ABI binds operations against: the store
    /// catalog's entries when bound, or a clean lock adoption's committed entries when no store is
    /// present. Empty while a first-run proposal is pending, since a proposal has no accepted ABI.
    pub(crate) accepted_entries: Vec<CatalogEntry>,
    pub(crate) ids: HashMap<CatalogKey, String>,
    /// Resolves a member's referent enum or store to its identity-aware leaf token. Covers
    /// proposal-only ids the accepted-only `ids` omits, and is never bound onto live facts.
    pub(crate) leaf_token_ids: HashMap<CatalogKey, String>,
    pub(crate) ambiguous_source_keys: HashSet<CatalogKey>,
    pub(crate) proposal: Option<CatalogMetadata>,
}

pub(crate) fn bind_catalog<'a, I>(
    accepted: Option<&CatalogMetadata>,
    lock: Option<&CatalogLock>,
    program: &mut CheckedProgram,
    evolve: &EvolveIntents,
    parsed_files: I,
    diagnostics: &mut Vec<CheckDiagnostic>,
) where
    I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
{
    let source = catalog_source(program, parsed_files);
    let binding = catalog_binding(program, accepted, lock, evolve, source, diagnostics);
    let declared_store_key_shapes = declared_store_key_shapes(program, &binding.leaf_token_ids);
    let declared_member_structs = declared_member_structs(program, &binding.leaf_token_ids);
    program
        .facts
        .bind_catalog_ids(&program.modules, &binding.ids);
    program.catalog.accepted_epoch = binding.accepted_epoch;
    program.catalog.accepted_digest = binding.accepted_digest;
    program.catalog.accepted_entries = binding.accepted_entries;
    // Defaults and transforms bind through the proposal id map, not the accepted-only ids:
    // a default or transform may target a brand-new member current source adds, whose stable
    // id lives only in the proposal until it is accepted. Discharge keys that member's
    // obligation by the same proposal id, so the fill resolves to the obligation it covers.
    program.catalog.evolve_defaults = bound_defaults(&evolve.defaults, &binding.leaf_token_ids);
    program.catalog.evolve_transforms =
        bound_transforms(&evolve.transforms, &binding.leaf_token_ids);
    program.catalog.declared_store_key_shapes = declared_store_key_shapes;
    program.catalog.declared_member_structs = declared_member_structs;
    program.catalog.ambiguous_source_keys = binding.ambiguous_source_keys;
    program.catalog.proposal = binding.proposal;
}

/// Reject a durable program without a native store backend. A store, enum, or resource needs
/// committed catalog identity, which a memory or omitted store cannot establish; the runtime would
/// fault `run.durable_store_required`. The trigger is source declaring any catalog-backed identity,
/// regardless of whether a committed lock already bound that identity as accepted.
pub(crate) fn require_durable_store(
    program: &CheckedProgram,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let source_entries = source_catalog_entries(program);
    let Some(anchor) = source_entries.first() else {
        return;
    };
    diagnostics.push(CheckDiagnostic::error(
        CHECK_DURABLE_STORE_REQUIRED,
        &anchor.file,
        anchor.span,
        "this program declares durable data, which requires a native store; the configured store has no durable identity",
    ));
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

/// A catalog-intent error for a project-level failure not tied to one declaration. It
/// names the file and points at its start, never an unplaceable `0:0` span.
fn catalog_diagnostic(file: std::path::PathBuf, message: String) -> CheckDiagnostic {
    catalog_error(file, crate::source_spans::start_of_file(), message)
}

fn catalog_error(file: std::path::PathBuf, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_CATALOG_INTENT, &file, span, message)
}

struct CatalogSource {
    entries: Vec<SourceCatalogEntry>,
    duplicate_keys: HashSet<CatalogKey>,
}

fn catalog_source<'a, I>(program: &CheckedProgram, parsed_files: I) -> CatalogSource
where
    I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
{
    let entries = source_catalog_entries(program);
    let mut duplicate_keys = duplicate_source_keys(&entries);
    duplicate_keys.extend(parsed_enum_member_duplicate_keys(program, parsed_files));
    CatalogSource {
        entries,
        duplicate_keys,
    }
}

fn catalog_binding(
    program: &CheckedProgram,
    accepted: Option<&CatalogMetadata>,
    lock: Option<&CatalogLock>,
    evolve: &EvolveIntents,
    source: CatalogSource,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> CatalogBinding {
    if let Some(catalog) = accepted
        && let Err(error) = catalog.validate()
    {
        diagnostics.push(catalog_diagnostic(
            first_source_file(&source.entries),
            format!("accepted catalog metadata is not valid: {}", error.message),
        ));
        return CatalogBinding {
            accepted_epoch: None,
            accepted_digest: None,
            accepted_entries: Vec::new(),
            ids: HashMap::new(),
            leaf_token_ids: HashMap::new(),
            ambiguous_source_keys: source.duplicate_keys,
            proposal: None,
        };
    }
    let mut ids = HashMap::new();
    // A live accepted store catalog is the sole identity authority: its ids bind onto
    // facts and the lock only raises the epoch floor for the advanced proposal. With no
    // accepted catalog the lock drives first-run binding — adopting committed identity
    // and the epoch high-water into the empty store, or minting fresh when absent.
    let proposal = match accepted {
        Some(catalog) => bind_against_accepted(
            program,
            catalog,
            lock,
            evolve,
            &source,
            &mut ids,
            diagnostics,
        ),
        None => match adopt_or_mint_first_run(program, evolve, &source, lock, diagnostics) {
            // A clean lock adoption with no store is the accepted reference itself, exactly as
            // the committed file once was: its committed identity binds onto facts at the lock's
            // epoch with no pending change, so a store-less surface ABI is stable.
            Ok(FirstRunOutcome::Accepted(accepted)) => {
                return accepted_lock_binding(accepted, source.duplicate_keys);
            }
            Ok(FirstRunOutcome::Proposal(proposal)) => Ok(Some(proposal)),
            Ok(FirstRunOutcome::Refused) => Ok(None),
            Err(error) => Err(error),
        },
    };
    let proposal = match proposal {
        Ok(proposal) => proposal,
        Err(CatalogProposalError::Allocation(error)) => {
            diagnostics.push(catalog_diagnostic(
                first_source_file(&source.entries),
                format!("failed to allocate catalog identity: {error}"),
            ));
            let CatalogSource {
                entries,
                duplicate_keys,
            } = source;
            return allocation_failure_binding(accepted, &entries, duplicate_keys);
        }
        Err(CatalogProposalError::Catalog(error)) => {
            diagnostics.push(catalog_diagnostic(
                first_source_file(&source.entries),
                format!("proposed catalog metadata is not valid: {}", error.message),
            ));
            None
        }
    };

    // The proposal is the catalog the commit path freezes when the program runs or an
    // evolution applies, so it must satisfy the same identity invariants. Validating it
    // here makes an identity collision the binding logic produced fail closed at check
    // time rather than at apply.
    if let Some(proposal) = &proposal
        && let Err(error) = proposal.validate()
    {
        diagnostics.push(catalog_diagnostic(
            first_source_file(&source.entries),
            format!("proposed catalog metadata is not valid: {}", error.message),
        ));
    }

    // The leaf token resolves a member's referent enum or store to its stable id. When a
    // proposal exists its entries carry every referent's id, including freshly-minted ones
    // the accepted-only `ids` map omits; when nothing changed, all referents are accepted
    // and `ids` already has them. This map is for token resolution only and is never bound
    // onto live facts, so a proposal-only identity does not leak into the program's facts.
    let leaf_token_ids = match &proposal {
        Some(proposal) => proposal_id_map_without(&proposal.entries, &source.duplicate_keys),
        None => ids.clone(),
    };

    CatalogBinding {
        accepted_epoch: accepted.map(|catalog| catalog.epoch),
        accepted_digest: accepted.map(|catalog| catalog.digest.clone()),
        accepted_entries: accepted
            .map(|catalog| catalog.entries.clone())
            .unwrap_or_default(),
        ids,
        leaf_token_ids,
        ambiguous_source_keys: source.duplicate_keys,
        proposal,
    }
}

fn allocation_failure_binding(
    accepted: Option<&CatalogMetadata>,
    source_entries: &[SourceCatalogEntry],
    duplicate_source_keys: HashSet<CatalogKey>,
) -> CatalogBinding {
    let ids = accepted
        .map(|catalog| {
            let accepted_index = AcceptedCatalog::new(catalog);
            unique_catalog_id_map(source_entries.iter().filter_map(|source| {
                let source_key = unique_source_key(&duplicate_source_keys, source)?;
                accepted_index
                    .active_entry(source.kind, &source.path)
                    .map(|binding| (source_key, binding.entry.stable_id.clone()))
            }))
        })
        .unwrap_or_default();
    CatalogBinding {
        accepted_epoch: accepted.map(|catalog| catalog.epoch),
        accepted_digest: accepted.map(|catalog| catalog.digest.clone()),
        accepted_entries: accepted
            .map(|catalog| catalog.entries.clone())
            .unwrap_or_default(),
        leaf_token_ids: ids.clone(),
        ids,
        ambiguous_source_keys: duplicate_source_keys,
        proposal: None,
    }
}

/// The binding a clean lock adoption produces with no accepted store: the committed identity
/// binds onto facts, the lock's epoch is the accepted epoch, the adopted entries are the accepted
/// ABI the surface binds operations against, and there is no pending proposal — the same shape a
/// live store would bind. The accepted digest is derived from the adopted catalog, identical to
/// the digest a present store snapshot of the same identity carries, so a read-only context digest
/// — and the surface computed-read tag that folds it in — is the same whether a checkout binds
/// from the committed lock or from a present (or momentarily writer-locked) store.
fn accepted_lock_binding(
    accepted: CatalogMetadata,
    duplicate_source_keys: HashSet<CatalogKey>,
) -> CatalogBinding {
    let ids = proposal_id_map_without(&accepted.entries, &duplicate_source_keys);
    CatalogBinding {
        accepted_epoch: Some(accepted.epoch),
        accepted_digest: Some(accepted.digest),
        accepted_entries: accepted.entries,
        leaf_token_ids: ids.clone(),
        ids,
        ambiguous_source_keys: duplicate_source_keys,
        proposal: None,
    }
}

fn duplicate_source_keys(source_entries: &[SourceCatalogEntry]) -> HashSet<CatalogKey> {
    let mut seen = HashSet::new();
    let mut duplicate = HashSet::new();
    for source in source_entries {
        let key = CatalogKey::new(source.kind, source.path.clone());
        if !seen.insert(key.clone()) {
            duplicate.insert(key);
        }
    }
    duplicate
}

fn parsed_enum_member_duplicate_keys<'a, I>(
    program: &CheckedProgram,
    parsed_files: I,
) -> HashSet<CatalogKey>
where
    I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
{
    let module_by_file: HashMap<&Path, &str> = program
        .modules
        .iter()
        .map(|module| (module.source_file.as_path(), module.name.as_str()))
        .collect();
    let mut seen = HashSet::new();
    let mut duplicate = HashSet::new();
    for (file, parsed) in parsed_files {
        let Some(module) = module_by_file.get(file) else {
            continue;
        };
        for declaration in &parsed.file.declarations {
            let Declaration::Enum(enum_decl) = declaration else {
                continue;
            };
            let mut member_path = Vec::new();
            collect_parsed_enum_member_keys(
                module,
                &enum_decl.name,
                &enum_decl.members,
                &mut member_path,
                &mut seen,
                &mut duplicate,
            );
        }
    }
    duplicate
}

fn collect_parsed_enum_member_keys(
    module: &str,
    enum_name: &str,
    members: &[EnumMember],
    member_path: &mut Vec<String>,
    seen: &mut HashSet<CatalogKey>,
    duplicate: &mut HashSet<CatalogKey>,
) {
    for member in members {
        member_path.push(member.name.clone());
        let key = CatalogKey::new(
            CatalogEntryKind::EnumMember,
            enum_member_source_path(module, enum_name, member_path),
        );
        if !seen.insert(key.clone()) {
            duplicate.insert(key);
        }
        collect_parsed_enum_member_keys(
            module,
            enum_name,
            &member.members,
            member_path,
            seen,
            duplicate,
        );
        member_path.pop();
    }
}

fn enum_member_source_path(module: &str, enum_name: &str, members: &[String]) -> String {
    format!("{}::{}", enum_path(module, enum_name), members.join("::"))
}

fn unique_catalog_id_map<I>(entries: I) -> HashMap<CatalogKey, String>
where
    I: IntoIterator<Item = (CatalogKey, String)>,
{
    let mut by_key: HashMap<CatalogKey, Option<String>> = HashMap::new();
    for (key, stable_id) in entries {
        by_key
            .entry(key)
            .and_modify(|current| *current = None)
            .or_insert(Some(stable_id));
    }
    by_key
        .into_iter()
        .filter_map(|(key, stable_id)| stable_id.map(|stable_id| (key, stable_id)))
        .collect()
}

fn unique_source_key(
    duplicate_source_keys: &HashSet<CatalogKey>,
    source: &SourceCatalogEntry,
) -> Option<CatalogKey> {
    let key = CatalogKey::new(source.kind, source.path.clone());
    (!duplicate_source_keys.contains(&key)).then_some(key)
}

fn bind_source_id(
    ids: &mut HashMap<CatalogKey, String>,
    source_key: Option<CatalogKey>,
    stable_id: String,
) {
    if let Some(source_key) = source_key {
        ids.insert(source_key, stable_id);
    }
}

/// Bind current source against an existing accepted catalog: carry accepted identity forward,
/// apply renames and retires, mint identity for new entities, and record signatures, binding
/// the resolved stable ids into `ids`. Returns the advanced proposal on any real change, or
/// `None` when the source matches the accepted catalog exactly.
fn bind_against_accepted(
    program: &CheckedProgram,
    catalog: &CatalogMetadata,
    lock: Option<&CatalogLock>,
    evolve: &EvolveIntents,
    source: &CatalogSource,
    ids: &mut HashMap<CatalogKey, String>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Result<Option<CatalogMetadata>, CatalogProposalError> {
    let accepted_index = AcceptedCatalog::new(catalog);
    let source_catalog = SourceCatalog::new(&source.entries);
    let mut renames = resolve_renames(
        &accepted_index,
        &source_catalog,
        &evolve.renames,
        diagnostics,
    );
    let mut proposal_entries = catalog.entries.clone();
    let mut allocator = StableIdAllocator::over(lock_ledger(lock), &proposal_entries);
    let mut changed = bind_source_entries(
        &accepted_index,
        source,
        &mut renames,
        ids,
        &mut proposal_entries,
        &mut allocator,
        diagnostics,
    )?;
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
    if drop_bare_removed_entries(&mut proposal_entries, &source_catalog) {
        changed = true;
    }
    if record_signatures_into(program, &mut proposal_entries, Some(catalog)) {
        changed = true;
    }
    if record_transform_marks(&mut proposal_entries, &evolve.transforms, catalog) {
        changed = true;
    }
    if changed {
        Ok(Some(CatalogMetadata::new(
            advance_epoch(catalog.epoch, lock_high_water(lock)),
            proposal_entries,
        )?))
    } else {
        Ok(None)
    }
}

/// Stamp every live `evolve transform` target with its own per-transform identity (a hash of
/// the target's stable id and the transform body), so a shape-neutral in-place transform becomes
/// a real catalog change that advances the epoch on apply. A transform rewrites an existing
/// member's data without changing its type, so it moves no structural signature; without this
/// mark the proposal would equal the accepted catalog and the data work would re-derive on every
/// preview, re-executing forever. Keying the mark on the transform's own target and body — not the
/// whole-program shape — is what keeps a later unrelated durable edit from drifting the mark and
/// re-running a transform against already-migrated data. Once the accepted entry carries this
/// identity, a re-bind of the same transform records the same value and reports no change, while a
/// changed body computes a different identity that reads as a fresh obligation. Returns whether any
/// target gained a new mark.
///
/// The target id resolves through the proposal id map, not the accepted-only `ids`: a transform
/// may target a member current source first mints in this same proposal, whose stable id lives
/// only in the proposal entries until acceptance. Resolving from those entries stamps the mark in
/// the same activation that adds the member, so a transform onto a newly-added field settles in
/// one apply rather than splitting the structural add and the discharge mark across two epochs.
fn record_transform_marks(
    proposal_entries: &mut [CatalogEntry],
    transforms: &[TransformIntent],
    accepted: &CatalogMetadata,
) -> bool {
    let proposal_ids = proposal_id_map(proposal_entries);
    let identities: HashMap<String, String> = transforms
        .iter()
        .filter_map(|transform| {
            let id = member_target_id(&transform.path, &proposal_ids)?;
            let identity = transform_identity(&id, &transform.body_text);
            Some((id, identity))
        })
        .collect();
    if identities.is_empty() {
        return false;
    }
    let accepted_marks: HashMap<&str, &Option<String>> = accepted
        .entries
        .iter()
        .map(|entry| (entry.stable_id.as_str(), &entry.applied_transform))
        .collect();
    let mut changed = false;
    for entry in proposal_entries.iter_mut() {
        let Some(identity) = identities.get(&entry.stable_id) else {
            continue;
        };
        let already = accepted_marks
            .get(entry.stable_id.as_str())
            .copied()
            .and_then(Option::as_deref);
        if already != Some(identity.as_str()) {
            changed = true;
        }
        entry.applied_transform = Some(identity.clone());
    }
    changed
}

/// Resolve each current source entry to its identity — carry an accepted active entry's id
/// forward, relocate a renamed one, or mint identity for a new entity — binding it into `ids`
/// and returning whether any entry is a real change. An accepted entry whose source
/// declaration has disappeared but is neither renamed nor retired stays active with no source
/// backing: dropping a sparse field is a legal no-op, so it is a discharge obligation rather
/// than a binding error.
fn bind_source_entries<E: CatalogIdEntropy>(
    accepted_index: &AcceptedCatalog<'_>,
    source: &CatalogSource,
    renames: &mut HashMap<String, ResolvedRename>,
    ids: &mut HashMap<CatalogKey, String>,
    proposal_entries: &mut Vec<CatalogEntry>,
    allocator: &mut StableIdAllocator<E>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> io::Result<bool> {
    let mut changed = false;
    for source_entry in &source.entries {
        let source_key = unique_source_key(&source.duplicate_keys, source_entry);
        let rename = renames.remove(&source_entry.path);
        if let Some(binding) = accepted_index.active_entry(source_entry.kind, &source_entry.path) {
            // A rename onto a path that already names a live accepted entity cannot move
            // identity there; report the no-op intent instead of dropping it.
            if rename.is_some() {
                push_rename_target_live(source_entry, diagnostics);
            }
            let stable_id = binding.entry.stable_id.clone();
            bind_source_id(ids, source_key, stable_id);
        } else if let Some(reserved) =
            accepted_index.reserved_entry(source_entry.kind, &source_entry.path)
        {
            push_reserved_reuse(source_entry, reserved.entry, diagnostics);
            changed = true;
        } else if let Some(rename) = rename {
            apply_rename(
                proposal_entries,
                source_entry,
                &rename.from_path,
                ids,
                source_key,
            );
            changed = true;
        } else {
            let entry = proposed_catalog_entry(source_entry, allocator)?;
            push_pending_identity(source_entry, diagnostics);
            prepare_proposal_path(proposal_entries, source_entry.kind, &source_entry.path);
            proposal_entries.push(entry);
            changed = true;
        }
    }
    Ok(changed)
}

/// Report every rename whose target the source never declares; it relocates nothing, so the
/// intent must not vanish silently.
fn report_unresolved_renames(
    renames: &HashMap<String, ResolvedRename>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for rename in renames.values() {
        if let Some((file, span)) = &rename.origin {
            report_unresolved_intent(file, *span, diagnostics);
        }
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

/// The next epoch a real change advances to: strictly above both the accepted epoch and the
/// lock's epoch high-water. Folding the high-water in keeps the version line monotone across
/// store loss — a wiped store whose committed lock reached epoch N cannot mint a new proposal
/// at or below N and silently reuse a witnessed epoch for different identity. The single owner
/// of the advance rule, consumed by both bind paths and by the run baseline.
///
/// The add saturates rather than overflowing: a wrap to zero on a write-adjacent path would
/// silently reuse a witnessed epoch for different identity, and a debug panic would crash the
/// checker. The lock codec already rejects an un-advanceable high-water, so saturation is the
/// belt to that suspenders, never the primary defense.
fn advance_epoch(accepted_epoch: u64, lock_high_water: u64) -> u64 {
    accepted_epoch.max(lock_high_water).saturating_add(1)
}

/// The lock's append-only id ledger, or an empty slice when no lock is present. The
/// never-reuse authority a fresh mint is seeded against and an adopted id is checked against.
fn lock_ledger(lock: Option<&CatalogLock>) -> &[LockLedgerTombstone] {
    lock.map(|lock| lock.ledger.as_slice()).unwrap_or_default()
}

/// The lock's epoch high-water, or zero when no lock is present.
fn lock_high_water(lock: Option<&CatalogLock>) -> u64 {
    lock.map(|lock| lock.epoch_high_water).unwrap_or(0)
}

/// Bind current source with no accepted catalog: a present committed lock drives first-run
/// adoption of its identity and epoch high-water into the empty store, an absent lock mints a
/// fresh baseline at epoch 1. Every rename or retire is an unresolved intent (nothing to carry
/// forward). When the lock adopts the source CLEANLY — every source `(kind, path)` matches a
/// committed lock entry with none left over, no rename or retire pending, and the source shape
/// digest equals the lock's recorded digest — the lock IS the accepted reference, so the binding
/// is [`FirstRunOutcome::Accepted`] at the lock's epoch with no pending change, restoring the
/// store-less stable-ABI guarantee the committed file once gave. Any drift (a new or removed
/// entity, a pending rename/retire, a stale digest) keeps the binding a proposal: a drifted
/// source has no committed ABI and must not be falsely reported as accepted. Returns
/// [`FirstRunOutcome::Refused`], having pushed the typed [`CHECK_LOCK_CORRUPT`], when a present
/// lock refuses adoption (a tombstone reissue).
fn adopt_or_mint_first_run(
    program: &CheckedProgram,
    evolve: &EvolveIntents,
    source: &CatalogSource,
    lock: Option<&CatalogLock>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Result<FirstRunOutcome, CatalogProposalError> {
    let Some(lock) = lock else {
        // With no committed identity to carry forward, every rename or retire relocates nothing.
        for rename in &evolve.renames {
            report_unresolved_intent(&rename.file, rename.span, diagnostics);
        }
        for retire in &evolve.retires {
            report_unresolved_intent(&retire.file, retire.span, diagnostics);
        }
        return mint_first_run(program, &source.entries, &[]).map(FirstRunOutcome::Proposal);
    };
    let Some(mut proposal_entries) =
        adopt_first_run_entries(&source.entries, lock, &lock.ledger, evolve, diagnostics)?
    else {
        return Ok(FirstRunOutcome::Refused);
    };
    // Record the source shape signatures onto the adopted entries: clean or not, the accepted ABI
    // and the proposal both carry the current shape under the committed identity.
    record_signatures_into(program, &mut proposal_entries, None);
    // A kept consumed rename or retire block resolves against the seeded catalog exactly as it
    // does against a present store: a rename whose old spelling the lock carries as an alias, and a
    // retire whose path the lock's ledger reconstructs as a reserved row, are already recorded, not
    // unresolved. Only an intent naming nothing the seed reconstructs is a `check.evolve_target`.
    let unresolved_intent =
        report_seeded_unresolved_intents(evolve, &proposal_entries, source, diagnostics);
    if !unresolved_intent && lock_adopts_source_cleanly(program, source, lock) {
        return Ok(FirstRunOutcome::Accepted(CatalogMetadata::new(
            lock.epoch_high_water,
            proposal_entries,
        )?));
    }
    Ok(FirstRunOutcome::Proposal(CatalogMetadata::new(
        lock.epoch_high_water,
        proposal_entries,
    )?))
}

/// Resolve the evolve renames and retires against the seeded first-run catalog, reporting only
/// those that name nothing the seed reconstructs. A kept consumed rename — its old spelling carried
/// as an alias on the active seeded entry — and a kept consumed retire — its path reconstructed as
/// a reserved row from the lock's ledger — are already recorded and push no diagnostic, exactly as
/// they resolve against a present store. The seeded entries are not mutated: the lock already
/// reflects the post-transition state, so this only classifies each intent as consumed or
/// genuinely unresolvable. Returns whether any intent was reported unresolved.
fn report_seeded_unresolved_intents(
    evolve: &EvolveIntents,
    seeded_entries: &[CatalogEntry],
    source: &CatalogSource,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> bool {
    let before = diagnostics.len();
    let seeded = CatalogMetadata::new(0, seeded_entries.to_vec());
    let Ok(seeded) = seeded else {
        // A seeded catalog that fails its identity invariants is reported by the caller's
        // proposal validation; resolution against it would be meaningless, so treat every intent
        // as unresolved rather than silently accept it.
        return !evolve.renames.is_empty() || !evolve.retires.is_empty();
    };
    let accepted_index = AcceptedCatalog::new(&seeded);
    let source_catalog = SourceCatalog::new(&source.entries);
    resolve_renames(
        &accepted_index,
        &source_catalog,
        &evolve.renames,
        diagnostics,
    );
    for retire in &evolve.retires {
        // A `Rejected` retire has already pushed its typed diagnostic; `Consumed` and `Active`
        // are both legitimate resolutions of a kept block against the seeded reserved row.
        let _ = retire_target(
            seeded_entries,
            &accepted_index,
            &source_catalog,
            retire,
            diagnostics,
        );
    }
    diagnostics.len() != before
}

/// Whether the committed lock adopts the current source as its exact accepted reference. The
/// load-bearing cleanliness gate: the source `(kind, path)` set must equal the lock's committed
/// entries one-for-one — no source entity the lock never recorded, no committed entry the source
/// no longer declares (a removal or rename) — and the source shape digest must match the digest
/// the lock was produced under, so a shape edit the lock predates is read as drift. An ambiguous
/// source path (a duplicate `(kind, path)`) is never clean: its identity is unresolved. When this
/// holds, the lock carries a complete, current accepted ABI and binds as accepted; otherwise the
/// source has drifted from the lock and stays a proposal.
fn lock_adopts_source_cleanly(
    program: &CheckedProgram,
    source: &CatalogSource,
    lock: &CatalogLock,
) -> bool {
    if !source.duplicate_keys.is_empty() {
        return false;
    }
    let source_keys: HashSet<(CatalogEntryKind, &str)> = source
        .entries
        .iter()
        .map(|entry| (entry.kind, entry.path.as_str()))
        .collect();
    let lock_keys: HashSet<(CatalogEntryKind, &str)> = lock
        .entries
        .iter()
        .map(|entry| (entry.kind, entry.path.as_str()))
        .collect();
    source_keys == lock_keys && analyzed_source_digest(program) == lock.source_digest
}

/// Mint a fresh first-run proposal at epoch 1: every source entity gets a newly allocated id,
/// re-rolled past every id the ledger has tombstoned so a retired id is never reissued.
fn mint_first_run(
    program: &CheckedProgram,
    source_entries: &[SourceCatalogEntry],
    ledger: &[LockLedgerTombstone],
) -> Result<CatalogMetadata, CatalogProposalError> {
    let mut allocator = StableIdAllocator::empty(ledger);
    let mut proposal_entries: Vec<CatalogEntry> = source_entries
        .iter()
        .map(|source| proposed_catalog_entry(source, &mut allocator))
        .collect::<io::Result<_>>()?;
    record_signatures_into(program, &mut proposal_entries, None);
    Ok(CatalogMetadata::new(1, proposal_entries)?)
}

/// Build the first-run proposal entries for a present lock: materialize the lock's ledger
/// tombstones as Reserved entries, carry a committed id forward onto each source entity whose
/// `(kind, path)` the lock records — or whose pending evolve rename names a committed old path,
/// recording that old spelling as an alias — reserve the committed entry a pending retire names,
/// and mint a fresh id (seeded never to reuse a committed or tombstoned id) for a genuinely new
/// entity the lock records nowhere. Resolving the pending intents against the lock's committed
/// identity is what makes a fresh checkout carry the same identity forward a present store would.
/// Materializing the tombstones is what
/// makes the never-reuse defense survive store loss: a fresh checkout seeded from the lock alone
/// commits these reserved rows into its baseline store, so a later projection re-derives the same
/// ledger and re-declaring a retired path still fails closed.
///
/// A source `(kind, path)` that re-declares a reserved tombstone fails closed with the same typed
/// reserved-path-reuse diagnostic the live-store binding gives, and is not adopted as an active
/// entry — only the reserved row survives, exactly as it would against a live store. Returns
/// `None`, having pushed [`CHECK_LOCK_CORRUPT`], when an adopted id reissues a ledger tombstone:
/// the binding fails closed before any id is bound rather than resurrect a retired identity.
/// Adoption keys on the `(kind, path)` anchor, never the shape fingerprint: a first-run source
/// pre-image records none of the accepted shape a committed entry was fingerprinted under, so a
/// fingerprint match would silently miss every shaped entity, and distinct entities sharing a
/// shape would collide. The fingerprint is a drift signal for later staleness detection, not an
/// identity key.
fn adopt_first_run_entries(
    source_entries: &[SourceCatalogEntry],
    lock: &CatalogLock,
    ledger: &[LockLedgerTombstone],
    evolve: &EvolveIntents,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Result<Option<Vec<CatalogEntry>>, CatalogProposalError> {
    // Seed the never-reuse set, through the allocator's single never-reuse owner, with the ledger
    // tombstones and every committed lock id, so a fresh mint for an unrecorded entity re-rolls
    // past an id the lock already commits or has retired. The committed ids enter as entry stubs
    // because the allocator seeds from catalog entries.
    let committed_stubs = committed_id_stubs(&lock.entries);
    let allocator = StableIdAllocator::over(ledger, &committed_stubs);
    adopt_first_run_entries_with(source_entries, lock, ledger, evolve, allocator, diagnostics)
}

/// The adoption body, generic over the allocator's entropy so a fault-injecting test can drive the
/// mint path. An OS-entropy `allocate()` failure (fd exhaustion, a sandbox blocking `/dev/urandom`)
/// propagates as [`CatalogProposalError::Allocation`] rather than collapsing to a diagnostic-less
/// refusal, so the binding fails closed loudly exactly as the no-lock mint does. The Refused result
/// (`Ok(None)`) is reserved for the tombstone-reissue case, which has already pushed its typed
/// diagnostic.
fn adopt_first_run_entries_with<E: CatalogIdEntropy>(
    source_entries: &[SourceCatalogEntry],
    lock: &CatalogLock,
    ledger: &[LockLedgerTombstone],
    evolve: &EvolveIntents,
    mut allocator: StableIdAllocator<E>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Result<Option<Vec<CatalogEntry>>, CatalogProposalError> {
    let committed: HashMap<(CatalogEntryKind, &str), &LockEntry> = lock
        .entries
        .iter()
        .map(|entry| ((entry.kind, entry.path.as_str()), entry))
        .collect();
    let reserved: HashMap<(CatalogEntryKind, &str), CatalogEntry> = ledger
        .iter()
        .map(|tombstone| {
            (
                (tombstone.kind, tombstone.path.as_str()),
                tombstone.reserved_catalog_entry(),
            )
        })
        .collect();
    // The accepted catalog the committed lock represents. A pending (unapplied) rename or retire
    // names its old `(kind, path)` against this committed identity, so resolving the intent here
    // carries the committed id forward exactly as a present store does against its live accepted
    // catalog. The resolution only seeds identity; report_seeded_unresolved_intents is the single
    // authoritative reporter, so these diagnostics are discarded.
    let lock_accepted = lock_accepted_catalog(lock)?;
    let accepted_index = AcceptedCatalog::new(&lock_accepted);
    let source_catalog = SourceCatalog::new(source_entries);
    let mut discarded = Vec::new();
    let renames = resolve_renames(
        &accepted_index,
        &source_catalog,
        &evolve.renames,
        &mut discarded,
    );
    let mut proposal_entries = Vec::with_capacity(source_entries.len() + reserved.len());
    for source in source_entries {
        if let Some(reserved_entry) = reserved.get(&(source.kind, source.path.as_str())) {
            // A source declaration at a reserved path cannot reclaim the retired identity; report
            // the reuse and keep only the reserved row, exactly as the live-store binding does.
            push_reserved_reuse(source, reserved_entry, diagnostics);
            continue;
        }
        let entry = if let Some(committed) = committed.get(&(source.kind, source.path.as_str())) {
            adopted_catalog_entry(source, &committed.stable_id, &committed.aliases)
        } else if let Some(carried) = renamed_carry_forward(source, &renames, &committed) {
            carried
        } else {
            proposed_catalog_entry(source, &mut allocator)?
        };
        proposal_entries.push(entry);
    }
    reserve_retired_committed_entries(
        &lock_accepted.entries,
        &accepted_index,
        &source_catalog,
        &evolve.retires,
        &mut proposal_entries,
    );
    if let Some(reissued) = tombstone_reissue(&proposal_entries, ledger) {
        diagnostics.push(lock_corrupt_diagnostic(source_entries, reissued));
        return Ok(None);
    }
    proposal_entries.extend(reserved.into_values());
    Ok(Some(proposal_entries))
}

/// The accepted catalog the committed lock represents: its committed entries at their recorded
/// identity, aliases, and lifecycle, plus the reserved rows its ledger tombstones reconstruct.
/// A fresh checkout resolves a pending evolve rename or retire against this exactly as a present
/// store resolves it against its live accepted catalog, so the carry-forward the present store
/// performs is performed identically on the seed. Shape signatures stay `None`: the lock records
/// them only as fingerprints, and intent resolution reads identity and lifecycle, never shape.
fn lock_accepted_catalog(lock: &CatalogLock) -> Result<CatalogMetadata, CatalogProposalError> {
    let mut entries: Vec<CatalogEntry> = lock
        .entries
        .iter()
        .map(lock_entry_to_catalog_entry)
        .collect();
    entries.extend(
        lock.ledger
            .iter()
            .map(LockLedgerTombstone::reserved_catalog_entry),
    );
    Ok(CatalogMetadata::new(0, entries)?)
}

fn lock_entry_to_catalog_entry(entry: &LockEntry) -> CatalogEntry {
    CatalogEntry {
        kind: entry.kind,
        path: entry.path.clone(),
        stable_id: entry.stable_id.clone(),
        aliases: entry.aliases.clone(),
        lifecycle: entry.lifecycle,
        accepted_key_shape: None,
        accepted_index_shape: None,
        accepted_struct: None,
        applied_transform: None,
    }
}

/// Carry the committed identity a pending rename relocates onto this source path. When the source
/// entity is a resolved rename target whose old `(kind, path)` the lock records as a committed
/// active entry, adopt that committed id and record the old spelling as an alias, mirroring the
/// present store's `apply_rename`. Returns `None` when the source path is not a rename target or
/// the lock records no committed active entry at the old path, so the caller mints a fresh id.
fn renamed_carry_forward(
    source: &SourceCatalogEntry,
    renames: &HashMap<String, ResolvedRename>,
    committed: &HashMap<(CatalogEntryKind, &str), &LockEntry>,
) -> Option<CatalogEntry> {
    let rename = renames.get(&source.path)?;
    let committed_entry = committed.get(&(source.kind, rename.from_path.as_str()))?;
    if committed_entry.lifecycle != CatalogLifecycle::Active {
        return None;
    }
    let mut aliases = committed_entry.aliases.clone();
    if !aliases.iter().any(|alias| alias == &rename.from_path) {
        aliases.push(rename.from_path.clone());
    }
    Some(adopted_catalog_entry(
        source,
        &committed_entry.stable_id,
        &aliases,
    ))
}

/// Reserve each pending retire's committed active entry. A retire whose path names a committed
/// active entry the source no longer declares reconstructs the reserved row the present store's
/// `apply_retires` would leave, carrying the committed identity and aliases at the retired path so
/// the seed reconstructs the never-reuse reservation. A retire naming a ledger-reserved path, a
/// still-declared source entity, or nothing is left to the authoritative reporter.
fn reserve_retired_committed_entries(
    lock_entries: &[CatalogEntry],
    accepted_index: &AcceptedCatalog<'_>,
    source_catalog: &SourceCatalog<'_>,
    retires: &[RetireIntent],
    proposal_entries: &mut Vec<CatalogEntry>,
) {
    let mut discarded = Vec::new();
    for retire in retires {
        if let RetireTarget::Active(index) = retire_target(
            lock_entries,
            accepted_index,
            source_catalog,
            retire,
            &mut discarded,
        ) {
            let mut reserved_row = lock_entries[index].clone();
            reserved_row.lifecycle = CatalogLifecycle::Reserved;
            proposal_entries.push(reserved_row);
        }
    }
}

/// Adopt a committed lock id onto an unkeyed source view directly, for the binding's
/// fail-closed guard tests. Production binding reaches the same path through
/// [`adopt_or_mint_first_run`]; this exposes the adopting builder with the ledger threaded
/// explicitly so the tombstone-reissue refusal is exercised without a structurally invalid lock.
#[cfg(test)]
fn adopt_first_run(
    source_entries: &[SourceCatalogEntry],
    lock: &CatalogLock,
    ledger: &[LockLedgerTombstone],
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<CatalogMetadata> {
    let entries = match adopt_first_run_entries(
        source_entries,
        lock,
        ledger,
        &EvolveIntents::default(),
        diagnostics,
    ) {
        Ok(entries) => entries?,
        Err(_) => panic!("adoption mints without an entropy fault in these fixtures"),
    };
    CatalogMetadata::new(lock.epoch_high_water, entries).ok()
}

/// Catalog-entry stubs carrying each committed lock id, so the allocator's never-reuse seed
/// (its single owner) includes the committed ids without a second never-reuse set in this module.
/// The stubs are seed-only: their kind and path are placeholders never published into a proposal.
fn committed_id_stubs(committed: &[LockEntry]) -> Vec<CatalogEntry> {
    committed
        .iter()
        .map(|entry| CatalogEntry {
            kind: CatalogEntryKind::Resource,
            path: String::from("lock-committed-id-seed"),
            stable_id: entry.stable_id.clone(),
            aliases: Vec::new(),
            lifecycle: CatalogLifecycle::Reserved,
            accepted_key_shape: None,
            accepted_index_shape: None,
            accepted_struct: None,
            applied_transform: None,
        })
        .collect()
}

/// The first ACTIVE adopted id that a ledger tombstone records, or `None` when adoption reissues
/// no retired id as a live identity. A valid lock keeps its committed ids off its own ledger, so
/// this guards the adopted proposal independently of the lock's self-validation. The reserved rows
/// materialized from the ledger legitimately carry tombstoned ids, so only active entries are
/// checked: a reserved row holding a tombstoned id is the recorded retirement, not a reissue.
fn tombstone_reissue<'a>(
    proposal_entries: &'a [CatalogEntry],
    ledger: &[LockLedgerTombstone],
) -> Option<&'a str> {
    let tombstoned: HashSet<&str> = ledger.iter().map(|stone| stone.id.as_str()).collect();
    proposal_entries
        .iter()
        .filter(|entry| entry.lifecycle == CatalogLifecycle::Active)
        .find(|entry| tombstoned.contains(entry.stable_id.as_str()))
        .map(|entry| entry.stable_id.as_str())
}

fn lock_corrupt_diagnostic(
    source_entries: &[SourceCatalogEntry],
    reissued_id: &str,
) -> CheckDiagnostic {
    CheckDiagnostic::error(
        CHECK_LOCK_CORRUPT,
        &first_source_file(source_entries),
        crate::source_spans::start_of_file(),
        format!(
            "marrow.lock is corrupt: adopting catalog id `{reissued_id}` would reissue an id its \
             ledger has retired"
        ),
    )
}

/// A proposed first-run catalog entry carrying a specific freshly-minted stable id. Shares the
/// lifecycle and empty-signature shape [`proposed_catalog_entry`] mints, recording no aliases — a
/// minted entity has no prior spelling to carry forward.
fn proposed_catalog_entry_with_id(source: &SourceCatalogEntry, stable_id: &str) -> CatalogEntry {
    adopted_catalog_entry(source, stable_id, &[])
}

/// A first-run catalog entry adopting a committed lock identity: the committed stable id and the
/// rename aliases the lock records at this `(kind, path)`. Reconstructing the aliases is what lets
/// a fresh checkout resolve a kept consumed rename block's old-spelling carry-forward against the
/// seed-from-lock catalog exactly as it resolves against a present store.
fn adopted_catalog_entry(
    source: &SourceCatalogEntry,
    stable_id: &str,
    aliases: &[String],
) -> CatalogEntry {
    CatalogEntry {
        kind: source.kind,
        path: source.path.clone(),
        stable_id: stable_id.to_string(),
        aliases: aliases.to_vec(),
        lifecycle: CatalogLifecycle::Active,
        accepted_key_shape: None,
        accepted_index_shape: None,
        accepted_struct: None,
        applied_transform: None,
    }
}

/// The `(kind, path) -> stable id` map of a proposal's active entries. Unlike the accepted-only
/// binding map, this covers freshly-minted and renamed referents, so the leaf token can resolve
/// an enum or store the accepted catalog does not yet record.
fn proposal_id_map(entries: &[CatalogEntry]) -> HashMap<CatalogKey, String> {
    proposal_id_map_without(entries, &HashSet::new())
}

fn proposal_id_map_without(
    entries: &[CatalogEntry],
    excluded: &HashSet<CatalogKey>,
) -> HashMap<CatalogKey, String> {
    unique_catalog_id_map(
        entries
            .iter()
            .filter(|entry| entry.lifecycle == CatalogLifecycle::Active)
            .filter_map(|entry| {
                let key = CatalogKey::new(entry.kind, entry.path.clone());
                (!excluded.contains(&key)).then(|| (key, entry.stable_id.clone()))
            }),
    )
}

fn active_proposal_id<'a>(
    entries: &'a [CatalogEntry],
    kind: CatalogEntryKind,
    path: &str,
) -> Option<&'a str> {
    let mut matches = entries
        .iter()
        .filter(|entry| {
            entry.lifecycle == CatalogLifecycle::Active && entry.kind == kind && entry.path == path
        })
        .map(|entry| entry.stable_id.as_str());
    let stable_id = matches.next()?;
    matches.next().is_none().then_some(stable_id)
}

pub(crate) fn active_program_proposal_id<'a>(
    program: &'a CheckedProgram,
    kind: CatalogEntryKind,
    path: &str,
) -> Option<&'a str> {
    if program
        .catalog
        .ambiguous_source_keys
        .contains(&CatalogKey::new(kind, path.to_string()))
    {
        return None;
    }
    let proposal = program.catalog.proposal.as_ref()?;
    active_proposal_id(&proposal.entries, kind, path)
}

/// The proposal identity map for activation-only readers, exposed so executable places reuse
/// catalog binding's proposal identity semantics rather than rebuilding them.
pub(crate) fn active_proposal_id_map(program: &CheckedProgram) -> HashMap<CatalogKey, String> {
    program
        .catalog
        .proposal
        .as_ref()
        .map(|proposal| {
            proposal_id_map_without(&proposal.entries, &program.catalog.ambiguous_source_keys)
        })
        .unwrap_or_default()
}

/// The id a structural `(kind, path)` resolves to in a prebuilt proposal map. The
/// map already drops keys with no unique active id, so a hit is the same id the
/// per-call proposal scan in [`active_program_proposal_id`] would return. The one
/// owner of this lookup; batch readers resolve every member of a declaration
/// through it instead of rescanning the proposal per member.
pub(crate) fn proposal_id(
    ids: &HashMap<CatalogKey, String>,
    kind: CatalogEntryKind,
    path: impl Into<String>,
) -> Option<String> {
    ids.get(&CatalogKey::new(kind, path)).cloned()
}

/// The stable id a member-target evolve path binds to, or `None` when it names no resource
/// member (the type pass already reported it).
fn member_target_id(path: &str, ids: &HashMap<CatalogKey, String>) -> Option<String> {
    proposal_id(ids, CatalogEntryKind::ResourceMember, path)
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
                body_text: transform.body_text.clone(),
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
///
/// `origin` is the declaration to blame when the rename relocates nothing: a
/// developer-written rename carries its file and span, while a rename derived by
/// cascading a renamed enum category onto its descendant leaves carries `None`. An
/// unresolved derived rename is a leaf the renamed subtree no longer declares — an
/// ordinary member removal the accepted entry's discharge already covers — so it is
/// dropped silently rather than reported as a malformed unplaceable intent.
struct ResolvedRename {
    from_path: String,
    origin: Option<(std::path::PathBuf, SourceSpan)>,
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
    let mut cascading_renames: Vec<(CatalogEntryKind, String, String)> = Vec::new();
    for rename in renames {
        let kind = match source_catalog.path_kind(rename.to_path.as_str()) {
            SourcePathKind::Absent => {
                // A kept consumed rename whose target was itself renamed onward names a path the
                // source no longer declares, yet the active catalog still carries both spellings on
                // one current identity. That block only describes work already applied, so it stays
                // inert exactly as deleting it would, rather than failing as an unresolved intent.
                if !rename_consumed(accepted, rename) {
                    report_unresolved_intent(&rename.file, rename.span, diagnostics);
                }
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
        if rename_consumed(accepted, rename) {
            // The accepted catalog already carries this entity's identity at `to_path` with
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
        if let Some(descendant_kind) = cascade_descendant_kind(kind) {
            cascading_renames.push((
                descendant_kind,
                rename.to_path.clone(),
                rename.from_path.clone(),
            ));
        }
        resolved.insert(
            rename.to_path.clone(),
            ResolvedRename {
                from_path: rename.from_path.clone(),
                origin: Some((rename.file.clone(), rename.span)),
            },
        );
    }
    cascade_subtree_renames(accepted, &cascading_renames, &mut resolved);
    resolved
}

/// The descendant kind a rename of `kind` carries its whole accepted subtree forward for, or
/// `None` when a rename relocates only the named entry. An entry's saved-data identity is keyed
/// on its full ancestor path, so renaming an ancestor must relocate every descendant too or its
/// stored values would orphan even though a rename is identity-preserving: renaming an enum
/// category (`Pet::mammal`) carries its descendant enum members, and renaming a resource (`Book`)
/// carries its resource members (`Book::note`).
fn cascade_descendant_kind(kind: CatalogEntryKind) -> Option<CatalogEntryKind> {
    match kind {
        CatalogEntryKind::EnumMember => Some(CatalogEntryKind::EnumMember),
        CatalogEntryKind::Resource => Some(CatalogEntryKind::ResourceMember),
        _ => None,
    }
}

/// Carry every descendant's identity forward when its ancestor is renamed. A saved leaf's
/// identity is keyed on its full ancestor path, so renaming an enum category
/// (`Pet::mammal` -> `Pet::beast`) or a resource (`Book` -> `Volume`) must relocate each
/// descendant (`Pet::mammal::dog` -> `Pet::beast::dog`, `Book::note` -> `Volume::note`) too, or
/// its stored values would orphan even though a rename is identity-preserving. The explicit
/// rename names the ancestor; the subtree below it travels automatically from the accepted
/// catalog, so a developer never spells out a per-descendant rename for a structurally unchanged
/// subtree.
///
/// A derived rename is skipped when its new path already has an explicit rename: an explicit
/// intent always wins, so reshaping a descendant out of the renamed subtree in the same evolve
/// block is honored over the automatic carry-forward.
fn cascade_subtree_renames(
    accepted: &AcceptedCatalog<'_>,
    cascading_renames: &[(CatalogEntryKind, String, String)],
    resolved: &mut HashMap<String, ResolvedRename>,
) {
    for (descendant_kind, to_path, from_path) in cascading_renames {
        for descendant in accepted.active_descendants(*descendant_kind, from_path) {
            let suffix = &descendant[from_path.len()..];
            let new_path = format!("{to_path}{suffix}");
            resolved.entry(new_path).or_insert_with(|| ResolvedRename {
                from_path: descendant.to_string(),
                origin: None,
            });
        }
    }
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

/// Whether a kept `rename from_path -> to_path` block has no relocation work left: the owning
/// active entry already records `from_path` as a rename alias (it has moved off that spelling)
/// while still owning `to_path`, so the block only describes work already applied. A rename whose
/// `from_path` is the entry's current canonical path is live relocation work — including a
/// round-trip back onto a former alias — and is not consumed.
fn rename_consumed(accepted: &AcceptedCatalog<'_>, rename: &RenameIntent) -> bool {
    accepted.rename_already_applied(&rename.from_path, &rename.to_path)
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
    source_key: Option<CatalogKey>,
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
    let stable_id = entry.stable_id.clone();
    // The new canonical path is canonical again for this id, so any same-kind entry that
    // still records it as an alias would collide a path against a live entry. Dropping it
    // from every sibling's aliases — the single owner the new-entry path uses — clears both
    // a round-trip rename's own redundant alias and a sibling's stale alias.
    prepare_proposal_path(entries, source.kind, &source.path);
    bind_source_id(ids, source_key, stable_id);
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

/// Record each store index's declaration shape into its proposal entry.
fn record_store_index_shapes(
    program: &CheckedProgram,
    entries: &mut [CatalogEntry],
    ids: &HashMap<CatalogKey, String>,
    accepted: Option<&CatalogMetadata>,
) -> bool {
    record_signatures(
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
/// implementation of the record-or-diff rule for source-derived accepted shape fields.
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

/// Remove from the proposal each active entry current source no longer declares and no intent
/// relocated or retired, returning whether any was dropped. A bare removal of a durable entity —
/// a resource, store, field, enum member, or derived index — leaves it active in the accepted
/// catalog with no source backing; carrying it forward would keep it `lifecycle:"active"` in the
/// lock, so an in-place run would diverge from a fresh reseed of the same source, which never
/// records it at all. Dropping it makes the one-way store->lock projection identical to a reseed:
/// same entries, and a re-add later mints fresh identity rather than resurrecting the dropped id.
///
/// This advances the proposal but does not authorize data loss. A dropped index is derived schema,
/// so activation simply deletes its generated cells. A dropped data entity is fenced closed by
/// activation discharge when it still holds records, read against the accepted snapshot, before
/// this proposal can commit. A rename leaves the entry active under its new path, and a retire
/// leaves it reserved, so neither is a bare removal.
fn drop_bare_removed_entries(
    entries: &mut Vec<CatalogEntry>,
    source_catalog: &SourceCatalog<'_>,
) -> bool {
    let before = entries.len();
    entries.retain(|entry| {
        entry.lifecycle != CatalogLifecycle::Active
            || source_catalog.contains(entry.kind, entry.path.as_str())
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

    /// Whether a kept `rename from -> to` has already been applied: one active entry records `from`
    /// as a rename alias — it has moved off that spelling — while still owning `to` as its canonical
    /// path or another alias. Requiring `from` to be a historical alias, not the entry's current
    /// canonical path, separates consumed work from a live round-trip relocation back onto a former
    /// spelling, which still has work to perform. An onward rename flattens every prior spelling
    /// onto the entry's new canonical path, so this single pass resolves a consumed chain of
    /// arbitrary depth: `from` an alias and `to` either the current canonical (one hop) or another
    /// alias (a deeper chain).
    fn rename_already_applied(&self, from: &str, to: &str) -> bool {
        self.entries.values().any(|binding| {
            let from_is_alias = binding.entry.aliases.iter().any(|alias| alias == from);
            let owns_to =
                binding.entry.path == to || binding.entry.aliases.iter().any(|alias| alias == to);
            from_is_alias && owns_to
        })
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

    /// The paths of every active entry of `kind` strictly below `ancestor_path` in the tree
    /// (each `ancestor_path::…`). A rename moves the whole subtree's identity, so these
    /// descendants must travel with it: enum members under a renamed category, resource members
    /// under a renamed resource.
    fn active_descendants(&self, kind: CatalogEntryKind, ancestor_path: &str) -> Vec<&'a str> {
        let prefix = format!("{ancestor_path}::");
        self.entries
            .iter()
            .filter_map(|((entry_kind, path), _)| {
                (*entry_kind == kind && path.starts_with(&prefix)).then_some(*path)
            })
            .collect()
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
    /// Whether this is a `required` plain field. A required field added over an established
    /// store cannot be recorded by a plain `marrow run` — backfilling existing records is
    /// data work an explicit `evolve apply` must drive — so its pending-identity remedy
    /// directs there rather than to a plain run.
    pub(crate) required_field: bool,
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
            required_field: false,
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
            required_field: node.is_required_field(),
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
/// `check` stays read-only and exits clean. The remedy names how the entity is recorded: a
/// `required` field added over an established store needs explicit data-evolution work to
/// backfill existing records, while every additive change a plain `marrow run` auto-applies.
fn push_pending_identity(source: &SourceCatalogEntry, diagnostics: &mut Vec<CheckDiagnostic>) {
    let records = if source.required_field {
        PendingRecord::EvolveApply
    } else {
        PendingRecord::RunOrEvolveApply
    };
    let remedy = match records {
        PendingRecord::RunOrEvolveApply => "the next marrow run or marrow evolve apply records it",
        PendingRecord::EvolveApply => {
            "marrow evolve preview then marrow evolve apply records it, supplying a default or transform for existing records"
        }
    };
    diagnostics.push(
        CheckDiagnostic::warning(
            CHECK_CATALOG_INTENT,
            &source.file,
            source.span,
            format!(
                "`{}` is new and has not been saved to the store yet; {remedy}",
                source.path
            ),
        )
        .with_payload(DiagnosticPayload::CatalogIntent(
            CatalogIntentDiagnostic::PendingIdentity {
                path: source.path.clone(),
                records,
            },
        )),
    );
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

fn proposed_catalog_entry<E: CatalogIdEntropy>(
    source: &SourceCatalogEntry,
    allocator: &mut StableIdAllocator<E>,
) -> io::Result<CatalogEntry> {
    // Source-derived shape signatures are recorded in post-passes once every referent's id is
    // bound; freshly minted entries start without them.
    Ok(proposed_catalog_entry_with_id(
        source,
        &allocator.allocate()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_level_catalog_diagnostic_points_at_the_start_of_its_file() {
        let diagnostic = catalog_diagnostic(
            std::path::PathBuf::from("src/books.mw"),
            "accepted catalog metadata is not valid".to_string(),
        );
        assert_eq!(diagnostic.code, CHECK_CATALOG_INTENT);
        assert_eq!(diagnostic.span.line, 1);
        assert_eq!(diagnostic.span.column, 1);
    }

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
            applied_transform: None,
        }
    }

    fn reserved_entry(kind: CatalogEntryKind, path: &str, stable_id: &str) -> CatalogEntry {
        CatalogEntry {
            lifecycle: CatalogLifecycle::Reserved,
            ..active_entry(kind, path, stable_id)
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
            required_field: false,
        }
    }

    struct FailingEntropy;

    impl stable_id::CatalogIdEntropy for FailingEntropy {
        fn next_id_bytes(&mut self) -> std::io::Result<[u8; 16]> {
            Err(std::io::Error::other("entropy unavailable"))
        }
    }

    fn retire(path: &str) -> RetireIntent {
        RetireIntent {
            path: path.to_string(),
            file: std::path::PathBuf::from("src/books.mw"),
            span: SourceSpan::default(),
        }
    }

    /// A `cat_<32 hex>` id from a single distinguishing byte, for fixtures that pin a
    /// specific committed or tombstoned id.
    fn fixed_id(byte: u8) -> String {
        format!("cat_{byte:032x}")
    }

    /// A committed lock entry carrying `stable_id` for `(kind, path)` — the adoption anchor a
    /// fresh source entity at the same `(kind, path)` carries the id forward by. `key_shape`
    /// records a real accepted shape so the entry is fingerprinted as a SHAPED entity, proving
    /// adoption keys on the path rather than a fingerprint a first-run pre-image cannot match.
    fn committed_lock_entry(
        kind: CatalogEntryKind,
        path: &str,
        stable_id: &str,
        key_shape: Option<&str>,
    ) -> marrow_catalog::LockEntry {
        let entry = CatalogEntry {
            accepted_key_shape: key_shape.map(|shape| shape.to_string()),
            ..active_entry(kind, path, stable_id)
        };
        marrow_catalog::LockEntry::from_catalog_entry(&entry)
    }

    fn lock(
        entries: Vec<marrow_catalog::LockEntry>,
        ledger: Vec<LockLedgerTombstone>,
        epoch_high_water: u64,
    ) -> CatalogLock {
        CatalogLock::new(
            entries,
            ledger,
            epoch_high_water,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        )
        .expect("lock builds")
    }

    /// A reserved tombstone for `stable_id` at a retired member path distinct from any source
    /// entity, so a reissue of the id surfaces as an active entry carrying a tombstoned id rather
    /// than a reserved-path reuse at the same `(kind, path)`.
    fn tombstone(stable_id: &str, high_water: u64) -> LockLedgerTombstone {
        LockLedgerTombstone {
            kind: CatalogEntryKind::ResourceMember,
            path: "books::Book::retired".to_string(),
            id: stable_id.to_string(),
            lifecycle: CatalogLifecycle::Reserved,
            high_water,
        }
    }

    /// An OS-entropy failure while minting a fresh id for a source entity the lock does not record
    /// must surface as a hard allocation error, exactly as the no-lock mint does, not collapse to a
    /// diagnostic-less Refused that lets the check go green with no identity bound.
    #[test]
    fn adoption_mint_entropy_failure_is_a_hard_allocation_error_not_a_silent_refusal() {
        // The lock records nothing, so the lone source entity must be freshly minted; the injected
        // entropy fails, so the mint cannot succeed.
        let committed = lock(Vec::new(), Vec::new(), 0);
        let source_entries = vec![source_entry(CatalogEntryKind::Store, "books::^books")];
        let allocator =
            StableIdAllocator::with_entropy(std::collections::HashSet::new(), FailingEntropy);
        let mut diagnostics = Vec::new();

        let result = adopt_first_run_entries_with(
            &source_entries,
            &committed,
            &committed.ledger,
            &EvolveIntents::default(),
            allocator,
            &mut diagnostics,
        );

        assert!(
            matches!(result, Err(CatalogProposalError::Allocation(_))),
            "an entropy fault must propagate as an allocation error, got {:?}",
            result
                .as_ref()
                .map(|outcome| outcome.is_some())
                .map_err(|_| ())
        );
    }

    /// A reserved tombstone for a retired entity at `(kind, path)`, materialized into a first-run
    /// proposal as a Reserved row carrying the tombstoned id. Distinct from [`tombstone`]: the
    /// `(kind, path)` is the one under test, so the materialized reserved row reseeds the never-reuse
    /// defense rather than serving as a reissue probe.
    fn reserved_tombstone_at(
        kind: CatalogEntryKind,
        path: &str,
        stable_id: &str,
        high_water: u64,
    ) -> LockLedgerTombstone {
        LockLedgerTombstone {
            kind,
            path: path.to_string(),
            id: stable_id.to_string(),
            lifecycle: CatalogLifecycle::Reserved,
            high_water,
        }
    }

    /// A first-run proposal materialized from a lock whose ledger retired a STORE INDEX must build
    /// and validate: the reserved row that reseeds the never-reuse defense is identity-only and
    /// holds no live index shape. Before the validate relaxation this proposal failed closed on a
    /// synthesized catalog.invalid, bricking a fresh store re-seeded from a valid committed lock.
    #[test]
    fn first_run_materializes_a_store_index_tombstone_without_a_shape() {
        let retired_index_id = fixed_id(0x51);
        let retired_index_path = "books::^books::by_title";
        let ledger = vec![reserved_tombstone_at(
            CatalogEntryKind::StoreIndex,
            retired_index_path,
            &retired_index_id,
            7,
        )];
        let committed = lock(Vec::new(), ledger.clone(), 7);
        let mut diagnostics = Vec::new();

        // No source entity re-declares the retired index, so the only proposal row is the
        // materialized reserved tombstone. Build through CatalogMetadata::new + validate, exactly
        // as the first-run proposal path does, so the reserved StoreIndex row is held to the real
        // accepted invariants.
        let proposal = adopt_first_run(&[], &committed, &ledger, &mut diagnostics)
            .expect("a store-index tombstone materializes into a valid first-run proposal");
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let reserved = proposal
            .entries
            .iter()
            .find(|entry| entry.kind == CatalogEntryKind::StoreIndex)
            .expect("the proposal carries the reserved store-index row");
        assert_eq!(
            reserved.stable_id, retired_index_id,
            "preserves the retired id"
        );
        assert_eq!(reserved.lifecycle, CatalogLifecycle::Reserved);
        assert!(
            reserved.accepted_index_shape.is_none(),
            "a reserved store-index tombstone holds no live shape"
        );
        proposal
            .validate()
            .expect("the materialized proposal satisfies the accepted invariants");
    }

    /// The Store-kind control: a retired STORE tombstone reseeds the same way, confirming the
    /// relaxation is StoreIndex-specific and the sibling kind was always sound.
    #[test]
    fn first_run_materializes_a_store_tombstone_without_a_shape() {
        let retired_store_id = fixed_id(0x52);
        let retired_store_path = "archive::^archive";
        let ledger = vec![reserved_tombstone_at(
            CatalogEntryKind::Store,
            retired_store_path,
            &retired_store_id,
            7,
        )];
        let committed = lock(Vec::new(), ledger.clone(), 7);
        let mut diagnostics = Vec::new();

        let proposal = adopt_first_run(&[], &committed, &ledger, &mut diagnostics)
            .expect("a store tombstone materializes into a valid first-run proposal");
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let reserved = proposal
            .entries
            .iter()
            .find(|entry| entry.kind == CatalogEntryKind::Store)
            .expect("the proposal carries the reserved store row");
        assert_eq!(
            reserved.stable_id, retired_store_id,
            "preserves the retired id"
        );
        proposal
            .validate()
            .expect("the materialized proposal satisfies the accepted invariants");
    }

    /// First-run binding with a present committed lock adopts the lock's identity by `(kind,
    /// path)` and its epoch high-water instead of minting fresh ids at epoch 1, even for a SHAPED
    /// entity whose committed fingerprint a fresh source pre-image cannot reproduce; it refuses an
    /// adopted id that would reissue a tombstone, and advances re-bind from the high-water.
    #[test]
    fn first_run_with_present_lock_adopts_lock_identity_and_epoch_high_water() {
        let program = CheckedProgram::default();
        let evolve = EvolveIntents::default();

        // A single SHAPED store source entity, and a lock that committed it under `store_id` at
        // epoch high-water 12. The committed entry was fingerprinted with a real `int` key shape
        // the fresh source pre-image (no accepted shape) cannot match, so only path-keyed
        // adoption carries the id forward.
        let store_id = fixed_id(0xa1);
        let store_path = "books::^books";
        let source_entries = vec![source_entry(CatalogEntryKind::Store, store_path)];
        let source = CatalogSource {
            duplicate_keys: duplicate_source_keys(&source_entries),
            entries: source_entries,
        };
        let high_water = 12;
        let committed = lock(
            vec![committed_lock_entry(
                CatalogEntryKind::Store,
                store_path,
                &store_id,
                Some("int"),
            )],
            Vec::new(),
            high_water,
        );

        // Oracle 1 + 2: the first-run binding adopts the committed id and the lock's epoch
        // high-water rather than minting fresh at epoch 1. The fixture program captures no source
        // renderings, so its shape digest cannot match the lock's recorded digest: the lock does
        // not adopt cleanly here, so the binding is a proposal carrying the adopted identity.
        let mut diagnostics = Vec::new();
        let Ok(FirstRunOutcome::Proposal(proposal)) = adopt_or_mint_first_run(
            &program,
            &evolve,
            &source,
            Some(&committed),
            &mut diagnostics,
        ) else {
            panic!("a present lock carries an adopting first-run proposal");
        };
        assert_eq!(
            proposal.epoch, high_water,
            "adopts the lock epoch high-water"
        );
        let adopted = proposal
            .entries
            .iter()
            .find(|entry| entry.kind == CatalogEntryKind::Store && entry.path == store_path)
            .expect("proposal carries the store");
        assert_eq!(adopted.stable_id, store_id, "adopts the committed lock id");

        // Oracle 3: a committed id adoption would carry forward that the ledger also
        // tombstones makes the binding push the typed check.lock_corrupt code and carry no
        // adopting proposal (assert no Active entry binds the tombstoned id). The lock's own
        // codec keeps a committed id off its ledger, so the refusal is the binding's
        // independent fail-closed gate over the adopted result, exercised here directly.
        let tombstoned = store_id.clone();
        let mut refusal_diagnostics = Vec::new();
        let refused = adopt_first_run(
            &source.entries,
            &committed,
            &[tombstone(&tombstoned, high_water)],
            &mut refusal_diagnostics,
        );
        assert!(
            refused.is_none(),
            "a tombstone-reissuing adoption carries no proposal"
        );
        assert!(
            refusal_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == crate::CHECK_LOCK_CORRUPT),
            "the refusal pushes the typed check.lock_corrupt code: {refusal_diagnostics:#?}"
        );

        // Oracle 4: re-binding against an accepted catalog whose epoch is below the lock's
        // high-water advances from the high-water, not from accepted.epoch.
        assert_eq!(
            advance_epoch(5, high_water),
            13,
            "max(accepted, high_water) + 1"
        );
        assert_eq!(
            advance_epoch(9, 3),
            10,
            "max favors the larger accepted epoch"
        );
    }

    /// A committed lock entry carrying `stable_id` and a rename `alias` for `(kind, path)`, the
    /// fingerprint over the active shape. A fresh checkout reconstructs the alias onto the adopted
    /// entry, so a kept consumed rename block resolves its old spelling against the seed.
    fn committed_lock_entry_with_alias(
        kind: CatalogEntryKind,
        path: &str,
        stable_id: &str,
        alias: &str,
    ) -> marrow_catalog::LockEntry {
        let entry = CatalogEntry {
            aliases: vec![alias.to_string()],
            ..active_entry(kind, path, stable_id)
        };
        marrow_catalog::LockEntry::from_catalog_entry(&entry)
    }

    fn rename(from_path: &str, to_path: &str) -> RenameIntent {
        RenameIntent {
            from_path: from_path.to_string(),
            to_path: to_path.to_string(),
            file: std::path::PathBuf::from("src/books.mw"),
            span: SourceSpan::default(),
        }
    }

    /// A fresh checkout seeded from a lock that carries a renamed enum category's old spelling as
    /// an alias resolves a KEPT consumed category-rename block as already-recorded, not as an
    /// unresolvable `check.evolve_target`. The category and its descendant leaf both carry their
    /// old spellings forward as aliases the seeded catalog reconstructs.
    #[test]
    fn first_run_seeds_a_kept_enum_category_rename_block_from_lock_aliases() {
        let program = CheckedProgram::default();
        let category_id = fixed_id(0xc1);
        let leaf_id = fixed_id(0xc2);
        let source_entries = vec![
            source_entry(CatalogEntryKind::EnumMember, "pets::Pet::beast"),
            source_entry(CatalogEntryKind::EnumMember, "pets::Pet::beast::dog"),
        ];
        let source = CatalogSource {
            duplicate_keys: duplicate_source_keys(&source_entries),
            entries: source_entries,
        };
        let committed = lock(
            vec![
                committed_lock_entry_with_alias(
                    CatalogEntryKind::EnumMember,
                    "pets::Pet::beast",
                    &category_id,
                    "pets::Pet::mammal",
                ),
                committed_lock_entry_with_alias(
                    CatalogEntryKind::EnumMember,
                    "pets::Pet::beast::dog",
                    &leaf_id,
                    "pets::Pet::mammal::dog",
                ),
            ],
            Vec::new(),
            4,
        );
        let mut evolve = EvolveIntents::default();
        evolve
            .renames
            .push(rename("pets::Pet::mammal", "pets::Pet::beast"));

        let mut diagnostics = Vec::new();
        let Ok(outcome) = adopt_or_mint_first_run(
            &program,
            &evolve,
            &source,
            Some(&committed),
            &mut diagnostics,
        ) else {
            panic!("seeding a kept category-rename block binds");
        };
        assert!(
            matches!(
                outcome,
                FirstRunOutcome::Accepted(_) | FirstRunOutcome::Proposal(_)
            ),
            "a kept consumed category rename seeds rather than refusing"
        );
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != CHECK_EVOLVE_TARGET),
            "a kept consumed category rename is already recorded, not an evolve-target failure: {diagnostics:#?}"
        );
    }

    /// A rename whose old spelling the seeded catalog records nowhere — neither as an active alias
    /// nor a reserved row — still fails closed `check.evolve_target`. The alias projection narrows
    /// the consumed case; it never blesses a genuinely-unresolvable target.
    #[test]
    fn first_run_reports_a_genuinely_unresolvable_rename_target() {
        let program = CheckedProgram::default();
        let id = fixed_id(0xd1);
        let source_entries = vec![source_entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::label",
        )];
        let source = CatalogSource {
            duplicate_keys: duplicate_source_keys(&source_entries),
            entries: source_entries,
        };
        // The lock records `Book.label` with NO alias: nothing named `Book.ghost` was ever
        // accepted, so the rename relocates nothing.
        let committed = lock(
            vec![committed_lock_entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::label",
                &id,
                None,
            )],
            Vec::new(),
            4,
        );
        let mut evolve = EvolveIntents::default();
        evolve
            .renames
            .push(rename("books::Book::ghost", "books::Book::label"));

        let mut diagnostics = Vec::new();
        assert!(
            adopt_or_mint_first_run(
                &program,
                &evolve,
                &source,
                Some(&committed),
                &mut diagnostics
            )
            .is_ok(),
            "an unresolvable rename still binds a proposal"
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == CHECK_EVOLVE_TARGET),
            "a rename naming nothing the seed reconstructs fails closed: {diagnostics:#?}"
        );
    }

    /// A fresh checkout whose committed lock still records an enum category at its OLD spelling,
    /// with a pending (unapplied) category rename in source. The seed must carry the committed
    /// identity of the category and every descendant leaf forward under the new spelling — exactly
    /// as a present store relocates them — and resolve the kept rename block against that, not fail
    /// `check.evolve_target` because the empty store could not be seeded.
    #[test]
    fn first_run_seeds_a_pending_enum_category_rename_against_the_committed_old_name() {
        let program = CheckedProgram::default();
        let category_id = fixed_id(0xe1);
        let leaf_id = fixed_id(0xe2);
        let source_entries = vec![
            source_entry(CatalogEntryKind::EnumMember, "pets::Pet::beast"),
            source_entry(CatalogEntryKind::EnumMember, "pets::Pet::beast::dog"),
        ];
        let source = CatalogSource {
            duplicate_keys: duplicate_source_keys(&source_entries),
            entries: source_entries,
        };
        // The lock records the OLD spellings as canonical with NO aliases: the rename is pending,
        // never applied, so the committed identity rests at `Pet::mammal` and `Pet::mammal::dog`.
        let committed = lock(
            vec![
                committed_lock_entry(
                    CatalogEntryKind::EnumMember,
                    "pets::Pet::mammal",
                    &category_id,
                    None,
                ),
                committed_lock_entry(
                    CatalogEntryKind::EnumMember,
                    "pets::Pet::mammal::dog",
                    &leaf_id,
                    None,
                ),
            ],
            Vec::new(),
            4,
        );
        let mut evolve = EvolveIntents::default();
        evolve
            .renames
            .push(rename("pets::Pet::mammal", "pets::Pet::beast"));

        let mut diagnostics = Vec::new();
        let Ok(outcome) = adopt_or_mint_first_run(
            &program,
            &evolve,
            &source,
            Some(&committed),
            &mut diagnostics,
        ) else {
            panic!("a pending category rename binds");
        };
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != CHECK_EVOLVE_TARGET),
            "a pending category rename resolves against the committed old spelling: {diagnostics:#?}"
        );
        let FirstRunOutcome::Proposal(proposal) = outcome else {
            panic!("a pending rename drifts the source from the lock, so it is a proposal");
        };
        let category = proposal
            .entries
            .iter()
            .find(|entry| entry.path == "pets::Pet::beast")
            .expect("proposal carries the renamed category");
        assert_eq!(category.stable_id, category_id, "category carries its id");
        assert!(
            category
                .aliases
                .iter()
                .any(|alias| alias == "pets::Pet::mammal"),
            "the category's old spelling is recorded as an alias: {category:#?}"
        );
        let leaf = proposal
            .entries
            .iter()
            .find(|entry| entry.path == "pets::Pet::beast::dog")
            .expect("proposal carries the cascaded descendant leaf");
        assert_eq!(leaf.stable_id, leaf_id, "descendant leaf carries its id");
        assert!(
            leaf.aliases
                .iter()
                .any(|alias| alias == "pets::Pet::mammal::dog"),
            "the descendant leaf's old spelling is recorded as an alias: {leaf:#?}"
        );
    }

    /// The single advance owner never panics or wraps to zero at the integer ceiling. A
    /// codec-valid lock can no longer carry an un-advanceable high-water, but the advance rule is
    /// the load-bearing monotonicity guarantee, so it saturates rather than overflowing even if a
    /// ceiling value reaches it: a wrap to zero on a write-adjacent path would silently reuse a
    /// witnessed epoch for different identity.
    #[test]
    fn advance_epoch_saturates_at_the_ceiling_without_panicking() {
        assert_eq!(
            advance_epoch(u64::MAX, 0),
            u64::MAX,
            "an accepted epoch at the ceiling saturates rather than wrapping to zero"
        );
        assert_eq!(
            advance_epoch(0, u64::MAX),
            u64::MAX,
            "a lock high-water at the ceiling saturates rather than wrapping to zero"
        );
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
        let accepted = CatalogMetadata::new(1, entries.clone()).expect("catalog builds");
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

    fn rename_intent(from_path: &str, to_path: &str) -> RenameIntent {
        RenameIntent {
            from_path: from_path.to_string(),
            to_path: to_path.to_string(),
            file: std::path::PathBuf::from("src/app.mw"),
            span: SourceSpan::default(),
        }
    }

    /// An explicit `evolve rename` of an enum category resolves the category itself and, by
    /// carry-forward from the accepted catalog, every descendant leaf and nested category in its
    /// subtree — so a single category rename keeps every stored descendant value valid without a
    /// per-leaf rename. A leaf the renamed subtree no longer declares carries no derived rename: it
    /// is an ordinary member removal the accepted entry's own discharge covers.
    #[test]
    fn renaming_an_enum_category_cascades_to_its_accepted_descendants() {
        let category = active_entry(
            CatalogEntryKind::EnumMember,
            "app::Pet::mammal",
            &fixed_id(1),
        );
        let dog = active_entry(
            CatalogEntryKind::EnumMember,
            "app::Pet::mammal::dog",
            &fixed_id(2),
        );
        let cat = active_entry(
            CatalogEntryKind::EnumMember,
            "app::Pet::mammal::cat",
            &fixed_id(3),
        );
        let fish = active_entry(CatalogEntryKind::EnumMember, "app::Pet::fish", &fixed_id(4));
        let accepted = CatalogMetadata::new(1, vec![category, dog, cat, fish]).expect("builds");
        // Source declares the renamed subtree (`beast` with `dog`/`cat`) plus the untouched `fish`.
        let source = vec![
            source_entry(CatalogEntryKind::EnumMember, "app::Pet::beast"),
            source_entry(CatalogEntryKind::EnumMember, "app::Pet::beast::dog"),
            source_entry(CatalogEntryKind::EnumMember, "app::Pet::beast::cat"),
            source_entry(CatalogEntryKind::EnumMember, "app::Pet::fish"),
        ];
        let mut diagnostics = Vec::new();

        let resolved = resolve_renames(
            &AcceptedCatalog::new(&accepted),
            &SourceCatalog::new(&source),
            &[rename_intent("app::Pet::mammal", "app::Pet::beast")],
            &mut diagnostics,
        );

        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let from_for = |to_path: &str| {
            resolved
                .get(to_path)
                .map(|rename| rename.from_path.as_str())
        };
        assert_eq!(from_for("app::Pet::beast"), Some("app::Pet::mammal"));
        assert_eq!(
            from_for("app::Pet::beast::dog"),
            Some("app::Pet::mammal::dog"),
            "the descendant leaf identity travels with the category"
        );
        assert_eq!(
            from_for("app::Pet::beast::cat"),
            Some("app::Pet::mammal::cat")
        );
        assert!(
            from_for("app::Pet::fish").is_none(),
            "a sibling outside the renamed subtree is untouched"
        );
    }

    /// An explicit `evolve rename` of a resource resolves the resource itself and, by
    /// carry-forward from the accepted catalog, every resource member in its subtree — so a single
    /// resource rename keeps every stored member value valid without a per-member rename, exactly
    /// as an enum-category rename cascades to its descendant leaves. A member's saved identity is
    /// keyed on its full ancestor path (which includes the resource), so the rename must travel.
    #[test]
    fn renaming_a_resource_cascades_to_its_accepted_members() {
        let book = active_entry(CatalogEntryKind::Resource, "app::Book", &fixed_id(1));
        let note = active_entry(
            CatalogEntryKind::ResourceMember,
            "app::Book::note",
            &fixed_id(2),
        );
        let nested = active_entry(
            CatalogEntryKind::ResourceMember,
            "app::Book::name::first",
            &fixed_id(3),
        );
        let other = active_entry(CatalogEntryKind::Resource, "app::Author", &fixed_id(4));
        let accepted = CatalogMetadata::new(1, vec![book, note, nested, other]).expect("builds");
        // Source declares the renamed resource (`Volume` with `note`/`name::first`) plus `Author`.
        let source = vec![
            source_entry(CatalogEntryKind::Resource, "app::Volume"),
            source_entry(CatalogEntryKind::ResourceMember, "app::Volume::note"),
            source_entry(CatalogEntryKind::ResourceMember, "app::Volume::name::first"),
            source_entry(CatalogEntryKind::Resource, "app::Author"),
        ];
        let mut diagnostics = Vec::new();

        let resolved = resolve_renames(
            &AcceptedCatalog::new(&accepted),
            &SourceCatalog::new(&source),
            &[rename_intent("app::Book", "app::Volume")],
            &mut diagnostics,
        );

        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let from_for = |to_path: &str| {
            resolved
                .get(to_path)
                .map(|rename| rename.from_path.as_str())
        };
        assert_eq!(from_for("app::Volume"), Some("app::Book"));
        assert_eq!(
            from_for("app::Volume::note"),
            Some("app::Book::note"),
            "the member identity travels with the resource"
        );
        assert_eq!(
            from_for("app::Volume::name::first"),
            Some("app::Book::name::first"),
            "a nested member identity travels with the resource"
        );
        assert!(
            from_for("app::Author").is_none(),
            "a sibling resource outside the rename is untouched"
        );
    }

    /// A kept consumed rename whose target was itself renamed onward — a depth>1 alias chain
    /// (`title -> label -> name`) — is recognized as already-applied against the present accepted
    /// catalog, not as an unresolvable `check.evolve_target`. The active entry carries every prior
    /// spelling as a flattened alias, so the stale `rename title -> label` block, whose `label`
    /// target the source no longer declares, resolves inert exactly as deleting it would.
    #[test]
    fn a_kept_rename_through_an_onward_alias_chain_is_recognized_as_consumed() {
        let current = CatalogEntry {
            aliases: vec![
                "books::Book::label".to_string(),
                "books::Book::title".to_string(),
            ],
            ..active_entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::name",
                &fixed_id(1),
            )
        };
        let accepted = CatalogMetadata::new(1, vec![current]).expect("builds");
        let source = vec![source_entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::name",
        )];
        let mut diagnostics = Vec::new();

        let resolved = resolve_renames(
            &AcceptedCatalog::new(&accepted),
            &SourceCatalog::new(&source),
            &[
                rename_intent("books::Book::title", "books::Book::label"),
                rename_intent("books::Book::label", "books::Book::name"),
            ],
            &mut diagnostics,
        );

        assert!(
            diagnostics.is_empty(),
            "both consumed renames in the chain are inert: {diagnostics:#?}"
        );
        assert!(
            resolved.is_empty(),
            "a fully-consumed rename chain leaves no relocation work"
        );
    }

    /// A rename whose target names nothing the accepted catalog carries — neither a canonical entry
    /// nor any recorded alias — still fails closed `check.evolve_target`. The onward-chain
    /// recognition narrows the consumed case; it never blesses a genuinely-unresolvable target.
    #[test]
    fn a_rename_onto_an_unrecorded_target_still_fails_evolve_target() {
        let current = CatalogEntry {
            aliases: vec![
                "books::Book::label".to_string(),
                "books::Book::title".to_string(),
            ],
            ..active_entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::name",
                &fixed_id(1),
            )
        };
        let accepted = CatalogMetadata::new(1, vec![current]).expect("builds");
        let source = vec![source_entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::name",
        )];
        let mut diagnostics = Vec::new();

        resolve_renames(
            &AcceptedCatalog::new(&accepted),
            &SourceCatalog::new(&source),
            &[rename_intent("books::Book::ghost", "books::Book::phantom")],
            &mut diagnostics,
        );

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == CHECK_EVOLVE_TARGET),
            "a target the accepted catalog never carried fails closed: {diagnostics:#?}"
        );
    }

    #[test]
    fn allocation_failure_preserves_prior_catalog_diagnostics() {
        let reserved_path = "books::Book::title";
        let accepted = CatalogMetadata::new(
            1,
            vec![reserved_entry(
                CatalogEntryKind::ResourceMember,
                reserved_path,
                "cat_000000000000000000000000000000aa",
            )],
        )
        .expect("catalog builds");
        let source_entries = vec![
            source_entry(CatalogEntryKind::ResourceMember, reserved_path),
            source_entry(CatalogEntryKind::ResourceMember, "books::Book::pages"),
        ];
        let source = CatalogSource {
            duplicate_keys: duplicate_source_keys(&source_entries),
            entries: source_entries,
        };
        let accepted_index = AcceptedCatalog::new(&accepted);
        let mut proposal_entries = accepted.entries.clone();
        let mut allocator = StableIdAllocator::with_entropy(
            proposal_entries
                .iter()
                .map(|entry| entry.stable_id.clone())
                .collect(),
            FailingEntropy,
        );
        let mut ids = HashMap::new();
        let mut renames = HashMap::new();
        let mut diagnostics = Vec::new();

        let result = bind_source_entries(
            &accepted_index,
            &source,
            &mut renames,
            &mut ids,
            &mut proposal_entries,
            &mut allocator,
            &mut diagnostics,
        );

        assert_eq!(
            result.as_ref().map_err(|error| error.kind()),
            Err(std::io::ErrorKind::Other)
        );
        assert!(ids.is_empty());
        assert_eq!(proposal_entries, accepted.entries);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(
            diagnostics[0].payload,
            DiagnosticPayload::ReservedCatalogPathReuse {
                source_kind: CatalogEntryKind::ResourceMember,
                source_path: reserved_path.to_string(),
                reserved_stable_id: "cat_000000000000000000000000000000aa".to_string(),
            }
        );
    }
}
