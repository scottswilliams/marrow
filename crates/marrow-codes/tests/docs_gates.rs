//! Standing documentation gates over every tracked Markdown file.
//!
//! Two invariants, both enforced from the files themselves rather than from
//! prose in the contributor rules:
//!
//! 1. Every relative link target resolves to a path that exists, and every
//!    `#fragment` resolves to a heading anchor in the file it names, under
//!    GitHub's slug rules including duplicate-heading suffixes. External
//!    `http(s)` links are checked for syntactic sanity only: the battery is
//!    offline and must stay offline.
//! 2. No sentence states a banned claim family. The documentation standard
//!    bans these claims everywhere, so `docs/future/` is in scope on the same
//!    terms as the current reference: a future page may record a goal, but it
//!    may not assert the claim.
//!
//! Both scans blank every literal first — fenced code, inline code spans, and
//! HTML comments — so a gate can never be silently disabled by a banned word
//! or a stale link that happens to live inside an example. Blanking preserves
//! byte offsets and line structure, which lets the sentinel below assert that
//! each scan reached the final byte of each file rather than stopping at the
//! first structure it failed to parse.
//!
//! The claim patterns are spelled here, so this file is not itself a scanned
//! subject: only `.md` files are.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Claim families
// ---------------------------------------------------------------------------

/// One banned claim family and the literal phrases that state it. Each phrase
/// matches case-insensitively at word boundaries, so `fast-forward` never
/// matches a speed claim and `instant` as a Marrow type name never matches the
/// speed family: the family spells the claim phrase, not the bare word.
struct ClaimFamily {
    name: &'static str,
    phrases: &'static [&'static str],
}

const CLAIM_FAMILIES: &[ClaimFamily] = &[
    ClaimFamily {
        name: "speed",
        phrases: &[
            "blazing",
            "blazingly fast",
            "lightning fast",
            "lightning-fast",
            "compiles fast",
            "compile fast",
            "tests fast",
            "tests are fast",
            "is fast",
            "are fast",
            "runs fast",
            "extremely fast",
            "very fast",
            "ultra-fast",
            "instantaneous",
            "near-instant",
            "feels instant",
            "feel instant",
            "instant feedback",
            "instant compile",
        ],
    },
    ClaimFamily {
        name: "readiness",
        phrases: &[
            "production-ready",
            "production ready",
            "battle-tested",
            "battle tested",
            "enterprise-grade",
            "industrial-strength",
            "bulletproof",
            "rock-solid",
            "mainframe-grade",
        ],
    },
    ClaimFamily {
        name: "security",
        phrases: &[
            "is secure",
            "are secure",
            "secure by default",
            "fully secure",
            "completely secure",
            "system secure",
            "unhackable",
        ],
    },
    ClaimFamily {
        name: "proof",
        // A proof claim is a claim about quality. Stating a fact the compiler
        // or the protocol decides ("provably impossible", "provably never
        // ran") is evidence, not a claim, so the family pairs the proof word
        // with the adjective it would be selling.
        phrases: &[
            "formally proven",
            "provably secure",
            "provably safe",
            "provably correct",
            "mathematically proven",
            "mathematically guaranteed",
            "proven correct",
            "proven safe",
            "proven secure",
            "compiler-proven",
        ],
    },
    ClaimFamily {
        name: "scale",
        phrases: &["scalable", "web-scale", "web scale", "scales infinitely"],
    },
    ClaimFamily {
        name: "ai-native",
        phrases: &["ai-native", "ai native", "ai-first"],
    },
    ClaimFamily {
        name: "zero-cost",
        phrases: &["zero-cost", "zero cost"],
    },
];

/// Markers that make a claim phrase a mention rather than an assertion: a
/// prohibition ("do not write that Marrow compiles fast") or a governance
/// statement about the word itself ("what may be called production-ready").
/// A marker counts only when it occurs *before* the phrase in the same
/// sentence, so an ordinary claim cannot buy an exemption by mentioning a
/// negation later on.
const MENTION_MARKERS: &[&str] = &[
    "do not",
    "does not",
    "must not",
    "may not",
    "cannot",
    "never",
    "makes no",
    "no measure",
    "no current claim",
    "not a claim",
    "may be called",
    "may be described",
    "described as",
    "called",
    "write that",
    "avoid",
    "forbidden",
    "prohibited",
];

/// Exact `(tracked path, trimmed line text)` pairs the claim gate accepts
/// despite matching a family. Entries are recorded findings pending routing,
/// never a way to keep new prose: each names why the line stands.
const CLAIM_ALLOWLIST: &[(&str, &str)] = &[];

// ---------------------------------------------------------------------------
// Violations
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum ViolationKind {
    MissingTarget {
        target: String,
    },
    MissingAnchor {
        target: String,
        fragment: String,
    },
    MalformedExternal {
        target: String,
    },
    UndefinedReference {
        label: String,
    },
    BannedClaim {
        family: &'static str,
        phrase: String,
    },
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTarget { target } => write!(f, "link target does not exist: {target}"),
            Self::MissingAnchor { target, fragment } => {
                let where_ = if target.is_empty() {
                    "this file"
                } else {
                    target
                };
                write!(f, "no heading anchor `#{fragment}` in {where_}")
            }
            Self::MalformedExternal { target } => write!(f, "malformed external link: {target}"),
            Self::UndefinedReference { label } => {
                write!(f, "undefined link reference label: [{label}]")
            }
            Self::BannedClaim { family, phrase } => {
                write!(f, "banned {family} claim: \"{phrase}\"")
            }
        }
    }
}

#[derive(Debug)]
struct Violation {
    file: String,
    line: usize,
    kind: ViolationKind,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.kind)
    }
}

fn report(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|violation| format!("  {violation}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Literal blanking
// ---------------------------------------------------------------------------

/// Replaces every literal byte with a space while preserving byte offsets,
/// line count, and total length. Blanked regions are fenced code blocks
/// (including their delimiter lines), inline code spans, and HTML comments.
///
/// Length preservation is the sentinel's contract: a scan over the blanked
/// text addresses the same offsets as the original file, so reaching the end
/// of the blanked text proves the scan reached the end of the file.
fn blank_literals(source: &str) -> String {
    let mut bytes = source.as_bytes().to_vec();
    blank_html_comments(&mut bytes);

    let mut fence: Option<(u8, usize)> = None;
    for (start, end) in line_spans(source) {
        let line = &source[start..end];
        match fence {
            Some((marker, width)) => {
                let closes = closing_fence(line, marker, width);
                blank_range(&mut bytes, start, end);
                if closes {
                    fence = None;
                }
            }
            None => match opening_fence(line) {
                Some(open) => {
                    fence = Some(open);
                    blank_range(&mut bytes, start, end);
                }
                None => blank_inline_code(&mut bytes, source, start, end),
            },
        }
    }

    String::from_utf8(bytes).expect("blanking replaces whole ranges with ascii spaces")
}

/// Byte spans of each line, including its trailing newline.
fn line_spans(source: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (offset, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            spans.push((start, offset + 1));
            start = offset + 1;
        }
    }
    if start < source.len() {
        spans.push((start, source.len()));
    }
    spans
}

fn blank_range(bytes: &mut [u8], start: usize, end: usize) {
    for byte in &mut bytes[start..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

fn blank_html_comments(bytes: &mut [u8]) {
    let mut from = 0;
    while let Some(open) = find_bytes(bytes, b"<!--", from) {
        let close = find_bytes(bytes, b"-->", open + 4).map_or(bytes.len(), |at| at + 3);
        blank_range(bytes, open, close);
        from = close;
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

/// The fence character and run width when `line` opens a fenced code block.
fn opening_fence(line: &str) -> Option<(u8, usize)> {
    let trimmed = line.trim_start_matches(' ');
    for marker in [b'`', b'~'] {
        let width = trimmed.bytes().take_while(|byte| *byte == marker).count();
        if width >= 3 {
            return Some((marker, width));
        }
    }
    None
}

fn closing_fence(line: &str, marker: u8, width: usize) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= width
        && trimmed.bytes().all(|byte| byte == marker)
        && trimmed.bytes().take_while(|byte| *byte == marker).count() >= width
}

/// Blanks backtick-delimited inline code spans in one line. A run of `n`
/// backticks opens a span that the next run of exactly `n` backticks closes;
/// an unclosed run is left alone so a stray backtick cannot blank the rest of
/// the line.
fn blank_inline_code(bytes: &mut [u8], source: &str, start: usize, end: usize) {
    let line = &source.as_bytes()[start..end];
    let mut at = 0;
    while at < line.len() {
        if line[at] != b'`' {
            at += 1;
            continue;
        }
        let open_width = line[at..].iter().take_while(|byte| **byte == b'`').count();
        let mut cursor = at + open_width;
        let mut closed = None;
        while cursor < line.len() {
            if line[cursor] == b'`' {
                let width = line[cursor..]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                if width == open_width {
                    closed = Some(cursor + width);
                    break;
                }
                cursor += width;
            } else {
                cursor += 1;
            }
        }
        match closed {
            Some(span_end) => {
                blank_range(bytes, start + at, start + span_end);
                at = span_end;
            }
            None => at += open_width,
        }
    }
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

struct Document {
    /// Path relative to the scan root, in reporting form.
    rel: String,
    path: PathBuf,
    raw: String,
    blanked: String,
    anchors: BTreeSet<String>,
}

impl Document {
    fn load(root: &Path, path: &Path) -> Self {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let blanked = blank_literals(&raw);
        assert_eq!(
            blanked.len(),
            raw.len(),
            "blanking must preserve byte offsets in {}",
            path.display()
        );
        let anchors = heading_anchors(&raw);
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        Self {
            rel,
            path: path.to_path_buf(),
            raw,
            blanked,
            anchors,
        }
    }

    fn line_of(&self, offset: usize) -> usize {
        self.raw[..offset.min(self.raw.len())]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1
    }

    fn line_text(&self, offset: usize) -> &str {
        let capped = offset.min(self.raw.len());
        let start = self.raw[..capped].rfind('\n').map_or(0, |at| at + 1);
        let end = self.raw[capped..]
            .find('\n')
            .map_or(self.raw.len(), |at| capped + at);
        self.raw[start..end].trim()
    }
}

/// Every heading anchor in the file, under GitHub's slug rules: the heading
/// text is lowercased, characters other than letters, digits, `-`, `_`, and
/// spaces are dropped, spaces become hyphens, and a repeated slug takes the
/// next `-1`, `-2`, … suffix.
fn heading_anchors(raw: &str) -> BTreeSet<String> {
    let mut anchors = BTreeSet::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut fence: Option<(u8, usize)> = None;
    for line in raw.lines() {
        match fence {
            Some((marker, width)) => {
                if closing_fence(line, marker, width) {
                    fence = None;
                }
                continue;
            }
            None => {
                if let Some(open) = opening_fence(line) {
                    fence = Some(open);
                    continue;
                }
            }
        }
        let Some(text) = heading_text(line) else {
            continue;
        };
        let slug = slugify(&text);
        if slug.is_empty() {
            continue;
        }
        let count = seen.entry(slug.clone()).or_insert(0);
        let anchor = if *count == 0 {
            slug
        } else {
            format!("{slug}-{count}")
        };
        *count += 1;
        anchors.insert(anchor);
    }
    anchors
}

fn heading_text(line: &str) -> Option<String> {
    let trimmed = line.strip_prefix("   ").unwrap_or(line);
    let trimmed = trimmed.trim_start_matches(' ');
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some(rest.trim().trim_end_matches('#').trim().to_string())
}

/// Reduces heading markup to the text GitHub slugs: link labels replace their
/// link, and emphasis and code delimiters drop out.
fn slugify(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '[' => {}
            ']' => {
                // Drop an inline link's destination, keeping its label.
                if chars.peek() == Some(&'(') {
                    let mut depth = 0usize;
                    for inner in chars.by_ref() {
                        match inner {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            '`' | '*' | '~' => {}
            _ => plain.push(ch),
        }
    }

    let mut slug = String::with_capacity(plain.len());
    for ch in plain.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            slug.push(ch);
        } else if ch == ' ' {
            slug.push('-');
        }
    }
    slug
}

// ---------------------------------------------------------------------------
// Link gate
// ---------------------------------------------------------------------------

struct Link {
    offset: usize,
    target: String,
}

/// Inline link destinations in blanked text, so links inside examples are not
/// subjects. Image links share the inline form and are checked the same way.
fn inline_links(blanked: &str) -> Vec<Link> {
    let bytes = blanked.as_bytes();
    let mut links = Vec::new();
    let mut at = 0;
    while let Some(found) = find_bytes(bytes, b"](", at) {
        let open = found + 2;
        let mut depth = 1usize;
        let mut cursor = open;
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                b'\n' => break,
                _ => {}
            }
            cursor += 1;
        }
        if depth == 0 {
            links.push(Link {
                offset: open,
                target: destination(&blanked[open..cursor]),
            });
            at = cursor + 1;
        } else {
            at = open;
        }
    }
    links
}

/// The destination part of an inline link body: an optional angle-bracketed
/// path, otherwise everything before the optional title.
fn destination(body: &str) -> String {
    let body = body.trim();
    if let Some(rest) = body.strip_prefix('<') {
        return rest.split('>').next().unwrap_or("").to_string();
    }
    body.split_whitespace().next().unwrap_or("").to_string()
}

/// Reference definitions (`[label]: target`) declared in blanked text.
fn reference_definitions(blanked: &str) -> Vec<(String, usize, String)> {
    let mut definitions = Vec::new();
    for (start, end) in line_spans(blanked) {
        let line = blanked[start..end].trim_end();
        let trimmed = line.trim_start();
        if !trimmed.starts_with('[') {
            continue;
        }
        let Some(close) = trimmed.find("]:") else {
            continue;
        };
        let label = trimmed[1..close].trim().to_lowercase();
        let target = destination(&trimmed[close + 2..]);
        if label.is_empty() || target.is_empty() {
            continue;
        }
        definitions.push((label, start, target));
    }
    definitions
}

/// Reference usages (`[text][label]`) in blanked text. Only scanned when the
/// file defines at least one label, so a bracket pair in ordinary prose cannot
/// invent a violation.
fn reference_usages(blanked: &str) -> Vec<(String, usize)> {
    let bytes = blanked.as_bytes();
    let mut usages = Vec::new();
    let mut at = 0;
    while let Some(found) = find_bytes(bytes, b"][", at) {
        let open = found + 2;
        match find_bytes(bytes, b"]", open) {
            Some(close) => {
                usages.push((blanked[open..close].trim().to_lowercase(), open));
                at = close + 1;
            }
            None => break,
        }
    }
    usages
}

/// Normalizes `.`/`..` components without touching the filesystem, so a link
/// is judged by the path it spells.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn is_external(target: &str) -> bool {
    let scheme = target.split(':').next().unwrap_or("");
    !scheme.is_empty()
        && scheme.len() < target.len()
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '.' || ch == '-')
        && scheme.chars().next().is_some_and(char::is_alphabetic)
}

fn external_is_sane(target: &str) -> bool {
    if let Some(rest) = target.strip_prefix("mailto:") {
        return rest.contains('@') && !rest.contains(char::is_whitespace);
    }
    let Some(rest) = target
        .strip_prefix("https://")
        .or_else(|| target.strip_prefix("http://"))
    else {
        // Any other scheme is out of the gate's contract and must be spelled
        // deliberately rather than slipping through unchecked.
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    !host.is_empty() && host.contains('.') && !target.contains(char::is_whitespace)
}

fn check_links(root: &Path, documents: &[Document]) -> Vec<Violation> {
    let by_rel: BTreeMap<&str, &Document> = documents
        .iter()
        .map(|document| (document.rel.as_str(), document))
        .collect();
    let mut violations = Vec::new();

    for document in documents {
        let definitions = reference_definitions(&document.blanked);
        let defined: BTreeSet<&str> = definitions
            .iter()
            .map(|(label, _, _)| label.as_str())
            .collect();
        if !defined.is_empty() {
            for (label, offset) in reference_usages(&document.blanked) {
                if !defined.contains(label.as_str()) {
                    violations.push(Violation {
                        file: document.rel.clone(),
                        line: document.line_of(offset),
                        kind: ViolationKind::UndefinedReference { label },
                    });
                }
            }
        }

        let links = inline_links(&document.blanked).into_iter().chain(
            definitions
                .into_iter()
                .map(|(_, offset, target)| Link { offset, target }),
        );

        for link in links {
            check_one_link(root, document, &by_rel, &link, &mut violations);
        }
    }

    violations
}

fn check_one_link(
    root: &Path,
    document: &Document,
    by_rel: &BTreeMap<&str, &Document>,
    link: &Link,
    violations: &mut Vec<Violation>,
) {
    let target = link.target.as_str();
    if target.is_empty() {
        return;
    }
    let line = document.line_of(link.offset);
    let push = |violations: &mut Vec<Violation>, kind| {
        violations.push(Violation {
            file: document.rel.clone(),
            line,
            kind,
        });
    };

    if is_external(target) {
        if !external_is_sane(target) {
            push(
                violations,
                ViolationKind::MalformedExternal {
                    target: target.to_string(),
                },
            );
        }
        return;
    }

    let (path_part, fragment) = match target.split_once('#') {
        Some((path_part, fragment)) => (path_part, Some(fragment)),
        None => (target, None),
    };

    if path_part.is_empty() {
        if let Some(fragment) = fragment
            && !document.anchors.contains(fragment)
        {
            push(
                violations,
                ViolationKind::MissingAnchor {
                    target: String::new(),
                    fragment: fragment.to_string(),
                },
            );
        }
        return;
    }

    let parent = document.path.parent().unwrap_or(root);
    let resolved = normalize(&parent.join(path_part));
    if !resolved.exists() {
        push(
            violations,
            ViolationKind::MissingTarget {
                target: target.to_string(),
            },
        );
        return;
    }

    let Some(fragment) = fragment else {
        return;
    };
    let rel = resolved
        .strip_prefix(root)
        .unwrap_or(&resolved)
        .to_string_lossy()
        .replace('\\', "/");
    match by_rel.get(rel.as_str()) {
        Some(other) if other.anchors.contains(fragment) => {}
        Some(_) | None => push(
            violations,
            ViolationKind::MissingAnchor {
                target: rel,
                fragment: fragment.to_string(),
            },
        ),
    }
}

// ---------------------------------------------------------------------------
// Claim gate
// ---------------------------------------------------------------------------

/// Sentence spans of blanked text, as `(offset, lowercased sentence)`. A
/// sentence ends at `.`, `!`, or `?` followed by whitespace, at a blank line,
/// or at end of input, so a claim in the last bytes of a file with no trailing
/// newline is still a subject.
fn sentences(blanked: &str) -> Vec<(usize, String)> {
    let bytes = blanked.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        let terminator = matches!(bytes[at], b'.' | b'!' | b'?')
            && bytes
                .get(at + 1)
                .is_none_or(|byte| byte.is_ascii_whitespace());
        let paragraph_break = bytes[at] == b'\n' && bytes.get(at + 1) == Some(&b'\n');
        if terminator || paragraph_break {
            let end = at + 1;
            spans.push((start, blanked[start..end].to_lowercase()));
            start = end;
        }
        at += 1;
    }
    if start < bytes.len() {
        spans.push((start, blanked[start..].to_lowercase()));
    }
    spans
}

fn word_boundary_match(haystack: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'));
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'));
        if before_ok && after_ok {
            return Some(start);
        }
        from = end;
    }
    None
}

fn mentioned_not_asserted(sentence: &str, phrase_at: usize) -> bool {
    MENTION_MARKERS
        .iter()
        .any(|marker| sentence[..phrase_at].contains(marker))
}

fn check_claims(documents: &[Document]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for document in documents {
        for (offset, sentence) in sentences(&document.blanked) {
            for family in CLAIM_FAMILIES {
                for phrase in family.phrases {
                    let Some(at) = word_boundary_match(&sentence, phrase) else {
                        continue;
                    };
                    if mentioned_not_asserted(&sentence, at) {
                        continue;
                    }
                    let absolute = offset + at;
                    let line_text = document.line_text(absolute);
                    if CLAIM_ALLOWLIST
                        .iter()
                        .any(|(file, text)| *file == document.rel && *text == line_text)
                    {
                        continue;
                    }
                    violations.push(Violation {
                        file: document.rel.clone(),
                        line: document.line_of(absolute),
                        kind: ViolationKind::BannedClaim {
                            family: family.name,
                            phrase: (*phrase).to_string(),
                        },
                    });
                }
            }
        }
    }
    violations
}

/// The last byte offset the claim scan addressed. Equal to the blanked length
/// exactly when the scan reached the end of the file.
fn claim_scan_end(blanked: &str) -> usize {
    sentences(blanked)
        .last()
        .map_or(0, |(offset, sentence)| offset + sentence.len())
}

// ---------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow-codes`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .to_path_buf()
}

fn tracked_markdown(root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "*.md"])
        .output()
        .unwrap_or_else(|error| panic!("run git ls-files: {error}"));
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing = String::from_utf8(output.stdout).expect("git output is utf-8");
    let files: Vec<PathBuf> = listing.lines().map(|line| root.join(line)).collect();
    assert!(
        !files.is_empty(),
        "no tracked markdown found under {}",
        root.display()
    );
    files
}

fn corpus() -> (PathBuf, Vec<Document>) {
    let root = workspace_root();
    let documents = tracked_markdown(&root)
        .iter()
        .map(|path| Document::load(&root, path))
        .collect();
    (root, documents)
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

#[test]
fn every_documentation_link_and_anchor_resolves() {
    let (root, documents) = corpus();
    let violations = check_links(&root, &documents);
    assert!(
        violations.is_empty(),
        "unresolved documentation links:\n{}",
        report(&violations)
    );
}

#[test]
fn no_documentation_states_a_banned_claim_family() {
    let (_root, documents) = corpus();
    let violations = check_claims(&documents);
    assert!(
        violations.is_empty(),
        "banned claim families in documentation:\n{}",
        report(&violations)
    );
}

#[test]
fn every_scanned_file_is_read_to_its_final_byte() {
    let (_root, documents) = corpus();
    for document in &documents {
        assert_eq!(
            document.blanked.len(),
            document.raw.len(),
            "{}: blanking changed the file length",
            document.rel
        );
        assert_eq!(
            claim_scan_end(&document.blanked),
            document.blanked.len(),
            "{}: the claim scan stopped before the final byte",
            document.rel
        );
    }
}

// ---------------------------------------------------------------------------
// Plant probes
// ---------------------------------------------------------------------------

/// A scratch corpus outside the repository, so a probe exercises the real
/// filesystem-resolving link gate rather than a stub.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "marrow-docs-gate-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create scratch corpus");
        Self { root }
    }

    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create scratch directory");
        }
        std::fs::write(&path, contents).expect("write scratch document");
        path
    }

    fn documents(&self, paths: &[PathBuf]) -> Vec<Document> {
        paths
            .iter()
            .map(|path| Document::load(&self.root, path))
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn the_link_gate_detects_every_failure_direction() {
    let scratch = Scratch::new("links");
    let target = scratch.write("target.md", "# Alpha\n\n## Alpha\n\nBody.\n");
    let subject = scratch.write(
        "subject.md",
        concat!(
            "# Subject\n\n",
            "[gone](missing.md)\n",
            "[bad fragment](target.md#nope)\n",
            "[third duplicate](target.md#alpha-2)\n",
            "[first duplicate](target.md#alpha-1)\n",
            "[self](#subject)\n",
            "[fenced is exempt]\n\n",
            "```\n[ignored](also-missing.md)\n```\n\n",
            "`[inline](never-here.md)`\n",
            "[external](https://example.com/x)\n",
            "[malformed](https:///)\n",
        ),
    );
    let documents = scratch.documents(&[target, subject]);
    let violations = check_links(&scratch.root, &documents);
    let rendered = report(&violations);

    assert!(
        rendered.contains("link target does not exist: missing.md"),
        "broken link undetected:\n{rendered}"
    );
    assert!(
        rendered.contains("no heading anchor `#nope` in target.md"),
        "broken fragment undetected:\n{rendered}"
    );
    assert!(
        rendered.contains("no heading anchor `#alpha-2` in target.md"),
        "duplicate-heading fragment miss undetected:\n{rendered}"
    );
    assert!(
        rendered.contains("malformed external link"),
        "malformed external link undetected:\n{rendered}"
    );
    assert!(
        !rendered.contains("alpha-1"),
        "the duplicate-heading suffix must resolve:\n{rendered}"
    );
    assert!(
        !rendered.contains("#subject"),
        "a same-file anchor must resolve:\n{rendered}"
    );
    assert!(
        !rendered.contains("also-missing.md") && !rendered.contains("never-here.md"),
        "a link inside a literal is not a subject:\n{rendered}"
    );
    assert!(
        !rendered.contains("example.com"),
        "a well-formed external link is not fetched or faulted:\n{rendered}"
    );
    assert_eq!(violations.len(), 4, "unexpected violations:\n{rendered}");
}

#[test]
fn the_claim_gate_detects_every_failure_direction() {
    let scratch = Scratch::new("claims");
    // The banned sentence sits in the final bytes with no trailing newline, so
    // a scan that stops early cannot pass this probe.
    let subject = scratch.write(
        "subject.md",
        concat!(
            "# Subject\n\n",
            "Marrow is production-ready today.\n\n",
            "Do not write that Marrow compiles fast in public documentation.\n\n",
            "```\nThe compiler is blazingly fast.\n```\n\n",
            "The type `instant` names a point in time.\n\n",
            "What may be called production-ready is governed by the status page.\n\n",
            "The runtime is secure",
        ),
    );
    let documents = scratch.documents(&[subject]);
    let violations = check_claims(&documents);
    let rendered = report(&violations);

    assert!(
        rendered.contains("banned readiness claim: \"production-ready\""),
        "asserted readiness claim undetected:\n{rendered}"
    );
    assert!(
        rendered.contains("banned security claim: \"is secure\""),
        "a claim in the final bytes undetected:\n{rendered}"
    );
    assert!(
        !rendered.contains("blazing"),
        "a claim inside a code fence is not a subject:\n{rendered}"
    );
    assert!(
        !rendered.contains("speed"),
        "a prohibition and an inline-code phrase are not claims:\n{rendered}"
    );
    assert_eq!(violations.len(), 2, "unexpected violations:\n{rendered}");

    let lines: Vec<usize> = violations.iter().map(|violation| violation.line).collect();
    assert_eq!(lines, vec![3, 15], "claims are reported at their line");
}

#[test]
fn literal_blanking_preserves_offsets_and_hides_only_literals() {
    let source = "a `code` b\n\n```rust\nlet x = 1;\n```\n\n<!-- note -->tail\n";
    let blanked = blank_literals(source);
    assert_eq!(blanked.len(), source.len());
    assert!(blanked.contains("a        b"), "{blanked:?}");
    assert!(!blanked.contains("let x"), "{blanked:?}");
    assert!(!blanked.contains("note"), "{blanked:?}");
    assert!(blanked.contains("tail"), "{blanked:?}");
    assert_eq!(blanked.lines().count(), source.lines().count());
}

#[test]
fn heading_slugs_follow_the_duplicate_suffix_rule() {
    let anchors = heading_anchors(
        "# Types and Values\n## The `mw` Command\n## Types and Values\n## Types and Values\n\n```\n# Not a heading\n```\n",
    );
    let expected: BTreeSet<String> = [
        "types-and-values",
        "the-mw-command",
        "types-and-values-1",
        "types-and-values-2",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(anchors, expected);
}

/// Anti-vacuity floor. A parser regression that silently found no links or no
/// headings would make the resolution gate pass by scanning nothing, so the
/// corpus's measured shape is asserted alongside the invariant it feeds.
#[test]
fn the_link_gate_has_a_corpus_to_resolve() {
    let (_root, documents) = corpus();
    let links: Vec<Link> = documents
        .iter()
        .flat_map(|document| inline_links(&document.blanked))
        .collect();
    let fragments = links
        .iter()
        .filter(|link| link.target.contains('#'))
        .count();
    let anchors: usize = documents
        .iter()
        .map(|document| document.anchors.len())
        .sum();
    assert!(documents.len() >= 50, "documents: {}", documents.len());
    assert!(links.len() >= 200, "links: {}", links.len());
    assert!(fragments >= 50, "fragment links: {fragments}");
    assert!(anchors >= 300, "heading anchors: {anchors}");
}
