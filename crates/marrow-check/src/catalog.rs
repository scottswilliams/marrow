use std::collections::{HashMap, HashSet};
use std::path::Path;

use marrow_project::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};
use marrow_syntax::{Severity, SourceSpan};

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
    program
        .facts
        .bind_catalog_ids(&program.modules, &binding.ids);
    program.catalog.accepted_epoch = binding.accepted_epoch;
    program.catalog.accepted_digest = binding.accepted_digest;
    program.catalog.accepted_entries = accepted.map(|catalog| catalog.entries).unwrap_or_default();
    program.catalog.evolve_defaults = bound_defaults(&evolve.defaults, &binding.ids);
    program.catalog.evolve_transforms = bound_transforms(&evolve.transforms, &binding.ids);
    program.catalog.proposal = binding.proposal;
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
            let mut used_stable_ids = stable_ids(&proposal_entries);
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
                    ids.insert(
                        CatalogKey::new(source.kind, source.path.clone()),
                        binding.entry.stable_id.clone(),
                    );
                } else if let Some(rename) = rename {
                    apply_rename(&mut proposal_entries, source, &rename.from_path, &mut ids);
                    changed = true;
                } else {
                    push_missing_intent(source, diagnostics);
                    prepare_proposal_path(&mut proposal_entries, source.kind, &source.path);
                    proposal_entries.push(proposed_catalog_entry(source, &mut used_stable_ids));
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
            changed.then(|| CatalogMetadata::new(catalog.epoch + 1, proposal_entries))
        }
        None => {
            for rename in &evolve.renames {
                report_unresolved_intent(&rename.file, rename.span, diagnostics);
            }
            for retire in &evolve.retires {
                report_unresolved_intent(&retire.file, retire.span, diagnostics);
            }
            let mut used_stable_ids = HashSet::new();
            Some(CatalogMetadata::new(
                1,
                source_entries
                    .iter()
                    .map(|source| proposed_catalog_entry(source, &mut used_stable_ids))
                    .collect(),
            ))
        }
    };

    // The proposal is the catalog a future accept commits, so it must satisfy the
    // same identity invariants. Validating it here makes an identity collision the
    // binding logic produced fail closed at check time rather than at apply.
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

    CatalogBinding {
        accepted_epoch: accepted.map(|catalog| catalog.epoch),
        accepted_digest: accepted.map(|catalog| catalog.digest.clone()),
        ids,
        proposal,
    }
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

/// Resolve each `evolve transform` to the stable id its data cells use, carrying the
/// body fingerprint forward for the source digest.
fn bound_transforms(
    transforms: &[TransformIntent],
    ids: &HashMap<CatalogKey, String>,
) -> Vec<EvolveTransform> {
    transforms
        .iter()
        .filter_map(|transform| {
            member_target_id(&transform.path, ids).map(|catalog_id| EvolveTransform { catalog_id })
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

/// Mark each retired entity removed in the proposal. A retire names a destructive
/// intent over an accepted entry whose source declaration is gone; a path that
/// matches no active accepted entry is a target diagnostic. A retire of an entry
/// the source still declares is rejected: marking it removed would silently drop
/// data the running program still reads and writes, so the destructive intent only
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
        // against whichever same-path entry was found first: marking a still-declared
        // entry removed would drop data the running program still reads and writes.
        // Once no source entry declares the path, the lone active accepted entry
        // there is genuinely orphaned and safe to remove.
        if source_kinds.contains_key(retire.path.as_str()) {
            push_retire_source_declared(retire, diagnostics);
            continue;
        }
        match entries
            .iter_mut()
            .find(|entry| entry.lifecycle == CatalogLifecycle::Active && entry.path == retire.path)
        {
            Some(entry) => {
                entry.lifecycle = CatalogLifecycle::Removed;
                changed = true;
            }
            None => report_unresolved_intent(&retire.file, retire.span, diagnostics),
        }
    }
    changed
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
}

#[derive(Clone, Copy)]
struct AcceptedEntry<'a> {
    entry: &'a CatalogEntry,
}

impl<'a> AcceptedCatalog<'a> {
    fn new(catalog: &'a CatalogMetadata) -> Self {
        let mut entries = HashMap::new();
        for entry in &catalog.entries {
            if entry.lifecycle != CatalogLifecycle::Active {
                continue;
            }
            let binding = AcceptedEntry { entry };
            entries.insert((entry.kind, entry.path.as_str()), binding);
        }
        Self { entries }
    }

    fn active_entry(&self, kind: CatalogEntryKind, path: &str) -> Option<AcceptedEntry<'a>> {
        self.entries.get(&(kind, path)).copied()
    }
}

#[derive(Debug)]
pub(crate) struct SourceCatalogEntry {
    pub(crate) kind: CatalogEntryKind,
    pub(crate) path: String,
    pub(crate) file: std::path::PathBuf,
    pub(crate) span: SourceSpan,
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
            });
            collect_resource_members(&mut entries, module, &resource.name, &[], &resource.members);
        }
        for store in &module.stores {
            entries.push(SourceCatalogEntry {
                kind: CatalogEntryKind::Store,
                path: store_path(&module.name, &store.root),
                file: module.source_file.clone(),
                span: SourceSpan::default(),
            });
            for index in &store.indexes {
                entries.push(SourceCatalogEntry {
                    kind: CatalogEntryKind::StoreIndex,
                    path: store_index_path(&module.name, &store.root, &index.name),
                    file: module.source_file.clone(),
                    span: SourceSpan::default(),
                });
            }
        }
        for enum_schema in &module.enums {
            entries.push(SourceCatalogEntry {
                kind: CatalogEntryKind::Enum,
                path: enum_path(&module.name, &enum_schema.name),
                file: module.source_file.clone(),
                span: SourceSpan::default(),
            });
            for index in 0..enum_schema.members.len() {
                entries.push(SourceCatalogEntry {
                    kind: CatalogEntryKind::EnumMember,
                    path: enum_member_path(&module.name, &enum_schema.name, index, enum_schema),
                    file: module.source_file.clone(),
                    span: SourceSpan::default(),
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
        });
        collect_resource_members(entries, module, resource, &path, &node.members);
    }
}

fn push_missing_intent(source: &SourceCatalogEntry, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_CATALOG_INTENT,
        severity: Severity::Error,
        file: source.file.clone(),
        message: format!(
            "accepted catalog metadata has no active entry for `{}`; accept a catalog proposal before renaming durable identity",
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

fn prepare_proposal_path(entries: &mut Vec<CatalogEntry>, kind: CatalogEntryKind, path: &str) {
    entries.retain(|entry| {
        !(entry.kind == kind && entry.path == path && entry.lifecycle != CatalogLifecycle::Active)
    });
    for entry in entries.iter_mut().filter(|entry| entry.kind == kind) {
        entry.aliases.retain(|alias| alias != path);
    }
}

fn stable_ids(entries: &[CatalogEntry]) -> HashSet<String> {
    entries
        .iter()
        .map(|entry| entry.stable_id.clone())
        .collect()
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

fn proposal_stable_id(kind: CatalogEntryKind, path: &str) -> String {
    let payload = format!("{kind:?}:{path}");
    format!("cat_{:016x}", fnv1a64(payload.as_bytes()))
}

fn proposed_catalog_entry(
    source: &SourceCatalogEntry,
    used_stable_ids: &mut HashSet<String>,
) -> CatalogEntry {
    let stable_id = unique_proposal_stable_id(source.kind, &source.path, used_stable_ids);
    CatalogEntry {
        kind: source.kind,
        path: source.path.clone(),
        stable_id,
        aliases: Vec::new(),
        lifecycle: CatalogLifecycle::Active,
    }
}

fn unique_proposal_stable_id(
    kind: CatalogEntryKind,
    path: &str,
    used_stable_ids: &mut HashSet<String>,
) -> String {
    let base = proposal_stable_id(kind, path);
    if used_stable_ids.insert(base.clone()) {
        return base;
    }
    for suffix in 1u64.. {
        let candidate = format!("{base}_{suffix}");
        if used_stable_ids.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("unbounded suffix search always returns")
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// A stable digest of the analyzed program's durable and evolution surface, in the
/// same `fnv1a64:<hex>` form the catalog digest uses.
///
/// The digest is gap-free by construction: rather than enumerate durable facts field
/// by field, it renders every durable and evolution declaration — each `resource`,
/// `store`, `enum`, and `evolve` block — through the canonical formatter and hashes
/// the normalized text. Reformatting binds every member type, required flag, identity
/// key, index uniqueness and columns, keyed-layer key name and type at any nesting
/// depth, enum member, evolve default value, and transform body, so any change to the
/// shape a stored snapshot must satisfy drifts the digest while a pure whitespace
/// reformat of the same declarations leaves it unchanged. The evolution witness
/// records it so apply can abort if the source it activates no longer matches what was
/// discharged.
///
/// The rendering reads each module's source file because the formatter operates on the
/// syntax tree, which the checked program drops. A source file that no longer reads or
/// parses (a checked-program invariant violation) contributes a path-tagged marker so
/// the digest stays deterministic and never silently collides with a clean rendering.
pub(crate) fn analyzed_source_digest(program: &CheckedProgram) -> String {
    let mut entries: Vec<DurableRendering> = Vec::new();
    for module in &program.modules {
        let source = std::fs::read_to_string(&module.source_file).ok();
        let parsed = source.as_deref().map(marrow_syntax::parse_source);
        match (&source, &parsed) {
            (Some(source), Some(parsed)) => {
                for declaration in &parsed.file.declarations {
                    let Some(kind) = durable_kind(declaration) else {
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
    format!("fnv1a64:{:016x}", fnv1a64(payload.as_bytes()))
}

/// One durable declaration's normalized rendering, with the keys that order it
/// deterministically: its module, declaration kind, and declaration name.
struct DurableRendering {
    module: String,
    kind: DurableKind,
    name: String,
    text: String,
}

/// The declaration kinds whose shape a stored snapshot must satisfy. The discriminant
/// orders renderings deterministically within a module; an evolve block carries no
/// name, so its kind alone keeps it last.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DurableKind {
    Resource = 0,
    Store = 1,
    Enum = 2,
    Evolve = 3,
    Unreadable = 4,
}

/// The durable kind of a declaration, or `None` for a const or function, which carry
/// no durable shape the snapshot must satisfy.
fn durable_kind(declaration: &marrow_syntax::Declaration) -> Option<DurableKind> {
    match declaration {
        marrow_syntax::Declaration::Resource(_) => Some(DurableKind::Resource),
        marrow_syntax::Declaration::Store(_) => Some(DurableKind::Store),
        marrow_syntax::Declaration::Enum(_) => Some(DurableKind::Enum),
        marrow_syntax::Declaration::Evolve(_) => Some(DurableKind::Evolve),
        marrow_syntax::Declaration::Const(_) | marrow_syntax::Declaration::Function(_) => None,
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
        marrow_syntax::Declaration::Evolve(_)
        | marrow_syntax::Declaration::Const(_)
        | marrow_syntax::Declaration::Function(_) => String::new(),
    }
}
