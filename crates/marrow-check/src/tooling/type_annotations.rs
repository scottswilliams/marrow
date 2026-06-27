use std::path::Path;

use marrow_syntax::{Keyword, SourceSpan, Token, TokenKind, TypeRef};

use crate::analysis::AnalysisSnapshot;
use crate::annotation_refs::{TypeAnnotationBodies, walk_declaration_type_refs};
use crate::checks::file_prelude;
use crate::enums::resolve_type;
use crate::program::{CheckedProgram, MarrowType};
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
    let prelude = file_prelude(&snapshot.program, file, &analyzed.parsed);
    let lexed = marrow_syntax::lex_source(&analyzed.source);
    let mut facts = Vec::new();

    for declaration in &analyzed.parsed.file.declarations {
        walk_declaration_type_refs(declaration, TypeAnnotationBodies::Include, &mut |ty| {
            let resolved = resolve_type(ty, &snapshot.program, &prelude.aliases, file);
            let tokens = tokens_in_span(&lexed.tokens, ty.span);
            collect_identity_annotations(
                &mut facts,
                &snapshot.program,
                &analyzed.source,
                &tokens,
                &resolved,
            );
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
            if !span_covers(ty.span, offset) {
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

fn tokens_in_span(tokens: &[Token], span: SourceSpan) -> Vec<&Token> {
    tokens
        .iter()
        .filter(|token| {
            span.start_byte <= token.span.start_byte
                && token.span.end_byte <= span.end_byte
                && !matches!(
                    token.kind,
                    TokenKind::Comment
                        | TokenKind::DocComment
                        | TokenKind::Indent
                        | TokenKind::Dedent
                        | TokenKind::Newline
                        | TokenKind::Eof
                )
        })
        .collect()
}

fn collect_identity_annotations(
    facts: &mut Vec<IdentityTypeAnnotation>,
    program: &CheckedProgram,
    source: &str,
    tokens: &[&Token],
    ty: &MarrowType,
) {
    match ty {
        MarrowType::Identity(store_root) => {
            if resolve_store_by_root(program, store_root).is_some()
                && let Some((constructor_span, root_span)) =
                    identity_type_spans(source, tokens, store_root)
            {
                facts.push(IdentityTypeAnnotation {
                    constructor_span,
                    root_span,
                    store_root: store_root.clone(),
                });
            }
        }
        MarrowType::Sequence(element) => {
            collect_identity_annotations(
                facts,
                program,
                source,
                sequence_element_tokens(tokens),
                element,
            );
        }
        _ => {}
    }
}

fn type_annotation_cursor_fact(
    source: &str,
    ty: &TypeRef,
) -> Option<SourceTypeAnnotationCursorFact> {
    let text = source.get(ty.span.start_byte..ty.span.end_byte)?;
    Some(SourceTypeAnnotationCursorFact {
        span: ty.span,
        text: text.to_string(),
    })
}

fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

fn span_width(span: SourceSpan) -> usize {
    span.end_byte.saturating_sub(span.start_byte)
}

fn identity_type_spans(
    source: &str,
    tokens: &[&Token],
    store_root: &str,
) -> Option<(SourceSpan, SourceSpan)> {
    let [constructor, open, caret, root, close] = tokens else {
        return None;
    };
    if constructor.kind != TokenKind::Keyword(Keyword::Id)
        || open.kind != TokenKind::LeftParen
        || caret.kind != TokenKind::Caret
        || root.kind != TokenKind::Identifier
        || root.text(source) != store_root
        || close.kind != TokenKind::RightParen
    {
        return None;
    }

    Some((constructor.span, root.span))
}

fn sequence_element_tokens<'a>(tokens: &'a [&'a Token]) -> &'a [&'a Token] {
    if tokens.len() >= 3
        && tokens
            .first()
            .is_some_and(|token| token.kind == TokenKind::Keyword(Keyword::Sequence))
        && tokens
            .get(1)
            .is_some_and(|token| token.kind == TokenKind::LeftBracket)
        && tokens
            .last()
            .is_some_and(|token| token.kind == TokenKind::RightBracket)
    {
        &tokens[2..tokens.len() - 1]
    } else {
        &[]
    }
}
