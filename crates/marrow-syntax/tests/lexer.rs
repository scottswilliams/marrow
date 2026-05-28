use marrow_syntax::{Keyword, TokenKind, lex_source};
use std::path::Path;

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
        "module shelf::books\nfn main()\n    let title = \"Small Gods\"\n    write(title)\n";

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
            TokenKind::Keyword(Keyword::Let),
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
    assert_eq!(title.span.column, 9);
}

#[test]
fn blank_lines_and_comments_do_not_close_blocks() {
    let source = "fn main()\n    let title = \"Small Gods\"\n\n    ; keep the block open\n    return title\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Fn),
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Let),
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
fn preserves_doc_comments_as_tokens() {
    let source =
        ";; Books saved by id.\nresource Book at ^books(id: int)\n    required title: string\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::DocComment,
            TokenKind::Newline,
            TokenKind::Keyword(Keyword::Resource),
            TokenKind::Identifier,
            TokenKind::Keyword(Keyword::At),
            TokenKind::Caret,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Keyword(Keyword::Int),
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Keyword(Keyword::Required),
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Keyword(Keyword::String),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn indented_doc_comments_follow_block_layout() {
    let source = "resource Book at ^books(id: int)\n    ;; Display title.\n    @id(\"book.title\")\n    title: string\n";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Resource),
            TokenKind::Identifier,
            TokenKind::Keyword(Keyword::At),
            TokenKind::Caret,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Keyword(Keyword::Int),
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::DocComment,
            TokenKind::Newline,
            TokenKind::At,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::String,
            TokenKind::RightParen,
            TokenKind::Newline,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Keyword(Keyword::String),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_literals_operators_and_punctuation_boundaries() {
    let source = "let row = ^books(id).\"old-title\" != b\"gone\" and note _ \"ok\"\n";

    assert_eq!(
        texts(source),
        vec![
            "let",
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
            "_",
            "\"ok\"",
            "\n",
            "",
        ]
    );

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Let),
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
            TokenKind::Underscore,
            TokenKind::String,
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_interpolation_with_expression_boundaries() {
    let source = "write($\"book {id}: {{ready}}\")\n";

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
            "write",
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
    let source = "fn main()\n\treturn \"unterminated\n    #\n";
    let lexed = lex_source(source);

    assert!(lexed.has_errors());
    assert_eq!(lexed.diagnostics.len(), 3, "{:#?}", lexed.diagnostics);
    assert!(
        lexed
            .diagnostics
            .iter()
            .all(|diagnostic| { diagnostic.code == "parse.syntax" && diagnostic.kind == "parse" })
    );
    assert!(lexed.diagnostics[0].message.contains("tabs"));
    assert!(lexed.diagnostics[1].message.contains("unterminated string"));
    assert!(
        lexed.diagnostics[2]
            .message
            .contains("unexpected character")
    );
    assert_eq!(lexed.diagnostics[0].line, 2);
    assert_eq!(lexed.diagnostics[0].column, 1);
}

#[test]
fn lexes_all_language_reference_mw_blocks_without_errors() {
    for fixture in language_reference_mw_blocks() {
        let lexed = lex_source(&fixture.source);
        assert!(
            !lexed.has_errors(),
            "{} fenced mw block {} produced diagnostics:\n{:#?}\n{}",
            fixture.path,
            fixture.block,
            lexed.diagnostics,
            fixture.source
        );
        assert_eq!(
            lexed.tokens.last().map(|token| token.kind),
            Some(TokenKind::Eof),
            "{} fenced mw block {} did not end with EOF",
            fixture.path,
            fixture.block
        );
    }
}

struct MwFixture {
    path: String,
    block: usize,
    source: String,
}

fn language_reference_mw_blocks() -> Vec<MwFixture> {
    let docs_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("language");
    let mut fixtures = Vec::new();
    for entry in std::fs::read_dir(docs_dir).expect("read docs/language") {
        let path = entry.expect("language doc entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read language doc");
        let mut in_mw = false;
        let mut block = 0usize;
        let mut source = String::new();

        for line in text.lines() {
            if line.trim() == "```mw" {
                in_mw = true;
                block += 1;
                source.clear();
                continue;
            }
            if line.trim() == "```" && in_mw {
                fixtures.push(MwFixture {
                    path: path.display().to_string(),
                    block,
                    source: source.clone(),
                });
                in_mw = false;
                continue;
            }
            if in_mw {
                source.push_str(line);
                source.push('\n');
            }
        }
    }
    fixtures
}
