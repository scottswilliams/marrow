use std::collections::HashMap;
use std::path::Path;

use marrow_syntax::{LexedSource, ParsedSource, Token, TokenKind};

use crate::AnalysisSnapshot;

use super::syntax::is_path_segment_token;
use super::{
    ByteSpan, SourceSemanticTokenModifiers, SourceSemanticTokenRole, SourceSemanticTokenStyle,
    byte_span,
};
use crate::tooling::signatures::{
    CallableSignatureKind, callable_callee_contexts, intrinsic_callable_signature,
    intrinsic_callable_signature_for_file,
};

pub(super) fn context_free_callable_overrides(
    lexed: &LexedSource,
    parsed: &ParsedSource,
    source: &str,
) -> HashMap<ByteSpan, SourceSemanticTokenStyle> {
    callable_overrides(lexed, parsed, source, None)
}

pub(super) fn snapshot_callable_overrides(
    lexed: &LexedSource,
    parsed: &ParsedSource,
    source: &str,
    snapshot: &AnalysisSnapshot,
    file: &Path,
) -> HashMap<ByteSpan, SourceSemanticTokenStyle> {
    callable_overrides(lexed, parsed, source, Some((snapshot, file)))
}

fn callable_overrides(
    lexed: &LexedSource,
    parsed: &ParsedSource,
    source: &str,
    analysis: Option<(&AnalysisSnapshot, &Path)>,
) -> HashMap<ByteSpan, SourceSemanticTokenStyle> {
    let mut overrides = HashMap::new();
    let token_indices = token_indices_by_span(lexed);

    for call in callable_callee_contexts(source, lexed, parsed) {
        let Some(callable) = intrinsic_callable(analysis, &call.callee_path_segments) else {
            continue;
        };
        if callable == CallableSignatureKind::StandardLibrary {
            let leaf = token_indices
                .get(&byte_span(call.callee_leaf_span))
                .copied();
            for prefix in leaf
                .into_iter()
                .flat_map(|index| callee_prefix_tokens(lexed, index))
            {
                insert_builtin_namespace_prefix(&mut overrides, prefix);
            }
        }
        overrides.insert(
            byte_span(call.callee_leaf_span),
            SourceSemanticTokenStyle {
                role: callable_role(callable),
                modifiers: SourceSemanticTokenModifiers {
                    default_library: true,
                    ..Default::default()
                },
            },
        );
    }
    overrides
}

fn intrinsic_callable(
    analysis: Option<(&AnalysisSnapshot, &Path)>,
    segments: &[String],
) -> Option<CallableSignatureKind> {
    let signature = match analysis {
        Some((snapshot, file)) => intrinsic_callable_signature_for_file(snapshot, file, segments),
        None => intrinsic_callable_signature(segments),
    }?;
    Some(signature.kind)
}

fn callable_role(kind: CallableSignatureKind) -> SourceSemanticTokenRole {
    match kind {
        CallableSignatureKind::ScalarConversion => SourceSemanticTokenRole::TypeKeyword,
        CallableSignatureKind::Builtin
        | CallableSignatureKind::ErrorConstructor
        | CallableSignatureKind::IdentityConstructor
        | CallableSignatureKind::StandardLibrary => SourceSemanticTokenRole::Function,
    }
}

fn insert_builtin_namespace_prefix(
    overrides: &mut HashMap<ByteSpan, SourceSemanticTokenStyle>,
    token: &Token,
) {
    overrides.insert(
        byte_span(token.span),
        SourceSemanticTokenStyle {
            role: SourceSemanticTokenRole::Namespace,
            modifiers: SourceSemanticTokenModifiers {
                default_library: true,
                ..Default::default()
            },
        },
    );
}

fn token_indices_by_span(lexed: &LexedSource) -> HashMap<ByteSpan, usize> {
    lexed
        .tokens
        .iter()
        .enumerate()
        .map(|(index, token)| (byte_span(token.span), index))
        .collect()
}

fn callee_prefix_tokens(lexed: &LexedSource, leaf: usize) -> Vec<&Token> {
    let mut indices = vec![leaf];
    let mut index = leaf;
    while index >= 2
        && lexed.tokens[index - 1].kind == TokenKind::DoubleColon
        && is_path_segment_token(lexed.tokens[index - 2].kind)
    {
        index -= 2;
        indices.push(index);
    }
    indices.reverse();
    indices.pop();
    indices
        .into_iter()
        .map(|index| &lexed.tokens[index])
        .collect()
}
