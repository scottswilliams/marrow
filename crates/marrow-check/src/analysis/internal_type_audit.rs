//! Compiler-development audit for unresolved recovery types in an otherwise
//! error-free analysis snapshot.

use std::path::PathBuf;

use marrow_codes::Code;
use marrow_syntax::{Keyword, SourceSpan, Token, TokenKind, lex_source};

use crate::binding::build_binding_index_from_lexed;
use crate::tooling::{
    PrelexedSourceHover, SourceSavedRootCursorKind, source_hover_fact_at_prelexed,
    source_saved_root_cursor_facts, type_contains_recovery_unknown,
};
use crate::{
    AnalysisSnapshot, CheckDiagnostic, DiagnosticAnchor, DiagnosticPayload, InternalTypeIssueKind,
    type_at,
};

/// One compiler-internal type hole found at a source position that would
/// otherwise fall through to ordinary type hover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalTypeIssue {
    pub file: PathBuf,
    pub span: SourceSpan,
    pub kind: InternalTypeIssueKind,
}

/// Audit a clean analysis snapshot for unresolved recovery types.
///
/// The audit is intentionally separate from ordinary checking. It uses the
/// compiler's existing binding, hover-fact, and cursor-inference owners. A
/// recovery type inside a richer callable fact is still an issue; explicit
/// dynamic values, no-value results, and diagnosed poison are not.
pub fn internal_type_issues(snapshot: &AnalysisSnapshot) -> Vec<InternalTypeIssue> {
    if snapshot.report.has_errors() {
        return Vec::new();
    }

    let lexed = snapshot
        .files
        .iter()
        .map(|analyzed| lex_source(&analyzed.source))
        .collect::<Vec<_>>();
    let index = build_binding_index_from_lexed(snapshot, &lexed);
    let mut issues = Vec::new();
    for (analyzed, lexed) in snapshot.files.iter().zip(&lexed) {
        // `AnalysisSnapshot` retains configured test parses after restoring the
        // source-only checked program. Cursor inference has no checked module for
        // those files, so skip them explicitly rather than presenting their lack
        // of a program fact as an audited clean result.
        if snapshot
            .program
            .module_index_by_file(&analyzed.path)
            .is_none()
        {
            continue;
        }
        let prelexed = PrelexedSourceHover::new(analyzed, lexed);
        let non_value_root_spans = source_saved_root_cursor_facts(analyzed)
            .into_iter()
            .filter(|root| {
                root.kind == SourceSavedRootCursorKind::Expression
                    && snapshot
                        .program
                        .facts
                        .store_by_root(&root.root)
                        .is_some_and(|store| !store.identity_keys.is_empty())
            })
            .map(|root| root.span)
            .collect::<Vec<_>>();
        for (token_index, token) in lexed.tokens.iter().enumerate() {
            if !is_representative_type_probe(&lexed.tokens, token_index) {
                continue;
            }
            let offset = token.span.start_byte;
            let hover =
                source_hover_fact_at_prelexed(snapshot, &index, analyzed, &prelexed, offset);
            let recovery_unknown = match hover {
                Some(fact) => fact.contains_recovery_unknown(),
                None => {
                    // Keyed saved roots in expressions are cursor/navigation
                    // facts, including collection-shaped loop subjects. Generic
                    // value inference returns recovery for those non-value
                    // positions by design.
                    if non_value_root_spans
                        .iter()
                        .any(|span| span.start_byte <= offset && offset <= span.end_byte)
                    {
                        continue;
                    }
                    type_at(&snapshot.program, &analyzed.path, &analyzed.parsed, offset)
                        .as_ref()
                        .is_some_and(type_contains_recovery_unknown)
                }
            };
            if !recovery_unknown {
                continue;
            }
            issues.push(InternalTypeIssue {
                file: analyzed.path.clone(),
                span: token.span,
                kind: InternalTypeIssueKind::RecoveryUnknown,
            });
        }
    }
    issues.sort_by(|left, right| {
        (
            &left.file,
            left.span.start_byte,
            left.span.end_byte,
            left.span.line,
            left.span.column,
        )
            .cmp(&(
                &right.file,
                right.span.start_byte,
                right.span.end_byte,
                right.span.line,
                right.span.column,
            ))
    });
    issues.dedup_by(|left, right| left.file == right.file && left.span == right.span);
    issues
}

/// Convert internal type issues into the checker's canonical typed warning
/// diagnostics. CLI transports render these through their existing project
/// diagnostic paths.
pub fn internal_type_issue_diagnostics(snapshot: &AnalysisSnapshot) -> Vec<CheckDiagnostic> {
    let names = snapshot.program.decl_ids();
    internal_type_issues(snapshot)
        .into_iter()
        .map(|issue| {
            CheckDiagnostic::new(
                Code::CompilerDevUnknownType,
                DiagnosticAnchor::at(&issue.file, issue.span),
                DiagnosticPayload::InternalTypeIssue(issue.kind),
                &names,
            )
        })
        .collect()
}

fn is_representative_type_probe(tokens: &[Token], index: usize) -> bool {
    let kind = tokens[index].kind;
    if matches!(
        kind,
        TokenKind::Identifier | TokenKind::Keyword(_) | TokenKind::RightParen
    ) && next_significant_kind(tokens, index).is_some_and(|next| {
        matches!(
            next,
            TokenKind::DoubleColon | TokenKind::Dot | TokenKind::QuestionDot | TokenKind::LeftParen
        )
    }) {
        return false;
    }
    match kind {
        TokenKind::Identifier
        | TokenKind::Integer
        | TokenKind::Decimal
        | TokenKind::Duration
        | TokenKind::String
        | TokenKind::Bytes
        | TokenKind::DotDot
        | TokenKind::DotDotEqual
        | TokenKind::EqualEqual
        | TokenKind::BangEqual
        | TokenKind::QuestionDot
        | TokenKind::QuestionQuestion
        | TokenKind::Less
        | TokenKind::LessEqual
        | TokenKind::Greater
        | TokenKind::GreaterEqual
        | TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::PlusEqual
        | TokenKind::MinusEqual
        | TokenKind::StarEqual
        | TokenKind::SlashEqual
        | TokenKind::PercentEqual
        | TokenKind::Keyword(Keyword::True | Keyword::False | Keyword::Absent)
        | TokenKind::Keyword(Keyword::Not | Keyword::And | Keyword::Or | Keyword::Is) => true,
        TokenKind::Keyword(keyword) => marrow_syntax::is_expression_callable_keyword(keyword),
        TokenKind::RightParen => true,
        TokenKind::InterpolationStart
        | TokenKind::InterpolationText
        | TokenKind::InterpolationExprStart
        | TokenKind::InterpolationExprEnd
        | TokenKind::InterpolationEnd
        | TokenKind::Comment
        | TokenKind::DocComment
        | TokenKind::Indent
        | TokenKind::Dedent
        | TokenKind::Newline
        | TokenKind::Eof
        | TokenKind::LeftParen
        | TokenKind::LeftBracket
        | TokenKind::RightBracket
        | TokenKind::Colon
        | TokenKind::DoubleColon
        | TokenKind::Comma
        | TokenKind::Dot
        | TokenKind::Equal
        | TokenKind::Question
        | TokenKind::Caret => false,
    }
}

fn next_significant_kind(tokens: &[Token], index: usize) -> Option<TokenKind> {
    tokens.get(index + 1..)?.iter().find_map(|token| {
        (!matches!(
            token.kind,
            TokenKind::Comment | TokenKind::DocComment | TokenKind::Newline
        ))
        .then_some(token.kind)
    })
}
