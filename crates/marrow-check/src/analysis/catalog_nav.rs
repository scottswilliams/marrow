use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marrow_catalog::CatalogEntryKind;
use marrow_syntax::{Declaration, SourceSpan, StoreDecl, TypeRef};

use super::AnalyzedFile;
use crate::annotation_refs::{
    TypeAnnotationBodies, type_ref_path_leaf_span, walk_declaration_type_refs,
};
use crate::build_alias_map;
use crate::enums::{
    EnumAnnotationResolution, ResolvedResourceAnnotation, resolve_enum_annotation,
    resolve_resource_annotation,
};
use crate::executable::{
    CheckedBodyVisitor, walk_checked_body, walk_checked_expr, walk_checked_match_arm,
};
use crate::facts::ModuleId;
use crate::source_spans::{last_identifier_span_in, source_span_at};
use crate::{
    CheckedBody, CheckedCallTarget, CheckedExpr, CheckedMatchArm, CheckedProgram,
    CheckedResourceConstructor, CheckedSavedMember, CheckedSavedPlace, CheckedSavedTerminal,
};

/// The prebuilt proposal identity map the declaration collectors resolve first-run
/// catalog ids against in O(1) per declaration.
type CatalogProposalIds = HashMap<crate::catalog::CatalogKey, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseSite {
    pub file: PathBuf,
    pub span: SourceSpan,
    pub catalog_id: String,
    pub kind: UseSiteKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseSiteKind {
    SavedRoot,
    Resource,
    ResourceConstructor,
    ResourceMember,
    StoreIndex,
    Enum,
    EnumMember,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDeclaration {
    pub file: PathBuf,
    pub span: SourceSpan,
    pub catalog_id: String,
    pub kind: CatalogEntryKind,
    pub name: String,
}

pub(super) fn collect_use_sites(program: &CheckedProgram, files: &[AnalyzedFile]) -> Vec<UseSite> {
    let mut sites = Vec::new();
    // Resolve first-run identity for every use site against one prebuilt proposal
    // map, so a program with many enum members stays linear here too.
    let ids = program.proposal_id_map();
    let file_set: HashSet<&Path> = files.iter().map(|file| file.path.as_path()).collect();
    let sources: HashMap<&Path, &str> = files
        .iter()
        .map(|file| (file.path.as_path(), file.source.as_str()))
        .collect();
    collect_module_type_annotation_use_sites(program, &ids, files, &mut sites);
    let runtime = program.runtime();
    for module in runtime.modules() {
        if !file_set.contains(module.source_file.as_path()) {
            continue;
        }
        let Some(source) = sources.get(module.source_file.as_path()).copied() else {
            continue;
        };
        for constant in &module.constants {
            if let Some(value) = &constant.value {
                collect_expr_use_sites(
                    program,
                    &ids,
                    &module.source_file,
                    source,
                    value,
                    &mut sites,
                );
            }
        }
        for function in module.functions() {
            if let Some(body) = function.body() {
                collect_body_use_sites(
                    program,
                    &ids,
                    &module.source_file,
                    source,
                    body,
                    &mut sites,
                );
            }
        }
    }
    for transform in &program.catalog.evolve_transforms {
        if !file_set.contains(transform.file.as_path()) {
            continue;
        }
        let Some(source) = sources.get(transform.file.as_path()).copied() else {
            continue;
        };
        if let Some(body) = transform.runtime_body() {
            collect_body_use_sites(program, &ids, &transform.file, source, body, &mut sites);
        }
    }
    normalize_use_sites(&mut sites);
    sites
}

pub(super) fn normalize_use_sites(sites: &mut Vec<UseSite>) {
    sort_use_sites(sites);
    sites.dedup();
}

fn collect_module_type_annotation_use_sites(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    files: &[AnalyzedFile],
    sites: &mut Vec<UseSite>,
) {
    for file in files {
        let Some(module) = program
            .modules
            .iter()
            .find(|module| module.source_file == file.path)
        else {
            continue;
        };
        let aliases = build_alias_map(&module.imports);
        for declaration in &file.parsed.file.declarations {
            if let Declaration::Store(store) = declaration {
                collect_store_resource_use_site(
                    program,
                    ids,
                    &file.path,
                    &file.source,
                    module.name.as_str(),
                    store,
                    sites,
                );
            }
            walk_declaration_type_refs(declaration, TypeAnnotationBodies::Include, &mut |ty| {
                collect_type_ref_use_site(
                    program,
                    ids,
                    &file.path,
                    &file.source,
                    &aliases,
                    ty,
                    sites,
                );
            });
        }
    }
}

fn collect_type_ref_use_site(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    file: &Path,
    source: &str,
    aliases: &HashMap<String, Vec<String>>,
    ty: &TypeRef,
    sites: &mut Vec<UseSite>,
) {
    if let Some(resolved) = resolve_resource_annotation(ty, program, aliases, file) {
        let Some(resource_id) = resolved_resource_id(program, &resolved) else {
            return;
        };
        if let Some(catalog_id) = program.resource_catalog_id_in(ids, resource_id)
            && let Some(span) = type_ref_path_leaf_span(source, ty, &resolved.name)
        {
            push_use_site(file, span, &catalog_id, UseSiteKind::Resource, sites);
        }
        return;
    }

    let EnumAnnotationResolution::Visible(resolved) =
        resolve_enum_annotation(ty, program, aliases, file)
    else {
        return;
    };
    let Some(enum_id) = enum_id_by_name(program, &resolved.module, &resolved.name) else {
        return;
    };
    let Some(catalog_id) = enum_catalog_id(program, ids, enum_id) else {
        return;
    };
    let Some(span) = type_ref_path_leaf_span(source, ty, &resolved.name) else {
        return;
    };
    push_use_site(file, span, &catalog_id, UseSiteKind::Enum, sites);
}

fn collect_store_resource_use_site(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    file: &Path,
    source: &str,
    module_name: &str,
    store: &StoreDecl,
    sites: &mut Vec<UseSite>,
) {
    let Some(module_id) = program.facts.module_id(module_name) else {
        return;
    };
    let Some(resource_id) = program.facts.resource_id(module_id, &store.resource) else {
        return;
    };
    let resource = program.facts.resource(resource_id);
    let Some(catalog_id) = program.resource_catalog_id_in(ids, resource_id) else {
        return;
    };
    let Some(span) = store_resource_span(source, store, &resource.name) else {
        return;
    };
    push_use_site(file, span, &catalog_id, UseSiteKind::Resource, sites);
}

fn store_resource_span(source: &str, store: &StoreDecl, resource_name: &str) -> Option<SourceSpan> {
    if store.root.span.end_byte < store.span.start_byte
        || store.root.span.end_byte > store.span.end_byte
    {
        return None;
    }

    let header = source.get(store.root.span.end_byte..store.span.end_byte)?;
    let mut depth = 0usize;
    let colon = header.bytes().enumerate().find_map(|(offset, byte)| {
        match byte {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => return Some(store.root.span.end_byte + offset),
            _ => {}
        }
        None
    })?;

    let mut start = colon + 1;
    while source
        .as_bytes()
        .get(start)
        .is_some_and(u8::is_ascii_whitespace)
    {
        start += 1;
    }

    let end = start.checked_add(resource_name.len())?;
    if source.get(start..end)? != resource_name {
        return None;
    }
    if source
        .as_bytes()
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return None;
    }
    Some(source_span_at(source, start, end))
}

fn resolved_resource_id(
    program: &CheckedProgram,
    resolved: &ResolvedResourceAnnotation,
) -> Option<crate::ResourceId> {
    let module_id = program.facts.module_id(&resolved.module)?;
    program.facts.resource_id(module_id, &resolved.name)
}

fn sort_use_sites(sites: &mut [UseSite]) {
    sites.sort_by(|left, right| {
        (
            left.catalog_id.as_str(),
            left.file.as_path(),
            use_site_kind_rank(left.kind),
            left.span.start_byte,
            left.span.end_byte,
        )
            .cmp(&(
                right.catalog_id.as_str(),
                right.file.as_path(),
                use_site_kind_rank(right.kind),
                right.span.start_byte,
                right.span.end_byte,
            ))
    });
}

fn use_site_kind_rank(kind: UseSiteKind) -> u8 {
    match kind {
        UseSiteKind::SavedRoot => 0,
        UseSiteKind::Resource => 1,
        UseSiteKind::ResourceConstructor => 2,
        UseSiteKind::ResourceMember => 3,
        UseSiteKind::StoreIndex => 4,
        UseSiteKind::Enum => 5,
        UseSiteKind::EnumMember => 6,
    }
}

fn collect_body_use_sites(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    file: &Path,
    source: &str,
    body: &CheckedBody,
    sites: &mut Vec<UseSite>,
) {
    let mut collector = UseSiteCollector {
        program,
        ids,
        file,
        source,
        sites,
    };
    walk_checked_body(&mut collector, body);
}

fn collect_expr_use_sites(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    file: &Path,
    source: &str,
    expr: &CheckedExpr,
    sites: &mut Vec<UseSite>,
) {
    let mut collector = UseSiteCollector {
        program,
        ids,
        file,
        source,
        sites,
    };
    collector.visit_expr(expr);
}

struct UseSiteCollector<'a, 's> {
    program: &'a CheckedProgram,
    ids: &'a CatalogProposalIds,
    file: &'a Path,
    source: &'a str,
    sites: &'s mut Vec<UseSite>,
}

impl UseSiteCollector<'_, '_> {
    fn record_expr(&mut self, expr: &CheckedExpr) {
        if let Some(place) = expr.saved_place() {
            collect_place_use_sites(self.program, self.ids, self.file, place, self.sites);
        }

        if let CheckedExpr::Call { callee, target, .. } = expr
            && let CheckedCallTarget::ResourceConstructor(resource) = target
        {
            collect_resource_constructor_use_site(
                self.program,
                self.ids,
                self.file,
                self.source,
                callee,
                resource,
                self.sites,
            );
        }

        if let CheckedExpr::Name { enum_member, .. } = expr
            && let Some(enum_member) = enum_member
        {
            if let Some(catalog_id) =
                enum_catalog_id(self.program, self.ids, enum_member.enum_ref.enum_id)
                && let Some(span) = enum_member.enum_span
            {
                push_use_site(self.file, span, &catalog_id, UseSiteKind::Enum, self.sites);
            }
            for (member_id, span) in &enum_member.member_uses {
                if let Some(catalog_id) = enum_member_catalog_id(self.program, self.ids, *member_id)
                {
                    push_use_site(
                        self.file,
                        *span,
                        &catalog_id,
                        UseSiteKind::EnumMember,
                        self.sites,
                    );
                }
            }
        }
    }

    fn record_match_arm(&mut self, arm: &CheckedMatchArm) {
        for (member_id, span) in &arm.member_uses {
            if let Some(catalog_id) = enum_member_catalog_id(self.program, self.ids, *member_id) {
                push_use_site(
                    self.file,
                    *span,
                    &catalog_id,
                    UseSiteKind::EnumMember,
                    self.sites,
                );
            }
        }
    }
}

fn collect_resource_constructor_use_site(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    file: &Path,
    source: &str,
    callee: &CheckedExpr,
    resource: &CheckedResourceConstructor,
    sites: &mut Vec<UseSite>,
) {
    let Some(catalog_id) = resource_ref_catalog_id(program, ids, resource) else {
        return;
    };
    let Some(span) = constructor_leaf_span(source, callee) else {
        return;
    };
    push_use_site(
        file,
        span,
        &catalog_id,
        UseSiteKind::ResourceConstructor,
        sites,
    );
}

fn resource_ref_catalog_id(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    resource: &CheckedResourceConstructor,
) -> Option<String> {
    let module_id = ModuleId(resource.resource.module);
    let resource_id = program.facts.resource_id(module_id, &resource.name)?;
    program.resource_catalog_id_in(ids, resource_id)
}

fn constructor_leaf_span(source: &str, callee: &CheckedExpr) -> Option<SourceSpan> {
    let CheckedExpr::Name { segments, span, .. } = callee else {
        return None;
    };
    let segment = segments.last()?;
    last_identifier_span_in(source, *span, segment)
}

impl CheckedBodyVisitor for UseSiteCollector<'_, '_> {
    fn visit_expr(&mut self, expression: &CheckedExpr) {
        self.record_expr(expression);
        walk_checked_expr(self, expression);
    }

    fn visit_match_arm(&mut self, arm: &CheckedMatchArm) {
        self.record_match_arm(arm);
        walk_checked_match_arm(self, arm);
    }
}

fn collect_place_use_sites(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    file: &Path,
    place: &CheckedSavedPlace,
    sites: &mut Vec<UseSite>,
) {
    if let Some(catalog_id) = place
        .store_catalog_id
        .clone()
        .or_else(|| program.store_catalog_id_in(ids, place.store_id))
    {
        push_use_site(
            file,
            place.root_span,
            &catalog_id,
            UseSiteKind::SavedRoot,
            sites,
        );
    }
    for layer in &place.layers {
        if let Some(catalog_id) = layer.catalog_id.clone().or_else(|| {
            layer
                .id
                .and_then(|id| program.resource_member_catalog_id_in(ids, id))
        }) {
            push_use_site(
                file,
                layer.name_span,
                &catalog_id,
                UseSiteKind::ResourceMember,
                sites,
            );
        }
    }
    match &place.terminal {
        CheckedSavedTerminal::Record => {}
        CheckedSavedTerminal::Field {
            name,
            span,
            catalog_id,
            ..
        } => {
            if let Some(catalog_id) = catalog_id.clone().or_else(|| {
                checked_member_by_name(&place.members, name)
                    .and_then(|member| member.id)
                    .and_then(|id| program.resource_member_catalog_id_in(ids, id))
            }) {
                push_use_site(file, *span, &catalog_id, UseSiteKind::ResourceMember, sites);
            }
        }
        CheckedSavedTerminal::Index {
            name,
            span,
            catalog_id,
            ..
        } => {
            if let Some(catalog_id) = catalog_id.clone().or_else(|| {
                place
                    .indexes
                    .iter()
                    .find(|index| index.name == *name)
                    .and_then(|index| program.store_index_catalog_id_in(ids, index.id))
            }) {
                push_use_site(file, *span, &catalog_id, UseSiteKind::StoreIndex, sites);
            }
        }
    }
}

pub(super) fn collect_catalog_declarations(program: &CheckedProgram) -> Vec<CatalogDeclaration> {
    // Resolve every declaration's first-run identity against one prebuilt proposal
    // map. A per-declaration proposal scan made one resource or enum quadratic in
    // its member count.
    let ids = program.proposal_id_map();
    let mut declarations = Vec::new();
    collect_store_declarations(program, &ids, &mut declarations);
    collect_resource_declarations(program, &ids, &mut declarations);
    collect_resource_member_declarations(program, &ids, &mut declarations);
    collect_store_index_declarations(program, &ids, &mut declarations);
    collect_enum_declarations(program, &ids, &mut declarations);
    collect_enum_member_declarations(program, &ids, &mut declarations);
    declarations
}

fn collect_store_declarations(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for store in program.facts.stores() {
        let Some(module) = modules.get(store.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = program.store_catalog_id_in(ids, store.id) else {
            continue;
        };
        let Some(span) = exact_span(store.name_span) else {
            continue;
        };
        declarations.push(CatalogDeclaration {
            file: module.source_file.clone(),
            span,
            catalog_id,
            kind: CatalogEntryKind::Store,
            name: store.root.clone(),
        });
    }
}

fn collect_resource_declarations(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for resource in program.facts.resources() {
        let Some(module) = modules.get(resource.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = program.resource_catalog_id_in(ids, resource.id) else {
            continue;
        };
        let Some(span) = exact_span(resource.name_span) else {
            continue;
        };
        declarations.push(CatalogDeclaration {
            file: module.source_file.clone(),
            span,
            catalog_id,
            kind: CatalogEntryKind::Resource,
            name: resource.name.clone(),
        });
    }
}

fn collect_resource_member_declarations(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for member in program.facts.resource_members() {
        let resource = program.facts.resource(member.resource);
        let Some(module) = modules.get(resource.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = program.resource_member_catalog_id_in(ids, member.id) else {
            continue;
        };
        let Some(span) = exact_span(member.name_span) else {
            continue;
        };
        declarations.push(CatalogDeclaration {
            file: module.source_file.clone(),
            span,
            catalog_id,
            kind: CatalogEntryKind::ResourceMember,
            name: member.name.clone(),
        });
    }
}

fn collect_store_index_declarations(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for index in program.facts.store_indexes() {
        let store = program.facts.store(index.store);
        let Some(module) = modules.get(store.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = program.store_index_catalog_id_in(ids, index.id) else {
            continue;
        };
        let Some(span) = exact_span(index.name_span) else {
            continue;
        };
        declarations.push(CatalogDeclaration {
            file: module.source_file.clone(),
            span,
            catalog_id,
            kind: CatalogEntryKind::StoreIndex,
            name: index.name.clone(),
        });
    }
}

fn enum_id_by_name(
    program: &CheckedProgram,
    module_name: &str,
    enum_name: &str,
) -> Option<crate::EnumId> {
    let module = program
        .facts
        .modules()
        .iter()
        .find(|module| module.name == module_name)?;
    program.facts.enum_id(module.id, enum_name)
}

fn collect_enum_declarations(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for enum_fact in program.facts.enums() {
        let Some(module) = modules.get(enum_fact.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = enum_catalog_id(program, ids, enum_fact.id) else {
            continue;
        };
        let Some(span) = exact_span(enum_fact.name_span) else {
            continue;
        };
        declarations.push(CatalogDeclaration {
            file: module.source_file.clone(),
            span,
            catalog_id,
            kind: CatalogEntryKind::Enum,
            name: enum_fact.name.clone(),
        });
    }
}

fn collect_enum_member_declarations(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for member in program.facts.enum_members() {
        let Some(enum_fact) = program.facts.enum_(member.enum_id) else {
            continue;
        };
        let Some(module) = modules.get(enum_fact.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = enum_member_catalog_id(program, ids, member.id) else {
            continue;
        };
        let Some(span) = exact_span(member.name_span) else {
            continue;
        };
        declarations.push(CatalogDeclaration {
            file: module.source_file.clone(),
            span,
            catalog_id,
            kind: CatalogEntryKind::EnumMember,
            name: member.name.clone(),
        });
    }
}

fn exact_span(span: SourceSpan) -> Option<SourceSpan> {
    (span.end_byte > span.start_byte).then_some(span)
}

fn checked_member_by_name<'a>(
    members: &'a [CheckedSavedMember],
    name: &str,
) -> Option<&'a CheckedSavedMember> {
    members.iter().find(|member| member.name == name)
}

fn enum_catalog_id(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    id: crate::EnumId,
) -> Option<String> {
    let enum_fact = program.facts.enum_(id)?;
    if let Some(catalog_id) = enum_fact.catalog_id.as_deref() {
        return Some(catalog_id.to_string());
    }
    let module = program.facts.modules().get(enum_fact.module.0 as usize)?;
    let path = crate::catalog::enum_path(&module.name, &enum_fact.name);
    crate::catalog::proposal_id(ids, CatalogEntryKind::Enum, path)
}

fn enum_member_catalog_id(
    program: &CheckedProgram,
    ids: &CatalogProposalIds,
    id: crate::EnumMemberId,
) -> Option<String> {
    let member = program.facts.enum_member(id)?;
    if let Some(catalog_id) = member.catalog_id.as_deref() {
        return Some(catalog_id.to_string());
    }
    let path = program.facts.enum_member_catalog_path(id)?;
    crate::catalog::proposal_id(ids, CatalogEntryKind::EnumMember, path)
}

fn push_use_site(
    file: &Path,
    span: SourceSpan,
    catalog_id: &str,
    kind: UseSiteKind,
    sites: &mut Vec<UseSite>,
) {
    sites.push(UseSite {
        file: file.to_path_buf(),
        span,
        catalog_id: catalog_id.to_string(),
        kind,
    });
}
