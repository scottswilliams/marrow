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
    AliasDecl, Argument, ArmBinding, BinaryOp, Block, CheckedBind, Comment, CommentMarker,
    CommentPlacement, CompoundAssignOp, ConstDecl, Declaration, ElseIf, EnumDecl, EnumMember,
    EnumPayloadField, Expression, FieldDecl, ForBinding, ForName, FunctionDecl, GroupDecl,
    IdentityTypeExpr, IfConstBinding, IndexDecl, InterpolationPart, KeyParam, LiteralKind,
    LoopOrder, MatchArm, ModuleDecl, NominalDecl, ParamDecl, ParsedSource, RangeExpr, ResourceDecl,
    ResourceMember, SavedRoot, SourceFile, Statement, StoreDecl, StructDecl, SupportSpelling,
    TestDecl, TraversalBound, TypeConstraint, TypeExpr, TypeParamDecl, UnaryOp, UseDecl,
    range_expr,
};
pub use diagnostic::{
    Diagnose, Diagnostic, DiagnosticReason, ExpectedSyntax, LexerDiagnosticReason,
    ObsoleteOperator, ParseDiagnosticReason, ReservedSyntax, Severity, SourceSpan,
    UnsupportedSyntax,
};
pub use format::{
    FormatRefusal, check_format, format_declaration, format_expression, format_preserves_comments,
    format_source,
};
pub use lexer::lex_source;
pub use literal::{
    BytesLiteralError, StringLiteralError, decode_bytes_escapes, decode_bytes_literal,
    decode_interpolation_text, decode_string_escapes, decode_string_literal, encode_string_literal,
    push_string_escapes,
};
use marrow_codes::Code;
pub use marrow_codes::kind_for_code;
pub use token::{
    Keyword, LexedSource, Token, TokenKind, duration_unit_forms, duration_unit_seconds,
    is_expression_callable_keyword, is_expression_path_segment_keyword, is_unfixed_duration_unit,
};

use parse_decl::DeclParser;

pub const PARSE_SYNTAX: &str = Code::ParseSyntax.as_str();

/// The maximum nesting depth the front end will structure before it stops and
/// reports [`NESTING_LIMIT`]. Two layers enforce it for `{ … }` blocks (`if`,
/// resource groups, enum members, …): the lexer reports the located finding when
/// the brace depth first exceeds the limit, and the recursive-descent parser skips
/// an over-deep block rather than descending into it, so the AST — and every later
/// walk over it — stays bounded no matter how deep the source nests. The
/// expression parser enforces it for token-level nesting (parentheses, unary and
/// binary operands) on a single line. Deeper source fails closed with a located
/// diagnostic rather than overflowing the native stack. 256 follows the
/// Clang/rustc convention; it is fixed in v0.1, not configurable.
pub const NESTING_DEPTH_LIMIT: usize = 256;

/// Reported when source nests deeper than [`NESTING_DEPTH_LIMIT`]. It renders as
/// a `check`-kind diagnostic so it surfaces alongside the type-check findings the
/// operator already reads, even though the front end raises it.
pub const NESTING_LIMIT: &str = Code::CheckNestingLimit.as_str();

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
    let gap = lexed
        .tokens
        .first()
        .map_or_else(SourceSpan::default, |token| token.span);
    let expression = match parse_expr::ExprParser::new(source, &lexed.tokens, gap)
        .parse_complete(&mut diagnostics)
    {
        parse_expr::ParseComplete::Complete(expr) => Some(expr),
        parse_expr::ParseComplete::Reported => None,
        parse_expr::ParseComplete::Incomplete(span) => {
            diagnostics.push(Diagnostic {
                code: PARSE_SYNTAX,
                reason: DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::Expression,
                )),
                severity: Severity::Error,
                message: "expected an expression".to_string(),
                help: None,
                span,
            });
            None
        }
    };
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
        assert_eq!(
            decl.ty.as_ref().map(ToString::to_string).as_deref(),
            Some("int")
        );
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

        // A bare type name is not an expression, so it is a syntax error. Total
        // parsing keeps the written value as an error node rather than dropping it.
        let ParsedSource { file, diagnostics } = parse_source("const Bad = int\n");
        assert!(
            diagnostics.iter().any(|d| d.code == PARSE_SYNTAX),
            "expected a parse error for a type in value position: {diagnostics:#?}"
        );
        let Some(Declaration::Const(decl)) = file.declarations.first() else {
            panic!("expected a const declaration: {file:#?}");
        };
        assert!(
            matches!(decl.value, Some(Expression::Error { .. })),
            "expected an error-node value for `const Bad = int`: {:#?}",
            decl.value
        );
    }

    /// A statement whose value expression is missing — a trailing `=`, statement
    /// keyword, or operator inside a function body — reports the missing operand
    /// at the gap just past the token that introduced it, not the generic
    /// "expected a statement" anchored at the keyword that legitimately starts
    /// the statement.
    #[test]
    fn missing_operand_reports_an_expression_gap_at_the_token() {
        use super::{DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason};

        // Each case names the byte just past the `=`/operator/keyword that the
        // operand should have followed.
        for (source, gap_byte) in [
            (
                "fn f() {\n    const x: int =\n}\n",
                "fn f() {\n    const x: int =".len(),
            ),
            (
                "fn f() {\n    return 1 +\n}\n",
                "fn f() {\n    return 1 +".len(),
            ),
            ("fn f() {\n    delete\n}\n", "fn f() {\n    delete".len()),
            ("fn f() {\n    x =\n}\n", "fn f() {\n    x =".len()),
            (
                "fn f() {\n    if const x = {\n        x\n    }\n}\n",
                "fn f() {\n    if const x =".len(),
            ),
            (
                "fn f() {\n    if {\n        x\n    }\n}\n",
                "fn f() {\n    if".len(),
            ),
            (
                "fn f() {\n    while {\n        x\n    }\n}\n",
                "fn f() {\n    while".len(),
            ),
            (
                "fn f() {\n    match {\n        a => { x }\n    }\n}\n",
                "fn f() {\n    match".len(),
            ),
            (
                "fn f() {\n    if true {\n        x\n    } else if {\n        x\n    }\n}\n",
                "fn f() {\n    if true {\n        x\n    } else if".len(),
            ),
        ] {
            let ParsedSource { diagnostics, .. } = parse_source(source);
            let expression = diagnostics
                .iter()
                .find(|d| {
                    d.reason
                        == DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                            ExpectedSyntax::Expression,
                        ))
                })
                .unwrap_or_else(|| {
                    panic!("expected an expression-gap diagnostic for {source:?}: {diagnostics:#?}")
                });
            assert_eq!(expression.code, PARSE_SYNTAX);
            assert_eq!(
                expression.span.start_byte, gap_byte,
                "gap span should sit just past the introducing token in {source:?}: {diagnostics:#?}"
            );
            assert!(
                expression.message.contains("expected an expression"),
                "{:?}",
                expression.message
            );
            assert!(
                !diagnostics
                    .iter()
                    .any(|d| d.message.contains("expected a statement")),
                "the generic statement fallback must be suppressed for {source:?}: {diagnostics:#?}"
            );
        }
    }

    /// An empty `match` scrutinee reports the missing expression exactly once at
    /// the gap past `match`, with no spurious second diagnostic against the first
    /// arm header: the arms still parse cleanly as member paths.
    #[test]
    fn empty_match_scrutinee_reports_one_expression_gap() {
        use super::{DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason};

        let source = "fn f() {\n    match {\n        a => { x }\n    }\n}\n";
        let ParsedSource { diagnostics, .. } = parse_source(source);
        let gaps: Vec<_> = diagnostics
            .iter()
            .filter(|d| {
                d.reason
                    == DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                        ExpectedSyntax::Expression,
                    ))
            })
            .collect();
        assert_eq!(
            gaps.len(),
            1,
            "exactly one expression gap for an empty match scrutinee: {diagnostics:#?}"
        );
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.message.contains("match arm is a member path")),
            "the arm `a` must parse cleanly, with no spurious arm diagnostic: {diagnostics:#?}"
        );
    }

    /// An empty assignment target reports the missing expression just before the
    /// `=`, at a valid source position rather than the start of input.
    #[test]
    fn empty_assignment_target_reports_a_gap_before_the_equals() {
        use super::{DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason};

        let source = "fn f() {\n    = 5\n}\n";
        let ParsedSource { diagnostics, .. } = parse_source(source);
        let expression = diagnostics
            .iter()
            .find(|d| {
                d.reason
                    == DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                        ExpectedSyntax::Expression,
                    ))
            })
            .unwrap_or_else(|| panic!("expected an expression-gap diagnostic: {diagnostics:#?}"));
        assert_eq!(expression.span.start_byte, "fn f() {\n    ".len());
        assert!(expression.message.contains("expected an expression"));
    }

    /// A malformed `for` header — an empty iterable or step — reports a single
    /// header diagnostic at a valid position; the inner missing operand is owned
    /// by that header, so it never adds a duplicate expression gap.
    #[test]
    fn empty_for_operand_reports_a_single_header_diagnostic() {
        for source in [
            "fn f() {\n    for x in {\n        x\n    }\n}\n",
            "fn f() {\n    for x in 1..2 by {\n        x\n    }\n}\n",
        ] {
            let ParsedSource { diagnostics, .. } = parse_source(source);
            let header: Vec<_> = diagnostics
                .iter()
                .filter(|d| d.message.contains("expected `for <binding> in <iterable>`"))
                .collect();
            assert_eq!(
                header.len(),
                1,
                "exactly one for-header diagnostic for {source:?}: {diagnostics:#?}"
            );
            assert!(
                !diagnostics
                    .iter()
                    .any(|d| d.message.contains("expected an expression")),
                "the for header owns recovery; no separate expression gap for {source:?}: {diagnostics:#?}"
            );
        }
    }

    /// An unterminated `(` or a call argument list missing its `,`/`)`, with a
    /// complete operand present, names the missing delimiter at the gap just past
    /// that operand — never the generic "expected a statement" at the keyword, and
    /// never a line-0/column-0 span.
    #[test]
    fn unclosed_delimiter_with_a_complete_operand_names_the_delimiter() {
        // Each case names the missing-delimiter substring and the byte the gap
        // should sit at, just past the complete operand.
        for (source, expected, gap_byte) in [
            (
                "fn f() {\n    return (1\n}\n",
                "expected `)`",
                "fn f() {\n    return (1".len(),
            ),
            (
                "fn f() {\n    return g(1\n}\n",
                "expected `)`",
                "fn f() {\n    return g(1".len(),
            ),
            (
                "fn f() {\n    return g(1 2)\n}\n",
                "expected `,`",
                "fn f() {\n    return g(1".len(),
            ),
        ] {
            let ParsedSource { diagnostics, .. } = parse_source(source);
            let delimiter = diagnostics
                .iter()
                .find(|d| d.message.contains(expected))
                .unwrap_or_else(|| {
                    panic!("expected a {expected:?} diagnostic for {source:?}: {diagnostics:#?}")
                });
            assert_eq!(delimiter.code, PARSE_SYNTAX);
            assert_eq!(
                delimiter.span.start_byte, gap_byte,
                "delimiter gap should sit just past the operand in {source:?}: {diagnostics:#?}"
            );
            assert!(
                delimiter.span.line >= 1 && delimiter.span.column >= 1,
                "valid 1-based span for {source:?}: {delimiter:#?}"
            );
            assert!(
                !diagnostics
                    .iter()
                    .any(|d| d.message.contains("expected a statement")),
                "the generic statement fallback must be suppressed for {source:?}: {diagnostics:#?}"
            );
        }
    }

    /// A `for` iterable with an unterminated `(` keeps its single header
    /// diagnostic and does not also emit a separate close-delimiter gap: the
    /// header owns recovery for everything inside it.
    #[test]
    fn for_header_unclosed_paren_reports_only_the_header_diagnostic() {
        let source = "fn f() {\n    for x in (1 {\n        x\n    }\n}\n";
        let ParsedSource { diagnostics, .. } = parse_source(source);
        let header: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.message.contains("expected `for <binding> in <iterable>`"))
            .collect();
        assert_eq!(
            header.len(),
            1,
            "exactly one for-header diagnostic for {source:?}: {diagnostics:#?}"
        );
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.message.contains("expected `)`")),
            "the for header owns recovery; no separate close-delimiter gap for {source:?}: {diagnostics:#?}"
        );
    }

    /// No empty-operand or empty-header statement form may report a diagnostic
    /// at line 0 or column 0 — those are never valid source positions (lines and
    /// columns are 1-based). Every expression-taking statement form is
    /// enumerated: the const/var RHS, return/throw/delete value, assignment RHS
    /// and LHS, the empty `for` header along with its empty iterable and step,
    /// and the `if`/`else if`/`while`/`match` header expressions. The empty
    /// header forms anchor on the consumed keyword span, so an absent operand
    /// never falls back to the line-0/column-0 default.
    #[test]
    fn no_empty_operand_form_reports_a_line_or_column_zero_span() {
        for source in [
            "fn f()\n    const x: int =\n",
            "fn f()\n    var x: int =\n",
            "fn f()\n    return 1 +\n",
            "fn f()\n    throw\n",
            "fn f()\n    delete\n",
            "fn f()\n    x =\n",
            "fn f()\n    = 5\n",
            "fn f()\n    for\n        x\n",
            "fn f()\n    for x in\n        x\n",
            "fn f()\n    for x in 1..2 by\n        x\n",
            "fn f()\n    if\n        x\n",
            "fn f()\n    if const x =\n        x\n",
            "fn f()\n    if true\n        x\n    else if\n        x\n",
            "fn f()\n    while\n        x\n",
            "fn f()\n    match\n        a\n            x\n",
        ] {
            let ParsedSource { diagnostics, .. } = parse_source(source);
            assert!(
                !diagnostics.is_empty(),
                "expected at least one diagnostic for {source:?}"
            );
            for diagnostic in &diagnostics {
                assert!(
                    diagnostic.span.line >= 1 && diagnostic.span.column >= 1,
                    "diagnostic at line {} column {} for {source:?}: {diagnostic:#?}",
                    diagnostic.span.line,
                    diagnostic.span.column
                );
            }
        }
    }

    /// A line the parser cannot begin as an expression reports one diagnostic at
    /// the offending token: `*` cannot start an expression, so it reports there.
    #[test]
    fn a_non_statement_line_reports_at_the_failure_token() {
        let ParsedSource { diagnostics, .. } = parse_source("fn f() {\n    * nope\n}\n");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert!(
            diagnostics[0].message.contains("expected an expression"),
            "{diagnostics:#?}"
        );
        assert_eq!(
            (diagnostics[0].span.line, diagnostics[0].span.column),
            (2, 5),
            "{diagnostics:#?}"
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
    /// level opens one more brace and holds the next `if`.
    fn nested_ifs(depth: usize) -> String {
        let mut source = String::from("module app\n\npub fn main() {\n");
        for level in 0..depth {
            source.push_str(&format!("if {level} < {} {{\n", level + 1));
        }
        source.push_str("return\n");
        for _ in 0..depth {
            source.push_str("}\n");
        }
        source.push_str("}\n");
        source
    }

    /// A source returning `depth` nested parentheses, the deep-expression form.
    fn nested_parens(depth: usize) -> String {
        let expr = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
        format!("module app\n\npub fn main() {{\n    return {expr}\n}}\n")
    }

    /// A flat left-associated `1 op 1 op …` chain of `depth` operators. The AST
    /// nests as deep as the chain is long even though the source is one wide line,
    /// so it must be counted toward the same nesting limit as parentheses.
    fn flat_operator_chain(op: &str, depth: usize) -> String {
        let chain = vec!["1"; depth + 1].join(&format!(" {op} "));
        format!("module app\n\npub fn main() {{\n    return {chain}\n}}\n")
    }

    /// A flat field-access chain `a.f.f.…` of `depth` segments. Each `.f` deepens
    /// the AST by one, so it must be counted like a parenthesis.
    fn field_access_chain(depth: usize) -> String {
        let chain = format!("a{}", ".f".repeat(depth));
        format!("module app\n\npub fn main() {{\n    return {chain}\n}}\n")
    }

    /// `depth` enum members each nested under the previous one as a category via
    /// braces.
    fn nested_enum_members(depth: usize) -> String {
        let mut source = String::from("module app\n\nenum E {\n");
        for level in 0..depth {
            source.push_str(&format!("m{level} {{\n"));
        }
        for _ in 0..depth {
            source.push_str("}\n");
        }
        source.push_str("}\n");
        source
    }

    /// `depth` resource groups each nested under the previous one via braces, with a
    /// leaf field at the bottom so the innermost group has a body.
    fn nested_resource_groups(depth: usize) -> String {
        let mut source = String::from("module app\n\nresource R {\n");
        for level in 0..depth {
            source.push_str(&format!("g{level}(k: int) {{\n"));
        }
        source.push_str("leaf: int\n");
        for _ in 0..depth {
            source.push_str("}\n");
        }
        source.push_str("}\n");
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

    /// The recursive-descent parser fails closed at the nesting limit: past it a
    /// deep brace nest skips its body rather than recursing, so the AST (and every
    /// later walk over it) stays bounded no matter how deep the braces go. Without
    /// this bound a deep nest would materialize an AST node and walk frame per
    /// level. A 12.5x-deeper nest must yield essentially the same node count.
    #[test]
    fn over_deep_braces_yield_a_bounded_ast() {
        fn statement_nodes(block: &super::Block) -> usize {
            block
                .statements
                .iter()
                .map(|statement| {
                    1 + match statement {
                        super::Statement::If { then_block, .. } => statement_nodes(then_block),
                        super::Statement::While { body, .. }
                        | super::Statement::For { body, .. }
                        | super::Statement::Transaction { body, .. } => statement_nodes(body),
                        _ => 0,
                    }
                })
                .sum()
        }
        let count = |depth: usize| {
            let parsed = parse_on_large_stack(nested_ifs(depth));
            parsed
                .file
                .function("main")
                .map(|function| statement_nodes(&function.body))
                .unwrap_or(0)
        };
        let shallow = count(NESTING_DEPTH_LIMIT * 4);
        let deep = count(NESTING_DEPTH_LIMIT * 50);
        assert_eq!(
            shallow, deep,
            "a deeper brace nest must not grow the AST with depth \
             (4x-limit={shallow}, 50x-limit={deep})"
        );
    }

    /// A whole over-deep region trips the limit once, not once per brace, so a
    /// deeply nested file yields a single diagnostic rather than a flood.
    #[test]
    fn over_deep_braces_report_the_limit_once() {
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
            let mut source = format!("enum {name} {{\n");
            for level in 0..(NESTING_DEPTH_LIMIT * 2) {
                source.push_str(&format!("m{level} {{\n"));
            }
            for _ in 0..(NESTING_DEPTH_LIMIT * 2) {
                source.push_str("}\n");
            }
            source.push_str("}\n");
            source
        };
        let source = format!("module app\n\n{}\n{}", nest("A"), nest("B"));
        assert_eq!(nesting_limit_count(&source), 2);
    }

    /// Sibling members or statements at the same depth are not nesting; an
    /// arbitrarily wide body must never trip the nesting limit.
    #[test]
    fn wide_bodies_do_not_trip_the_limit() {
        let mut wide_resource = String::from("module app\n\nresource R {\n");
        for index in 0..(NESTING_DEPTH_LIMIT * 10) {
            wide_resource.push_str(&format!("    f{index}: int\n"));
        }
        wide_resource.push_str("}\n");
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
