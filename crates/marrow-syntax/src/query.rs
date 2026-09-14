use crate::{Declaration, SourceFile, UseDecl};

/// Syntax for one position query, including all declaration headers and uses.
///
/// Closed function/test bodies outside the position have spans but no statements.
/// This is not a complete parse and cannot supply compilation, formatting or
/// diagnostic results. The position uses inclusive body spans.
pub struct QuerySyntax {
    file: SourceFile,
    offset: usize,
}

impl QuerySyntax {
    /// Parse one file for a byte position. An offset outside the file selects no body.
    pub fn parse(source: &str, offset: usize) -> Self {
        let mut diagnostics = crate::diagnostic::SyntaxDiagnosticCollector::new();
        let tokens = crate::lexer::lex_tokens(source, diagnostics.lexer_sink());
        Self {
            file: crate::parse_decl::DeclParser::new(source, &tokens, diagnostics.parser_sink())
                .for_query(offset)
                .parse(),
            offset,
        }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Declarations for this query; unselected function/test bodies are omitted.
    pub fn declarations(&self) -> &[Declaration] {
        &self.file.declarations
    }

    pub fn uses(&self) -> &[UseDecl] {
        &self.file.uses
    }
}

#[cfg(test)]
std::thread_local! {
    pub(crate) static MATERIALIZED_BODIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_selected_closed_body_is_materialized() {
        let source = "fn first() {\n a\n}\nfn second() {\n b\n}\n";
        let full = crate::parse_source(source).file;
        MATERIALIZED_BODIES.set(0);
        let query = QuerySyntax::parse(source, source.find(" a").expect("selected body") + 1);
        assert_eq!(MATERIALIZED_BODIES.get(), 1);
        assert_eq!(query.declarations().len(), full.declarations.len());
        for (index, (actual, expected)) in query
            .declarations()
            .iter()
            .zip(&full.declarations)
            .enumerate()
        {
            let (Declaration::Function(actual), Declaration::Function(expected)) =
                (actual, expected)
            else {
                panic!("function fixtures");
            };
            assert_eq!(actual.name, expected.name);
            assert_eq!(actual.span, expected.span);
            assert_eq!(actual.body.span, expected.body.span);
            assert_eq!(actual.body.statements.len(), usize::from(index == 0));
        }
    }
}
