//! The Marrow syntax crate: lexing and parsing of `.mw` source into an AST,
//! plus the shared diagnostic surface the rest of the toolchain renders.
//!
//! The crate's surface is the AST (`ast`), the diagnostic types (`diagnostic`),
//! the token model (`token`), and the two entry points `lex_source`/`parse_source`.
//! Everything else (the lexer and the expression/declaration parsers) is an
//! internal carve of one pipeline.

mod ast;
mod diagnostic;
mod format;
mod lexer;
mod parse_decl;
mod parse_expr;
mod token;

pub use ast::*;
pub use diagnostic::*;
pub use format::{format_expression, format_source};
pub use lexer::lex_source;
pub use token::{Keyword, LexedSource, Token, TokenKind, duration_unit_seconds};

use parse_decl::DeclParser;

pub const PARSE_SYNTAX: &str = "parse.syntax";

pub fn parse_source(source: &str) -> ParsedSource {
    let lexed = lex_source(source);
    let mut parsed = DeclParser::new(source, &lexed.tokens).parse();
    let mut combined = lexed.diagnostics;
    combined.append(&mut parsed.diagnostics);
    combined.sort_by_key(|diagnostic| (diagnostic.span.line, diagnostic.span.start_byte));
    parsed.diagnostics = combined;
    parsed
}

#[cfg(test)]
mod decl_parser_corpus {
    use super::*;

    /// Parsing is a pure function of the source, so a second parse must yield the
    /// identical AST and diagnostics. Running each corpus input through this also
    /// asserts the parser does not panic on it.
    fn assert_deterministic(source: &str) {
        let first = parse_source(source);
        let second = parse_source(source);
        assert_eq!(
            first.file, second.file,
            "AST is not deterministic for {source:?}"
        );
        assert_eq!(
            first.diagnostics, second.diagnostics,
            "diagnostics are not deterministic for {source:?}"
        );
    }

    #[test]
    fn parses_documented_modules() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("docs")
            .join("language");
        let mut entries = std::fs::read_dir(&dir)
            .expect("read docs/language")
            .map(|entry| entry.expect("language doc entry").path())
            .collect::<Vec<_>>();
        entries.sort();
        let mut module_blocks = 0usize;
        for path in entries {
            if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read language doc");
            let mut in_block = false;
            let mut source = String::new();
            for line in text.lines() {
                if line.trim() == "```mw" {
                    in_block = true;
                    source.clear();
                    continue;
                }
                if line.trim() == "```" && in_block {
                    if source.trim_start().starts_with("module ") {
                        module_blocks += 1;
                        assert_deterministic(&source);
                    }
                    in_block = false;
                    continue;
                }
                if in_block {
                    source.push_str(line);
                    source.push('\n');
                }
            }
        }
        assert!(
            module_blocks >= 5,
            "expected several documented module files, found {module_blocks}"
        );
    }

    #[test]
    fn parses_edge_cases_deterministically() {
        let cases = [
            // module / use
            "module app\n",
            "module shelf::sample\n",
            "module app\nmodule again\n",
            "module 1bad\n",
            "module\n",
            "use std::math\nuse other\n",
            "use 1bad\n",
            "module app\nuse a::b\nconst X: int = 5\n",
            // const, including multi-line and the unparsed/value paths
            "const Max: int = 5\n",
            "const Default = SomeName\n",
            "const Pi2: decimal = std::math::PI\n",
            "const Total: int = 60 * 60\n",
            "const Bad = int\n",
            "const Bad = @nope\n",
            "const Bad: bool = a = b = c\n",
            "const X = some::call(\n  a: 1,\n  b: 2,\n)\n",
            "const X\n",
            "const X: =\n",
            "const X: notatype = 5\n",
            "const 1: int = 5\n",
            // resources, groups, indexes, @id, keyed roots
            "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n",
            "resource Tag\n    name: string\n",
            "resource Book at ^books\n    @id(\"book.title\")\n    title: string\n    notes(noteId: string)\n        text: string\n    index byShelf(shelf, id)\n    index uniq(id) unique\n",
            "resource Book at ^books()\n    title: string\n",
            "resource Book at ^books\n",
            "resource\n    title: string\n",
            "resource Book at books\n    title: string\n",
            "resource Book at ^books\n    required missing\n",
            "resource Book at ^books\n    name: string\n        nested: int\n",
            "resource Book at ^books\n    @id(nope)\n    title: string\n",
            // functions and parameters
            "pub fn add(a: int, b: int): int\n    return a\n",
            "fn run()\n    return\n",
            "internal fn main()\n    return\n",
            "private fn main()\n    return\n",
            "fn f<T>(x: T)\n    return\n",
            "fn f(x: int = 5)\n    return\n",
            "fn main(value:)\n    return\n",
            "pub fn empty()\n",
            "fn weird(out a: int, inout b: string)\n    return\n",
            // top-level dispatch errors and stray indentation
            "type Foo = int\n",
            "wat\n",
            "    indented\n",
            "module app\n;; a doc comment\nfn main()\n    return\n",
            ";; leading docs\nresource Tag\n    name: string\n",
            // statement bodies that exercise StmtParser delegation
            "fn main()\n    foo +\n",
            "fn main()\n    const x: int\n",
            "fn touch(id: int)\n    ^events(id).at = now\n",
            "fn run()\n    log(level: 1, 2)\n",
            "fn classify(n: int)\n    if n < 0\n        return\n    else if n > 0\n        return\n    else\n        return\n",
            // interleaved blank lines and doc comments inside a resource body
            "resource Book at ^books\n    ;; a field\n    @id(\"book.title\")\n    required title: string\n\n    @id(\"book.author\")\n    required author: string\n",
            // trailing blank lines inside a function body before the next decl
            "fn a()\n    return\n\nfn b()\n    return\n",
            "fn a()\n    return\n\n\npub fn b(x: int)\n    return x\n",
            // empty and whitespace-only inputs
            "",
            "\n\n",
            ";; just docs\n",
        ];
        for source in cases {
            assert_deterministic(source);
        }
    }

    /// `const NAME (: type)? = expr` parses its value by reusing the expression
    /// parser. This pins the value path's AST: a structured expression when the
    /// grammar covers it, and a syntax error with no value when it does not.
    #[test]
    fn const_value_reuses_the_expression_parser() {
        let ParsedSource { file, diagnostics } = parse_source("const Total: int = 60 * 60\n");
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let Some(Declaration::Const(decl)) = file.declarations.first() else {
            panic!("expected a const declaration: {file:#?}");
        };
        assert_eq!(decl.name, "Total");
        assert_eq!(decl.ty.as_ref().map(|ty| ty.text.as_str()), Some("int"));
        assert!(
            matches!(
                decl.value,
                Some(Expression::Binary {
                    op: BinaryOp::Multiply,
                    ..
                })
            ),
            "expected a multiply expression: {:#?}",
            decl.value
        );

        // A bare type name is not an expression, so it is a syntax error and
        // carries no value rather than being silently accepted.
        let ParsedSource { file, diagnostics } = parse_source("const Bad = int\n");
        assert!(
            diagnostics.iter().any(|d| d.code == PARSE_SYNTAX),
            "expected a parse error for a type in value position: {diagnostics:#?}"
        );
        let Some(Declaration::Const(decl)) = file.declarations.first() else {
            panic!("expected a const declaration: {file:#?}");
        };
        assert!(
            decl.value.is_none(),
            "expected no value for `const Bad = int`: {:#?}",
            decl.value
        );
    }
}
