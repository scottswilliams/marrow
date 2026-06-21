use std::path::Path;

use marrow_syntax::{Declaration, EnumMember, ResourceMember, SourceFile, SourceSpan};

use crate::{AnalysisSnapshot, BindingIndex, SymbolKind, SymbolRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSymbolDocs {
    pub lines: Vec<String>,
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

fn span_contains_span(outer: SourceSpan, inner: SourceSpan) -> bool {
    outer.start_byte <= inner.start_byte && inner.end_byte <= outer.end_byte
}
