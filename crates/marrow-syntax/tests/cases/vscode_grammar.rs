//! Generator and drift checks for the VS Code TextMate grammar at the
//! editors/vscode/syntaxes/marrow.tmLanguage.json artifact.
//!
//! The parser owns every lexical classification. This projection consumes the
//! exhaustive Keyword and TokenKind facts directly; rendered documentation and
//! the committed JSON are outputs, never generator inputs. TextMate scopes are
//! assigned only to parser-owned lexical classes. Names, members, locals,
//! parameters, and other semantic identifier roles remain unscoped until
//! compiler facts can describe them.
//!
//! To regenerate after an intended change:
//!   cargo test -p marrow-syntax regenerate_vscode_grammar -- --ignored

use crate::common::CompletePayload;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use marrow_syntax::{
    Keyword, LexicalClass, TokenKind, duration_unit_spellings, is_reserved_word, lex_source,
};

const KEYWORD_CLASSES: [LexicalClass; 7] = [
    LexicalClass::ControlFlow,
    LexicalClass::Declaration,
    LexicalClass::Modifier,
    LexicalClass::Effect,
    LexicalClass::BuiltinType,
    LexicalClass::BuiltinValue,
    LexicalClass::WordOperator,
];

/// Standard TextMate scope families for parser-owned lexical classes. An
/// unscoped token deliberately has no editor scope.
const fn textmate_scope(class: LexicalClass) -> Option<&'static str> {
    match class {
        LexicalClass::Unscoped => None,
        LexicalClass::ControlFlow => Some("keyword.control.marrow"),
        LexicalClass::Declaration => Some("keyword.declaration.marrow"),
        LexicalClass::Modifier => Some("storage.modifier.marrow"),
        LexicalClass::Effect => Some("storage.modifier.effect.marrow"),
        LexicalClass::BuiltinType => Some("storage.type.builtin.marrow"),
        LexicalClass::BuiltinValue => Some("constant.language.marrow"),
        LexicalClass::IntegerLiteral => Some("constant.numeric.integer.marrow"),
        LexicalClass::DecimalLiteral => Some("constant.numeric.decimal.marrow"),
        LexicalClass::DurationLiteral => Some("constant.numeric.duration.marrow"),
        LexicalClass::StringLiteral => Some("string.quoted.double.marrow"),
        LexicalClass::InterpolationString => Some("string.interpolated.marrow"),
        LexicalClass::InterpolationDelimiter => Some("punctuation.section.embedded.marrow"),
        LexicalClass::BytesLiteral => Some("string.quoted.binary.marrow"),
        LexicalClass::Comment => Some("comment.line.double-slash.marrow"),
        LexicalClass::DocumentationComment => Some("comment.line.documentation.marrow"),
        LexicalClass::Operator => Some("keyword.operator.marrow"),
        LexicalClass::WordOperator => Some("keyword.operator.word.marrow"),
        LexicalClass::Delimiter => Some("punctuation.section.group.marrow"),
        LexicalClass::Punctuation => Some("punctuation.separator.marrow"),
        LexicalClass::PathSeparator => Some("punctuation.separator.namespace.marrow"),
        LexicalClass::DurableRootSigil => Some("punctuation.definition.variable.marrow"),
    }
}

const GRAMMAR_TEMPLATE: &str = r##"{
  "$schema": "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json",
  "name": "Marrow",
  "scopeName": "source.marrow",
  "patterns": [
    { "include": "#expression" }
  ],
  "repository": {
    "expression": {
      "patterns": [
        { "include": "#comments" },
        { "include": "#lexical-expression" }
      ]
    },
    "lexical-expression": {
      "patterns": [
        { "include": "#strings" },
        { "include": "#numbers" },
        { "include": "#durable-root" },
        { "include": "#keywords" },
        { "include": "#namespace" },
        { "include": "#operators" },
        { "include": "#delimiters" },
        { "include": "#punctuation" },
        { "include": "#identifiers" }
      ]
    },
    "comments": {
      "patterns": [
        { "name": "%%DOC_COMMENT_SCOPE%%", "match": "///[^\\r\\n]*" },
        { "name": "%%COMMENT_SCOPE%%", "match": "//[^\\r\\n]*" }
      ]
    },
    "hole-comments": {
      "patterns": [
        { "name": "%%DOC_COMMENT_SCOPE%%", "match": "///[^}\\r\\n]*" },
        { "name": "%%COMMENT_SCOPE%%", "match": "//[^}\\r\\n]*" }
      ]
    },
    "keywords": {
      "patterns": [
%%KEYWORD_PATTERNS%%
      ]
    },
    "numbers": {
      "patterns": [
        { "name": "%%DURATION_SCOPE%%", "match": "%%DURATION_PATTERN%%" },
        { "name": "%%DECIMAL_SCOPE%%", "match": "[0-9]+\\.[0-9]+" },
        { "name": "%%INTEGER_SCOPE%%", "match": "[0-9]+" }
      ]
    },
    "durable-root": {
      "patterns": [
        { "name": "%%DURABLE_ROOT_SCOPE%%", "match": "%%DURABLE_ROOT_PATTERN%%" }
      ]
    },
    "namespace": {
      "patterns": [
        { "name": "%%PATH_SEPARATOR_SCOPE%%", "match": "%%PATH_SEPARATOR_PATTERN%%" }
      ]
    },
    "operators": {
      "patterns": [
        { "name": "%%OPERATOR_SCOPE%%", "match": "%%OPERATOR_PATTERN%%" }
      ]
    },
    "delimiters": {
      "patterns": [
        { "name": "%%DELIMITER_SCOPE%%", "match": "%%DELIMITER_PATTERN%%" }
      ]
    },
    "punctuation": {
      "patterns": [
        { "name": "%%PUNCTUATION_SCOPE%%", "match": "%%PUNCTUATION_PATTERN%%" }
      ]
    },
    "identifiers": {
      "patterns": [
        { "match": "[A-Za-z_][A-Za-z0-9_]*" }
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
      "name": "%%STRING_SCOPE%%",
      "begin": "\"",
      "beginCaptures": { "0": { "name": "punctuation.definition.string.begin.marrow" } },
      "end": "\"|(?=$)",
      "endCaptures": { "0": { "name": "punctuation.definition.string.end.marrow" } },
      "patterns": [ { "include": "#escape" } ]
    },
    "bytes-string": {
      "name": "%%BYTES_SCOPE%%",
      "begin": "b\"",
      "beginCaptures": { "0": { "name": "punctuation.definition.string.begin.marrow" } },
      "end": "\"|(?=$)",
      "endCaptures": { "0": { "name": "punctuation.definition.string.end.marrow" } },
      "patterns": [ { "include": "#bytes-escape" } ]
    },
    "interpolation": {
      "name": "%%INTERPOLATION_SCOPE%%",
      "begin": "%%INTERPOLATION_BEGIN%%",
      "beginCaptures": { "0": { "name": "punctuation.definition.string.begin.marrow" } },
      "end": "\"|(?=$)",
      "endCaptures": { "0": { "name": "punctuation.definition.string.end.marrow" } },
      "patterns": [
        { "name": "constant.character.escape.marrow", "match": "\\{\\{|\\}\\}" },
        { "include": "#escape" },
        { "include": "#interpolation-hole" }
      ]
    },
    "escaped-hole-string": {
      "name": "%%STRING_SCOPE%%",
      "begin": "%%ESCAPED_HOLE_QUOTE%%",
      "beginCaptures": { "0": { "name": "punctuation.definition.string.begin.marrow" } },
      "end": "%%ESCAPED_HOLE_QUOTE%%",
      "endCaptures": { "0": { "name": "punctuation.definition.string.end.marrow" } },
      "patterns": [ { "include": "#escape" } ]
    },
    "interpolation-hole": {
      "name": "meta.embedded.line.marrow",
      "begin": "%%INTERPOLATION_HOLE_BEGIN%%",
      "beginCaptures": { "0": { "name": "%%INTERPOLATION_DELIMITER_SCOPE%%" } },
      "end": "%%INTERPOLATION_HOLE_END%%",
      "endCaptures": { "0": { "name": "%%INTERPOLATION_DELIMITER_SCOPE%%" } },
      "patterns": [
        { "include": "#hole-comments" },
        { "include": "#escaped-hole-string" },
        { "include": "#lexical-expression" }
      ]
    }
  }
}
"##;

#[derive(Debug)]
struct KeywordRule {
    class: LexicalClass,
    scope: &'static str,
    spellings: Vec<&'static str>,
    pattern: String,
}

fn grammar_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("editors")
        .join("vscode")
        .join("syntaxes")
        .join("marrow.tmLanguage.json")
}

fn regex_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '.' | '^' | '$' | '|' | '?' | '*' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn json_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn longest_first_alternation<I>(spellings: I) -> String
where
    I: IntoIterator<Item = &'static str>,
{
    let mut spellings: Vec<&str> = spellings.into_iter().collect();
    spellings.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    spellings
        .into_iter()
        .map(regex_escape)
        .collect::<Vec<_>>()
        .join("|")
}

fn keyword_rules() -> Vec<KeywordRule> {
    KEYWORD_CLASSES
        .into_iter()
        .map(|class| {
            let mut spellings: Vec<&str> = Keyword::ALL
                .into_iter()
                .filter(|keyword| keyword.lexical_class() == class)
                .map(Keyword::spelling)
                .collect();
            spellings
                .sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
            let alternation = longest_first_alternation(spellings.iter().copied());
            KeywordRule {
                class,
                scope: textmate_scope(class).expect("keyword classes have TextMate scopes"),
                spellings,
                pattern: format!(r"({alternation})(?![A-Za-z0-9_])"),
            }
        })
        .collect()
}

fn render_keyword_patterns() -> String {
    keyword_rules()
        .into_iter()
        .map(|rule| {
            format!(
                "        {{ \"name\": \"{}\", \"match\": \"{}\" }}",
                rule.scope,
                json_escape(&rule.pattern)
            )
        })
        .collect::<Vec<_>>()
        .join(",\n")
}

fn fixed_token_alternation(class: LexicalClass) -> String {
    longest_first_alternation(
        TokenKind::INVENTORY
            .into_iter()
            .filter(|kind| kind.lexical_class() == class)
            .filter_map(TokenKind::fixed_spelling),
    )
}

fn fixed_token_pattern(class: LexicalClass) -> String {
    format!("({})", fixed_token_alternation(class))
}

fn one_fixed_token_pattern(kind: TokenKind) -> String {
    regex_escape(
        kind.fixed_spelling()
            .expect("the requested token has a fixed spelling"),
    )
}

fn scope(class: LexicalClass) -> &'static str {
    textmate_scope(class).expect("rendered lexical classes have TextMate scopes")
}

fn replace(mut grammar: String, marker: &str, replacement: &str) -> String {
    assert!(grammar.contains(marker), "missing grammar marker {marker}");
    grammar = grammar.replace(marker, replacement);
    grammar
}

fn render_grammar() -> String {
    let duration_pattern = format!(
        r"[0-9]+\.({})(?![A-Za-z0-9_])",
        longest_first_alternation(duration_unit_spellings())
    );
    let mut grammar = GRAMMAR_TEMPLATE.to_string();
    for (marker, replacement) in [
        (
            "%%DOC_COMMENT_SCOPE%%",
            scope(LexicalClass::DocumentationComment),
        ),
        ("%%COMMENT_SCOPE%%", scope(LexicalClass::Comment)),
        ("%%DURATION_SCOPE%%", scope(LexicalClass::DurationLiteral)),
        ("%%DECIMAL_SCOPE%%", scope(LexicalClass::DecimalLiteral)),
        ("%%INTEGER_SCOPE%%", scope(LexicalClass::IntegerLiteral)),
        (
            "%%DURABLE_ROOT_SCOPE%%",
            scope(LexicalClass::DurableRootSigil),
        ),
        (
            "%%PATH_SEPARATOR_SCOPE%%",
            scope(LexicalClass::PathSeparator),
        ),
        ("%%OPERATOR_SCOPE%%", scope(LexicalClass::Operator)),
        ("%%DELIMITER_SCOPE%%", scope(LexicalClass::Delimiter)),
        ("%%PUNCTUATION_SCOPE%%", scope(LexicalClass::Punctuation)),
        ("%%STRING_SCOPE%%", scope(LexicalClass::StringLiteral)),
        ("%%BYTES_SCOPE%%", scope(LexicalClass::BytesLiteral)),
        (
            "%%INTERPOLATION_SCOPE%%",
            scope(LexicalClass::InterpolationString),
        ),
        (
            "%%INTERPOLATION_DELIMITER_SCOPE%%",
            scope(LexicalClass::InterpolationDelimiter),
        ),
    ] {
        grammar = replace(grammar, marker, replacement);
    }
    for (marker, pattern) in [
        ("%%DURATION_PATTERN%%", duration_pattern),
        (
            "%%DURABLE_ROOT_PATTERN%%",
            one_fixed_token_pattern(TokenKind::Caret),
        ),
        (
            "%%PATH_SEPARATOR_PATTERN%%",
            one_fixed_token_pattern(TokenKind::DoubleColon),
        ),
        (
            "%%OPERATOR_PATTERN%%",
            fixed_token_pattern(LexicalClass::Operator),
        ),
        (
            "%%DELIMITER_PATTERN%%",
            fixed_token_pattern(LexicalClass::Delimiter),
        ),
        (
            "%%PUNCTUATION_PATTERN%%",
            fixed_token_pattern(LexicalClass::Punctuation),
        ),
        (
            "%%INTERPOLATION_BEGIN%%",
            one_fixed_token_pattern(TokenKind::InterpolationStart),
        ),
        (
            "%%INTERPOLATION_HOLE_BEGIN%%",
            one_fixed_token_pattern(TokenKind::InterpolationExprStart),
        ),
        (
            "%%INTERPOLATION_HOLE_END%%",
            one_fixed_token_pattern(TokenKind::InterpolationExprEnd),
        ),
        ("%%ESCAPED_HOLE_QUOTE%%", regex_escape(r#"\""#)),
    ] {
        grammar = replace(grammar, marker, &json_escape(&pattern));
    }
    replace(grammar, "%%KEYWORD_PATTERNS%%", &render_keyword_patterns())
}

fn regex_unescape(text: &str) -> String {
    let mut unescaped = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            unescaped.push(chars.next().expect("escaped regex character"));
        } else {
            unescaped.push(ch);
        }
    }
    unescaped
}

fn keyword_alternatives(pattern: &str) -> Vec<String> {
    let body = pattern
        .strip_prefix('(')
        .and_then(|body| body.strip_suffix(r")(?![A-Za-z0-9_])"))
        .or_else(|| {
            pattern
                .strip_prefix(r"\b(")
                .and_then(|body| body.strip_suffix(r")\b"))
        })
        .expect("keyword patterns have explicit identifier boundaries");
    body.split('|').map(regex_unescape).collect()
}

/// The end of the longest match of an emitted keyword pattern at `start`, if any.
/// The generator renders exactly one keyword-pattern shape — an alternation of
/// literal spellings followed by `(?![A-Za-z0-9_])` — so the alternation is
/// compared literally and the trailing guard is the one rule it encodes. Longest
/// wins, matching the alternation order the generator emits.
fn keyword_pattern_match_end(pattern: &str, source: &str, start: usize) -> Option<usize> {
    keyword_alternatives(pattern)
        .into_iter()
        .filter_map(|word| {
            let end = start + word.len();
            (source.get(start..end)? == word
                && !source[end..].starts_with(|ch: char| ch.is_ascii_alphanumeric() || ch == '_'))
            .then_some(end)
        })
        .max()
}

/// The emitted pattern claims exactly the span the lexer classifies as a keyword:
/// the editor rule and the parser agree on both ends of every keyword token.
fn keyword_pattern_matches(pattern: &str, source: &str) -> bool {
    lex_source(source).tokens.into_iter().any(|token| {
        matches!(token.kind, TokenKind::Keyword(_))
            && keyword_pattern_match_end(pattern, source, token.span.start_byte)
                == Some(token.span.end_byte)
    })
}

fn canonical_spelling_from_variant(keyword: Keyword) -> String {
    let variant = format!("{keyword:?}");
    match keyword {
        Keyword::Id => variant,
        _ => variant.to_ascii_lowercase(),
    }
}

fn validate_keyword_facts(facts: &[(Keyword, &'static str)]) -> Result<(), String> {
    if facts.len() != Keyword::ALL.len() {
        return Err(format!(
            "keyword inventory length {} does not equal owner length {}",
            facts.len(),
            Keyword::ALL.len()
        ));
    }
    let mut spellings = BTreeSet::new();
    for (index, (keyword, spelling)) in facts.iter().copied().enumerate() {
        if Keyword::ALL[index] != keyword {
            return Err(format!(
                "keyword inventory mismatch at {index}: {keyword:?}"
            ));
        }
        let expected = canonical_spelling_from_variant(keyword);
        if spelling != expected {
            return Err(format!(
                "keyword spelling mismatch for {keyword:?}: '{spelling}' != '{expected}'"
            ));
        }
        if !spellings.insert(spelling) {
            return Err(format!("duplicate keyword spelling '{spelling}'"));
        }
        let lexed = lex_source(spelling);
        if !lexed.diagnostics.complete().is_empty()
            || lexed.tokens.first().map(|token| token.kind) != Some(TokenKind::Keyword(keyword))
            || lexed.tokens.first().map(|token| token.text(spelling)) != Some(spelling)
        {
            return Err(format!(
                "lexer lookup disagrees for {keyword:?} / '{spelling}': {lexed:#?}"
            ));
        }
    }
    Ok(())
}

fn validate_token_inventory(inventory: &[TokenKind]) -> Result<(), String> {
    if inventory != TokenKind::INVENTORY.as_slice() {
        return Err("token inventory differs from the single owner".to_string());
    }
    let mut names = BTreeSet::new();
    for kind in inventory.iter().copied() {
        let actual = kind.inventory_name();
        let debug = format!("{kind:?}");
        let expected = debug
            .split_once('(')
            .map_or(debug.as_str(), |(name, _)| name);
        if actual != expected {
            return Err(format!(
                "token inventory name mismatch for {kind:?}: '{actual}' != '{expected}'"
            ));
        }
        names.insert(actual);
    }
    if names.len() != inventory.len() {
        return Err("token inventory contains duplicate variant names".to_string());
    }
    Ok(())
}

#[test]
fn generated_grammar_is_committed() {
    let path = grammar_path();
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(
        committed,
        render_grammar(),
        "{} drifted from the parser-owned generator; regenerate with \
         'cargo test -p marrow-syntax regenerate_vscode_grammar -- --ignored'",
        path.display()
    );
}

#[test]
fn canonical_keyword_taxonomy_kat() {
    assert_eq!(Keyword::Int.lexical_class(), LexicalClass::BuiltinType);
    assert_eq!(Keyword::True.lexical_class(), LexicalClass::BuiltinValue);
    assert_eq!(Keyword::False.lexical_class(), LexicalClass::BuiltinValue);
    assert_eq!(Keyword::Absent.lexical_class(), LexicalClass::BuiltinValue);

    let rules = keyword_rules();
    let type_words = rules
        .iter()
        .find(|rule| rule.class == LexicalClass::BuiltinType)
        .expect("built-in type rule");
    let value_words = rules
        .iter()
        .find(|rule| rule.class == LexicalClass::BuiltinValue)
        .expect("built-in value rule");
    assert!(type_words.spellings.contains(&"int"));
    assert!(!value_words.spellings.contains(&"int"));
    assert_eq!(
        rules
            .iter()
            .flat_map(|rule| &rule.spellings)
            .filter(|spelling| **spelling == "int")
            .count(),
        1
    );
}

#[test]
fn single_owner_rejects_spelling_and_inventory_mutations() {
    let keyword_facts: Vec<_> = Keyword::ALL
        .into_iter()
        .map(|keyword| (keyword, keyword.spelling()))
        .collect();
    validate_keyword_facts(&keyword_facts).expect("production keyword facts agree");
    validate_token_inventory(&TokenKind::INVENTORY).expect("production token inventory agrees");

    let mut spelling_permutation = keyword_facts.clone();
    let first = spelling_permutation[0].1;
    spelling_permutation[0].1 = spelling_permutation[1].1;
    spelling_permutation[1].1 = first;
    assert!(
        validate_keyword_facts(&spelling_permutation)
            .expect_err("a spelling permutation must fail")
            .contains("spelling mismatch")
    );

    let mut missing_keyword = keyword_facts;
    missing_keyword.pop();
    assert!(
        validate_keyword_facts(&missing_keyword)
            .expect_err("a keyword inventory omission must fail")
            .contains("inventory length")
    );

    let mut missing_token = TokenKind::INVENTORY.to_vec();
    missing_token.pop();
    assert_eq!(
        validate_token_inventory(&missing_token),
        Err("token inventory differs from the single owner".to_string())
    );
}

#[test]
fn keyword_taxonomy_is_total_disjoint_and_parser_owned() {
    let mut seen = BTreeSet::new();
    let rules = keyword_rules();
    for rule in &rules {
        assert!(!rule.spellings.is_empty(), "empty class: {:?}", rule.class);
        for spelling in &rule.spellings {
            assert!(seen.insert(*spelling), "duplicate keyword '{spelling}'");
            assert!(is_reserved_word(spelling));
            assert!(keyword_pattern_matches(&rule.pattern, spelling));
        }
        for keyword in Keyword::ALL {
            assert_eq!(
                keyword_pattern_matches(&rule.pattern, keyword.spelling()),
                keyword.lexical_class() == rule.class,
                "cross-class match for '{}' in {:?}",
                keyword.spelling(),
                rule.class
            );
        }
    }
    assert_eq!(seen.len(), Keyword::ALL.len());
    for keyword in Keyword::ALL {
        assert_ne!(keyword.lexical_class(), LexicalClass::Unscoped);
        assert!(seen.contains(keyword.spelling()));
    }
}

#[test]
fn keyword_patterns_are_longest_first_escaped_and_bounded() {
    for rule in keyword_rules() {
        let alternatives = keyword_alternatives(&rule.pattern);
        assert!(
            alternatives
                .windows(2)
                .all(|pair| pair[0].len() >= pair[1].len()),
            "a longer spelling must render before any prefix of it: {alternatives:?}"
        );
    }

    assert_eq!(
        regex_escape(r"a+b(c)[d]{e}.^$|?*\\"),
        r"a\+b\(c\)\[d\]\{e\}\.\^\$\|\?\*\\\\"
    );
    for rule in keyword_rules() {
        for spelling in rule.spellings {
            assert!(keyword_pattern_matches(&rule.pattern, spelling));
            assert!(keyword_pattern_matches(
                &rule.pattern,
                &format!("({spelling})")
            ));
            for near_miss in [
                format!("x{spelling}"),
                format!("{spelling}x"),
                format!("_{spelling}"),
                format!("{spelling}_"),
            ] {
                assert!(
                    !keyword_pattern_matches(&rule.pattern, &near_miss),
                    "'{near_miss}' overmatched '{spelling}'"
                );
            }
        }
    }
}

#[test]
fn token_taxonomy_is_total_and_contextual_words_are_unscoped() {
    let inventory_names: BTreeSet<_> = TokenKind::INVENTORY
        .into_iter()
        .map(TokenKind::inventory_name)
        .collect();
    assert_eq!(inventory_names.len(), TokenKind::INVENTORY.len());
    assert_eq!(
        TokenKind::Identifier.lexical_class(),
        LexicalClass::Unscoped
    );
    assert_eq!(TokenKind::Newline.lexical_class(), LexicalClass::Unscoped);
    assert_eq!(TokenKind::Eof.lexical_class(), LexicalClass::Unscoped);
    for keyword in Keyword::ALL {
        assert_eq!(
            TokenKind::Keyword(keyword).lexical_class(),
            keyword.lexical_class()
        );
    }

    for spelling in [
        "category", "reversed", "by", "at", "most", "from", "on", "more", "equality", "order",
    ] {
        assert!(!is_reserved_word(spelling), "'{spelling}' became reserved");
        let lexed = lex_source(spelling);
        assert!(
            lexed.diagnostics.complete().is_empty(),
            "{:#?}",
            lexed.diagnostics
        );
        assert_eq!(
            lexed.tokens.first().map(|token| token.kind.lexical_class()),
            Some(LexicalClass::Unscoped),
            "contextual spelling '{spelling}' gained a global role"
        );
    }
}

#[test]
fn literal_and_fixed_form_rules_follow_parser_facts() {
    let grammar = render_grammar();
    for class in [
        LexicalClass::IntegerLiteral,
        LexicalClass::DecimalLiteral,
        LexicalClass::DurationLiteral,
        LexicalClass::StringLiteral,
        LexicalClass::InterpolationString,
        LexicalClass::InterpolationDelimiter,
        LexicalClass::BytesLiteral,
        LexicalClass::Comment,
        LexicalClass::DocumentationComment,
        LexicalClass::Operator,
        LexicalClass::Delimiter,
        LexicalClass::Punctuation,
        LexicalClass::PathSeparator,
        LexicalClass::DurableRootSigil,
    ] {
        assert!(
            grammar.contains(scope(class)),
            "missing scope for {class:?}"
        );
    }

    for (source, expected) in [
        ("42", LexicalClass::IntegerLiteral),
        ("1.5", LexicalClass::DecimalLiteral),
        ("2.days", LexicalClass::DurationLiteral),
        (r#""text""#, LexicalClass::StringLiteral),
        (r#"b"bytes""#, LexicalClass::BytesLiteral),
        ("// comment", LexicalClass::Comment),
        ("/// docs", LexicalClass::DocumentationComment),
        ("::", LexicalClass::PathSeparator),
        ("^", LexicalClass::DurableRootSigil),
        ("??", LexicalClass::Operator),
        ("(", LexicalClass::Delimiter),
        (",", LexicalClass::Punctuation),
    ] {
        let lexed = lex_source(source);
        assert_eq!(
            lexed.tokens.first().map(|token| token.kind.lexical_class()),
            Some(expected),
            "unexpected class for '{source}': {:#?}",
            lexed.tokens
        );
    }

    let duration_alternation = longest_first_alternation(duration_unit_spellings());
    for unit in duration_unit_spellings() {
        assert!(duration_alternation.split('|').any(|part| part == unit));
    }
    assert_ne!(
        lex_source("1.month")
            .tokens
            .first()
            .map(|token| token.kind.lexical_class()),
        Some(LexicalClass::DurationLiteral)
    );

    let interpolation = lex_source(r#"$"hello {value}""#);
    assert!(interpolation.diagnostics.complete().is_empty());
    assert_eq!(
        interpolation
            .tokens
            .iter()
            .map(|token| token.kind.lexical_class())
            .collect::<Vec<_>>(),
        [
            LexicalClass::InterpolationString,
            LexicalClass::InterpolationString,
            LexicalClass::InterpolationDelimiter,
            LexicalClass::Unscoped,
            LexicalClass::InterpolationDelimiter,
            LexicalClass::InterpolationString,
            LexicalClass::Unscoped,
        ]
    );
}

#[test]
fn fixed_form_alternations_are_longest_first() {
    let operators = fixed_token_alternation(LexicalClass::Operator);
    let alternatives: Vec<&str> = operators.split('|').collect();
    for (longer, shorter) in [(r"\.\.=", r"\.\."), (">=", ">"), ("==", "=")] {
        let longer = alternatives
            .iter()
            .position(|spelling| *spelling == longer)
            .expect("long operator");
        let shorter = alternatives
            .iter()
            .position(|spelling| *spelling == shorter)
            .expect("short operator");
        assert!(longer < shorter);
    }
    let grammar = render_grammar();
    assert!(
        grammar.find("#operators").expect("operator include")
            < grammar.find("#punctuation").expect("punctuation include"),
        "operators must win before their punctuation prefixes"
    );
}

#[test]
fn generated_grammar_has_no_semantic_guesses_or_custom_colors() {
    let grammar = render_grammar();
    for forbidden in [
        "keyword.other.marrow",
        "entity.name.function",
        "entity.name.type",
        "variable.other",
        "variable.parameter",
        "support.function",
        "semanticToken",
        "foreground",
        "fontStyle",
    ] {
        assert!(
            !grammar.contains(forbidden),
            "forbidden grammar text '{forbidden}'"
        );
    }
    for spelling in [
        "category", "reversed", "by", "at", "most", "from", "on", "more", "equality", "order",
    ] {
        assert!(
            keyword_rules()
                .iter()
                .all(|rule| !keyword_alternatives(&rule.pattern)
                    .iter()
                    .any(|word| word == spelling)),
            "contextual spelling '{spelling}' entered the generated inventory"
        );
    }
}

#[test]
fn rendering_is_deterministic() {
    assert_eq!(render_grammar(), render_grammar());
}

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
