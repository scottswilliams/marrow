//! Drift check for the machine-readable language facts published in
//! `docs/tools/ai-legibility.md`. The parser owns syntax; that page republishes
//! two lexical inventories the parser derives — the reserved words and the token
//! kinds — and this test proves the committed lists still match the parser.
//!
//! The parser publishes exhaustive read-only inventories and renderings. Adding,
//! removing, or renaming a token or keyword variant therefore fails in the owner
//! before this projection can silently drift.

use marrow_syntax::{Keyword, TokenKind, is_reserved_word};

/// The path to the committed artifact, resolved from this crate's manifest
/// directory (`crates/marrow-syntax`) up to the repository root.
fn artifact_text() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("tools")
        .join("ai-legibility.md");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The whitespace-separated words inside the fenced block delimited by the
/// `BEGIN <name>` / `END <name>` HTML-comment markers, with the fence lines
/// (```` ``` ````) removed. A missing marker is a drift-test wiring error.
fn published_set(text: &str, name: &str) -> std::collections::BTreeSet<String> {
    let begin = format!("<!-- BEGIN {name} -->");
    let end = format!("<!-- END {name} -->");
    let start = text
        .find(&begin)
        .unwrap_or_else(|| panic!("artifact is missing the `{begin}` marker"))
        + begin.len();
    let stop = text[start..]
        .find(&end)
        .unwrap_or_else(|| panic!("artifact is missing the `{end}` marker"))
        + start;
    text[start..stop]
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .flat_map(str::split_whitespace)
        .map(str::to_owned)
        .collect()
}

fn parser_set<T: Copy>(
    all: &[T],
    render: fn(T) -> &'static str,
) -> std::collections::BTreeSet<String> {
    all.iter().map(|item| render(*item).to_owned()).collect()
}

/// The published reserved-word list equals the set the parser recognizes. The
/// parser set is rendered from the keyword type's own spellings, so this fails
/// whenever the two disagree in either direction.
#[test]
fn published_reserved_words_match_the_parser() {
    let published = published_set(&artifact_text(), "reserved-words");
    let parser = parser_set(&Keyword::ALL, Keyword::spelling);
    assert_eq!(
        published, parser,
        "docs/tools/ai-legibility.md reserved-words drifted from the parser; \
         update the block in the same change as the parser"
    );
}

/// Every rendered keyword spelling is in fact reserved by the lexer, so the
/// spellings this test renders are the parser's own truth, not a second list.
#[test]
fn every_rendered_keyword_is_reserved() {
    for keyword in Keyword::ALL {
        let spelling = keyword.spelling();
        assert!(
            is_reserved_word(spelling),
            "`{spelling}` is rendered as a keyword spelling but the lexer does not reserve it"
        );
    }
    // A plain identifier that is not a keyword must not be reported reserved, so
    // the predicate the parser set relies on is not vacuously true.
    assert!(!is_reserved_word("bookstore"));
    assert!(!is_reserved_word("error"));
}

/// The published token-kind inventory equals the parser's token kinds.
#[test]
fn published_token_kinds_match_the_parser() {
    let published = published_set(&artifact_text(), "token-kinds");
    let parser = parser_set(&TokenKind::INVENTORY, TokenKind::inventory_name);
    assert_eq!(
        published, parser,
        "docs/tools/ai-legibility.md token-kinds drifted from the parser; \
         update the block in the same change as the parser"
    );
}
