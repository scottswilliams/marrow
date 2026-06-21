use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marrow_syntax::{
    Declaration, EnumDecl, EnumMember, EvolveDecl, EvolveStep, FieldDecl, FunctionDecl, GroupDecl,
    IndexDecl, ResourceDecl, ResourceMember, SourceFile, SourceSpan, StoreDecl, SurfaceDecl,
};

use crate::source_spans::source_span_at;
use crate::{
    AnalysisSnapshot, CatalogDeclaration, CatalogEntryKind, CheckedFacts, EnumMemberFact,
    ModuleFact, ModuleId, ResourceMemberFact,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSymbol {
    pub name: String,
    pub kind: SourceSymbolKind,
    pub file: PathBuf,
    pub span: SourceSpan,
    pub container: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceSymbolKind {
    Constant,
    Function,
    Resource,
    Store,
    StoreIndex,
    ResourceMember,
    Enum,
    EnumMember,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: DocumentSymbolKind,
    pub span: SourceSpan,
    pub selection_span: SourceSpan,
    pub children: Vec<DocumentSymbol>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentSymbolKind {
    Constant,
    Function,
    Enum,
    EnumMember,
    Resource,
    ResourceField,
    ResourceGroup,
    Store,
    StoreIndex,
    Surface,
    Evolve,
    EvolveStep,
}

pub fn document_symbols(file: &SourceFile, source: &str) -> Vec<DocumentSymbol> {
    file.declarations
        .iter()
        .map(|declaration| document_symbol(declaration, source))
        .collect()
}

pub fn source_symbols(snapshot: &AnalysisSnapshot) -> Vec<SourceSymbol> {
    let files: HashSet<&Path> = snapshot
        .files
        .iter()
        .map(|file| file.path.as_path())
        .collect();
    let containers = CatalogContainers::new(snapshot);
    let mut symbols = Vec::new();

    for module in &snapshot.program.modules {
        if !files.contains(module.source_file.as_path()) {
            continue;
        }
        let container = non_empty(&module.name).map(ToString::to_string);
        for function in &module.functions {
            symbols.push(SourceSymbol {
                name: function.name.clone(),
                kind: SourceSymbolKind::Function,
                file: module.source_file.clone(),
                span: function.span,
                container: container.clone(),
            });
        }
        for constant in &module.constants {
            symbols.push(SourceSymbol {
                name: constant.name.clone(),
                kind: SourceSymbolKind::Constant,
                file: module.source_file.clone(),
                span: constant.span,
                container: container.clone(),
            });
        }
    }

    for declaration in snapshot.catalog_declarations() {
        if !files.contains(declaration.file.as_path()) {
            continue;
        }
        symbols.push(SourceSymbol {
            name: catalog_symbol_name(declaration),
            kind: catalog_symbol_kind(declaration.kind),
            file: declaration.file.clone(),
            span: declaration.span,
            container: containers.get(declaration).map(ToString::to_string),
        });
    }

    symbols
}

pub fn source_symbols_matching(
    snapshot: &AnalysisSnapshot,
    search_text: &str,
) -> Vec<SourceSymbol> {
    let mut symbols = source_symbols(snapshot);
    let search_text = search_text.trim().to_lowercase();
    if search_text.is_empty() {
        return symbols;
    }

    symbols.retain(|symbol| source_symbol_match_rank(symbol, &search_text).is_some());
    symbols.sort_by_key(|symbol| source_symbol_match_rank(symbol, &search_text));
    symbols
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SourceSymbolMatchRank {
    NameExact,
    NamePrefix,
    NameContains,
    QualifiedPrefix,
    QualifiedContains,
}

fn source_symbol_match_rank(
    symbol: &SourceSymbol,
    search_text: &str,
) -> Option<SourceSymbolMatchRank> {
    let name = symbol.name.to_lowercase();
    if name == search_text {
        return Some(SourceSymbolMatchRank::NameExact);
    }
    if name.starts_with(search_text) {
        return Some(SourceSymbolMatchRank::NamePrefix);
    }
    if name.contains(search_text) {
        return Some(SourceSymbolMatchRank::NameContains);
    }

    let container = symbol.container.as_ref()?;
    let qualified = format!("{container}::{}", symbol.name).to_lowercase();
    if qualified.starts_with(search_text) {
        return Some(SourceSymbolMatchRank::QualifiedPrefix);
    }
    qualified
        .contains(search_text)
        .then_some(SourceSymbolMatchRank::QualifiedContains)
}

fn document_symbol(declaration: &Declaration, source: &str) -> DocumentSymbol {
    match declaration {
        Declaration::Const(constant) => outline_symbol(
            &constant.name,
            constant.ty.as_ref().map(|ty| ty.text.clone()),
            DocumentSymbolKind::Constant,
            constant.span,
            source,
            Vec::new(),
        ),
        Declaration::Function(function) => outline_symbol(
            &function.name,
            Some(function_signature(function)),
            DocumentSymbolKind::Function,
            function.span,
            source,
            Vec::new(),
        ),
        Declaration::Enum(enum_decl) => enum_symbol(enum_decl, source),
        Declaration::Resource(resource) => resource_symbol(resource, source),
        Declaration::Store(store) => store_symbol(store, source),
        Declaration::Surface(surface) => surface_symbol(surface, source),
        Declaration::Evolve(evolve) => evolve_symbol(evolve, source),
    }
}

fn evolve_symbol(evolve: &EvolveDecl, source: &str) -> DocumentSymbol {
    outline_symbol(
        "evolve",
        None,
        DocumentSymbolKind::Evolve,
        evolve.span,
        source,
        evolve
            .steps
            .iter()
            .map(|step| evolve_step_symbol(step, source))
            .collect(),
    )
}

fn evolve_step_symbol(step: &EvolveStep, source: &str) -> DocumentSymbol {
    let name = match step {
        EvolveStep::Rename { .. } => "rename",
        EvolveStep::Default { .. } => "default",
        EvolveStep::Retire { .. } => "retire",
        EvolveStep::Transform { .. } => "transform",
    };
    outline_symbol(
        name,
        None,
        DocumentSymbolKind::EvolveStep,
        step.span(),
        source,
        Vec::new(),
    )
}

fn enum_symbol(enum_decl: &EnumDecl, source: &str) -> DocumentSymbol {
    outline_symbol(
        &enum_decl.name,
        None,
        DocumentSymbolKind::Enum,
        enum_decl.span,
        source,
        enum_decl
            .members
            .iter()
            .map(|member| enum_member_symbol(member, source))
            .collect(),
    )
}

fn enum_member_symbol(member: &EnumMember, source: &str) -> DocumentSymbol {
    outline_symbol(
        &member.name,
        None,
        DocumentSymbolKind::EnumMember,
        member.span,
        source,
        Vec::new(),
    )
}

fn resource_symbol(resource: &ResourceDecl, source: &str) -> DocumentSymbol {
    outline_symbol(
        &resource.name,
        None,
        DocumentSymbolKind::Resource,
        resource.span,
        source,
        resource
            .members
            .iter()
            .map(|member| member_symbol(member, source))
            .collect(),
    )
}

fn member_symbol(member: &ResourceMember, source: &str) -> DocumentSymbol {
    match member {
        ResourceMember::Field(field) => field_symbol(field, source),
        ResourceMember::Group(group) => group_symbol(group, source),
    }
}

fn store_symbol(store: &StoreDecl, source: &str) -> DocumentSymbol {
    outline_symbol(
        &format!("^{}", store.root.root),
        Some(store.resource.clone()),
        DocumentSymbolKind::Store,
        store.span,
        source,
        store
            .indexes
            .iter()
            .map(|idx| index_symbol(idx, source))
            .collect(),
    )
}

fn surface_symbol(surface: &SurfaceDecl, source: &str) -> DocumentSymbol {
    outline_symbol(
        &surface.name,
        Some(format!("^{}", surface.store.root)),
        DocumentSymbolKind::Surface,
        surface.span,
        source,
        Vec::new(),
    )
}

fn field_symbol(field: &FieldDecl, source: &str) -> DocumentSymbol {
    outline_symbol(
        &field.name,
        Some(field.ty.text.clone()),
        DocumentSymbolKind::ResourceField,
        field.span,
        source,
        Vec::new(),
    )
}

fn group_symbol(group: &GroupDecl, source: &str) -> DocumentSymbol {
    outline_symbol(
        &group.name,
        None,
        DocumentSymbolKind::ResourceGroup,
        group.span,
        source,
        group
            .members
            .iter()
            .map(|member| member_symbol(member, source))
            .collect(),
    )
}

fn index_symbol(idx: &IndexDecl, source: &str) -> DocumentSymbol {
    outline_symbol(
        &idx.name,
        Some(format!("index({})", idx.args.join(", "))),
        DocumentSymbolKind::StoreIndex,
        idx.span,
        source,
        Vec::new(),
    )
}

fn outline_symbol(
    name: &str,
    detail: Option<String>,
    kind: DocumentSymbolKind,
    span: SourceSpan,
    source: &str,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    DocumentSymbol {
        name: name.to_string(),
        detail,
        kind,
        span,
        selection_span: selection_span(name, span, source),
        children,
    }
}

fn selection_span(name: &str, span: SourceSpan, source: &str) -> SourceSpan {
    let Some(slice) = source.get(span.start_byte..span.end_byte) else {
        return span;
    };
    let Some(offset) = slice.find(name) else {
        return span;
    };
    let start = span.start_byte + offset;
    let end = start + name.len();
    if source.get(start..end).is_none() {
        return span;
    }
    source_span_at(source, start, end)
}

fn function_signature(function: &FunctionDecl) -> String {
    let params = function
        .params
        .iter()
        .map(|param| format!("{}: {}", param.name, param.ty.text))
        .collect::<Vec<_>>()
        .join(", ");
    match &function.return_type {
        Some(ty) => format!("({params}): {}", ty.text),
        None => format!("({params})"),
    }
}

struct CatalogContainers {
    by_catalog_id: HashMap<String, String>,
}

impl CatalogContainers {
    fn new(snapshot: &AnalysisSnapshot) -> Self {
        let mut containers = Self {
            by_catalog_id: HashMap::new(),
        };
        let facts = &snapshot.program.facts;
        let modules = facts.modules();

        for declaration in snapshot.catalog_declarations() {
            if let Some(owner) = declaration_owner(facts, modules, declaration) {
                containers.insert_owner(declaration.catalog_id.as_str(), owner);
            }
        }

        containers
    }

    fn insert_owner(&mut self, catalog_id: &str, owner: String) {
        if !owner.is_empty() {
            self.by_catalog_id.insert(catalog_id.to_string(), owner);
        }
    }

    fn get(&self, declaration: &CatalogDeclaration) -> Option<&str> {
        self.by_catalog_id
            .get(&declaration.catalog_id)
            .map(String::as_str)
    }
}

fn declaration_owner(
    facts: &CheckedFacts,
    modules: &[ModuleFact],
    declaration: &CatalogDeclaration,
) -> Option<String> {
    match declaration.kind {
        CatalogEntryKind::Resource => facts
            .resources()
            .iter()
            .find(|resource| {
                resource.name == declaration.name
                    && resource.name_span == declaration.span
                    && fact_file(modules, resource.module) == Some(declaration.file.as_path())
            })
            .map(|resource| module_name(modules, resource.module).to_string()),
        CatalogEntryKind::Store => facts
            .stores()
            .iter()
            .find(|store| {
                store.root == declaration.name
                    && store.name_span == declaration.span
                    && fact_file(modules, store.module) == Some(declaration.file.as_path())
            })
            .map(|store| module_name(modules, store.module).to_string()),
        CatalogEntryKind::StoreIndex => facts.store_indexes().iter().find_map(|index| {
            let store = facts.store(index.store);
            (index.name == declaration.name
                && index.name_span == declaration.span
                && fact_file(modules, store.module) == Some(declaration.file.as_path()))
            .then(|| {
                let module = module_name(modules, store.module);
                join_owner(module, [format!("^{}", store.root)])
            })
        }),
        CatalogEntryKind::ResourceMember => facts.resource_members().iter().find_map(|member| {
            let resource = facts.resource(member.resource);
            (member.name == declaration.name
                && member.name_span == declaration.span
                && fact_file(modules, resource.module) == Some(declaration.file.as_path()))
            .then(|| {
                let module = module_name(modules, resource.module);
                join_owner(module, resource_member_owner_parts(facts, member))
            })
        }),
        CatalogEntryKind::Enum => facts
            .enums()
            .iter()
            .find(|enum_fact| {
                enum_fact.name == declaration.name
                    && enum_fact.name_span == declaration.span
                    && fact_file(modules, enum_fact.module) == Some(declaration.file.as_path())
            })
            .map(|enum_fact| module_name(modules, enum_fact.module).to_string()),
        CatalogEntryKind::EnumMember => facts.enum_members().iter().find_map(|member| {
            let enum_fact = facts.enum_(member.enum_id)?;
            (member.name == declaration.name
                && member.name_span == declaration.span
                && fact_file(modules, enum_fact.module) == Some(declaration.file.as_path()))
            .then(|| {
                let module = module_name(modules, enum_fact.module);
                join_owner(module, enum_member_owner_parts(facts, member))
            })
        }),
    }
}

fn module_name(modules: &[ModuleFact], id: ModuleId) -> &str {
    modules
        .get(id.0 as usize)
        .map_or("", |module| module.name.as_str())
}

fn fact_file(modules: &[ModuleFact], id: ModuleId) -> Option<&Path> {
    modules
        .get(id.0 as usize)
        .map(|module| module.source_file.as_path())
}

fn resource_member_owner_parts(facts: &CheckedFacts, member: &ResourceMemberFact) -> Vec<String> {
    let resource = facts.resource(member.resource);
    let mut parts = vec![resource.name.clone()];
    let mut parents = resource_member_parent_names(facts, member);
    parts.append(&mut parents);
    parts
}

fn resource_member_parent_names(facts: &CheckedFacts, member: &ResourceMemberFact) -> Vec<String> {
    let mut names = Vec::new();
    let mut parent = member.parent;
    while let Some(parent_id) = parent {
        let Some(parent_member) = facts.resource_members().get(parent_id.0 as usize) else {
            break;
        };
        names.push(parent_member.name.clone());
        parent = parent_member.parent;
    }
    names.reverse();
    names
}

fn enum_member_owner_parts(facts: &CheckedFacts, member: &EnumMemberFact) -> Vec<String> {
    let Some(enum_fact) = facts.enum_(member.enum_id) else {
        return Vec::new();
    };
    let mut parts = vec![enum_fact.name.clone()];
    let mut parents = enum_member_parent_names(facts, member);
    parts.append(&mut parents);
    parts
}

fn enum_member_parent_names(facts: &CheckedFacts, member: &EnumMemberFact) -> Vec<String> {
    let mut names = Vec::new();
    let mut parent = member.parent;
    while let Some(parent_id) = parent {
        let Some(parent_member) = facts.enum_member(parent_id) else {
            break;
        };
        names.push(parent_member.name.clone());
        parent = parent_member.parent;
    }
    names.reverse();
    names
}

fn join_owner(module: &str, parts: impl IntoIterator<Item = String>) -> String {
    let mut owner = String::new();
    if !module.is_empty() {
        owner.push_str(module);
    }
    for part in parts {
        if !owner.is_empty() {
            owner.push_str("::");
        }
        owner.push_str(&part);
    }
    owner
}

fn catalog_symbol_name(declaration: &CatalogDeclaration) -> String {
    match declaration.kind {
        CatalogEntryKind::Store => format!("^{}", declaration.name),
        _ => declaration.name.clone(),
    }
}

fn catalog_symbol_kind(kind: CatalogEntryKind) -> SourceSymbolKind {
    match kind {
        CatalogEntryKind::Resource => SourceSymbolKind::Resource,
        CatalogEntryKind::Store => SourceSymbolKind::Store,
        CatalogEntryKind::StoreIndex => SourceSymbolKind::StoreIndex,
        CatalogEntryKind::ResourceMember => SourceSymbolKind::ResourceMember,
        CatalogEntryKind::Enum => SourceSymbolKind::Enum,
        CatalogEntryKind::EnumMember => SourceSymbolKind::EnumMember,
    }
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}
