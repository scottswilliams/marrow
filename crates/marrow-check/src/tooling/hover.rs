use std::path::Path;

use marrow_schema::{IndexSchema, Node, NodeKind, ResourceSchema, StoreSchema};
use marrow_syntax::{
    Declaration, EnumMember, IndexDecl, KeyParam, ResourceMember, SourceFile, SourceSpan,
    StoreDecl, TokenKind, lex_source,
};

use crate::{
    AnalysisSnapshot, BindingIndex, ModuleId, ResourceMemberKind, StoreFact, SymbolKind,
    SymbolOccurrence, SymbolRef,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSymbolDocs {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedPlaceHoverFact {
    Field {
        name: String,
        key_params: Vec<SavedPlaceHoverKeyParam>,
        ty: String,
        required: bool,
        docs: Vec<String>,
    },
    Layer {
        name: String,
        key_params: Vec<SavedPlaceHoverKeyParam>,
        docs: Vec<String>,
    },
    Index {
        name: String,
        args: Vec<String>,
        unique: bool,
        docs: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPlaceHoverKeyParam {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRootHoverFact {
    pub root: String,
    pub identity_keys: Vec<SavedPlaceHoverKeyParam>,
    pub resource: String,
    pub store_docs: Vec<String>,
    pub members: Vec<StoreRootHoverMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRootHoverPathSegment {
    pub name: String,
    pub key_params: Vec<SavedPlaceHoverKeyParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreRootHoverMember {
    Field {
        path: Vec<StoreRootHoverPathSegment>,
        required: bool,
        ty: String,
    },
    Layer {
        path: Vec<StoreRootHoverPathSegment>,
    },
    Index {
        name: String,
        args: Vec<String>,
        unique: bool,
    },
}

pub fn source_symbol_docs_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceSymbolDocs> {
    let symbol = index.definition(file, offset)?;
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == symbol.file)?;
    let lines = declaration_docs(&analyzed.parsed.file, &symbol)?;
    (!lines.is_empty()).then(|| SourceSymbolDocs {
        lines: lines.to_vec(),
    })
}

pub fn store_root_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<StoreRootHoverFact> {
    let analyzed = snapshot.files.iter().find(|f| f.path == file)?;
    let declaration = store_root_declaration_at(&analyzed.parsed.file, &analyzed.source, offset)?;
    let store = snapshot.program.facts.stores().iter().find(|store| {
        store.root == declaration.root.root
            && store.name_span == declaration.root.span
            && fact_file(snapshot, store.module) == Some(file)
    })?;
    store_root_hover_fact(snapshot, store)
}

pub fn saved_place_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SavedPlaceHoverFact> {
    let occurrence = index.occurrence(file, offset)?;
    let symbol = &occurrence.definition;
    if !matches!(
        symbol.kind,
        SymbolKind::Field | SymbolKind::Layer | SymbolKind::Index
    ) || !is_saved_place_hover_target(snapshot, file, offset, &occurrence)
    {
        return None;
    }

    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == symbol.file)?;
    match symbol.kind {
        SymbolKind::Field | SymbolKind::Layer => member_hover_fact(snapshot, analyzed, symbol),
        SymbolKind::Index => index_hover_fact(snapshot, analyzed, symbol),
        _ => None,
    }
}

fn store_root_declaration_at<'a>(
    source: &'a SourceFile,
    text: &str,
    offset: usize,
) -> Option<&'a StoreDecl> {
    source
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Store(store) => {
                let (start, end) = root_token_span(text, store)?;
                (start <= offset && offset < end).then_some(store)
            }
            _ => None,
        })
}

fn root_token_span(source: &str, store: &StoreDecl) -> Option<(usize, usize)> {
    lex_source(source).tokens.windows(2).find_map(|tokens| {
        let [caret, name] = tokens else {
            return None;
        };
        if caret.kind == TokenKind::Caret
            && name.kind == TokenKind::Identifier
            && span_covers(store.span, caret.span.start_byte)
            && span_covers(store.span, name.span.end_byte)
            && name.text(source) == store.root.root
        {
            Some((caret.span.start_byte, name.span.end_byte))
        } else {
            None
        }
    })
}

fn store_root_hover_fact(
    snapshot: &AnalysisSnapshot,
    store: &StoreFact,
) -> Option<StoreRootHoverFact> {
    let (schema, resource) = store_root_schemas(snapshot, store)?;
    Some(StoreRootHoverFact {
        root: schema.root.clone(),
        identity_keys: schema.identity_keys.iter().map(schema_key_param).collect(),
        resource: schema.resource.clone(),
        store_docs: schema.docs.clone(),
        members: store_root_members(resource, &schema.indexes),
    })
}

fn store_root_schemas<'a>(
    snapshot: &'a AnalysisSnapshot,
    store: &StoreFact,
) -> Option<(&'a StoreSchema, &'a ResourceSchema)> {
    let module = snapshot.program.modules.get(store.module.0 as usize)?;
    let schema = module
        .stores
        .iter()
        .find(|schema| schema.root == store.root)?;
    let resource = module
        .resources
        .iter()
        .find(|resource| resource.name == schema.resource)?;
    Some((schema, resource))
}

fn store_root_members(
    resource: &ResourceSchema,
    indexes: &[IndexSchema],
) -> Vec<StoreRootHoverMember> {
    let mut members = Vec::new();
    for member in &resource.members {
        resource_hover_members(member, &mut Vec::new(), &mut members);
    }
    members.extend(indexes.iter().map(|index| StoreRootHoverMember::Index {
        name: index.name.clone(),
        args: index.args.clone(),
        unique: index.unique,
    }));
    members
}

fn resource_hover_members(
    member: &Node,
    path: &mut Vec<StoreRootHoverPathSegment>,
    members: &mut Vec<StoreRootHoverMember>,
) {
    path.push(StoreRootHoverPathSegment {
        name: member.name.clone(),
        key_params: member.key_params.iter().map(schema_key_param).collect(),
    });
    match &member.kind {
        NodeKind::Slot { ty, required, .. } => {
            members.push(StoreRootHoverMember::Field {
                path: path.clone(),
                required: *required,
                ty: render_schema_leaf_type(member, ty),
            });
        }
        NodeKind::Group => {
            members.push(StoreRootHoverMember::Layer { path: path.clone() });
            for child in &member.members {
                resource_hover_members(child, path, members);
            }
        }
    }
    path.pop();
}

fn schema_key_param(key: &marrow_schema::KeyDef) -> SavedPlaceHoverKeyParam {
    SavedPlaceHoverKeyParam {
        name: key.name.clone(),
        ty: key.ty.to_string(),
    }
}

fn render_schema_leaf_type(member: &Node, ty: &marrow_schema::Type) -> String {
    if member.is_error_code() {
        "ErrorCode".to_string()
    } else {
        ty.to_string()
    }
}

fn is_saved_place_hover_target(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
    occurrence: &SymbolOccurrence,
) -> bool {
    let symbol = &occurrence.definition;
    is_saved_place_declaration_name(file, offset, symbol)
        || is_saved_place_reference_leaf(snapshot, file, offset, occurrence)
}

fn is_saved_place_reference_leaf(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
    occurrence: &SymbolOccurrence,
) -> bool {
    let symbol = &occurrence.definition;
    let reference = &occurrence.reference;
    reference.kind == symbol.kind
        && reference.file == file
        && (reference.file != symbol.file || reference.span != symbol.span)
        && span_covers(reference.span, offset)
        && offset_is_on_last_identifier(snapshot, file, reference.span, offset)
}

fn is_saved_place_declaration_name(file: &Path, offset: usize, symbol: &SymbolRef) -> bool {
    symbol.file == file && span_covers(symbol.span, offset)
}

fn member_hover_fact(
    snapshot: &AnalysisSnapshot,
    analyzed: &crate::AnalyzedFile,
    symbol: &SymbolRef,
) -> Option<SavedPlaceHoverFact> {
    let member_fact = snapshot
        .program
        .facts
        .resource_members()
        .iter()
        .find(|member| {
            let resource = snapshot.program.facts.resource(member.resource);
            member.name_span == symbol.span
                && fact_file(snapshot, resource.module) == Some(symbol.file.as_path())
                && matches!(
                    (member.kind, symbol.kind),
                    (ResourceMemberKind::Field, SymbolKind::Field)
                        | (ResourceMemberKind::Group, SymbolKind::Layer)
                )
        })?;
    let member = resource_member_at(&analyzed.parsed.file, member_fact.span)?;
    match member {
        ResourceMember::Field(field) => Some(SavedPlaceHoverFact::Field {
            name: field.name.clone(),
            key_params: hover_key_params(&field.keys),
            ty: field.ty.text.clone(),
            required: field.required,
            docs: field.docs.clone(),
        }),
        ResourceMember::Group(group) => Some(SavedPlaceHoverFact::Layer {
            name: group.name.clone(),
            key_params: hover_key_params(&group.keys),
            docs: group.docs.clone(),
        }),
    }
}

fn index_hover_fact(
    snapshot: &AnalysisSnapshot,
    analyzed: &crate::AnalyzedFile,
    symbol: &SymbolRef,
) -> Option<SavedPlaceHoverFact> {
    let index_fact = snapshot
        .program
        .facts
        .store_indexes()
        .iter()
        .find(|index| {
            let store = snapshot.program.facts.store(index.store);
            index.name_span == symbol.span
                && fact_file(snapshot, store.module) == Some(symbol.file.as_path())
        })?;
    let index = store_index_at(&analyzed.parsed.file, index_fact.span)?;
    Some(SavedPlaceHoverFact::Index {
        name: index.name.clone(),
        args: index.args.clone(),
        unique: index.unique,
        docs: index.docs.clone(),
    })
}

fn fact_file(snapshot: &AnalysisSnapshot, module: ModuleId) -> Option<&Path> {
    snapshot
        .program
        .facts
        .modules()
        .get(module.0 as usize)
        .map(|module| module.source_file.as_path())
}

fn hover_key_params(keys: &[KeyParam]) -> Vec<SavedPlaceHoverKeyParam> {
    keys.iter()
        .map(|key| SavedPlaceHoverKeyParam {
            name: key.name.clone(),
            ty: key.ty.text.clone(),
        })
        .collect()
}

fn declaration_docs<'a>(source: &'a SourceFile, symbol: &SymbolRef) -> Option<&'a [String]> {
    match symbol.kind {
        SymbolKind::Function => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Function(function)
                        if span_contains_span(function.span, symbol.span) =>
                    {
                        Some(function.docs.as_slice())
                    }
                    _ => None,
                })
        }
        SymbolKind::ModuleConst => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Const(constant)
                        if span_contains_span(constant.span, symbol.span) =>
                    {
                        Some(constant.docs.as_slice())
                    }
                    _ => None,
                })
        }
        SymbolKind::Resource => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Resource(resource)
                        if span_contains_span(resource.span, symbol.span) =>
                    {
                        Some(resource.docs.as_slice())
                    }
                    _ => None,
                })
        }
        SymbolKind::Enum => source
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Enum(enum_decl) if span_contains_span(enum_decl.span, symbol.span) => {
                    Some(enum_decl.docs.as_slice())
                }
                _ => None,
            }),
        SymbolKind::EnumMember => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Enum(enum_decl) => {
                        enum_member_docs(&enum_decl.members, symbol.span)
                    }
                    _ => None,
                })
        }
        SymbolKind::Field | SymbolKind::Layer => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Resource(resource) => member_docs(&resource.members, symbol.span),
                    _ => None,
                })
        }
        SymbolKind::Index => source
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Store(store) => store
                    .indexes
                    .iter()
                    .find(|index| span_contains_span(index.span, symbol.span))
                    .map(|index| index.docs.as_slice()),
                _ => None,
            }),
        SymbolKind::Local | SymbolKind::Param | SymbolKind::ModuleRef => None,
    }
}

fn enum_member_docs(members: &[EnumMember], span: SourceSpan) -> Option<&[String]> {
    for member in members {
        if span_contains_span(member.span, span) {
            return Some(&member.docs);
        }
        if let Some(docs) = enum_member_docs(&member.members, span) {
            return Some(docs);
        }
    }
    None
}

fn member_docs(members: &[ResourceMember], span: SourceSpan) -> Option<&[String]> {
    for member in members {
        match member {
            ResourceMember::Field(field) if span_contains_span(field.span, span) => {
                return Some(&field.docs);
            }
            ResourceMember::Group(group) if span_contains_span(group.span, span) => {
                return Some(&group.docs);
            }
            ResourceMember::Group(group) => {
                if let Some(docs) = member_docs(&group.members, span) {
                    return Some(docs);
                }
            }
            _ => {}
        }
    }
    None
}

fn resource_member_at(source: &SourceFile, span: SourceSpan) -> Option<&ResourceMember> {
    for declaration in &source.declarations {
        let Declaration::Resource(resource) = declaration else {
            continue;
        };
        if let Some(member) = resource_member_in(&resource.members, span) {
            return Some(member);
        }
    }
    None
}

fn resource_member_in(members: &[ResourceMember], span: SourceSpan) -> Option<&ResourceMember> {
    for member in members {
        match member {
            ResourceMember::Field(field) if span_contains_span(field.span, span) => {
                return Some(member);
            }
            ResourceMember::Group(group) if span_contains_span(group.span, span) => {
                return Some(member);
            }
            ResourceMember::Group(group) => {
                if let Some(member) = resource_member_in(&group.members, span) {
                    return Some(member);
                }
            }
            _ => {}
        }
    }
    None
}

fn store_index_at(source: &SourceFile, span: SourceSpan) -> Option<&IndexDecl> {
    source
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Store(store) => store
                .indexes
                .iter()
                .find(|index| span_contains_span(index.span, span)),
            _ => None,
        })
}

fn offset_is_on_last_identifier(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    span: SourceSpan,
    offset: usize,
) -> bool {
    let Some(analyzed) = snapshot.files.iter().find(|f| f.path == file) else {
        return false;
    };
    let Some((start, end)) = last_identifier_span(span, &analyzed.source) else {
        return false;
    };
    start <= offset && offset <= end
}

fn last_identifier_span(span: SourceSpan, source: &str) -> Option<(usize, usize)> {
    let lexed = lex_source(source);
    let mut found = None;
    for token in lexed.tokens {
        if token.kind == TokenKind::Identifier
            && span_covers(span, token.span.start_byte)
            && span_covers(span, token.span.end_byte)
        {
            found = Some((token.span.start_byte, token.span.end_byte));
        }
    }
    found
}

fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

fn span_contains_span(outer: SourceSpan, inner: SourceSpan) -> bool {
    outer.start_byte <= inner.start_byte && inner.end_byte <= outer.end_byte
}
