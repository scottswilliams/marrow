use std::path::Path;

use marrow_syntax::{IdentityTypeExpr, SourceSpan, TypeExpr};

use crate::analysis::AnalysisSnapshot;
use crate::annotation_refs::{TypeAnnotationBodies, walk_declaration_type_refs};
use crate::program::CheckedProgram;
use crate::resolve::resolve_store_by_root;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityTypeAnnotation {
    pub constructor_span: SourceSpan,
    pub root_span: SourceSpan,
    pub store_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTypeAnnotationCursorFact {
    pub span: SourceSpan,
    pub text: String,
}

pub fn identity_type_annotations(
    snapshot: &AnalysisSnapshot,
    file: &Path,
) -> Vec<IdentityTypeAnnotation> {
    let Some(analyzed) = snapshot.files.iter().find(|analyzed| analyzed.path == file) else {
        return Vec::new();
    };
    if !snapshot
        .program
        .modules
        .iter()
        .any(|module| module.source_file == analyzed.path)
    {
        return Vec::new();
    }
    let mut facts = Vec::new();
    for declaration in &analyzed.parsed.file.declarations {
        walk_declaration_type_refs(declaration, TypeAnnotationBodies::Include, &mut |ty| {
            collect_identity_annotations(&snapshot.program, ty, &mut facts);
        });
    }
    facts
}

pub fn source_type_annotation_cursor_fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<SourceTypeAnnotationCursorFact> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let mut best = None;

    for declaration in &analyzed.parsed.file.declarations {
        walk_declaration_type_refs(declaration, TypeAnnotationBodies::Include, &mut |ty| {
            if !span_covers(ty.span(), offset) {
                return;
            }
            let Some(fact) = type_annotation_cursor_fact(&analyzed.source, ty) else {
                return;
            };
            if best
                .as_ref()
                .is_none_or(|current: &SourceTypeAnnotationCursorFact| {
                    span_width(fact.span) < span_width(current.span)
                })
            {
                best = Some(fact);
            }
        });
    }

    best
}

/// Collect the saved-store identity references a type annotation names, addressing
/// each `Id(^root)` node by the spans the parser recorded. Only an identity whose
/// root names a declared store yields a fact. An optional-wrapped identity carries
/// no addressable saved-root reference here, matching the identity-annotation
/// contract.
fn collect_identity_annotations(
    program: &CheckedProgram,
    ty: &TypeExpr,
    facts: &mut Vec<IdentityTypeAnnotation>,
) {
    match ty {
        TypeExpr::Identity(IdentityTypeExpr {
            root,
            keyword_span,
            root_span,
            ..
        }) => {
            if resolve_store_by_root(program, root).is_some() {
                facts.push(IdentityTypeAnnotation {
                    constructor_span: *keyword_span,
                    root_span: *root_span,
                    store_root: root.clone(),
                });
            }
        }
        TypeExpr::Sequence { element, .. } => {
            collect_identity_annotations(program, element, facts);
        }
        TypeExpr::Optional { .. } | TypeExpr::Name { .. } => {}
    }
}

fn type_annotation_cursor_fact(
    source: &str,
    ty: &TypeExpr,
) -> Option<SourceTypeAnnotationCursorFact> {
    let span = ty.span();
    let text = source.get(span.start_byte..span.end_byte)?;
    Some(SourceTypeAnnotationCursorFact {
        span,
        text: text.to_string(),
    })
}

fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

fn span_width(span: SourceSpan) -> usize {
    span.end_byte.saturating_sub(span.start_byte)
}
