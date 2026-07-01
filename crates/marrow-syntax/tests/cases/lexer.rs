use crate::common;
use marrow_syntax::{
    Diagnose, DiagnosticReason, Keyword, LexedSource, LexerDiagnosticReason, ObsoleteOperator,
    Severity, TokenKind, lex_source,
};
/// Whether the lexed source carries any error-severity diagnostic.
fn has_errors(lexed: &LexedSource) -> bool {
    lexed
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

fn kinds(source: &str) -> Vec<TokenKind> {
    lex_source(source)
        .tokens
        .into_iter()
        .map(|token| token.kind)
        .collect()
}

fn texts(source: &str) -> Vec<String> {
    lex_source(source)
        .tokens
        .into_iter()
        .map(|token| token.text(source).to_string())
        .collect()
}

#[test]
fn lexes_indentation_tokens_for_blocks() {
    let source =
        "module shelf::books\nfn main()\n    const title = \"Small Gods\"\n    print(title)\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Module),
            TokenKind::Identifier,
            TokenKind::DoubleColon,
            TokenKind::Identifier,
            TokenKind::Newline,
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::String,
            TokenKind::Newline,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::Identifier,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );

    let lexed = lex_source(source);
    let title = lexed
        .tokens
        .iter()
        .find(|token| token.text(source) == "title")
        .expect("title token");
    assert_eq!(title.span.line, 3);
    assert_eq!(title.span.column, 11);
}

#[test]
fn blank_lines_and_comments_do_not_close_blocks() {
    let source = "fn main()\n    const title = \"Small Gods\"\n\n    ; keep the block open\n    return title\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::String,
            TokenKind::Newline,
            TokenKind::Comment,
            TokenKind::Newline,
            TokenKind::Keyword(Keyword::Return),
            TokenKind::Identifier,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn column_zero_comment_continuing_a_body_stays_inside_the_block() {
    // The comment outdents to column zero but the body continues with an
    // indented line below it, so it is trailing trivia of the open block, not a
    // top-level comment: no DEDENT precedes it.
    let source = "fn main()\n    const a = 1\n\n; still in main\n    const b = 2\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Integer,
            TokenKind::Newline,
            TokenKind::Comment,
            TokenKind::Newline,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Integer,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn consecutive_column_zero_comments_continuing_a_body_stay_inside_the_block() {
    // A run of two column-zero comments inside a body, followed by an indented
    // body line, is classified by the next NON-COMMENT significant line: it
    // continues the block, so neither comment emits a DEDENT.
    let source = "fn main()\n    const a = 1\n\n; one\n; two\n    const b = 2\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Integer,
            TokenKind::Newline,
            TokenKind::Comment,
            TokenKind::Newline,
            TokenKind::Comment,
            TokenKind::Newline,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Integer,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn consecutive_column_zero_comments_before_a_top_level_decl_close_the_block() {
    // A run of two column-zero comments whose next non-comment significant line
    // is a top-level declaration closes the open block before the run.
    let source = "fn one()\n    const a = 1\n\n; one\n; two\nfn two()\n    const b = 2\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Integer,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Comment,
            TokenKind::Newline,
            TokenKind::Comment,
            TokenKind::Newline,
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Integer,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn column_zero_comment_between_top_level_decls_closes_the_block() {
    // The comment outdents to column zero and the next significant line is also
    // at the top level, so the open block closes before the comment and the
    // comment docks at the file's top level, attaching to the declaration below.
    let source = "fn one()\n    const a = 1\n\n; about two\nfn two()\n    const b = 2\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Integer,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Comment,
            TokenKind::Newline,
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Integer,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_optional_return_type_and_absent_value() {
    let source = "fn f(): int?\n    return absent\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Colon,
            TokenKind::Keyword(Keyword::Int),
            TokenKind::Question,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Return),
            TokenKind::Keyword(Keyword::Absent),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_surface_as_the_only_application_surface_keyword() {
    let source = "surface from fields collection as create update";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Surface),
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_future_data_keywords_as_keywords() {
    let source = "journal sensitive declassify Id";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Journal),
            TokenKind::Keyword(Keyword::Sensitive),
            TokenKind::Keyword(Keyword::Declassify),
            TokenKind::Keyword(Keyword::Id),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn preserves_doc_comments_as_tokens() {
    let source = ";; Books saved by id.\nresource Book\n    required title: string\nstore ^books(id: int): Book\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::DocComment,
            TokenKind::Newline,
            TokenKind::Keyword(Keyword::Resource),
            TokenKind::Identifier,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Required),
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Keyword(Keyword::String),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Keyword(Keyword::Store),
            TokenKind::Caret,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Keyword(Keyword::Int),
            TokenKind::RightParen,
            TokenKind::Colon,
            TokenKind::Identifier,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn indented_doc_comments_follow_block_layout() {
    let source =
        "resource Book\n    ;; Display title.\n    title: string\nstore ^books(id: int): Book\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Resource),
            TokenKind::Identifier,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::DocComment,
            TokenKind::Newline,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Keyword(Keyword::String),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Keyword(Keyword::Store),
            TokenKind::Caret,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Keyword(Keyword::Int),
            TokenKind::RightParen,
            TokenKind::Colon,
            TokenKind::Identifier,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_literals_operators_and_punctuation_boundaries() {
    let source = "const row = ^books(id).\"old-title\" != b\"gone\" and note + \"ok\"\n";

    assert_eq!(
        texts(source),
        vec![
            "const",
            "row",
            "=",
            "^",
            "books",
            "(",
            "id",
            ")",
            ".",
            "\"old-title\"",
            "!=",
            "b\"gone\"",
            "and",
            "note",
            "+",
            "\"ok\"",
            "\n",
            "",
        ]
    );

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Const),
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Caret,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::Identifier,
            TokenKind::RightParen,
            TokenKind::Dot,
            TokenKind::String,
            TokenKind::BangEqual,
            TokenKind::Bytes,
            TokenKind::Keyword(Keyword::And),
            TokenKind::Identifier,
            TokenKind::Plus,
            TokenKind::String,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_duration_literals_for_known_units() {
    // A number followed by a dot and a known fixed-span unit is one duration
    // token; singular and plural spellings are both accepted.
    let source = "1.day 2.hours 30.seconds 4.weeks\n";

    assert_eq!(
        texts(source),
        vec!["1.day", "2.hours", "30.seconds", "4.weeks", "\n", ""]
    );
    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Duration,
            TokenKind::Duration,
            TokenKind::Duration,
            TokenKind::Duration,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn duration_lexing_does_not_disturb_decimals_fields_or_unknown_units() {
    // `1.5` is still a decimal; `x.field` is still field access; an unknown unit
    // such as `month` or `year` leaves the number, dot, and word untouched.
    let source = "1.5 x.field 1.month 1.year\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Decimal,
            TokenKind::Identifier,
            TokenKind::Dot,
            TokenKind::Identifier,
            TokenKind::Integer,
            TokenKind::Dot,
            TokenKind::Identifier,
            TokenKind::Integer,
            TokenKind::Dot,
            TokenKind::Identifier,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_interpolation_with_expression_boundaries() {
    let source = "print($\"book {id}: {{ready}}\")\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::InterpolationStart,
            TokenKind::InterpolationText,
            TokenKind::InterpolationExprStart,
            TokenKind::Identifier,
            TokenKind::InterpolationExprEnd,
            TokenKind::InterpolationText,
            TokenKind::InterpolationEnd,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        texts(source),
        vec![
            "print",
            "(",
            "$\"",
            "book ",
            "{",
            "id",
            "}",
            ": {{ready}}",
            "\"",
            ")",
            "\n",
            "",
        ]
    );
}

#[test]
fn lexes_utf8_strings_bytes_and_interpolation_boundaries() {
    let source = "print(\"café\", b\"naïve\", $\"olá {name}: €\")\n";
    let lexed = lex_source(source);

    assert!(lexed.diagnostics.is_empty(), "{:#?}", lexed.diagnostics);
    assert_eq!(
        lexed
            .tokens
            .iter()
            .map(|token| token.kind)
            .collect::<Vec<_>>(),
        vec![
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::String,
            TokenKind::Comma,
            TokenKind::Bytes,
            TokenKind::Comma,
            TokenKind::InterpolationStart,
            TokenKind::InterpolationText,
            TokenKind::InterpolationExprStart,
            TokenKind::Identifier,
            TokenKind::InterpolationExprEnd,
            TokenKind::InterpolationText,
            TokenKind::InterpolationEnd,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        lexed
            .tokens
            .iter()
            .map(|token| token.text(source))
            .collect::<Vec<_>>(),
        vec![
            "print",
            "(",
            "\"café\"",
            ",",
            "b\"naïve\"",
            ",",
            "$\"",
            "olá ",
            "{",
            "name",
            "}",
            ": €",
            "\"",
            ")",
            "\n",
            "",
        ]
    );
}

#[test]
fn suppresses_layout_inside_open_delimiters() {
    let source = "throw Error(\n    code: \"book.absent\",\n    message: \"missing\",\n)\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Throw),
            TokenKind::Keyword(Keyword::Error),
            TokenKind::LeftParen,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::String,
            TokenKind::Comma,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::String,
            TokenKind::Comma,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn comment_lines_inside_open_delimiters_do_not_emit_newlines() {
    let source = "throw Error(\n    ; generated message\n    code: \"book.absent\",\n)\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Throw),
            TokenKind::Keyword(Keyword::Error),
            TokenKind::LeftParen,
            TokenKind::Comment,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::String,
            TokenKind::Comma,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn reports_lexical_errors_with_parse_syntax_diagnostics() {
    let source = "fn main()\n\treturn \"unterminated\n    ~\n";
    let lexed = lex_source(source);

    assert!(has_errors(&lexed));
    assert_eq!(lexed.diagnostics.len(), 3, "{:#?}", lexed.diagnostics);
    assert!(
        lexed.diagnostics.iter().all(|diagnostic| {
            diagnostic.code == "parse.syntax" && diagnostic.kind() == "parse"
        })
    );
    assert_eq!(
        lexed.diagnostics[0].reason,
        DiagnosticReason::Lexer(LexerDiagnosticReason::TabIndentation)
    );
    assert_eq!(
        lexed.diagnostics[1].reason,
        DiagnosticReason::Lexer(LexerDiagnosticReason::UnterminatedString)
    );
    assert_eq!(
        lexed.diagnostics[2].reason,
        DiagnosticReason::Lexer(LexerDiagnosticReason::ReservedTilde)
    );
    assert_eq!(lexed.diagnostics[0].span.line, 2);
    assert_eq!(lexed.diagnostics[0].span.column, 1);
}

#[test]
fn reserves_tilde_for_ephemeral_roots() {
    let lexed = lex_source("fn main()\n    return ~cache\n");

    assert!(has_errors(&lexed));
    let diagnostic = lexed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason == DiagnosticReason::Lexer(LexerDiagnosticReason::ReservedTilde)
        })
        .expect("reserved tilde diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.span.line, 2);
}

#[test]
fn rejects_obsolete_operators_with_marrow_guidance() {
    let cases: &[(&str, ObsoleteOperator, &str, usize)] = &[
        ("a && b", ObsoleteOperator::AndAnd, "`and`", 2),
        ("a || b", ObsoleteOperator::OrOr, "`or`", 2),
        ("not_done = !ready", ObsoleteOperator::Bang, "`not`", 1),
        (
            "count # 1",
            ObsoleteOperator::Hash,
            "Marrow uses `;` for comments",
            1,
        ),
    ];

    for (source, expected_operator, expected_help, expected_len) in cases {
        let lexed = lex_source(source);
        assert!(
            has_errors(&lexed),
            "expected {expected_operator:?} to be rejected by the lexer, got {:#?}",
            lexed.diagnostics
        );
        let diagnostic = lexed
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.reason
                    == DiagnosticReason::Lexer(LexerDiagnosticReason::ObsoleteOperator(
                        *expected_operator,
                    ))
            })
            .unwrap_or_else(|| panic!("expected diagnostic for {expected_operator:?}"));
        assert_eq!(diagnostic.code, "parse.syntax");
        assert_eq!(diagnostic.kind(), "parse");
        assert_eq!(
            diagnostic.span.end_byte - diagnostic.span.start_byte,
            *expected_len,
            "diagnostic span for {expected_operator:?} should cover the obsolete token"
        );
        let help = diagnostic
            .help
            .as_deref()
            .unwrap_or_else(|| panic!("expected help text for {expected_operator:?}"));
        assert!(
            help.contains(expected_help),
            "expected help to suggest {expected_help}, got {help}"
        );
    }
}

#[test]
fn keeps_valid_operators_after_obsolete_check() {
    let source = "if a != b\n    print(\"ne\")\n";
    let lexed = lex_source(source);

    assert!(
        !has_errors(&lexed),
        "valid `!=` should still lex cleanly, got {:#?}",
        lexed.diagnostics
    );
    assert!(
        lexed
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::BangEqual),
        "expected a BangEqual token"
    );
}

#[test]
fn lexes_equality_operator() {
    let source = "if a == b\n    print(\"eq\")\n";
    let lexed = lex_source(source);

    assert!(
        !has_errors(&lexed),
        "`==` should lex cleanly as the equality operator, got {:#?}",
        lexed.diagnostics
    );
    assert!(
        lexed
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::EqualEqual),
        "expected an EqualEqual token"
    );
}

#[test]
fn lexes_is_as_a_keyword() {
    // `is` is a reserved word operator, lexed as a keyword like `and`/`or`/`not`.
    let kinds = kinds("print(pet is Cat::tiger)\n");
    assert!(
        kinds.contains(&TokenKind::Keyword(Keyword::Is)),
        "expected an `is` keyword, got {kinds:?}"
    );
}

#[test]
fn lexes_absence_operators() {
    // `?.` and `??` each lex as a single multi-character punctuation token.
    let lexed = lex_source("print(a?.b ?? c)\n");
    assert!(
        !has_errors(&lexed),
        "`?.` and `??` should lex cleanly, got {:#?}",
        lexed.diagnostics
    );
    assert!(
        lexed
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::QuestionDot),
        "expected a QuestionDot token"
    );
    assert!(
        lexed
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::QuestionQuestion),
        "expected a QuestionQuestion token"
    );
}

#[test]
fn lexes_optional_suffix_with_longest_match() {
    // One trailing `?` is the optional type suffix and lexes as `Question`. The
    // multi-character table runs first, so `??` stays a single `QuestionQuestion`
    // token even in type-suffix position, and two spaced `?` are two `Question`.
    assert_eq!(
        kinds("title?\n"),
        vec![
            TokenKind::Identifier,
            TokenKind::Question,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        kinds("title??\n"),
        vec![
            TokenKind::Identifier,
            TokenKind::QuestionQuestion,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        kinds("title ? ?\n"),
        vec![
            TokenKind::Identifier,
            TokenKind::Question,
            TokenKind::Question,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn rejects_an_at_sign_at_its_own_column() {
    // `@` is not part of any operator or grammar production, so it is an
    // unexpected character reported at its own column, exactly like `?`/`#`/`!`,
    // rather than deferred to a downstream statement-level diagnostic.
    let lexed = lex_source("print(a @ b)\n");
    let diagnostic = lexed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason
                == DiagnosticReason::Lexer(LexerDiagnosticReason::UnexpectedCharacter('@'))
        })
        .unwrap_or_else(|| {
            panic!(
                "expected a bare `@` to be rejected, got {:#?}",
                lexed.diagnostics
            )
        });
    assert_eq!(
        diagnostic.span.column, 9,
        "the `@` diagnostic must point at the `@`, not a later token"
    );
}

/// Corpus smoke test (one owner): every fenced `mw` block in the language
/// reference lexes without errors and ends with EOF. It guards the documented
/// examples as a whole; the per-token and per-error lexer contracts are owned by
/// the focused tests above.
#[test]
fn lexes_all_language_reference_mw_blocks_without_errors() {
    for block in common::mw_blocks() {
        let lexed = lex_source(&block.source);
        assert!(
            !has_errors(&lexed),
            "{} fenced mw block {} produced diagnostics:\n{:#?}\n{}",
            block.path,
            block.index,
            lexed.diagnostics,
            block.source
        );
        assert_eq!(
            lexed.tokens.last().map(|token| token.kind),
            Some(TokenKind::Eof),
            "{} fenced mw block {} did not end with EOF",
            block.path,
            block.index
        );
    }
}
