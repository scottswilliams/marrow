mod callables;
mod declarations;
mod identity_annotations;
mod references;
mod syntax;

use std::collections::HashMap;
use std::path::Path;

use marrow_syntax::{LexedSource, ParsedSource, SourceSpan, Token, TokenKind, lex_source};

use crate::{AnalysisSnapshot, BindingIndex};

type ByteSpan = (usize, usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSemanticTokenFact {
    pub span: SourceSpan,
    pub role: SourceSemanticTokenRole,
    pub modifiers: SourceSemanticTokenModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceSemanticTokenRole {
    Keyword,
    TypeKeyword,
    StringLiteral,
    NumberLiteral,
    BooleanLiteral,
    Comment,
    Operator,
    Namespace,
    Variable,
    SavedRoot,
    Function,
    Resource,
    Surface,
    Enum,
    EnumMember,
    ResourceMember,
    Index,
    Parameter,
    KeyParameter,
    IdentityTypeConstructor,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceSemanticTokenModifiers {
    /// The source spelling is a syntactic marker for saved-data access, such as
    /// the `^root` marker. LSP maps this to its `modification` modifier.
    pub modification: bool,
    /// The token is owned by Marrow's intrinsic or standard-library surface, not
    /// project source.
    pub default_library: bool,
    /// The token names an immutable module-level source binding.
    pub readonly: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SourceSemanticTokenStyle {
    pub(super) role: SourceSemanticTokenRole,
    pub(super) modifiers: SourceSemanticTokenModifiers,
}

impl SourceSemanticTokenStyle {
    pub(super) fn plain(role: SourceSemanticTokenRole) -> Self {
        Self {
            role,
            modifiers: SourceSemanticTokenModifiers::default(),
        }
    }
}

pub fn source_semantic_token_facts(
    source: &str,
    lexed: &LexedSource,
    parsed: &ParsedSource,
) -> Vec<SourceSemanticTokenFact> {
    semantic_token_facts_from_parts(
        source,
        lexed,
        parsed,
        callables::context_free_callable_overrides(lexed, parsed, source),
        HashMap::new(),
        HashMap::new(),
    )
}

pub fn source_semantic_token_facts_for_file(
    snapshot: &AnalysisSnapshot,
    binding_index: &BindingIndex,
    file: &Path,
) -> Option<Vec<SourceSemanticTokenFact>> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let lexed = lex_source(&analyzed.source);
    Some(semantic_token_facts_from_parts(
        &analyzed.source,
        &lexed,
        &analyzed.parsed,
        callables::snapshot_callable_overrides(
            &lexed,
            &analyzed.parsed,
            &analyzed.source,
            snapshot,
            file,
        ),
        references::reference_overrides(&lexed, &analyzed.source, binding_index, file),
        identity_annotations::identity_type_annotation_overrides(snapshot, file),
    ))
}

fn semantic_token_facts_from_parts(
    source: &str,
    lexed: &LexedSource,
    parsed: &ParsedSource,
    callable_overrides: HashMap<ByteSpan, SourceSemanticTokenStyle>,
    reference_overrides: HashMap<ByteSpan, SourceSemanticTokenStyle>,
    identity_overrides: HashMap<ByteSpan, SourceSemanticTokenStyle>,
) -> Vec<SourceSemanticTokenFact> {
    let file = &parsed.file;
    let const_declarations = declarations::const_declaration_overrides(lexed, file, source);
    let declarations = declarations::declaration_overrides(lexed, file, source);

    let mut facts = Vec::new();
    let mut i = 0;
    while i < lexed.tokens.len() {
        let token = &lexed.tokens[i];

        if token.kind == TokenKind::Caret {
            facts.push(fact(token.span, saved_root_style()));
            if let Some(name) = lexed.tokens.get(i + 1)
                && name.kind == TokenKind::Identifier
            {
                facts.push(fact(name.span, saved_root_style()));
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        let span = byte_span(token.span);
        if let Some(style) = const_declarations
            .get(&span)
            .copied()
            .or_else(|| declarations.get(&span).copied())
            .or_else(|| callable_overrides.get(&span).copied())
            .or_else(|| reference_overrides.get(&span).copied())
            .or_else(|| identity_overrides.get(&span).copied())
            .or_else(|| syntax::syntax_style(token.kind))
        {
            facts.push(fact(token.span, style));
        }
        i += 1;
    }

    facts
}

fn saved_root_style() -> SourceSemanticTokenStyle {
    SourceSemanticTokenStyle {
        role: SourceSemanticTokenRole::SavedRoot,
        modifiers: SourceSemanticTokenModifiers {
            modification: true,
            ..Default::default()
        },
    }
}

fn fact(span: SourceSpan, style: SourceSemanticTokenStyle) -> SourceSemanticTokenFact {
    SourceSemanticTokenFact {
        span,
        role: style.role,
        modifiers: style.modifiers,
    }
}

pub(super) fn insert_override(
    overrides: &mut HashMap<ByteSpan, SourceSemanticTokenStyle>,
    token: &Token,
    role: SourceSemanticTokenRole,
) {
    insert_style_override(overrides, token, SourceSemanticTokenStyle::plain(role));
}

pub(super) fn insert_style_override(
    overrides: &mut HashMap<ByteSpan, SourceSemanticTokenStyle>,
    token: &Token,
    style: SourceSemanticTokenStyle,
) {
    overrides.insert(byte_span(token.span), style);
}

pub(super) fn byte_span(span: SourceSpan) -> ByteSpan {
    (span.start_byte, span.end_byte)
}
