use std::path::{Path, PathBuf};

use marrow_catalog::CatalogEntryKind;
use marrow_syntax::SourceSpan;

use crate::executable::{
    CheckedBodyVisitor, walk_checked_body, walk_checked_expr, walk_checked_match_arm,
};
use crate::{
    CheckedBody, CheckedExpr, CheckedMatchArm, CheckedProgram, CheckedSavedMember,
    CheckedSavedPlace, CheckedSavedTerminal,
};

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

pub(super) fn collect_use_sites(program: &CheckedProgram) -> Vec<UseSite> {
    let mut sites = Vec::new();
    let runtime = program.runtime();
    for module in runtime.modules() {
        for constant in &module.constants {
            if let Some(value) = &constant.value {
                collect_expr_use_sites(program, &module.source_file, value, &mut sites);
            }
        }
        for function in module.functions() {
            if let Some(body) = function.body() {
                collect_body_use_sites(program, &module.source_file, body, &mut sites);
            }
        }
    }
    for transform in &program.catalog.evolve_transforms {
        if let Some(body) = transform.runtime_body() {
            collect_body_use_sites(program, &transform.file, body, &mut sites);
        }
    }
    sort_use_sites(&mut sites);
    sites.dedup();
    sites
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
        UseSiteKind::ResourceMember => 1,
        UseSiteKind::StoreIndex => 2,
        UseSiteKind::Enum => 3,
        UseSiteKind::EnumMember => 4,
    }
}

fn collect_body_use_sites(
    program: &CheckedProgram,
    file: &Path,
    body: &CheckedBody,
    sites: &mut Vec<UseSite>,
) {
    let mut collector = UseSiteCollector {
        program,
        file,
        sites,
    };
    walk_checked_body(&mut collector, body);
}

fn collect_expr_use_sites(
    program: &CheckedProgram,
    file: &Path,
    expr: &CheckedExpr,
    sites: &mut Vec<UseSite>,
) {
    let mut collector = UseSiteCollector {
        program,
        file,
        sites,
    };
    collector.visit_expr(expr);
}

struct UseSiteCollector<'a, 's> {
    program: &'a CheckedProgram,
    file: &'a Path,
    sites: &'s mut Vec<UseSite>,
}

impl UseSiteCollector<'_, '_> {
    fn record_expr(&mut self, expr: &CheckedExpr) {
        if let Some(place) = expr.saved_place() {
            collect_place_use_sites(self.program, self.file, place, self.sites);
        }

        if let CheckedExpr::Name { enum_member, .. } = expr
            && let Some(enum_member) = enum_member
        {
            if let Some(catalog_id) = enum_catalog_id(self.program, enum_member.enum_ref.enum_id)
                && let Some(span) = enum_member.enum_span
            {
                push_use_site(self.file, span, &catalog_id, UseSiteKind::Enum, self.sites);
            }
            for (member_id, span) in &enum_member.member_uses {
                if let Some(catalog_id) = enum_member_catalog_id(self.program, *member_id) {
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
            if let Some(catalog_id) = enum_member_catalog_id(self.program, *member_id) {
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
    file: &Path,
    place: &CheckedSavedPlace,
    sites: &mut Vec<UseSite>,
) {
    if let Some(catalog_id) = place
        .store_catalog_id
        .as_deref()
        .or_else(|| program.store_catalog_id(place.store_id))
    {
        push_use_site(
            file,
            place.root_span,
            catalog_id,
            UseSiteKind::SavedRoot,
            sites,
        );
    }
    for layer in &place.layers {
        if let Some(catalog_id) = layer.catalog_id.as_deref().or_else(|| {
            layer
                .id
                .and_then(|id| resource_member_catalog_id(program, id))
        }) {
            push_use_site(
                file,
                layer.name_span,
                catalog_id,
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
            if let Some(catalog_id) = catalog_id.as_deref().or_else(|| {
                checked_member_by_name(&place.members, name)
                    .and_then(|member| member.id)
                    .and_then(|id| resource_member_catalog_id(program, id))
            }) {
                push_use_site(file, *span, catalog_id, UseSiteKind::ResourceMember, sites);
            }
        }
        CheckedSavedTerminal::Index {
            name,
            span,
            catalog_id,
            ..
        } => {
            if let Some(catalog_id) = catalog_id.as_deref().or_else(|| {
                place
                    .indexes
                    .iter()
                    .find(|index| index.name == *name)
                    .and_then(|index| program.store_index_catalog_id(index.id))
            }) {
                push_use_site(file, *span, catalog_id, UseSiteKind::StoreIndex, sites);
            }
        }
    }
}

pub(super) fn collect_catalog_declarations(program: &CheckedProgram) -> Vec<CatalogDeclaration> {
    let mut declarations = Vec::new();
    collect_store_declarations(program, &mut declarations);
    collect_resource_declarations(program, &mut declarations);
    collect_resource_member_declarations(program, &mut declarations);
    collect_store_index_declarations(program, &mut declarations);
    collect_enum_declarations(program, &mut declarations);
    collect_enum_member_declarations(program, &mut declarations);
    declarations
}

fn collect_store_declarations(
    program: &CheckedProgram,
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for store in program.facts.stores() {
        let Some(module) = modules.get(store.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = program.store_catalog_id(store.id) else {
            continue;
        };
        let Some(span) = exact_span(store.name_span) else {
            continue;
        };
        declarations.push(CatalogDeclaration {
            file: module.source_file.clone(),
            span,
            catalog_id: catalog_id.to_string(),
            kind: CatalogEntryKind::Store,
            name: store.root.clone(),
        });
    }
}

fn collect_resource_declarations(
    program: &CheckedProgram,
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for resource in program.facts.resources() {
        let Some(module) = modules.get(resource.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = resource_catalog_id(program, resource.id) else {
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
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for member in program.facts.resource_members() {
        let resource = program.facts.resource(member.resource);
        let Some(module) = modules.get(resource.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = resource_member_catalog_id(program, member.id) else {
            continue;
        };
        let Some(span) = exact_span(member.name_span) else {
            continue;
        };
        declarations.push(CatalogDeclaration {
            file: module.source_file.clone(),
            span,
            catalog_id: catalog_id.to_string(),
            kind: CatalogEntryKind::ResourceMember,
            name: member.name.clone(),
        });
    }
}

fn collect_store_index_declarations(
    program: &CheckedProgram,
    declarations: &mut Vec<CatalogDeclaration>,
) {
    let modules = program.facts.modules();
    for index in program.facts.store_indexes() {
        let store = program.facts.store(index.store);
        let Some(module) = modules.get(store.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = program.store_index_catalog_id(index.id) else {
            continue;
        };
        let Some(span) = exact_span(index.name_span) else {
            continue;
        };
        declarations.push(CatalogDeclaration {
            file: module.source_file.clone(),
            span,
            catalog_id: catalog_id.to_string(),
            kind: CatalogEntryKind::StoreIndex,
            name: index.name.clone(),
        });
    }
}

fn collect_enum_declarations(program: &CheckedProgram, declarations: &mut Vec<CatalogDeclaration>) {
    let modules = program.facts.modules();
    for enum_fact in program.facts.enums() {
        let Some(module) = modules.get(enum_fact.module.0 as usize) else {
            continue;
        };
        let Some(catalog_id) = enum_catalog_id(program, enum_fact.id) else {
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
        let Some(catalog_id) = enum_member_catalog_id(program, member.id) else {
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

fn resource_catalog_id(program: &CheckedProgram, id: crate::ResourceId) -> Option<String> {
    let resource = program.facts.resource(id);
    if let Some(catalog_id) = resource.catalog_id.as_deref() {
        return Some(catalog_id.to_string());
    }
    let module = program.facts.modules().get(resource.module.0 as usize)?;
    let path = crate::catalog::resource_path(&module.name, &resource.name);
    proposal_catalog_id(program, CatalogEntryKind::Resource, &path).map(ToString::to_string)
}

fn resource_member_catalog_id(
    program: &CheckedProgram,
    id: crate::ResourceMemberId,
) -> Option<&str> {
    let member = program.facts.resource_members().get(id.0 as usize)?;
    if let Some(catalog_id) = member.catalog_id.as_deref() {
        return Some(catalog_id);
    }
    let resource = program.facts.resource(member.resource);
    let module = program.facts.modules().get(resource.module.0 as usize)?;
    let path = crate::catalog::resource_member_path(
        &module.name,
        &resource.name,
        &resource_member_path_names(program, id)?,
    );
    proposal_catalog_id(program, CatalogEntryKind::ResourceMember, &path)
}

fn enum_catalog_id(program: &CheckedProgram, id: crate::EnumId) -> Option<String> {
    let enum_fact = program.facts.enum_(id)?;
    if let Some(catalog_id) = enum_fact.catalog_id.as_deref() {
        return Some(catalog_id.to_string());
    }
    let module = program.facts.modules().get(enum_fact.module.0 as usize)?;
    let path = crate::catalog::enum_path(&module.name, &enum_fact.name);
    proposal_catalog_id(program, CatalogEntryKind::Enum, &path).map(ToString::to_string)
}

fn enum_member_catalog_id(program: &CheckedProgram, id: crate::EnumMemberId) -> Option<String> {
    let member = program.facts.enum_member(id)?;
    if let Some(catalog_id) = member.catalog_id.as_deref() {
        return Some(catalog_id.to_string());
    }
    let path = program.facts.enum_member_catalog_path(id)?;
    proposal_catalog_id(program, CatalogEntryKind::EnumMember, &path).map(ToString::to_string)
}

fn resource_member_path_names(
    program: &CheckedProgram,
    id: crate::ResourceMemberId,
) -> Option<Vec<String>> {
    let mut names = Vec::new();
    let mut current = Some(id);
    while let Some(id) = current {
        let member = program.facts.resource_members().get(id.0 as usize)?;
        names.push(member.name.clone());
        current = member.parent;
    }
    names.reverse();
    Some(names)
}

fn proposal_catalog_id<'a>(
    program: &'a CheckedProgram,
    kind: CatalogEntryKind,
    path: &str,
) -> Option<&'a str> {
    crate::catalog::active_program_proposal_id(program, kind, path)
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
