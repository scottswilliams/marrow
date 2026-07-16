//! A frozen snapshot of the layout (indentation-based) lexer and declaration
//! parser, as they stood immediately before the brace-surface migration (BS01).
//!
//! This module exists solely as the input side of the corpus converter: the
//! converter parses the old layout `.mw` corpus with this pipeline and re-prints
//! it with the new brace formatter, gated on span-erased AST round-trip equality.
//! It is not part of the production surface and is deleted whole in the converter
//! flip, once the corpus has been migrated. Nothing outside the converter may
//! depend on it.
//!
//! The snapshot shares the crate's AST, token, diagnostic, and literal owners
//! (those are unchanged by the migration); only the lexer and the block-framing
//! declaration parser diverge between the two surfaces, so only those are frozen
//! here. Because the converter is not yet built, the pipeline currently has no
//! caller, so dead-code analysis is silenced for the snapshot as a whole.
#![allow(dead_code)]

mod lexer;
mod parse_decl;

use crate::ast::ParsedSource;

/// Parse layout (indentation-based) source with the frozen pre-migration
/// pipeline, mirroring the production [`crate::parse_source`] contract: lexer
/// diagnostics precede parser diagnostics, and the combined list is sorted by
/// source position.
pub(crate) fn parse_source_layout(source: &str) -> ParsedSource {
    let lexed = lexer::lex_source(source);
    let mut parsed = parse_decl::DeclParser::new(source, &lexed.tokens).parse();
    let mut combined = lexed.diagnostics;
    combined.append(&mut parsed.diagnostics);
    combined.sort_by_key(|diagnostic| (diagnostic.span.line, diagnostic.span.start_byte));
    parsed.diagnostics = combined;
    parsed
}

#[cfg(test)]
mod tests {
    use super::parse_source_layout;
    use crate::ast::Declaration;

    /// The frozen pipeline still parses layout source: an indented function body
    /// with a `;` comment and a `;;` doc comment yields a clean declaration. This
    /// is the converter's guarantee — the old corpus remains parseable — and the
    /// snapshot's single live caller.
    #[test]
    fn parses_layout_source_cleanly() {
        let source = "module app\n\n;; doc\npub fn add(a: int, b: int): int\n    ; a comment\n    return a + b\n";
        let parsed = parse_source_layout(source);
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        assert!(
            parsed
                .file
                .declarations
                .iter()
                .any(|decl| matches!(decl, Declaration::Function(f) if f.name == "add")),
            "expected the layout function to parse: {:#?}",
            parsed.file.declarations
        );
    }
}
