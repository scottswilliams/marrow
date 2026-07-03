use std::path::Path;

use marrow_syntax::{
    Block, Declaration, ElseIf, EvolveStep, Expression, IdentityTypeExpr, SourceSpan, Statement,
    SurfaceItem, SurfaceTarget, TypeExpr,
};

use crate::analysis::AnalysisSnapshot;
use crate::annotation_refs::{TypeAnnotationBodies, walk_declaration_type_refs};
use crate::source_spans::source_span_at;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSavedRootCursorFact {
    pub root: String,
    pub span: SourceSpan,
    pub kind: SourceSavedRootCursorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceSavedRootCursorKind {
    Declaration,
    SurfaceTarget,
    TypeAnnotation,
    Expression,
    EvolutionTarget,
}

pub fn source_saved_root_cursor_fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<SourceSavedRootCursorFact> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let mut facts = Vec::new();

    for declaration in &analyzed.parsed.file.declarations {
        collect_declaration_roots(&analyzed.source, declaration, &mut facts);
    }

    facts
        .into_iter()
        .filter(|fact| span_covers(fact.span, offset))
        .min_by_key(|fact| span_width(fact.span))
}

fn collect_declaration_roots(
    source: &str,
    declaration: &Declaration,
    facts: &mut Vec<SourceSavedRootCursorFact>,
) {
    walk_declaration_type_refs(declaration, TypeAnnotationBodies::Include, &mut |ty| {
        collect_type_ref_roots(source, ty, facts);
    });

    match declaration {
        Declaration::Const(constant) => {
            if let Some(value) = &constant.value {
                collect_expr_roots(source, value, SourceSavedRootCursorKind::Expression, facts);
            }
        }
        Declaration::Store(store) => {
            collect_saved_root(
                source,
                &store.root.root,
                store.root.span,
                SourceSavedRootCursorKind::Declaration,
                facts,
            );
        }
        Declaration::Surface(surface) => {
            collect_saved_root(
                source,
                &surface.store.root,
                surface.store.span,
                SourceSavedRootCursorKind::SurfaceTarget,
                facts,
            );
            for item in &surface.items {
                if let SurfaceItem::Collection { target, .. } = item {
                    collect_surface_target_root(source, target, facts);
                }
            }
        }
        Declaration::Function(function) => {
            collect_block_roots(source, &function.body, facts);
        }
        Declaration::Evolve(evolve) => {
            for step in &evolve.steps {
                collect_evolve_step_roots(source, step, facts);
            }
        }
        Declaration::Resource(_) | Declaration::Enum(_) => {}
    }
}

fn collect_type_ref_roots(source: &str, ty: &TypeExpr, facts: &mut Vec<SourceSavedRootCursorFact>) {
    match ty {
        TypeExpr::Identity(IdentityTypeExpr {
            root,
            caret_span,
            root_span,
            ..
        }) => {
            facts.push(SourceSavedRootCursorFact {
                root: root.clone(),
                span: source_span_at(source, caret_span.start_byte, root_span.end_byte),
                kind: SourceSavedRootCursorKind::TypeAnnotation,
            });
        }
        TypeExpr::Sequence { element, .. } => collect_type_ref_roots(source, element, facts),
        TypeExpr::Optional { inner, .. } => collect_type_ref_roots(source, inner, facts),
        TypeExpr::Name { .. } => {}
    }
}

fn collect_surface_target_root(
    source: &str,
    target: &SurfaceTarget,
    facts: &mut Vec<SourceSavedRootCursorFact>,
) {
    match target {
        SurfaceTarget::Root { root, span }
        | SurfaceTarget::Index { root, span, .. }
        | SurfaceTarget::IndexRange { root, span, .. } => collect_saved_root(
            source,
            root,
            *span,
            SourceSavedRootCursorKind::SurfaceTarget,
            facts,
        ),
    }
}

fn collect_evolve_step_roots(
    source: &str,
    step: &EvolveStep,
    facts: &mut Vec<SourceSavedRootCursorFact>,
) {
    match step {
        EvolveStep::Rename { from, to, .. } => {
            collect_expr_roots(
                source,
                from,
                SourceSavedRootCursorKind::EvolutionTarget,
                facts,
            );
            collect_expr_roots(
                source,
                to,
                SourceSavedRootCursorKind::EvolutionTarget,
                facts,
            );
        }
        EvolveStep::Default { target, value, .. } => {
            collect_expr_roots(
                source,
                target,
                SourceSavedRootCursorKind::EvolutionTarget,
                facts,
            );
            collect_expr_roots(source, value, SourceSavedRootCursorKind::Expression, facts);
        }
        EvolveStep::Retire { target, .. } => {
            collect_expr_roots(
                source,
                target,
                SourceSavedRootCursorKind::EvolutionTarget,
                facts,
            );
        }
        EvolveStep::Transform { target, body, .. } => {
            collect_expr_roots(
                source,
                target,
                SourceSavedRootCursorKind::EvolutionTarget,
                facts,
            );
            collect_block_roots(source, body, facts);
        }
    }
}

fn collect_block_roots(source: &str, block: &Block, facts: &mut Vec<SourceSavedRootCursorFact>) {
    for statement in &block.statements {
        collect_statement_roots(source, statement, facts);
    }
}

fn collect_branch_roots(
    source: &str,
    then_block: &Block,
    else_ifs: &[ElseIf],
    else_block: Option<&Block>,
    facts: &mut Vec<SourceSavedRootCursorFact>,
) {
    collect_block_roots(source, then_block, facts);
    for else_if in else_ifs {
        if let Some(condition) = &else_if.condition {
            collect_expr_roots(
                source,
                condition,
                SourceSavedRootCursorKind::Expression,
                facts,
            );
        }
        collect_block_roots(source, &else_if.block, facts);
    }
    if let Some(else_block) = else_block {
        collect_block_roots(source, else_block, facts);
    }
}

fn collect_statement_roots(
    source: &str,
    statement: &Statement,
    facts: &mut Vec<SourceSavedRootCursorFact>,
) {
    match statement {
        Statement::Const { value, .. }
        | Statement::Delete { path: value, .. }
        | Statement::Throw { value, .. }
        | Statement::Expr { value, .. } => {
            collect_expr_roots(source, value, SourceSavedRootCursorKind::Expression, facts);
        }
        Statement::Var { value, .. } | Statement::Return { value, .. } => {
            if let Some(value) = value {
                collect_expr_roots(source, value, SourceSavedRootCursorKind::Expression, facts);
            }
        }
        Statement::Assign { target, value, .. }
        | Statement::CompoundAssign { target, value, .. } => {
            collect_expr_roots(source, target, SourceSavedRootCursorKind::Expression, facts);
            collect_expr_roots(source, value, SourceSavedRootCursorKind::Expression, facts);
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if let Some(condition) = condition {
                collect_expr_roots(
                    source,
                    condition,
                    SourceSavedRootCursorKind::Expression,
                    facts,
                );
            }
            collect_branch_roots(source, then_block, else_ifs, else_block.as_ref(), facts);
        }
        Statement::IfConst {
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            collect_expr_roots(source, value, SourceSavedRootCursorKind::Expression, facts);
            collect_branch_roots(source, then_block, else_ifs, else_block.as_ref(), facts);
        }
        Statement::While {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_roots(
                    source,
                    condition,
                    SourceSavedRootCursorKind::Expression,
                    facts,
                );
            }
            collect_block_roots(source, body, facts);
        }
        Statement::For {
            iterable,
            step,
            body,
            ..
        } => {
            collect_expr_roots(
                source,
                iterable,
                SourceSavedRootCursorKind::Expression,
                facts,
            );
            if let Some(step) = step {
                collect_expr_roots(source, step, SourceSavedRootCursorKind::Expression, facts);
            }
            collect_block_roots(source, body, facts);
        }
        Statement::Transaction { body, .. } => collect_block_roots(source, body, facts),
        Statement::Try { body, catch, .. } => {
            collect_block_roots(source, body, facts);
            if let Some(catch) = catch {
                collect_block_roots(source, &catch.block, facts);
            }
        }
        Statement::Match {
            scrutinee, arms, ..
        } => {
            if let Some(scrutinee) = scrutinee {
                collect_expr_roots(
                    source,
                    scrutinee,
                    SourceSavedRootCursorKind::Expression,
                    facts,
                );
            }
            for arm in arms {
                collect_block_roots(source, &arm.block, facts);
            }
        }
        Statement::Break { .. } | Statement::Continue { .. } => {}
    }
}

fn collect_expr_roots(
    source: &str,
    expr: &Expression,
    kind: SourceSavedRootCursorKind,
    facts: &mut Vec<SourceSavedRootCursorFact>,
) {
    if let Expression::SavedRoot { name, span } = expr {
        collect_saved_root(source, name, *span, kind, facts);
    }
    crate::walk::for_each_child_expr(expr, |child| {
        collect_expr_roots(source, child, kind, facts);
    });
}

fn collect_saved_root(
    source: &str,
    root: &str,
    span: SourceSpan,
    kind: SourceSavedRootCursorKind,
    facts: &mut Vec<SourceSavedRootCursorFact>,
) {
    let Some(token_span) = saved_root_token_span(source, span, root) else {
        return;
    };
    facts.push(SourceSavedRootCursorFact {
        root: root.to_string(),
        span: token_span,
        kind,
    });
}

fn saved_root_token_span(source: &str, span: SourceSpan, root: &str) -> Option<SourceSpan> {
    let start = span.start_byte;
    let root_start = start.checked_add(1)?;
    let end = root_start.checked_add(root.len())?;
    if source.as_bytes().get(start) != Some(&b'^') || source.get(root_start..end)? != root {
        return None;
    }
    Some(source_span_at(source, start, end))
}

fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

fn span_width(span: SourceSpan) -> usize {
    span.end_byte.saturating_sub(span.start_byte)
}
