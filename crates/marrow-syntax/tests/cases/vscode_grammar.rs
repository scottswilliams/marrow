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

use std::collections::BTreeSet;
use std::ops::Range;
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

fn keyword_pattern_matches(pattern: &str, source: &str) -> bool {
    let alternatives = keyword_alternatives(pattern);
    lex_source(source).tokens.into_iter().any(|token| {
        matches!(token.kind, TokenKind::Keyword(_))
            && alternatives.iter().any(|word| word == token.text(source))
            && match_restricted_pattern(pattern, source, token.span.start_byte, token.span.end_byte)
                == Some(token.span.end_byte)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternEntry {
    Include(String),
    Match {
        scope: Option<String>,
        pattern: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopedSpan {
    span: Range<usize>,
    scope: String,
}

fn json_string(text: &str) -> (String, usize) {
    assert!(text.starts_with('"'), "JSON string must start with a quote");
    let mut value = String::new();
    let mut chars = text[1..].char_indices();
    while let Some((offset, ch)) = chars.next() {
        match ch {
            '"' => return (value, offset + 2),
            '\\' => {
                let (_, escaped) = chars.next().expect("complete JSON escape");
                match escaped {
                    '"' | '\\' | '/' => value.push(escaped),
                    'b' => value.push('\u{0008}'),
                    'f' => value.push('\u{000c}'),
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    other => panic!("unsupported JSON escape '\\{other}'"),
                }
            }
            other => value.push(other),
        }
    }
    panic!("unterminated JSON string")
}

fn json_string_field_opt(object: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":");
    let rest = object
        .get(object.find(&marker)? + marker.len()..)?
        .trim_start();
    Some(json_string(rest).0)
}

fn json_string_field(object: &str, field: &str) -> String {
    json_string_field_opt(object, field)
        .unwrap_or_else(|| panic!("missing JSON string field '{field}'"))
}

fn json_string_fields(object: &str, field: &str) -> Vec<String> {
    let marker = format!("\"{field}\":");
    let mut values = Vec::new();
    let mut rest = object;
    while let Some(index) = rest.find(&marker) {
        rest = &rest[index + marker.len()..];
        let trimmed = rest.trim_start();
        let (value, consumed) = json_string(trimmed);
        values.push(value);
        rest = &trimmed[consumed..];
    }
    values
}

fn repository_object<'a>(grammar: &'a str, name: &str) -> &'a str {
    let marker = format!("\"{name}\": {{");
    let key = grammar
        .find(&marker)
        .unwrap_or_else(|| panic!("missing grammar repository '{name}'"));
    let open = grammar[key..].find('{').expect("repository object") + key;
    let bytes = grammar.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for index in open..bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &grammar[open..=index];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated grammar repository '{name}'")
}

fn pattern_entries(object: &str) -> Vec<PatternEntry> {
    object
        .lines()
        .filter_map(|line| {
            if line.contains("\"include\":") {
                return Some(PatternEntry::Include(json_string_field(line, "include")));
            }
            if line.contains("\"match\":") {
                return Some(PatternEntry::Match {
                    scope: json_string_field_opt(line, "name"),
                    pattern: json_string_field(line, "match"),
                });
            }
            None
        })
        .collect()
}

fn emitted_patterns(grammar: &str) -> BTreeSet<String> {
    grammar
        .lines()
        .flat_map(|line| {
            ["match", "begin", "end"]
                .into_iter()
                .filter_map(|field| json_string_field_opt(line, field))
        })
        .collect()
}

fn is_word_at(source: &str, index: usize) -> bool {
    source
        .as_bytes()
        .get(index)
        .is_some_and(u8::is_ascii_alphanumeric)
        || source.as_bytes().get(index) == Some(&b'_')
}

fn is_boundary(source: &str, index: usize) -> bool {
    let left = index
        .checked_sub(1)
        .is_some_and(|left| is_word_at(source, left));
    let right = is_word_at(source, index);
    left != right
}

fn digits_end(source: &str, start: usize, limit: usize) -> usize {
    source.as_bytes()[start..limit]
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .map_or(limit, |offset| start + offset)
}

fn literal_alternation_matches(
    body: &str,
    source: &str,
    start: usize,
    limit: usize,
) -> Option<usize> {
    body.split('|')
        .map(|alternative| {
            regex_literal(alternative)
                .unwrap_or_else(|| panic!("unsupported restricted alternative '{alternative}'"))
        })
        .find_map(|literal| {
            source[start..limit]
                .starts_with(&literal)
                .then_some(start + literal.len())
        })
}

fn regex_literal(pattern: &str) -> Option<String> {
    let mut literal = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            literal.push(chars.next()?);
        } else if matches!(
            ch,
            '.' | '^' | '$' | '|' | '?' | '*' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
        ) {
            return None;
        } else {
            literal.push(ch);
        }
    }
    Some(literal)
}

fn match_escape(source: &str, start: usize, limit: usize, bytes: bool) -> Option<usize> {
    let tail = source.get(start..limit)?;
    if !tail.starts_with('\\') {
        return None;
    }
    let rest = &tail[1..];
    if let Some(simple) = rest.chars().next()
        && matches!(simple, '\\' | '"' | 'n' | 'r' | 't')
    {
        return Some(start + 1 + simple.len_utf8());
    }
    if bytes && rest.starts_with('x') {
        let hex = rest.as_bytes().get(1..3)?;
        return hex.iter().all(u8::is_ascii_hexdigit).then_some(start + 4);
    }
    if !bytes && rest.starts_with("u{") {
        let close = rest.find('}')?;
        let hex = &rest[2..close];
        return (!hex.is_empty()
            && hex.len() <= 6
            && hex.as_bytes().iter().all(u8::is_ascii_hexdigit))
        .then_some(start + 1 + close + 1);
    }
    None
}

fn match_restricted_pattern(
    pattern: &str,
    source: &str,
    start: usize,
    limit: usize,
) -> Option<usize> {
    if start > limit || !source.is_char_boundary(start) || !source.is_char_boundary(limit) {
        return None;
    }
    if pattern == "///.*$" {
        return source[start..].starts_with("///").then_some(source.len());
    }
    if pattern == "//.*$" {
        return source[start..].starts_with("//").then_some(source.len());
    }
    if matches!(
        pattern,
        r"///[^\r\n]*" | r"//[^\r\n]*" | r"///[^}\r\n]*" | r"//[^}\r\n]*"
    ) {
        let prefix = if pattern.starts_with("///") {
            "///"
        } else {
            "//"
        };
        if !source[start..limit].starts_with(prefix) {
            return None;
        }
        let body = &source[start + prefix.len()..limit];
        let end = body
            .char_indices()
            .find(|(_, ch)| matches!(ch, '\r' | '\n') || (pattern.contains("[^}") && *ch == '}'))
            .map_or(limit, |(offset, _)| start + prefix.len() + offset);
        return Some(end);
    }
    if pattern == r#"\\(u\{[0-9A-Fa-f]{1,6}\}|[\\"nrt])"# {
        return match_escape(source, start, limit, false);
    }
    if pattern == r#"\\(x[0-9A-Fa-f]{2}|[\\"nrt])"# {
        return match_escape(source, start, limit, true);
    }
    if pattern == r"\{\{|\}\}" {
        return literal_alternation_matches(pattern, source, start, limit);
    }
    if pattern == r"\b[0-9]+\b" || pattern == r"[0-9]+" {
        if pattern.starts_with(r"\b") && !is_boundary(source, start) {
            return None;
        }
        let end = digits_end(source, start, limit);
        if end == start || (pattern.ends_with(r"\b") && !is_boundary(source, end)) {
            return None;
        }
        return Some(end);
    }
    if pattern == r"\b[0-9]+\.[0-9]+\b" || pattern == r"[0-9]+\.[0-9]+" {
        if pattern.starts_with(r"\b") && !is_boundary(source, start) {
            return None;
        }
        let whole_end = digits_end(source, start, limit);
        if source.as_bytes().get(whole_end) != Some(&b'.') {
            return None;
        }
        let end = digits_end(source, whole_end + 1, limit);
        if end == whole_end + 1 || (pattern.ends_with(r"\b") && !is_boundary(source, end)) {
            return None;
        }
        return Some(end);
    }
    let (duration_prefix, duration_suffix) =
        if pattern.starts_with(r"\b[0-9]+\.(") && pattern.ends_with(r")\b") {
            (r"\b[0-9]+\.(", r")\b")
        } else if pattern.starts_with(r"[0-9]+\.(") && pattern.ends_with(r")(?![A-Za-z0-9_])") {
            (r"[0-9]+\.(", r")(?![A-Za-z0-9_])")
        } else {
            ("", "")
        };
    if !duration_prefix.is_empty() {
        if duration_prefix.starts_with(r"\b") && !is_boundary(source, start) {
            return None;
        }
        let whole_end = digits_end(source, start, limit);
        if source.as_bytes().get(whole_end) != Some(&b'.') {
            return None;
        }
        let body = &pattern[duration_prefix.len()..pattern.len() - duration_suffix.len()];
        let end = literal_alternation_matches(body, source, whole_end + 1, limit)?;
        if duration_suffix == r")\b" && !is_boundary(source, end) {
            return None;
        }
        if duration_suffix.contains("?!") && is_word_at(source, end) {
            return None;
        }
        return Some(end);
    }
    if pattern == r"[A-Za-z_][A-Za-z0-9_]*" {
        let first = *source.as_bytes().get(start)?;
        if !(first == b'_' || first.is_ascii_alphabetic()) {
            return None;
        }
        let end = source.as_bytes()[start + 1..limit]
            .iter()
            .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
            .map_or(limit, |offset| start + 1 + offset);
        return Some(end);
    }
    if pattern.starts_with(r"\b(") && pattern.ends_with(r")\b") {
        if !is_boundary(source, start) {
            return None;
        }
        let body = &pattern[3..pattern.len() - 3];
        let end = literal_alternation_matches(body, source, start, limit)?;
        return is_boundary(source, end).then_some(end);
    }
    if pattern.starts_with('(') && pattern.ends_with(r")(?![A-Za-z0-9_])") {
        let body = &pattern[1..pattern.len() - r")(?![A-Za-z0-9_])".len()];
        let end = literal_alternation_matches(body, source, start, limit)?;
        return (!is_word_at(source, end)).then_some(end);
    }
    if pattern == r#""|(?=$)"# {
        return (source.as_bytes().get(start) == Some(&b'"')).then_some(start + 1);
    }
    if pattern.starts_with('(') && pattern.ends_with(')') {
        return literal_alternation_matches(&pattern[1..pattern.len() - 1], source, start, limit);
    }
    let literal = regex_literal(pattern)
        .unwrap_or_else(|| panic!("unsupported emitted restricted pattern '{pattern}'"));
    source[start..limit]
        .starts_with(&literal)
        .then_some(start + literal.len())
}

struct TextMateOracle<'a> {
    grammar: &'a str,
    spans: Vec<ScopedSpan>,
    attempted: BTreeSet<String>,
}

impl<'a> TextMateOracle<'a> {
    fn new(grammar: &'a str) -> Self {
        Self {
            grammar,
            spans: Vec::new(),
            attempted: BTreeSet::new(),
        }
    }

    fn tokenize(mut self, source: &str) -> (Vec<ScopedSpan>, BTreeSet<String>) {
        let mut index = 0usize;
        while index < source.len() {
            if let Some(end) = self.try_repository("expression", source, index, source.len()) {
                assert!(end > index, "TextMate rule did not consume input");
                index = end;
            } else {
                index += source[index..]
                    .chars()
                    .next()
                    .expect("source character")
                    .len_utf8();
            }
        }
        (self.spans, self.attempted)
    }

    fn try_pattern(
        &mut self,
        pattern: &str,
        source: &str,
        start: usize,
        limit: usize,
    ) -> Option<usize> {
        self.attempted.insert(pattern.to_string());
        match_restricted_pattern(pattern, source, start, limit)
    }

    fn try_repository(
        &mut self,
        name: &str,
        source: &str,
        start: usize,
        limit: usize,
    ) -> Option<usize> {
        if matches!(
            name,
            "double-string"
                | "bytes-string"
                | "escaped-hole-string"
                | "interpolation"
                | "interpolation-hole"
        ) {
            return self.try_region(name, source, start, limit);
        }
        let object = repository_object(self.grammar, name).to_string();
        for entry in pattern_entries(&object) {
            match entry {
                PatternEntry::Include(include) => {
                    if let Some(end) =
                        self.try_repository(include.trim_start_matches('#'), source, start, limit)
                    {
                        return Some(end);
                    }
                }
                PatternEntry::Match { scope, pattern } => {
                    if let Some(end) = self.try_pattern(&pattern, source, start, limit) {
                        if let Some(scope) = scope {
                            self.spans.push(ScopedSpan {
                                span: start..end,
                                scope,
                            });
                        }
                        return Some(end);
                    }
                }
            }
        }
        None
    }

    fn try_region(
        &mut self,
        name: &str,
        source: &str,
        start: usize,
        limit: usize,
    ) -> Option<usize> {
        let object = repository_object(self.grammar, name).to_string();
        let begin = json_string_field(&object, "begin");
        let after_begin = self.try_pattern(&begin, source, start, limit)?;
        if name == "interpolation" {
            return Some(self.scan_interpolation(&object, source, start, after_begin, limit));
        }
        if name == "interpolation-hole" {
            return Some(self.scan_interpolation_hole(&object, source, start, after_begin, limit));
        }
        Some(self.scan_simple_region(&object, source, start, after_begin, limit))
    }

    fn scan_simple_region(
        &mut self,
        object: &str,
        source: &str,
        start: usize,
        mut index: usize,
        limit: usize,
    ) -> usize {
        let end_pattern = json_string_field(object, "end");
        let region_scope = json_string_field(object, "name");
        let entries = pattern_entries(object);
        while index < limit {
            if let Some(end) = self.try_pattern(&end_pattern, source, index, limit) {
                self.spans.push(ScopedSpan {
                    span: start..end,
                    scope: region_scope,
                });
                return end;
            }
            let mut consumed = None;
            for entry in &entries {
                match entry {
                    PatternEntry::Include(include) => {
                        consumed = self.try_repository(
                            include.trim_start_matches('#'),
                            source,
                            index,
                            limit,
                        );
                    }
                    PatternEntry::Match { scope, pattern } => {
                        consumed = self.try_pattern(pattern, source, index, limit);
                        if let (Some(end), Some(scope)) = (consumed, scope) {
                            self.spans.push(ScopedSpan {
                                span: index..end,
                                scope: scope.clone(),
                            });
                        }
                    }
                }
                if consumed.is_some() {
                    break;
                }
            }
            index = consumed.unwrap_or_else(|| {
                index
                    + source[index..limit]
                        .chars()
                        .next()
                        .expect("region character")
                        .len_utf8()
            });
        }
        self.spans.push(ScopedSpan {
            span: start..limit,
            scope: region_scope,
        });
        limit
    }

    fn scan_interpolation(
        &mut self,
        object: &str,
        source: &str,
        start: usize,
        mut index: usize,
        limit: usize,
    ) -> usize {
        let end_pattern = json_string_field(object, "end");
        let region_scope = json_string_field(object, "name");
        let entries = pattern_entries(object);
        let mut segment_start = start;
        while index < limit {
            if let Some(end) = self.try_pattern(&end_pattern, source, index, limit) {
                self.spans.push(ScopedSpan {
                    span: segment_start..end,
                    scope: region_scope,
                });
                return end;
            }
            let mut consumed = None;
            for entry in &entries {
                match entry {
                    PatternEntry::Include(include) if include == "#interpolation-hole" => {
                        consumed = self.try_repository("interpolation-hole", source, index, limit);
                        if let Some(end) = consumed {
                            if segment_start < index {
                                self.spans.push(ScopedSpan {
                                    span: segment_start..index,
                                    scope: region_scope.clone(),
                                });
                            }
                            segment_start = end;
                        }
                    }
                    PatternEntry::Include(include) => {
                        consumed = self.try_repository(
                            include.trim_start_matches('#'),
                            source,
                            index,
                            limit,
                        );
                    }
                    PatternEntry::Match { scope, pattern } => {
                        consumed = self.try_pattern(pattern, source, index, limit);
                        if let (Some(end), Some(scope)) = (consumed, scope) {
                            self.spans.push(ScopedSpan {
                                span: index..end,
                                scope: scope.clone(),
                            });
                        }
                    }
                }
                if consumed.is_some() {
                    break;
                }
            }
            index = consumed.unwrap_or_else(|| {
                index
                    + source[index..limit]
                        .chars()
                        .next()
                        .expect("interpolation character")
                        .len_utf8()
            });
        }
        if segment_start < limit {
            self.spans.push(ScopedSpan {
                span: segment_start..limit,
                scope: region_scope,
            });
        }
        limit
    }

    fn scan_interpolation_hole(
        &mut self,
        object: &str,
        source: &str,
        start: usize,
        mut index: usize,
        limit: usize,
    ) -> usize {
        let end_pattern = json_string_field(object, "end");
        let names = json_string_fields(object, "name");
        let delimiter_scope = names
            .into_iter()
            .find(|name| name.starts_with("punctuation.section.embedded"))
            .expect("interpolation delimiter capture scope");
        self.spans.push(ScopedSpan {
            span: start..index,
            scope: delimiter_scope.clone(),
        });
        let entries = pattern_entries(object);
        while index < limit {
            if let Some(end) = self.try_pattern(&end_pattern, source, index, limit) {
                self.spans.push(ScopedSpan {
                    span: index..end,
                    scope: delimiter_scope,
                });
                return end;
            }
            let mut consumed = None;
            for entry in &entries {
                match entry {
                    PatternEntry::Include(include) => {
                        consumed = self.try_repository(
                            include.trim_start_matches('#'),
                            source,
                            index,
                            limit,
                        );
                    }
                    PatternEntry::Match { scope, pattern } => {
                        consumed = self.try_pattern(pattern, source, index, limit);
                        if let (Some(end), Some(scope)) = (consumed, scope) {
                            self.spans.push(ScopedSpan {
                                span: index..end,
                                scope: scope.clone(),
                            });
                        }
                    }
                }
                if consumed.is_some() {
                    break;
                }
            }
            index = consumed.unwrap_or_else(|| {
                index
                    + source[index..limit]
                        .chars()
                        .next()
                        .expect("hole character")
                        .len_utf8()
            });
        }
        limit
    }
}

fn lexical_scopes() -> BTreeSet<&'static str> {
    [
        LexicalClass::ControlFlow,
        LexicalClass::Declaration,
        LexicalClass::Modifier,
        LexicalClass::Effect,
        LexicalClass::BuiltinType,
        LexicalClass::BuiltinValue,
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
        LexicalClass::WordOperator,
        LexicalClass::Delimiter,
        LexicalClass::Punctuation,
        LexicalClass::PathSeparator,
        LexicalClass::DurableRootSigil,
    ]
    .into_iter()
    .map(scope)
    .collect()
}

fn assert_emitted_scopes_match_lexer(source: &str, attempted: &mut BTreeSet<String>) {
    let grammar = render_grammar();
    let (spans, fixture_attempted) = TextMateOracle::new(&grammar).tokenize(source);
    attempted.extend(fixture_attempted);
    let lexed = lex_source(source);
    assert!(
        lexed.diagnostics.is_empty(),
        "fixture must be lexer-valid: {source:?}: {:#?}",
        lexed.diagnostics
    );
    let owned_scopes = lexical_scopes();
    for token in lexed.tokens {
        if matches!(token.kind, TokenKind::Newline | TokenKind::Eof) {
            continue;
        }
        let expected = textmate_scope(token.kind.lexical_class());
        for byte in token.span.start_byte..token.span.end_byte {
            let actual: BTreeSet<&str> = spans
                .iter()
                .filter(|span| {
                    span.span.contains(&byte) && owned_scopes.contains(span.scope.as_str())
                })
                .map(|span| span.scope.as_str())
                .collect();
            match expected {
                Some(expected) => assert_eq!(
                    actual,
                    BTreeSet::from([expected]),
                    "emitted grammar disagrees at byte {byte} of {source:?} for {:?}; spans={spans:#?}",
                    token.kind
                ),
                None => assert!(
                    actual.is_empty(),
                    "unscoped token {:?} gained {actual:?} at byte {byte} of {source:?}; spans={spans:#?}",
                    token.kind
                ),
            }
        }
    }
}

fn canonical_spelling_from_variant(keyword: Keyword) -> String {
    let variant = format!("{keyword:?}");
    match keyword {
        Keyword::Error | Keyword::ErrorCode | Keyword::Id => variant,
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
        if !lexed.diagnostics.is_empty()
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
fn generated_reserved_words_are_not_collapsed_into_one_scope() {
    let grammar = render_grammar();
    assert!(
        !grammar.contains("keyword.other.marrow"),
        "the shipped generator still collapses every reserved word into keyword.other.marrow"
    );
    assert_eq!(keyword_rules().len(), KEYWORD_CLASSES.len());
}

#[test]
fn canonical_keyword_taxonomy_kat() {
    assert_eq!(Keyword::Unknown.lexical_class(), LexicalClass::BuiltinType);
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
    assert!(type_words.spellings.contains(&"unknown"));
    assert!(!value_words.spellings.contains(&"unknown"));
    assert_eq!(
        rules
            .iter()
            .flat_map(|rule| &rule.spellings)
            .filter(|spelling| **spelling == "unknown")
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
fn emitted_textmate_patterns_match_lexer_tokens() {
    let mut attempted = BTreeSet::new();
    for source in [
        "42abc 1.2x 2.days 3.daysx 4.month",
        r#"b"x" ab"x" pub"x" b"\x41\n" "a\"b\u{41}""#,
        r#"$"plain {{ brace }} {name} tail""#,
        r#"$"escaped {\"}\"} tail""#,
        r#"$"comment {value // note} tail""#,
        "// ordinary comment",
        "/// documentation comment",
        "not and or is fn pub unknown true absent",
        "() [] {} => : :: , . .. ..= = == != ? ?. ?? < <= > >= + - * / % += -= *= /= %= ^root",
    ] {
        assert_emitted_scopes_match_lexer(source, &mut attempted);
    }
    assert_eq!(
        attempted,
        emitted_patterns(&render_grammar()),
        "every emitted restricted pattern must execute in the oracle corpus"
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
    let type_rule = keyword_rules()
        .into_iter()
        .find(|rule| rule.class == LexicalClass::BuiltinType)
        .expect("built-in type rule");
    let alternatives = keyword_alternatives(&type_rule.pattern);
    let error_code = alternatives
        .iter()
        .position(|word| word == "ErrorCode")
        .expect("ErrorCode");
    let error = alternatives
        .iter()
        .position(|word| word == "Error")
        .expect("Error");
    assert!(error_code < error, "longest live prefix must render first");

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
        assert!(lexed.diagnostics.is_empty(), "{:#?}", lexed.diagnostics);
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
    assert!(interpolation.diagnostics.is_empty());
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
