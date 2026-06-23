//! The Marrow syntax crate: lexing and parsing of `.mw` source into an AST,
//! plus the shared diagnostic surface the rest of the toolchain renders.
//!
//! The crate's surface is the AST (`ast`), the diagnostic types (`diagnostic`),
//! the token model (`token`), the canonical string- and bytes-literal decoders (`literal`),
//! and the public entry points `lex_source`/`parse_source`/`parse_expression`.
//! Everything else (the lexer and the expression/declaration parsers) is an
//! internal carve of one pipeline.

mod active_call;
mod ast;
mod diagnostic;
mod format;
mod lexer;
mod literal;
mod parse_decl;
mod parse_expr;
mod token;

pub use active_call::{
    ActiveCallableContext, CallableCalleeContext, active_callable_context, callable_callee_contexts,
};
pub use ast::{
    Argument, BinaryOp, Block, CatchClause, Comment, CommentMarker, CommentPlacement, ConstDecl,
    Declaration, ElseIf, EnumDecl, EnumMember, EvolveDecl, EvolveStep, Expression, FieldDecl,
    ForBinding, FunctionDecl, FunctionReturnPresence, GroupDecl, IndexDecl, InterpolationPart,
    KeyParam, LiteralKind, MatchArm, ModuleDecl, ParamDecl, ParsedSource, RangeExpr, ResourceDecl,
    ResourceMember, SavedRoot, SourceFile, Statement, StoreDecl, SurfaceDecl, SurfaceItem,
    SurfaceTarget, TypeRef, UnaryOp, UseDecl, range_expr,
};
pub use diagnostic::{
    Diagnose, Diagnostic, DiagnosticReason, ExpectedSyntax, LexerDiagnosticReason,
    ObsoleteOperator, ParseDiagnosticReason, ReservedSyntax, Severity, SourceSpan,
    UnsupportedSyntax, kind_for_code,
};
pub use format::{
    format_declaration, format_expression, format_preserves_comments, format_source,
    format_transform_body, strip_layout_blanks,
};
pub use lexer::lex_source;
pub use literal::{
    BytesLiteralError, StringLiteralError, decode_bytes_escapes, decode_bytes_literal,
    decode_string_escapes, decode_string_literal, encode_string_literal, push_string_escapes,
};
pub use token::{
    Keyword, LexedSource, Token, TokenKind, duration_unit_seconds, is_expression_callable_keyword,
    is_expression_path_segment_keyword,
};

use parse_decl::DeclParser;

pub const PARSE_SYNTAX: &str = "parse.syntax";

/// The maximum nesting depth the front end will structure before it stops and
/// reports [`NESTING_LIMIT`]. The lexer enforces it for layout blocks (`if`,
/// resource groups, enum members, …) by capping the indent stack, so the token
/// stream — and the AST and every later walk over it — stays bounded no matter
/// how deep the source nests; the expression parser enforces it for token-level
/// nesting (parentheses, unary and binary operands) on a single line. Deeper
/// source fails closed with a located diagnostic rather than overflowing the
/// native stack. 256 follows the Clang/rustc convention; it is fixed in v0.1,
/// not configurable.
pub const NESTING_DEPTH_LIMIT: usize = 256;

/// Reported when source nests deeper than [`NESTING_DEPTH_LIMIT`]. It renders as
/// a `check`-kind diagnostic so it surfaces alongside the type-check findings the
/// operator already reads, even though the front end raises it.
pub const NESTING_LIMIT: &str = "check.nesting_limit";

pub fn is_reserved_word(text: &str) -> bool {
    token::keyword(text).is_some()
}

pub fn parse_source(source: &str) -> ParsedSource {
    let lexed = lex_source(source);
    let mut parsed = DeclParser::new(source, &lexed.tokens).parse();
    let mut combined = lexed.diagnostics;
    combined.append(&mut parsed.diagnostics);
    combined.sort_by_key(|diagnostic| (diagnostic.span.line, diagnostic.span.start_byte));
    parsed.diagnostics = combined;
    parsed
}

pub fn parse_expression(source: &str) -> (Option<Expression>, Vec<Diagnostic>) {
    let lexed = lex_source(source);
    let mut diagnostics = lexed.diagnostics;
    let expression =
        parse_expr::ExprParser::new(source, &lexed.tokens).parse_complete(&mut diagnostics);
    if expression.is_none() && diagnostics.is_empty() {
        diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            reason: DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                ExpectedSyntax::Expression,
            )),
            severity: Severity::Error,
            message: "expected an expression".to_string(),
            help: None,
            span: SourceSpan::default(),
        });
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.span.line, diagnostic.span.start_byte));
    (expression, diagnostics)
}

#[cfg(test)]
mod decl_parser_corpus {
    use super::{BinaryOp, Declaration, Expression, PARSE_SYNTAX, ParsedSource, parse_source};

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
            // resources, groups, indexes, keyed roots
            "resource Book\n    required title: string\n    tags(pos: int): string\nstore ^books(id: int): Book\n",
            "resource Tag\n    name: string\n",
            "resource Book\n    title: string\n    notes(noteId: string)\n        text: string\nstore ^books: Book\n    index byShelf(shelf, id)\n    index uniq(id) unique\n",
            "resource Book\n    title: string\nstore ^books(): Book\n",
            "resource Book\nstore ^books: Book\n",
            "resource\n    title: string\n",
            "resource Book extra\n    title: string\n",
            "resource Book\n    required missing\nstore ^books: Book\n",
            "resource Book\n    name: string\n        nested: int\nstore ^books: Book\n",
            // functions and parameters
            "pub fn add(a: int, b: int): int\n    return a\n",
            "fn run()\n    return\n",
            "internal fn main()\n    return\n",
            "private fn main()\n    return\n",
            "fn f<T>(x: T)\n    return\n",
            "fn f(x: int = 5)\n    return\n",
            "fn main(value:)\n    return\n",
            "pub fn empty()\n",
            "fn weird(value:)\n    return\n",
            // top-level dispatch errors and stray indentation
            "type Foo = int\n",
            "wat\n",
            "    indented\n",
            "module app\n;; a doc comment\nfn main()\n    return\n",
            ";; leading docs\nresource Tag\n    name: string\n",
            // statement bodies that exercise StmtParser delegation
            "fn main()\n    foo +\n",
            "fn main()\n    const x: int\n",
            "fn touch(id: int)\n    ^events(id).status = now\n",
            "fn run()\n    log(level: 1, 2)\n",
            "fn classify(n: int)\n    if n < 0\n        return\n    else if n > 0\n        return\n    else\n        return\n",
            // interleaved blank lines and doc comments inside a resource body
            "resource Book\n    ;; a field\n    required title: string\n\n    required author: string\nstore ^books: Book\n",
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

#[cfg(test)]
mod nesting_limit {
    use super::{NESTING_DEPTH_LIMIT, NESTING_LIMIT, ParsedSource, parse_source};

    /// Parse on a worker thread with the same generous stack the CLI runs the
    /// parser on, so the nesting limit — calibrated for that stack — trips before
    /// the recursion overflows the small default test-thread stack.
    fn parse_on_large_stack(source: String) -> ParsedSource {
        std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(move || parse_source(&source))
            .expect("spawn parse worker")
            .join()
            .expect("parse worker did not panic")
    }

    fn codes(source: String) -> Vec<&'static str> {
        parse_on_large_stack(source)
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    /// A source with `depth` nested `if` blocks, the deep-statement form. Each
    /// level indents one more level and holds the next `if`.
    fn nested_ifs(depth: usize) -> String {
        let mut source = String::from("module app\n\npub fn main()\n");
        for level in 0..depth {
            let indent = "    ".repeat(level + 1);
            source.push_str(&format!("{indent}if {level} < {}\n", level + 1));
        }
        source.push_str(&"    ".repeat(depth + 1));
        source.push_str("return\n");
        source
    }

    /// A source returning `depth` nested parentheses, the deep-expression form.
    fn nested_parens(depth: usize) -> String {
        let expr = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
        format!("module app\n\npub fn main()\n    return {expr}\n")
    }

    /// A flat left-associated `1 op 1 op …` chain of `depth` operators. The AST
    /// nests as deep as the chain is long even though the source is one wide line,
    /// so it must be counted toward the same nesting limit as parentheses.
    fn flat_operator_chain(op: &str, depth: usize) -> String {
        let chain = vec!["1"; depth + 1].join(&format!(" {op} "));
        format!("module app\n\npub fn main()\n    return {chain}\n")
    }

    /// A flat field-access chain `a.f.f.…` of `depth` segments. Each `.f` deepens
    /// the AST by one, so it must be counted like a parenthesis.
    fn field_access_chain(depth: usize) -> String {
        let chain = format!("a{}", ".f".repeat(depth));
        format!("module app\n\npub fn main()\n    return {chain}\n")
    }

    /// `depth` enum members each nested under the previous one by indentation.
    fn nested_enum_members(depth: usize) -> String {
        let mut source = String::from("module app\n\nenum E\n");
        for level in 0..depth {
            source.push_str(&"    ".repeat(level + 1));
            source.push_str(&format!("m{level}\n"));
        }
        source
    }

    /// `depth` resource groups each nested under the previous one, with a leaf
    /// field at the bottom so the innermost group has a body.
    fn nested_resource_groups(depth: usize) -> String {
        let mut source = String::from("module app\n\nresource R\n");
        for level in 0..depth {
            source.push_str(&"    ".repeat(level + 1));
            source.push_str(&format!("g{level}(k: int)\n"));
        }
        source.push_str(&"    ".repeat(depth + 1));
        source.push_str("leaf: int\n");
        source
    }

    fn located_nesting_limit(source: String) -> super::Diagnostic {
        parse_on_large_stack(source)
            .diagnostics
            .into_iter()
            .find(|diagnostic| diagnostic.code == NESTING_LIMIT)
            .expect("a located nesting-limit diagnostic")
    }

    fn nesting_limit_count(source: &str) -> usize {
        parse_on_large_stack(source.to_string())
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == NESTING_LIMIT)
            .count()
    }

    /// Layout nesting is capped in the lexer, so however deep the source nests,
    /// the token stream the parser and every later walk see stays bounded by the
    /// limit. Without this bound a deep nest would materialize a token, AST node,
    /// and walk frame per level — the unbounded front-end work the depth limit
    /// exists to prevent. A 50x-deeper nest must produce essentially the same
    /// token count, not 50x more.
    #[test]
    fn over_deep_layout_yields_a_bounded_token_stream() {
        for build in [
            nested_resource_groups as fn(usize) -> String,
            nested_enum_members,
            nested_ifs,
        ] {
            let shallow = super::lex_source(&build(NESTING_DEPTH_LIMIT * 4))
                .tokens
                .len();
            let deep = super::lex_source(&build(NESTING_DEPTH_LIMIT * 50))
                .tokens
                .len();
            // The over-deep tail contributes no content tokens, so going 12.5x
            // deeper leaves the stream within a few framing tokens of the limit
            // rather than scaling with depth.
            assert!(
                deep <= shallow + 8,
                "a 50x-deeper nest must not grow the token stream with depth \
                 (4x-limit={shallow}, 50x-limit={deep})"
            );
        }
    }

    /// A whole over-deep region trips the limit once, not once per line, so a
    /// deeply nested file yields a single diagnostic rather than a flood.
    #[test]
    fn over_deep_layout_reports_the_limit_once() {
        assert_eq!(
            nesting_limit_count(&nested_resource_groups(NESTING_DEPTH_LIMIT * 4)),
            1
        );
        assert_eq!(
            nesting_limit_count(&nested_enum_members(NESTING_DEPTH_LIMIT * 4)),
            1
        );
        assert_eq!(nesting_limit_count(&nested_ifs(NESTING_DEPTH_LIMIT * 4)), 1);
    }

    /// Two independent over-deep enums each report their own overflow: leaving
    /// the first over-deep region re-arms the once-per-region diagnostic.
    #[test]
    fn separate_over_deep_regions_each_report() {
        let nest = |name: &str| {
            let mut source = format!("enum {name}\n");
            for level in 0..(NESTING_DEPTH_LIMIT * 2) {
                source.push_str(&"    ".repeat(level + 1));
                source.push_str(&format!("m{level}\n"));
            }
            source
        };
        let source = format!("module app\n\n{}\n{}", nest("A"), nest("B"));
        assert_eq!(nesting_limit_count(&source), 2);
    }

    /// Sibling members or statements at the same depth are not nesting; an
    /// arbitrarily wide body must never trip the layout limit.
    #[test]
    fn wide_bodies_do_not_trip_the_limit() {
        let mut wide_resource = String::from("module app\n\nresource R\n");
        for index in 0..(NESTING_DEPTH_LIMIT * 10) {
            wide_resource.push_str(&format!("    f{index}: int\n"));
        }
        assert_eq!(nesting_limit_count(&wide_resource), 0);
    }

    #[test]
    fn deep_flat_operator_chains_report_the_nesting_limit() {
        for op in ["+", "*", "and", "or"] {
            let located = located_nesting_limit(flat_operator_chain(op, NESTING_DEPTH_LIMIT + 50));
            assert!(
                located.span.line > 0,
                "the diagnostic for a deep `{op}` chain is located: {located:?}"
            );
        }
    }

    #[test]
    fn deep_field_access_chains_report_the_nesting_limit() {
        let located = located_nesting_limit(field_access_chain(NESTING_DEPTH_LIMIT + 50));
        assert!(
            located.span.line > 0,
            "the diagnostic for a deep field-access chain is located: {located:?}"
        );
    }

    #[test]
    fn deeply_nested_enum_members_report_the_nesting_limit() {
        let located = located_nesting_limit(nested_enum_members(NESTING_DEPTH_LIMIT + 50));
        assert!(
            located.span.line > 0,
            "the diagnostic for deep enum nesting is located: {located:?}"
        );
    }

    #[test]
    fn deeply_nested_resource_groups_report_the_nesting_limit() {
        let located = located_nesting_limit(nested_resource_groups(NESTING_DEPTH_LIMIT + 50));
        assert!(
            located.span.line > 0,
            "the diagnostic for deep resource-group nesting is located: {located:?}"
        );
    }

    #[test]
    fn deeply_nested_statements_report_the_nesting_limit() {
        let located = parse_on_large_stack(nested_ifs(NESTING_DEPTH_LIMIT + 50))
            .diagnostics
            .into_iter()
            .find(|diagnostic| diagnostic.code == NESTING_LIMIT)
            .expect("a nesting-limit diagnostic for deep `if` nesting");
        assert!(
            located.span.line > 0,
            "the diagnostic is located: {located:?}"
        );
    }

    #[test]
    fn deeply_nested_expressions_report_the_nesting_limit() {
        let located = parse_on_large_stack(nested_parens(NESTING_DEPTH_LIMIT + 50))
            .diagnostics
            .into_iter()
            .find(|diagnostic| diagnostic.code == NESTING_LIMIT)
            .expect("a nesting-limit diagnostic for deep parens");
        assert!(
            located.span.line > 0,
            "the diagnostic is located: {located:?}"
        );
    }

    #[test]
    fn nesting_just_under_the_limit_parses_clean() {
        assert!(
            !codes(nested_ifs(NESTING_DEPTH_LIMIT - 1)).contains(&NESTING_LIMIT),
            "statements just under the limit should parse without the nesting error"
        );
        assert!(
            !codes(nested_parens(NESTING_DEPTH_LIMIT - 1)).contains(&NESTING_LIMIT),
            "expressions just under the limit should parse without the nesting error"
        );
        for op in ["+", "*", "and", "or"] {
            assert!(
                !codes(flat_operator_chain(op, NESTING_DEPTH_LIMIT - 1)).contains(&NESTING_LIMIT),
                "a `{op}` chain just under the limit should parse without the nesting error"
            );
        }
        assert!(
            !codes(field_access_chain(NESTING_DEPTH_LIMIT - 1)).contains(&NESTING_LIMIT),
            "a field-access chain just under the limit should parse without the nesting error"
        );
        assert!(
            !codes(nested_enum_members(NESTING_DEPTH_LIMIT - 1)).contains(&NESTING_LIMIT),
            "enum nesting just under the limit should parse without the nesting error"
        );
        assert!(
            !codes(nested_resource_groups(NESTING_DEPTH_LIMIT - 1)).contains(&NESTING_LIMIT),
            "resource-group nesting just under the limit should parse without the nesting error"
        );
    }
}
