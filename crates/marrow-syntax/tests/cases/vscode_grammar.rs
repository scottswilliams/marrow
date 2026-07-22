//! Generator and drift check for the VS Code TextMate grammar at
//! `editors/vscode/syntaxes/marrow.tmLanguage.json`.
//!
//! The parser owns syntax. The editor grammar must not become a second
//! authority, so it is *generated*, not hand-written: [`render_grammar`] builds
//! the committed `.tmLanguage.json` bytes from a fixed template plus the one
//! dynamic input the parser owns — the reserved-word set. That set is not
//! re-derived here; it is read back from the drift-checked `reserved-words`
//! block in `docs/tools/ai-legibility.md`, which the sibling `grammar_drift`
//! test proves equal to the parser's own keyword table. The chain of custody is
//! therefore: parser → (grammar_drift guards) → ai-legibility.md → (this
//! generator) → the committed grammar. A lexer change that adds or renames a
//! keyword breaks `grammar_drift` first; once the published block is updated
//! this generator renders different bytes and [`generated_grammar_is_committed`]
//! fails until the grammar is regenerated in the same change.
//!
//! Only lexical forms the lexer actually owns are scoped: line and doc comments
//! (`//`, `///`), the three string kinds (`"…"`, `b"…"`, `$"…"`) with the exact
//! escapes each owner recognizes — text and interpolation strings decode the five
//! escapes plus `\u{…}`, byte strings the same five plus `\xNN` hex and not
//! `\u{…}` (`literal.rs`) — integer and decimal number literals, the durable
//! `^root` sigil, the `::` path separator, and the reserved words (one keyword
//! scope, because the parser publishes no finer keyword classification). No
//! speculative scopes — no guessed function, type-annotation, or member colors.
//! Duration-unit suffixes (`1.day`) are deliberately left unscoped: coloring the
//! unit would require a `duration-units` inventory derived from the lexer the way
//! reserved words are, not a hand-written unit list that would become a second
//! authority for a lexical form the parser owns.
//!
//! To regenerate after an intended change:
//!   cargo test -p marrow-syntax regenerate_vscode_grammar -- --ignored

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Template for the committed grammar. `%%KEYWORDS%%` is replaced by the sorted,
/// longest-first `|` alternation of the reserved words. Everything else is fixed:
/// the generator's only variable input is the parser-owned keyword set.
const GRAMMAR_TEMPLATE: &str = r##"{
  "$schema": "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json",
  "name": "Marrow",
  "scopeName": "source.marrow",
  "patterns": [
    { "include": "#comments" },
    { "include": "#expression" }
  ],
  "repository": {
    "expression": {
      "patterns": [
        { "include": "#strings" },
        { "include": "#numbers" },
        { "include": "#durable-root" },
        { "include": "#keywords" },
        { "include": "#namespace" }
      ]
    },
    "comments": {
      "patterns": [
        { "name": "comment.line.documentation.marrow", "match": "///.*$" },
        { "name": "comment.line.double-slash.marrow", "match": "//.*$" }
      ]
    },
    "keywords": {
      "patterns": [
        { "name": "keyword.other.marrow", "match": "\\b(%%KEYWORDS%%)\\b" }
      ]
    },
    "numbers": {
      "patterns": [
        { "name": "constant.numeric.decimal.marrow", "match": "\\b[0-9]+\\.[0-9]+\\b" },
        { "name": "constant.numeric.integer.marrow", "match": "\\b[0-9]+\\b" }
      ]
    },
    "durable-root": {
      "patterns": [
        {
          "match": "(\\^)([A-Za-z_][A-Za-z0-9_]*)",
          "captures": {
            "1": { "name": "punctuation.definition.variable.marrow" },
            "2": { "name": "variable.other.durable.marrow" }
          }
        }
      ]
    },
    "namespace": {
      "patterns": [
        { "name": "punctuation.separator.namespace.marrow", "match": "::" }
      ]
    },
    "escape": {
      "patterns": [
        { "name": "constant.character.escape.marrow", "match": "\\\\(u\\{[0-9A-Fa-f]{1,6}\\}|[\\\\\"nrt])" }
      ]
    },
    "bytes-escape": {
      "patterns": [
        { "name": "constant.character.escape.marrow", "match": "\\\\(x[0-9A-Fa-f]{2}|[\\\\\"nrt])" }
      ]
    },
    "strings": {
      "patterns": [
        { "include": "#interpolation" },
        { "include": "#bytes-string" },
        { "include": "#double-string" }
      ]
    },
    "double-string": {
      "name": "string.quoted.double.marrow",
      "begin": "\"",
      "beginCaptures": { "0": { "name": "punctuation.definition.string.begin.marrow" } },
      "end": "\"|(?=$)",
      "endCaptures": { "0": { "name": "punctuation.definition.string.end.marrow" } },
      "patterns": [ { "include": "#escape" } ]
    },
    "bytes-string": {
      "name": "string.quoted.binary.marrow",
      "begin": "b\"",
      "beginCaptures": { "0": { "name": "punctuation.definition.string.begin.marrow" } },
      "end": "\"|(?=$)",
      "endCaptures": { "0": { "name": "punctuation.definition.string.end.marrow" } },
      "patterns": [ { "include": "#bytes-escape" } ]
    },
    "interpolation": {
      "name": "string.interpolated.marrow",
      "begin": "\\$\"",
      "beginCaptures": { "0": { "name": "punctuation.definition.string.begin.marrow" } },
      "end": "\"|(?=$)",
      "endCaptures": { "0": { "name": "punctuation.definition.string.end.marrow" } },
      "patterns": [
        { "name": "constant.character.escape.marrow", "match": "\\{\\{|\\}\\}" },
        { "include": "#escape" },
        { "include": "#interpolation-hole" }
      ]
    },
    "interpolation-hole": {
      "name": "meta.embedded.line.marrow",
      "begin": "\\{",
      "beginCaptures": { "0": { "name": "punctuation.section.embedded.begin.marrow" } },
      "end": "\\}",
      "endCaptures": { "0": { "name": "punctuation.section.embedded.end.marrow" } },
      "patterns": [ { "include": "#expression" } ]
    }
  }
}
"##;

/// The repository-root-relative path to the committed grammar, resolved from this
/// crate's manifest directory (`crates/marrow-syntax`).
fn grammar_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("editors")
        .join("vscode")
        .join("syntaxes")
        .join("marrow.tmLanguage.json")
}

/// The drift-checked reserved-word inventory, read from the fenced block in
/// `docs/tools/ai-legibility.md`. That page is proven equal to the parser's
/// keyword table by the sibling `grammar_drift` test, so consuming it here keeps
/// this generator a consumer of a published fact rather than a second keyword
/// authority.
fn reserved_words() -> BTreeSet<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("tools")
        .join("ai-legibility.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let begin = "<!-- BEGIN reserved-words -->";
    let end = "<!-- END reserved-words -->";
    let start = text
        .find(begin)
        .unwrap_or_else(|| panic!("ai-legibility.md is missing `{begin}`"))
        + begin.len();
    let stop = text[start..]
        .find(end)
        .unwrap_or_else(|| panic!("ai-legibility.md is missing `{end}`"))
        + start;
    text[start..stop]
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .flat_map(str::split_whitespace)
        .map(str::to_owned)
        .collect()
}

/// The `|` alternation body for the keyword rule, ordered longest-first (then
/// lexically) so a reserved word that is a prefix of another — `Error` before
/// `ErrorCode` — cannot shadow the longer spelling regardless of the regex
/// engine's alternation semantics. The order is fully determined by the set, so
/// the rendered grammar is deterministic.
fn keyword_alternation() -> String {
    let mut words: Vec<String> = reserved_words().into_iter().collect();
    words.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    words.join("|")
}

/// The exact bytes the committed grammar must contain.
fn render_grammar() -> String {
    GRAMMAR_TEMPLATE.replace("%%KEYWORDS%%", &keyword_alternation())
}

#[test]
fn generated_grammar_is_committed() {
    let path = grammar_path();
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(
        committed,
        render_grammar(),
        "{} drifted from the generator; regenerate with \
         `cargo test -p marrow-syntax regenerate_vscode_grammar -- --ignored` \
         in the same change as the parser/keyword change",
        path.display()
    );
}

/// The generated keyword alternation covers exactly the reserved words and no
/// stray identifier, so the rule scopes the parser's own reserved set. Uses the
/// crate's `is_reserved_word` predicate as the independent check, mirroring
/// `grammar_drift::every_rendered_keyword_is_reserved`.
#[test]
fn every_scoped_keyword_is_reserved() {
    for word in reserved_words() {
        assert!(
            marrow_syntax::is_reserved_word(&word),
            "`{word}` is scoped as a keyword but the lexer does not reserve it"
        );
    }
    assert!(!marrow_syntax::is_reserved_word("bookstore"));
}

/// Writing generator. Ignored so a normal run only *checks* drift; run it
/// explicitly to rewrite the committed grammar after an intended change.
#[test]
#[ignore = "writes the committed grammar; run explicitly to regenerate"]
fn regenerate_vscode_grammar() {
    let path = grammar_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
    }
    std::fs::write(&path, render_grammar())
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    eprintln!("wrote {}", path.display());
}
