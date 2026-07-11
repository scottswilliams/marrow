//! Compiler-development audit for unresolved recovery types in an otherwise
//! error-free analysis snapshot.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_syntax::{Declaration, LexedSource, SourceSpan, TokenKind, lex_source};

use crate::binding::build_binding_index_from_lexed;
use crate::checks::{file_prelude, trace_function_recovery_types};
use crate::infer::{RecoveryExpressionSite, RecoveryTypeSite};
use crate::tooling::{PrelexedSourceHover, source_non_type_hover_fact_at_prelexed};
use crate::{
    AnalysisSnapshot, CheckDiagnostic, DiagnosticAnchor, DiagnosticPayload, InternalTypeIssueKind,
};

/// One compiler-internal type hole surfaced at a source hover position.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InternalTypeIssue {
    file: PathBuf,
    span: SourceSpan,
    kind: InternalTypeIssueKind,
}

#[derive(Clone, Copy)]
enum AuditedFiles<'a> {
    All,
    Only(&'a HashSet<PathBuf>),
}

impl AuditedFiles<'_> {
    fn includes(self, file: &Path) -> bool {
        match self {
            Self::All => true,
            Self::Only(files) => files.contains(file),
        }
    }
}

/// Audit a clean analysis snapshot for unresolved recovery types.
///
/// The audit is intentionally separate from ordinary checking. It uses the
/// compiler's production statement/type walk and canonical hover precedence. A
/// recovery type inside a richer callable fact is still an issue; explicit
/// dynamic values, no-value results, and diagnosed poison are not.
fn internal_type_issues(
    snapshot: &AnalysisSnapshot,
    audited_files: AuditedFiles<'_>,
) -> Vec<InternalTypeIssue> {
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
        if !audited_files.includes(&analyzed.path) {
            continue;
        }
        let prelexed = PrelexedSourceHover::new(analyzed, lexed);
        issues.extend(callable_signature_issues(snapshot, analyzed, lexed));

        let prelude = file_prelude(&snapshot.program, &analyzed.path, &analyzed.parsed);
        let mut recovery_sites = analyzed
            .parsed
            .file
            .declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::Function(function) => Some(trace_function_recovery_types(
                    &snapshot.program,
                    &analyzed.path,
                    function,
                    &prelude.module_constants,
                    &prelude.aliases,
                )),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        recovery_sites.sort_by_key(|site| recovery_site_order(site.expression));
        recovery_sites
            .dedup_by(|left, right| left.file == right.file && left.expression == right.expression);
        for site in recovery_sites {
            let Some((offset, span)) = recovery_site_probe(&site, lexed) else {
                continue;
            };
            if source_non_type_hover_fact_at_prelexed(snapshot, &index, analyzed, &prelexed, offset)
                .is_some_and(|fact| !fact.contains_recovery_unknown())
            {
                continue;
            }
            issues.push(InternalTypeIssue {
                file: site.file,
                span,
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

fn recovery_site_order(site: RecoveryExpressionSite) -> (u8, usize, usize, u32, u32) {
    let (kind, span) = match site {
        RecoveryExpressionSite::Name(span) => (0, span),
        RecoveryExpressionSite::SavedRoot(span) => (1, span),
        RecoveryExpressionSite::Call(span) => (2, span),
        RecoveryExpressionSite::Field(span) => (3, span),
    };
    (kind, span.start_byte, span.end_byte, span.line, span.column)
}

fn callable_signature_issues(
    snapshot: &AnalysisSnapshot,
    analyzed: &crate::AnalyzedFile,
    lexed: &LexedSource,
) -> Vec<InternalTypeIssue> {
    let Some(module) = snapshot.program.module_by_file(&analyzed.path) else {
        return Vec::new();
    };
    let mut issues = Vec::new();
    for function in &module.functions {
        let recovery = function
            .params
            .iter()
            .any(|param| param.ty.contains_recovery_unknown())
            || function
                .return_type
                .as_ref()
                .is_some_and(crate::MarrowType::contains_recovery_unknown);
        if recovery
            && let Some(span) =
                declaration_name_span(&analyzed.source, lexed, function.span, &function.name)
        {
            issues.push(InternalTypeIssue {
                file: analyzed.path.clone(),
                span,
                kind: InternalTypeIssueKind::RecoveryUnknown,
            });
        }
    }
    for constant in &module.constants {
        if constant
            .ty
            .as_ref()
            .is_some_and(crate::MarrowType::contains_recovery_unknown)
            && let Some(span) =
                declaration_name_span(&analyzed.source, lexed, constant.span, &constant.name)
        {
            issues.push(InternalTypeIssue {
                file: analyzed.path.clone(),
                span,
                kind: InternalTypeIssueKind::RecoveryUnknown,
            });
        }
    }
    issues
}

fn declaration_name_span(
    source: &str,
    lexed: &LexedSource,
    declaration: SourceSpan,
    name: &str,
) -> Option<SourceSpan> {
    let first = lexed
        .tokens
        .partition_point(|token| token.span.end_byte <= declaration.start_byte);
    lexed.tokens[first..]
        .iter()
        .take_while(|token| token.span.start_byte < declaration.end_byte)
        .find_map(|token| {
            (token.kind == TokenKind::Identifier
                && declaration.start_byte <= token.span.start_byte
                && token.span.end_byte <= declaration.end_byte
                && token.text(source) == name)
                .then_some(token.span)
        })
}

fn recovery_site_probe(
    site: &RecoveryTypeSite,
    lexed: &LexedSource,
) -> Option<(usize, SourceSpan)> {
    match site.expression {
        RecoveryExpressionSite::Name(span) | RecoveryExpressionSite::Field(span) => {
            Some((span.start_byte, span))
        }
        RecoveryExpressionSite::SavedRoot(span) => {
            let token = token_at(&lexed.tokens, span.end_byte.checked_sub(1)?)?;
            (token.kind == TokenKind::Identifier
                && span.start_byte <= token.span.start_byte
                && token.span.end_byte <= span.end_byte)
                .then_some((token.span.start_byte, token.span))
        }
        RecoveryExpressionSite::Call(span) => {
            let token = token_at(&lexed.tokens, span.end_byte.checked_sub(1)?)?;
            (token.kind == TokenKind::RightParen
                && span.start_byte <= token.span.start_byte
                && token.span.end_byte == span.end_byte)
                .then_some((token.span.start_byte, token.span))
        }
    }
}

fn token_at(tokens: &[marrow_syntax::Token], offset: usize) -> Option<&marrow_syntax::Token> {
    let index = tokens.partition_point(|token| token.span.end_byte <= offset);
    let token = tokens.get(index)?;
    (token.span.start_byte <= offset && offset < token.span.end_byte).then_some(token)
}

/// Convert internal type issues into the checker's canonical typed warning
/// diagnostics. CLI transports render these through their existing project
/// diagnostic paths.
pub(super) fn internal_type_issue_diagnostics(snapshot: &AnalysisSnapshot) -> Vec<CheckDiagnostic> {
    internal_type_issue_diagnostics_in(snapshot, AuditedFiles::All)
}

pub(super) fn internal_type_issue_diagnostics_for_files(
    snapshot: &AnalysisSnapshot,
    files: &HashSet<PathBuf>,
) -> Vec<CheckDiagnostic> {
    internal_type_issue_diagnostics_in(snapshot, AuditedFiles::Only(files))
}

fn internal_type_issue_diagnostics_in(
    snapshot: &AnalysisSnapshot,
    audited_files: AuditedFiles<'_>,
) -> Vec<CheckDiagnostic> {
    let names = snapshot.program.decl_ids();
    internal_type_issues(snapshot, audited_files)
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use marrow_project::parse_config;

    use super::*;
    use crate::{MarrowType, ProjectSources, analyze_project};

    struct TempProject(PathBuf);

    impl Drop for TempProject {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn callable_signature_recovery_is_a_representative_audit_position() {
        let root = std::env::temp_dir().join(format!(
            "marrow-internal-type-audit-callable-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos(),
        ));
        let project = TempProject(root);
        std::fs::create_dir_all(project.0.join("src")).expect("create source root");
        let source = "module m\n\nfn value(): int\n    return 1\n";
        let source_path = project.0.join("src/m.mw");
        std::fs::write(&source_path, source).expect("write source");
        let config =
            parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#)
                .expect("config");
        let sources = ProjectSources::new().with(source_path, source);
        let mut snapshot =
            analyze_project(&project.0, &config, &sources, None, None).expect("analyze");
        assert!(!snapshot.report.has_errors(), "{:#?}", snapshot.report);

        snapshot.program.modules[0].functions[0].return_type =
            Some(MarrowType::Sequence(Box::new(MarrowType::Unknown)));
        let issues = internal_type_issues(&snapshot, AuditedFiles::All);
        assert_eq!(issues.len(), 1, "{issues:#?}");
        assert_eq!(
            &source[issues[0].span.start_byte..issues[0].span.end_byte],
            "value",
        );
    }
}
