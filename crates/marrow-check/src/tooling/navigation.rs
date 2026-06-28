use std::path::{Path, PathBuf};

use marrow_syntax::SourceSpan;

use crate::{AnalysisSnapshot, CatalogDeclaration, CatalogEntryKind, UseSite, UseSiteKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCatalogLocationFact {
    pub file: PathBuf,
    pub span: SourceSpan,
}

pub fn source_catalog_definition_fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<SourceCatalogLocationFact> {
    let target = catalog_target_at(snapshot, file, offset)?;
    let declaration = target_declaration(snapshot.catalog_declaration(target.catalog_id)?)?;
    Some(location_fact(&declaration.file, declaration.span))
}

pub fn source_catalog_reference_facts_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
    include_declaration: bool,
) -> Option<Vec<SourceCatalogLocationFact>> {
    let target = catalog_target_at(snapshot, file, offset)?;
    let declaration = target_declaration(snapshot.catalog_declaration(target.catalog_id)?)?;

    let mut locations = Vec::new();
    if include_declaration {
        push_unique_location(
            &mut locations,
            location_fact(&declaration.file, declaration.span),
        );
    }
    for location in snapshot
        .use_sites()
        .iter()
        .filter(|site| site.catalog_id == target.catalog_id)
        .filter(|site| supported_reference_site(snapshot, &target, site))
        .map(|site| location_fact(&site.file, site.span))
    {
        push_unique_location(&mut locations, location);
    }
    Some(locations)
}

fn catalog_target_at<'a>(
    snapshot: &'a AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<CatalogTarget<'a>> {
    use_site_at(snapshot, file, offset).or_else(|| declaration_at(snapshot, file, offset))
}

fn use_site_at<'a>(
    snapshot: &'a AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<CatalogTarget<'a>> {
    snapshot
        .use_sites()
        .iter()
        .filter_map(supported_use_site)
        .filter(|site| site.file == file && span_covers(site.span, offset))
        .map(|site| CatalogTarget {
            file: site.file.as_path(),
            catalog_id: site.catalog_id.as_str(),
            span: site.span,
            kind: CatalogTargetKind::Use(site.kind),
        })
        .min_by_key(|target| span_len(target.span))
}

fn declaration_at<'a>(
    snapshot: &'a AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<CatalogTarget<'a>> {
    snapshot
        .catalog_declarations()
        .iter()
        .filter_map(cursor_declaration)
        .filter(|declaration| declaration.file == file && span_covers(declaration.span, offset))
        .map(|declaration| CatalogTarget {
            file: declaration.file.as_path(),
            catalog_id: declaration.catalog_id.as_str(),
            span: declaration.span,
            kind: CatalogTargetKind::Declaration,
        })
        .min_by_key(|target| span_len(target.span))
}

fn supported_use_site(site: &UseSite) -> Option<&UseSite> {
    matches!(
        site.kind,
        UseSiteKind::SavedRoot
            | UseSiteKind::ResourceConstructor
            | UseSiteKind::ResourceMember
            | UseSiteKind::StoreIndex
            | UseSiteKind::Enum
            | UseSiteKind::EnumMember
    )
    .then_some(site)
}

fn target_declaration(declaration: &CatalogDeclaration) -> Option<&CatalogDeclaration> {
    matches!(
        declaration.kind,
        CatalogEntryKind::Store
            | CatalogEntryKind::Resource
            | CatalogEntryKind::ResourceMember
            | CatalogEntryKind::StoreIndex
            | CatalogEntryKind::Enum
            | CatalogEntryKind::EnumMember
    )
    .then_some(declaration)
}

fn cursor_declaration(declaration: &CatalogDeclaration) -> Option<&CatalogDeclaration> {
    matches!(
        declaration.kind,
        CatalogEntryKind::Store
            | CatalogEntryKind::ResourceMember
            | CatalogEntryKind::StoreIndex
            | CatalogEntryKind::Enum
            | CatalogEntryKind::EnumMember
    )
    .then_some(declaration)
}

fn supported_reference_site(
    snapshot: &AnalysisSnapshot,
    target: &CatalogTarget<'_>,
    site: &UseSite,
) -> bool {
    if supported_use_site(site).is_none() {
        return false;
    }
    if target.is_enum_type_annotation(snapshot) {
        site.kind == UseSiteKind::Enum && enum_type_annotation_site(snapshot, site)
    } else {
        true
    }
}

fn enum_type_annotation_site(snapshot: &AnalysisSnapshot, site: &UseSite) -> bool {
    span_is_type_annotation(snapshot, &site.file, site.span)
}

fn location_fact(file: &Path, span: SourceSpan) -> SourceCatalogLocationFact {
    SourceCatalogLocationFact {
        file: file.to_path_buf(),
        span,
    }
}

fn span_len(span: SourceSpan) -> usize {
    span.end_byte.saturating_sub(span.start_byte)
}

fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

fn push_unique_location(
    locations: &mut Vec<SourceCatalogLocationFact>,
    location: SourceCatalogLocationFact,
) {
    if !locations.iter().any(|existing| existing == &location) {
        locations.push(location);
    }
}

struct CatalogTarget<'a> {
    file: &'a Path,
    catalog_id: &'a str,
    span: SourceSpan,
    kind: CatalogTargetKind,
}

impl CatalogTarget<'_> {
    fn is_enum_type_annotation(&self, snapshot: &AnalysisSnapshot) -> bool {
        matches!(self.kind, CatalogTargetKind::Use(UseSiteKind::Enum))
            && span_is_type_annotation(snapshot, self.file, self.span)
    }
}

fn span_is_type_annotation(snapshot: &AnalysisSnapshot, file: &Path, span: SourceSpan) -> bool {
    crate::tooling::source_type_annotation_cursor_fact_at(snapshot, file, span.start_byte).is_some()
}

#[derive(Clone, Copy)]
enum CatalogTargetKind {
    Use(UseSiteKind),
    Declaration,
}
