//! The documentation link gate over every tracked Markdown file.
//!
//! Every relative link target names a git-tracked path, and every `#fragment`
//! resolves to a heading anchor in the file it names, under GitHub's slug rules
//! including duplicate-heading suffixes. Existence is decided against the
//! tracked-path set rather than against `stat`, so the verdict is exact on a
//! case-insensitive filesystem and identical on every host. External `http(s)`
//! links are checked for syntactic sanity only: the battery is offline and must
//! stay offline.
//!
//! The scan reads one blanked text, produced by [`blank_literals`]. Blanking
//! replaces fenced code, inline code spans, and HTML comments with spaces while
//! preserving every byte offset, so a scan over the blanked text addresses the
//! same positions as the file.
//!
//! Literal blindness is the failure this gate is most exposed to, so every way a
//! literal could swallow the rest of a file is loud rather than silent: an
//! unterminated HTML comment or code fence panics with the file and the byte
//! offset of the opener, and one state machine decides comment/fence precedence
//! so a `<!--` inside a fence cannot govern blanking outside it and a fence line
//! inside a comment cannot open a fence.
//!
//! Two Markdown constructs are deliberately unmodelled and fail loudly rather
//! than passing unchecked: setext headings (`===`/`---` underlines) and explicit
//! HTML or attribute anchors (`<a name=`, `<a id=`, `{#slug}`). Both would create
//! anchors this gate cannot see; a file that introduces one must extend the gate.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path};
use std::sync::OnceLock;

#[path = "common/workspace.rs"]
mod workspace;

use workspace::{tracked_paths, workspace_root};

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
// Corpus
// ---------------------------------------------------------------------------

struct Corpus {
    tracked: BTreeSet<String>,
    documents: Vec<Document>,
}

/// The scanned corpus, built once.
fn corpus() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let root = workspace_root();
        let tracked = tracked_paths().clone();
        let documents: Vec<Document> = tracked
            .iter()
            .filter(|rel| rel.ends_with(".md"))
            .map(|rel| Document::read(root, rel))
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
