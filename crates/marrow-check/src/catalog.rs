use std::collections::{HashMap, HashSet};
use std::path::Path;

use marrow_project::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};
use marrow_syntax::{Severity, SourceSpan};

use crate::{CHECK_CATALOG_INTENT, CheckDiagnostic, CheckedProgram};

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
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let accepted = read_accepted_catalog(project_root, config, diagnostics);
    let binding = catalog_binding(program, accepted.as_ref(), diagnostics);
    program
        .facts
        .bind_catalog_ids(&program.modules, &binding.ids);
    program.catalog.accepted_epoch = binding.accepted_epoch;
    program.catalog.accepted_digest = binding.accepted_digest;
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
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> CatalogBinding {
    let source_entries = source_catalog_entries(program);
    let mut ids = HashMap::new();
    let proposal = match accepted {
        Some(catalog) => {
            let accepted_index = AcceptedCatalog::new(catalog);
            let mut proposal_entries = catalog.entries.clone();
            let mut used_stable_ids = stable_ids(&proposal_entries);
            let mut changed = false;
            for source in &source_entries {
                match accepted_index.active_entry(source.kind, &source.path) {
                    Some(binding) => {
                        ids.insert(
                            CatalogKey::new(source.kind, source.path.clone()),
                            binding.entry.stable_id.clone(),
                        );
                    }
                    None => {
                        push_missing_intent(source, diagnostics);
                        prepare_proposal_path(&mut proposal_entries, source.kind, &source.path);
                        proposal_entries.push(proposed_catalog_entry(source, &mut used_stable_ids));
                        changed = true;
                    }
                }
            }
            changed.then(|| CatalogMetadata::new(catalog.epoch + 1, proposal_entries))
        }
        None => {
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

    CatalogBinding {
        accepted_epoch: accepted.map(|catalog| catalog.epoch),
        accepted_digest: accepted.map(|catalog| catalog.digest.clone()),
        ids,
        proposal,
    }
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
