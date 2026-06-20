use marrow_syntax::{
    Block, Declaration, ElseIf, EvolveStep, KeyParam, ResourceMember, SourceSpan, Statement,
    TypeRef,
};

use crate::source_spans::source_span_at;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeAnnotationBodies {
    Include,
    Omit,
}

pub(crate) fn walk_declaration_type_refs(
    declaration: &Declaration,
    bodies: TypeAnnotationBodies,
    visit: &mut impl FnMut(&TypeRef),
) {
    match declaration {
        Declaration::Const(constant) => {
            if let Some(ty) = &constant.ty {
                visit(ty);
            }
        }
        Declaration::Resource(resource) => {
            walk_resource_member_type_refs(&resource.members, visit);
        }
        Declaration::Store(store) => {
            walk_key_type_refs(&store.root.keys, visit);
        }
        Declaration::Function(function) => {
            for param in &function.params {
                walk_key_type_refs(&param.keys, visit);
                visit(&param.ty);
            }
            if let Some(ty) = &function.return_type {
                visit(ty);
            }
            if bodies == TypeAnnotationBodies::Include {
                walk_block_type_refs(&function.body, visit);
            }
        }
        Declaration::Evolve(evolve) => {
            if bodies == TypeAnnotationBodies::Include {
                for step in &evolve.steps {
                    if let EvolveStep::Transform { body, .. } = step {
                        walk_block_type_refs(body, visit);
                    }
                }
            }
        }
        Declaration::Enum(_) | Declaration::Surface(_) => {}
    }
}

fn walk_resource_member_type_refs(members: &[ResourceMember], visit: &mut impl FnMut(&TypeRef)) {
    for member in members {
        match member {
            ResourceMember::Field(field) => {
                walk_key_type_refs(&field.keys, visit);
                visit(&field.ty);
            }
            ResourceMember::Group(group) => {
                walk_key_type_refs(&group.keys, visit);
                walk_resource_member_type_refs(&group.members, visit);
            }
        }
    }
}

fn walk_key_type_refs(keys: &[KeyParam], visit: &mut impl FnMut(&TypeRef)) {
    for key in keys {
        visit(&key.ty);
    }
}

pub(crate) fn walk_block_type_refs(block: &Block, visit: &mut impl FnMut(&TypeRef)) {
    for statement in &block.statements {
        walk_statement_type_refs(statement, visit);
    }
}

fn walk_branch_type_refs(
    then_block: &Block,
    else_ifs: &[ElseIf],
    else_block: Option<&Block>,
    visit: &mut impl FnMut(&TypeRef),
) {
    walk_block_type_refs(then_block, visit);
    for else_if in else_ifs {
        walk_block_type_refs(&else_if.block, visit);
    }
    if let Some(else_block) = else_block {
        walk_block_type_refs(else_block, visit);
    }
}

fn walk_statement_type_refs(statement: &Statement, visit: &mut impl FnMut(&TypeRef)) {
    match statement {
        Statement::Const { ty, .. } => {
            if let Some(ty) = ty {
                visit(ty);
            }
        }
        Statement::Var { keys, ty, .. } => {
            walk_key_type_refs(keys, visit);
            if let Some(ty) = ty {
                visit(ty);
            }
        }
        Statement::IfConst {
            ty,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if let Some(ty) = ty {
                visit(ty);
            }
            walk_branch_type_refs(then_block, else_ifs, else_block.as_ref(), visit);
        }
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            walk_branch_type_refs(then_block, else_ifs, else_block.as_ref(), visit);
        }
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. } => {
            walk_block_type_refs(body, visit);
        }
        Statement::Try { body, catch, .. } => {
            walk_block_type_refs(body, visit);
            if let Some(catch) = catch {
                if let Some(ty) = &catch.ty {
                    visit(ty);
                }
                walk_block_type_refs(&catch.block, visit);
            }
        }
        Statement::Match { arms, .. } => {
            for arm in arms {
                walk_block_type_refs(&arm.block, visit);
            }
        }
        Statement::Assign { .. }
        | Statement::Delete { .. }
        | Statement::Return { .. }
        | Statement::ReturnAbsent { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Throw { .. }
        | Statement::Expr { .. } => {}
    }
}

pub(crate) fn type_ref_enum_leaf_span(
    source: &str,
    ty: &TypeRef,
    enum_name: &str,
) -> Option<SourceSpan> {
    let end_byte = ty.span.end_byte.min(source.len());
    let text = source.get(ty.span.start_byte..end_byte)?;
    let offset = text.rfind(enum_name)?;
    let start_byte = ty.span.start_byte + offset;
    Some(source_span_at(
        source,
        start_byte,
        start_byte + enum_name.len(),
    ))
}
