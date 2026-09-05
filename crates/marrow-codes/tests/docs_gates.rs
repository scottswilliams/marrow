//! Standing documentation gates over every tracked Markdown file.
//!
//! Two invariants, both enforced from the files themselves rather than from
//! prose in the contributor rules:
//!
//! 1. Every relative link target names a git-tracked path, and every
//!    `#fragment` resolves to a heading anchor in the file it names, under
//!    GitHub's slug rules including duplicate-heading suffixes. Existence is
//!    decided against the tracked-path set rather than against `stat`, so the
//!    verdict is exact on a case-insensitive filesystem and identical on every
//!    host. External `http(s)` links are checked for syntactic sanity only:
//!    the battery is offline and must stay offline.
//! 2. No sentence states a banned claim family. The documentation standard
//!    bans these claims everywhere, so `docs/future/` is in scope on the same
//!    terms as the current reference: a future page may record a goal, but it
//!    may not assert the claim.
//!
//! Both scans read one blanked text, produced by the single pass in
//! [`blank_literals`]. That pass is this workspace's only Markdown blanker —
//! the Rust projection owner blanks a different grammar and shares nothing
//! with it. Blanking replaces fenced code, inline code spans, and HTML
//! comments with spaces while preserving every byte offset, so a scan over the
//! blanked text addresses the same positions as the file.
//!
//! Literal blindness is the failure this gate is most exposed to, so every way
//! a literal could swallow the rest of a file is loud rather than silent: an
//! unterminated HTML comment or code fence panics with the file and the byte
//! offset of the opener, and one state machine decides comment/fence
//! precedence so a `<!--` inside a fence cannot govern blanking outside it and
//! a fence line inside a comment cannot open a fence.
//!
//! Two Markdown constructs are deliberately unmodelled and fail loudly rather
//! than passing unchecked: setext headings (`===`/`---` underlines) and
//! explicit HTML or attribute anchors (`<a name=`, `<a id=`, `{#slug}`). Both
//! would create anchors this gate cannot see; a file that introduces one must
//! extend the gate.
//!
//! The claim patterns are spelled here, so this file is not itself a scanned
//! subject: only `.md` files are.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Claim families
// ---------------------------------------------------------------------------

/// One banned claim family and the literal phrases that state it. Phrases are
/// spelled with spaces and never hyphens: a hyphen in the text is normalized to
/// a space before matching, so `blazing-fast` and `blazing fast` are one claim
/// and neither spelling needs its own row. A phrase spells the claim, never the
/// bare word, so `instant` as a Marrow type name is not a speed claim.
struct ClaimFamily {
    name: &'static str,
    phrases: &'static [&'static str],
}

const CLAIM_FAMILIES: &[ClaimFamily] = &[
    ClaimFamily {
        name: "speed",
        phrases: &[
            "blazing",
            "blazing fast",
            "blazingly fast",
            "lightning fast",
            "screaming fast",
            "compiles fast",
            "compile fast",
            "tests fast",
            "tests are fast",
            "is fast",
            "are fast",
            "runs fast",
            "extremely fast",
            "very fast",
            "ultra fast",
            "faster than",
            "high performance",
            "instantaneous",
            "near instant",
            "feels instant",
            "feel instant",
            "instant feedback",
            "instant compile",
        ],
    },
    ClaimFamily {
        name: "readiness",
        phrases: &[
            "production ready",
            "battle tested",
            "enterprise grade",
            "industrial strength",
            "bulletproof",
            "rock solid",
            "mainframe grade",
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
            "compiler proven",
        ],
    },
    ClaimFamily {
        name: "scale",
        phrases: &["scalable", "web scale", "scales infinitely"],
    },
    ClaimFamily {
        name: "ai-native",
        phrases: &["ai native", "ai first"],
    },
    ClaimFamily {
        name: "zero-cost",
        phrases: &["zero cost"],
    },
];

/// Governance phrases that make a claim word a subject of discussion rather
/// than an assertion, as in "what may be called production-ready" or "do not
/// write that Marrow compiles fast". Each is a multi-word phrase, so a single
/// ordinary word cannot buy an exemption, and each counts only when it occurs
/// *before* the claim phrase in the same sentence.
const GOVERNANCE_MARKERS: &[&str] = &[
    "do not",
    "does not",
    "must not",
    "may not",
    "cannot",
    "makes no",
    "no measure",
    "no current claim",
    "not a claim",
    "may be called",
    "may be described",
    "referred to as",
    "write that",
];

/// Words that negate a claim when they sit within the three words immediately
/// before it. "Marrow is not production-ready" is the sentence the
/// documentation standard encourages, so it must not fail the gate that bans
/// the assertion.
const NEGATORS: &[&str] = &[
    "not", "isn't", "aren't", "no", "never", "nor", "neither", "without",
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
    UntrackedTarget {
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
            Self::UntrackedTarget { target } => {
                write!(f, "link target is not a tracked path: {target}")
            }
            Self::MissingAnchor { target, fragment } => {
                let owner = if target.is_empty() {
                    "this file"
                } else {
                    target
                };
                write!(f, "no heading anchor `#{fragment}` in {owner}")
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

/// The literal a line is currently inside, with the byte offset that opened it
/// so an unterminated one can name its own cause.
enum OpenLiteral {
    Fence { marker: u8, width: usize, at: usize },
    Comment { at: usize },
}

/// Replaces every literal byte with a space while preserving byte offsets,
/// line count, and total length, in one pass that owns comment/fence
/// precedence: inside a fence a `<!--` is ordinary text, and inside a comment a
/// fence line is ordinary text.
///
/// Panics on an unterminated comment or fence. Silently blanking to end of file
/// is the exact way a text gate disables itself, so the file and the opener's
/// offset are reported instead.
fn blank_literals(rel: &str, source: &str) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let mut open: Option<OpenLiteral> = None;

    for (start, end) in line_spans(source) {
        let line = &source[start..end];
        match open {
            Some(OpenLiteral::Fence { marker, width, .. }) => {
                let closes = closing_fence(line, marker, width);
                blank_range(&mut bytes, start, end);
                if closes {
                    open = None;
                }
            }
            Some(OpenLiteral::Comment { .. }) => match line.find("-->") {
                Some(offset) => {
                    let resume = start + offset + 3;
                    blank_range(&mut bytes, start, resume);
                    open = blank_prose(&mut bytes, source, resume, end);
                }
                None => blank_range(&mut bytes, start, end),
            },
            None => match opening_fence(line) {
                Some((marker, width)) => {
                    blank_range(&mut bytes, start, end);
                    open = Some(OpenLiteral::Fence {
                        marker,
                        width,
                        at: start,
                    });
                }
                None => open = blank_prose(&mut bytes, source, start, end),
            },
        }
    }

    match open {
        Some(OpenLiteral::Comment { at }) => {
            panic!("{rel}: unterminated HTML comment opened at byte {at}")
        }
        Some(OpenLiteral::Fence { at, .. }) => {
            panic!("{rel}: unterminated code fence opened at byte {at}")
        }
        None => {}
    }

    String::from_utf8(bytes).expect("blanking replaces whole ranges with ascii spaces")
}

/// Blanks inline code spans and HTML comments in one prose segment, which is
/// the whole line or the remainder of a line after a comment closed. Returns
/// the comment left open at `end`, if any: a comment continues across lines,
/// an inline code span does not.
fn blank_prose(bytes: &mut [u8], source: &str, from: usize, end: usize) -> Option<OpenLiteral> {
    let raw = source.as_bytes();
    let mut at = from;
    while at < end {
        match raw[at] {
            b'`' => at = blank_inline_code(bytes, raw, at, end),
            b'<' if raw[at..end].starts_with(b"<!--") => {
                match find_bytes(raw, b"-->", at + 4, end) {
                    Some(close) => {
                        blank_range(bytes, at, close + 3);
                        at = close + 3;
                    }
                    None => {
                        blank_range(bytes, at, end);
                        return Some(OpenLiteral::Comment { at });
                    }
                }
            }
            _ => at += 1,
        }
    }
    None
}

/// Blanks one backtick-delimited inline code span and returns the offset to
/// resume from. A run of `n` backticks opens a span the next run of exactly `n`
/// backticks closes; an unclosed run within the segment is left alone, so a
/// stray backtick cannot blank the rest of the line.
fn blank_inline_code(bytes: &mut [u8], raw: &[u8], at: usize, end: usize) -> usize {
    let open_width = raw[at..end]
        .iter()
        .take_while(|byte| **byte == b'`')
        .count();
    let mut cursor = at + open_width;
    while cursor < end {
        if raw[cursor] == b'`' {
            let width = raw[cursor..end]
                .iter()
                .take_while(|byte| **byte == b'`')
                .count();
            if width == open_width {
                blank_range(bytes, at, cursor + width);
                return cursor + width;
            }
            cursor += width;
        } else {
            cursor += 1;
        }
    }
    at + open_width
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
    let end = end.min(bytes.len());
    for byte in &mut bytes[start..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8], from: usize, to: usize) -> Option<usize> {
    if from >= to || needle.len() > to - from {
        return None;
    }
    haystack[from..to]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

/// The fence character and run width when `line` opens a fenced code block.
fn opening_fence(line: &str) -> Option<(u8, usize)> {
    let trimmed = line.trim_start_matches(' ');
    for marker in *b"`~" {
        let width = trimmed.bytes().take_while(|byte| *byte == marker).count();
        if width >= 3 {
            return Some((marker, width));
        }
    }
    None
}

fn closing_fence(line: &str, marker: u8, width: usize) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= width && trimmed.bytes().all(|byte| byte == marker)
}

// ---------------------------------------------------------------------------
// Unmodelled constructs
// ---------------------------------------------------------------------------

/// Panics when a file uses a construct this gate cannot resolve. Passing such a
/// file would report "no violations" about anchors it never saw.
fn reject_unmodelled_constructs(rel: &str, raw: &str, blanked: &str) {
    for marker in ["<a name=", "<a id=", "{#"] {
        if let Some(at) = blanked.find(marker) {
            panic!("{rel}: explicit anchor `{marker}` at byte {at} is not modelled by this gate");
        }
    }

    let mut previous_is_prose = false;
    for (index, line) in blanked.lines().enumerate() {
        let trimmed = line.trim();
        let underline = !trimmed.is_empty()
            && (trimmed.bytes().all(|byte| byte == b'=')
                || (trimmed.len() >= 2 && trimmed.bytes().all(|byte| byte == b'-')));
        if underline && previous_is_prose {
            let text = raw.lines().nth(index).unwrap_or(trimmed);
            panic!(
                "{rel}:{}: setext heading `{text}` is not modelled by this gate",
                index + 1
            );
        }
        previous_is_prose = !trimmed.is_empty() && !trimmed.starts_with(['|', '#', '>']);
    }
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

struct Document {
    /// Path relative to the repository root, in reporting form.
    rel: String,
    raw: String,
    blanked: String,
    anchors: BTreeSet<String>,
}

impl Document {
    fn new(rel: String, raw: String) -> Self {
        let blanked = blank_literals(&rel, &raw);
        assert_eq!(
            blanked.len(),
            raw.len(),
            "{rel}: blanking must preserve byte offsets"
        );
        reject_unmodelled_constructs(&rel, &raw, &blanked);
        let anchors = heading_anchors(&raw, &blanked);
        Self {
            rel,
            raw,
            blanked,
            anchors,
        }
    }

    fn read(root: &Path, rel: &str) -> Self {
        let path = root.join(rel);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        Self::new(rel.to_string(), raw)
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

    /// The directory this document's relative links resolve against.
    fn directory(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(dir, _)| dir)
    }
}

/// Every heading anchor in the file, under GitHub's slug rules: the heading
/// text is lowercased, characters other than letters, digits, `-`, `_`, and
/// spaces are dropped, spaces become hyphens, and a repeated slug takes the
/// next `-1`, `-2`, … suffix.
///
/// Heading *lines* are decided from the blanked text, so the one literal state
/// machine also governs which `#` lines are headings — a `# Title` inside a
/// fence or an HTML comment is not one. Heading *text* comes from the same line
/// of the raw file, because GitHub slugs the text inside inline code rather
/// than dropping it.
fn heading_anchors(raw: &str, blanked: &str) -> BTreeSet<String> {
    let mut anchors = BTreeSet::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for ((start, end), (raw_start, raw_end)) in line_spans(blanked).into_iter().zip(line_spans(raw))
    {
        if heading_text(&blanked[start..end]).is_none() {
            continue;
        }
        let Some(text) = heading_text(&raw[raw_start..raw_end]) else {
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
    let trimmed = line.trim_start_matches(' ');
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\n', '\r']) {
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
    while let Some(found) = find_bytes(bytes, b"](", at, bytes.len()) {
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

/// Reference usages (`[text][label]` and the collapsed `[text][]`) in blanked
/// text. Only scanned when the file defines at least one label, so a bracket
/// pair in ordinary prose cannot invent a violation. A collapsed usage reports
/// its own text, which is the label it means.
fn reference_usages(blanked: &str) -> Vec<(String, usize)> {
    let bytes = blanked.as_bytes();
    let mut usages = Vec::new();
    let mut at = 0;
    while let Some(found) = find_bytes(bytes, b"][", at, bytes.len()) {
        let open = found + 2;
        let Some(close) = find_bytes(bytes, b"]", open, bytes.len()) else {
            break;
        };
        let explicit = blanked[open..close].trim();
        let label = if explicit.is_empty() {
            let text_start = blanked[..found].rfind('[').map_or(found, |at| at + 1);
            blanked[text_start..found].trim().to_lowercase()
        } else {
            explicit.to_lowercase()
        };
        usages.push((label, open));
        at = close + 1;
    }
    usages
}

/// Normalizes `.`/`..` components without touching the filesystem, so a link is
/// judged by the path it spells.
fn normalize(path: &Path) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // A link that climbs above the repository root names no tracked
                // path, and saying so is the honest verdict.
                out.pop()?;
            }
            other => out.push(other.as_os_str().to_string_lossy().into_owned()),
        }
    }
    Some(out.join("/"))
}

/// Whether `rel` names a tracked file or a directory containing tracked files.
fn is_tracked(tracked: &BTreeSet<String>, rel: &str) -> bool {
    if tracked.contains(rel) {
        return true;
    }
    let prefix = format!("{rel}/");
    tracked
        .range(prefix.clone()..)
        .next()
        .is_some_and(|candidate| candidate.starts_with(&prefix))
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
        // Any other scheme is outside the gate's contract and must be spelled
        // deliberately rather than slipping through unchecked.
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    !host.is_empty() && host.contains('.') && !target.contains(char::is_whitespace)
}

/// The anchors each tracked document publishes. A fragment is resolved against
/// this index rather than against the subject list, so scanning one file still
/// judges its cross-file fragments against the real corpus.
type AnchorIndex<'a> = BTreeMap<&'a str, &'a BTreeSet<String>>;

fn anchor_index(documents: &[Document]) -> AnchorIndex<'_> {
    documents
        .iter()
        .map(|document| (document.rel.as_str(), &document.anchors))
        .collect()
}

fn check_links(
    subjects: &[Document],
    anchors: &AnchorIndex<'_>,
    tracked: &BTreeSet<String>,
) -> Vec<Violation> {
    let mut violations = Vec::new();

    for document in subjects {
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
            check_one_link(document, anchors, tracked, &link, &mut violations);
        }
    }

    violations
}

fn check_one_link(
    document: &Document,
    anchors: &AnchorIndex<'_>,
    tracked: &BTreeSet<String>,
    link: &Link,
    violations: &mut Vec<Violation>,
) {
    let target = link.target.as_str();
    if target.is_empty() {
        return;
    }
    let line = document.line_of(link.offset);
    let mut push = |kind| {
        violations.push(Violation {
            file: document.rel.clone(),
            line,
            kind,
        });
    };

    if is_external(target) {
        if !external_is_sane(target) {
            push(ViolationKind::MalformedExternal {
                target: target.to_string(),
            });
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
            push(ViolationKind::MissingAnchor {
                target: String::new(),
                fragment: fragment.to_string(),
            });
        }
        return;
    }

    let resolved = normalize(&Path::new(document.directory()).join(path_part));
    let Some(rel) = resolved.filter(|rel| is_tracked(tracked, rel)) else {
        push(ViolationKind::UntrackedTarget {
            target: target.to_string(),
        });
        return;
    };

    let Some(fragment) = fragment else {
        return;
    };
    if !anchors
        .get(rel.as_str())
        .is_some_and(|published| published.contains(fragment))
    {
        push(ViolationKind::MissingAnchor {
            target: rel,
            fragment: fragment.to_string(),
        });
    }
}

// ---------------------------------------------------------------------------
// Claim gate
// ---------------------------------------------------------------------------

/// Sentence spans of blanked text, as `(offset, sentence)` lowercased with
/// [`str::to_ascii_lowercase`]. Unicode lowercasing can change a string's byte
/// length and would drift every reported offset; every claim phrase and marker
/// is ASCII, so an ASCII fold loses no subject and keeps offsets exact. A span
/// ends at `.`, `!`, or `?` followed by whitespace, at a line whose successor
/// begins a new block (a blank line, a list item, a heading, a table row, or a
/// block quote), or at end of input. Breaking at block starts keeps a marker in
/// one bullet from exempting a claim in the next.
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
        let block_break = bytes[at] == b'\n' && begins_block(&blanked[at + 1..]);
        if terminator || block_break {
            let end = at + 1;
            spans.push((start, blanked[start..end].to_ascii_lowercase()));
            start = end;
        }
        at += 1;
    }
    if start < bytes.len() {
        spans.push((start, blanked[start..].to_ascii_lowercase()));
    }
    spans
}

fn begins_block(rest: &str) -> bool {
    let line = rest.split('\n').next().unwrap_or("");
    let trimmed = line.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return true;
    };
    if matches!(first, '-' | '*' | '+' | '#' | '|' | '>') {
        return true;
    }
    first.is_ascii_digit()
        && trimmed
            .split_once(['.', ')'])
            .is_some_and(|(number, _)| number.chars().all(|ch| ch.is_ascii_digit()))
}

/// Word characters for phrase boundaries. `-` is one, so a hyphen abutting a
/// phrase binds it into a longer word.
fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '\''
}

/// Renders a hyphenated compound as its spaced spelling. Both characters are
/// one ASCII byte, so the result shares every offset with its input.
fn hyphens_to_spaces(text: &str) -> String {
    text.replace('-', " ")
}

/// Finds `needle` in the hyphen-normalized text while testing its boundaries
/// against the original. Searching the normalized text makes `blazing-fast` and
/// `blazing fast` one claim; testing the original makes a hyphen that abuts the
/// phrase a word character, so `is fast` does not match inside
/// `is fast-forwarded`.
fn word_boundary_match(original: &str, normalized: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = normalized[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before_ok = original[..start].chars().next_back().is_none_or(|ch| {
            // A leading `-` binds the phrase into a longer word, but a leading
            // apostrophe does not.
            !(ch.is_alphanumeric() || ch == '_' || ch == '-')
        });
        let after_ok = original[end..]
            .chars()
            .next()
            .is_none_or(|ch| !is_word_char(ch));
        if before_ok && after_ok {
            return Some(start);
        }
        from = end;
    }
    None
}

/// Whether the sentence discusses the claim word rather than asserting it, or
/// negates it. Governance markers and a sentence-initial prohibition must
/// precede the phrase; a negator must sit in the three words immediately
/// before it.
fn mentioned_or_negated(sentence: &str, phrase_at: usize) -> bool {
    let before = &sentence[..phrase_at];
    if GOVERNANCE_MARKERS
        .iter()
        .any(|marker| before.contains(marker))
    {
        return true;
    }
    if sentence.trim_start().starts_with("never ") {
        return true;
    }
    before
        .split_whitespace()
        .rev()
        .take(3)
        .map(|word| word.trim_matches(|ch: char| !is_word_char(ch)))
        .any(|word| NEGATORS.contains(&word))
}

fn check_claims(documents: &[Document]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for document in documents {
        for (offset, sentence) in sentences(&document.blanked) {
            let normalized = hyphens_to_spaces(&sentence);
            for family in CLAIM_FAMILIES {
                // One sentence states a family once, however many of its
                // phrases overlap the same words.
                for phrase in family.phrases {
                    let Some(at) = word_boundary_match(&sentence, &normalized, phrase) else {
                        continue;
                    };
                    if mentioned_or_negated(&sentence, at) {
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
                    break;
                }
            }
        }
    }
    violations
}

// ---------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------

struct Corpus {
    tracked: BTreeSet<String>,
    documents: Vec<Document>,
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow-codes`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .to_path_buf()
}

fn tracked_paths(root: &Path) -> BTreeSet<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("ls-files")
        .output()
        .unwrap_or_else(|error| panic!("run git ls-files: {error}"));
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing = String::from_utf8(output.stdout).expect("git output is utf-8");
    let paths: BTreeSet<String> = listing.lines().map(str::to_string).collect();
    assert!(
        !paths.is_empty(),
        "no tracked files under {}",
        root.display()
    );
    paths
}

/// The scanned corpus, built once: the gates below and the tail sentinel all
/// read the same parse.
fn corpus() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let root = workspace_root();
        let tracked = tracked_paths(&root);
        let documents: Vec<Document> = tracked
            .iter()
            .filter(|rel| rel.ends_with(".md"))
            .map(|rel| Document::read(&root, rel))
            .collect();
        assert!(
            !documents.is_empty(),
            "no tracked markdown under {}",
            root.display()
        );
        Corpus { tracked, documents }
    })
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

#[test]
fn every_documentation_link_and_anchor_resolves() {
    let corpus = corpus();
    let violations = check_links(
        &corpus.documents,
        &anchor_index(&corpus.documents),
        &corpus.tracked,
    );
    assert!(
        violations.is_empty(),
        "unresolved documentation links:\n{}",
        report(&violations)
    );
}

#[test]
fn no_documentation_states_a_banned_claim_family() {
    let violations = check_claims(&corpus().documents);
    assert!(
        violations.is_empty(),
        "banned claim families in documentation:\n{}",
        report(&violations)
    );
}

/// Content sentinel: a claim and a broken link appended to the final bytes of
/// each real file, with no trailing newline, must both be reported. A scanner
/// that stops at the last structure it understands — an unclosed literal, a
/// table, a fence — reports nothing here and fails.
#[test]
fn both_gates_reach_the_final_byte_of_every_file() {
    let corpus = corpus();
    // Appending a paragraph publishes no new heading, so each probe's anchors
    // are the ones the corpus already indexed.
    let anchors = anchor_index(&corpus.documents);
    for document in &corpus.documents {
        let tail = "\n\nMarrow is production-ready, see [tail](absent-tail-target.md).";
        let probe = Document::new(
            document.rel.clone(),
            format!("{}{tail}", document.raw.trim_end()),
        );
        let last_line = probe.raw.lines().count();

        let claims = check_claims(std::slice::from_ref(&probe));
        assert_eq!(
            claims.len(),
            1,
            "{}: the claim scan did not reach the appended tail:\n{}",
            document.rel,
            report(&claims)
        );
        assert_eq!(claims[0].line, last_line, "{}", document.rel);

        assert_eq!(probe.anchors, document.anchors, "{}", document.rel);
        let links = check_links(std::slice::from_ref(&probe), &anchors, &corpus.tracked);
        assert_eq!(
            links.len(),
            1,
            "{}: the link scan did not reach the appended tail:\n{}",
            document.rel,
            report(&links)
        );
        assert_eq!(links[0].line, last_line, "{}", document.rel);
    }
}

/// Anti-vacuity floor. A parser regression that silently found no links or no
/// headings would make the resolution gate pass by scanning nothing, so the
/// corpus's measured shape is asserted alongside the invariant it feeds.
#[test]
fn the_link_gate_has_a_corpus_to_resolve() {
    let documents = &corpus().documents;
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

// ---------------------------------------------------------------------------
// Plant probes
// ---------------------------------------------------------------------------

fn probe(rel: &str, raw: &str) -> Document {
    Document::new(rel.to_string(), raw.to_string())
}

fn tracked_set(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|path| (*path).to_string()).collect()
}

#[test]
fn the_link_gate_detects_every_failure_direction() {
    let target = probe("docs/target.md", "# Alpha\n\n## Alpha\n\nBody.\n");
    let subject = probe(
        "docs/subject.md",
        concat!(
            "# Subject\n\n",
            "[gone](missing.md)\n\n",
            "[bad fragment](target.md#nope)\n\n",
            "[third duplicate](target.md#alpha-2)\n\n",
            "[first duplicate](target.md#alpha-1)\n\n",
            "[self](#subject)\n\n",
            "[up and out](../../../escape.md)\n\n",
            "```\n[ignored](also-missing.md)\n```\n\n",
            "`[inline](never-here.md)`\n\n",
            "[external](https://example.com/x)\n\n",
            "[malformed](https:///)\n",
        ),
    );
    let tracked = tracked_set(&["docs/target.md", "docs/subject.md"]);
    let documents = [target, subject];
    let violations = check_links(&documents, &anchor_index(&documents), &tracked);
    let rendered = report(&violations);

    for expected in [
        "link target is not a tracked path: missing.md",
        "link target is not a tracked path: ../../../escape.md",
        "no heading anchor `#nope` in docs/target.md",
        "no heading anchor `#alpha-2` in docs/target.md",
        "malformed external link",
    ] {
        assert!(
            rendered.contains(expected),
            "undetected: {expected}\n{rendered}"
        );
    }
    for unexpected in [
        "alpha-1",
        "#subject",
        "also-missing.md",
        "never-here.md",
        "example.com",
    ] {
        assert!(
            !rendered.contains(unexpected),
            "false positive: {unexpected}\n{rendered}"
        );
    }
    assert_eq!(violations.len(), 5, "unexpected violations:\n{rendered}");
}

#[test]
fn the_claim_gate_detects_every_failure_direction() {
    // The banned sentence sits in the final bytes with no trailing newline, so
    // a scan that stops early cannot pass this probe.
    let subject = probe(
        "docs/subject.md",
        concat!(
            "# Subject\n\n",
            "Marrow is production-ready today.\n\n",
            "Marrow is not production-ready.\n\n",
            "Do not write that Marrow compiles fast in public documentation.\n\n",
            "What may be called production-ready is governed by the status page.\n\n",
            "```\nThe compiler is blazingly fast.\n```\n\n",
            "<!-- The runtime is production-ready. -->\n\n",
            "The lane is fast-forwarded onto the integration line.\n\n",
            "The compiler is blazing-fast.\n\n",
            "- A bullet that must not overstate anything.\n",
            "- The compiler is blazingly fast.\n\n",
            "The runtime is secure",
        ),
    );
    let violations = check_claims(&[subject]);
    let rendered = report(&violations);

    for expected in [
        "docs/subject.md:3: banned readiness claim: \"production ready\"",
        "docs/subject.md:19: banned speed claim: \"blazing fast\"",
        "docs/subject.md:22: banned speed claim: \"blazingly fast\"",
        "docs/subject.md:24: banned security claim: \"is secure\"",
    ] {
        assert!(
            rendered.contains(expected),
            "undetected: {expected}\n{rendered}"
        );
    }
    assert_eq!(
        violations.len(),
        4,
        "an honest negation, a prohibition, a governance sentence, a fenced \
         claim, a commented claim, and `fast-forwarded` are not claims:\n{rendered}"
    );
}

#[test]
fn an_unterminated_comment_fails_loudly_instead_of_blanking_the_file() {
    let raw = "# Subject\n\n<!-- opened and never closed\n\nMarrow is production-ready.\n";
    let Err(panic) = std::panic::catch_unwind(|| probe("docs/subject.md", raw)) else {
        panic!("an unterminated comment must panic")
    };
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "non-string panic".to_string());
    assert!(
        message.contains("docs/subject.md: unterminated HTML comment opened at byte 11"),
        "{message}"
    );
}

#[test]
fn an_unterminated_fence_fails_loudly_instead_of_blanking_the_file() {
    let raw = "# Subject\n\n```rust\nlet x = 1;\n\nMarrow is production-ready.\n";
    let Err(panic) = std::panic::catch_unwind(|| probe("docs/subject.md", raw)) else {
        panic!("an unterminated fence must panic")
    };
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "non-string panic".to_string());
    assert!(
        message.contains("docs/subject.md: unterminated code fence opened at byte 11"),
        "{message}"
    );
}

#[test]
fn one_state_machine_decides_comment_and_fence_precedence() {
    // A comment opener inside a fence is fenced text, so the prose after the
    // fence is still scanned; a fence line inside a comment is comment text, so
    // the prose after the comment is still scanned.
    let subject = probe(
        "docs/subject.md",
        concat!(
            "# Subject\n\n",
            "```\n<!-- not a comment\n```\n\n",
            "Marrow is production-ready.\n\n",
            "<!--\n```\nstill a comment\n```\n-->\n\n",
            "The compiler is blazingly fast.\n",
        ),
    );
    let violations = check_claims(&[subject]);
    let rendered = report(&violations);
    assert!(
        rendered.contains("docs/subject.md:7: banned readiness claim"),
        "a fenced comment opener must not swallow the file:\n{rendered}"
    );
    assert!(
        rendered.contains("docs/subject.md:15: banned speed claim"),
        "a commented fence must not swallow the file:\n{rendered}"
    );
    assert_eq!(violations.len(), 2, "{rendered}");
}

#[test]
fn a_setext_heading_and_an_explicit_anchor_fail_loudly() {
    for (raw, expected) in [
        ("# Subject\n\nA Heading\n=========\n", "setext heading"),
        ("# Subject\n\n<a name=\"manual\"></a>\n", "explicit anchor"),
    ] {
        let Err(panic) = std::panic::catch_unwind(|| probe("docs/subject.md", raw)) else {
            panic!("an unmodelled construct must panic")
        };
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_else(|| "non-string panic".to_string());
        assert!(message.contains(expected), "{message}");
    }
}

#[test]
fn literal_blanking_preserves_offsets_and_hides_only_literals() {
    let source = "a `code` b\n\n```rust\nlet x = 1;\n```\n\n<!-- note -->tail\n";
    let blanked = blank_literals("probe.md", source);
    assert_eq!(blanked.len(), source.len());
    assert!(blanked.contains("a        b"), "{blanked:?}");
    assert!(!blanked.contains("let x"), "{blanked:?}");
    assert!(!blanked.contains("note"), "{blanked:?}");
    assert!(blanked.contains("tail"), "{blanked:?}");
    assert_eq!(blanked.lines().count(), source.lines().count());
}

#[test]
fn heading_slugs_follow_the_duplicate_suffix_rule() {
    let raw = "# Types and Values\n## The `mw` Command\n## Types and Values\n## Types and Values\n\n```\n# Not a heading\n```\n\n<!--\n# Also not a heading\n-->\n";
    let anchors = heading_anchors(raw, &blank_literals("probe.md", raw));
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

#[test]
fn a_reference_link_resolves_or_reports_its_label() {
    let subject = probe(
        "docs/subject.md",
        concat!(
            "# Subject\n\n",
            "See [the guide][guide] and [status][] and [absent][nowhere].\n\n",
            "[guide]: target.md\n",
            "[status]: target.md\n",
        ),
    );
    let documents = [subject];
    let violations = check_links(
        &documents,
        &anchor_index(&documents),
        &tracked_set(&["docs/target.md"]),
    );
    let rendered = report(&violations);
    assert!(
        rendered.contains("undefined link reference label: [nowhere]"),
        "{rendered}"
    );
    assert!(!rendered.contains("[guide]"), "{rendered}");
    assert!(!rendered.contains("[status]"), "{rendered}");
    assert_eq!(violations.len(), 1, "{rendered}");
}
