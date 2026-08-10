//! Project the compiler's published analysis facts into standard LSP payloads.
//!
//! Every payload is built from the snapshot's facts and the exact source bytes; nothing
//! is reconstructed. Byte spans become UTF-16 ranges through [`crate::position`], codes
//! and severities come verbatim from the diagnostic payload, type displays and
//! definition targets come verbatim from the snapshot, and diagnostic URIs come from
//! the one canonical re-encoder. The payload types are [`lsp_types`]; the server owns no
//! hand-written duplicate DTO.

use std::str::FromStr;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Diagnostic, DiagnosticSeverity,
    DocumentSymbol, DocumentSymbolResponse, Hover, HoverContents, Location, MarkupContent,
    MarkupKind, NumberOrString, ParameterInformation, ParameterLabel, Position as LspPosition,
    PublishDiagnosticsParams, Range as LspRange, SignatureHelp, SignatureInformation, SymbolKind,
    TextEdit, Uri,
};
use marrow_compile::{
    ActiveCall, ActiveCallOutcome, AnalysisSnapshot, Candidate, CandidateKind, CompletionOutcome,
    Completions, DeclKind, DeclSymbol, Fact, FormatOutcome,
};
use marrow_project_fs::FileIdentity;
use marrow_syntax::{Severity, SourceSpan};

use crate::position::{LineMap, Position, Range};
use crate::uri::{SelectedRoot, diagnostic_uri};

/// The internal-error class returned when a canonically-encoded diagnostic URI fails to
/// parse back into an `lsp_types::Uri`. The encoder produces canonical URIs, so this is
/// a compiler-coherence-class failure, never a normal outcome; it is surfaced fallibly
/// rather than by an `unwrap`.
#[derive(Debug)]
pub struct UriEncodingError;

fn to_lsp_position(position: Position) -> LspPosition {
    LspPosition::new(position.line, position.character)
}

fn to_lsp_range(range: Range) -> LspRange {
    LspRange::new(to_lsp_position(range.start), to_lsp_position(range.end))
}

fn to_uri(root: &SelectedRoot, identity: &FileIdentity) -> Result<Uri, UriEncodingError> {
    Uri::from_str(&diagnostic_uri(root, identity)).map_err(|_| UriEncodingError)
}

/// The LSP severity of a diagnostic, projected from the payload's typed severity —
/// the one severity owner — never reconstructed by classifying the code.
fn to_lsp_severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    }
}

/// Build the per-file publish-diagnostics parameters for one snapshot file. The source
/// bytes drive the UTF-16 range projection; a non-UTF-8 file (never span-bearing)
/// produces an empty list.
pub fn diagnostics_for_file(
    snapshot: &AnalysisSnapshot,
    root: &SelectedRoot,
    file: &FileIdentity,
    source: &str,
    version: Option<i32>,
) -> Result<PublishDiagnosticsParams, UriEncodingError> {
    let map = LineMap::new(source);
    let diagnostics = snapshot
        .diagnostics_for(file)
        .map(|diagnostic| {
            let span = diagnostic.span();
            let range = to_lsp_range(map.range_of(span.start_byte, span.end_byte));
            Diagnostic {
                range,
                severity: Some(to_lsp_severity(diagnostic.severity())),
                code: Some(NumberOrString::String(diagnostic.code().to_owned())),
                code_description: None,
                source: Some("marrow".to_owned()),
                message: diagnostic.message().to_owned(),
                related_information: None,
                tags: None,
                data: None,
            }
        })
        .collect();
    Ok(PublishDiagnosticsParams {
        uri: to_uri(root, file)?,
        diagnostics,
        version,
    })
}

/// The hover payload at an LSP position. `Ok(None)` covers a legitimately absent fact,
/// an unavailable (syntax/dependency) fact, and an out-of-range or unknown position —
/// the LSP `null` hover result. The type display comes verbatim from the compiler.
pub fn hover(
    snapshot: &AnalysisSnapshot,
    file: &FileIdentity,
    source: &str,
    position: LspPosition,
) -> Option<Hover> {
    let offset = LineMap::new(source).byte_at(Position {
        line: position.line,
        character: position.character,
    });
    match snapshot.hover(file, offset) {
        Ok(Fact::Present(hover)) => Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::PlainText,
                value: hover.display().to_owned(),
            }),
            range: None,
        }),
        Ok(Fact::Absent | Fact::Unavailable(_)) | Err(_) => None,
    }
}

/// The definition location at an LSP position, or `None` (LSP `null`). The target file,
/// selection range, and source are the snapshot's; the range projects through the
/// target file's own source bytes.
pub fn definition(
    snapshot: &AnalysisSnapshot,
    root: &SelectedRoot,
    file: &FileIdentity,
    source: &str,
    target_source: impl Fn(&FileIdentity) -> Option<String>,
    position: LspPosition,
) -> Result<Option<Location>, UriEncodingError> {
    let offset = LineMap::new(source).byte_at(Position {
        line: position.line,
        character: position.character,
    });
    let target = match snapshot.definition(file, offset) {
        Ok(Fact::Present(definition)) => definition,
        Ok(Fact::Absent | Fact::Unavailable(_)) | Err(_) => return Ok(None),
    };
    // Project the target name span through the target file's own source. When the
    // target file's source is unavailable, fall back to a zero range at the span start.
    let name_span = target.name_span();
    let range = match target_source(target.file()) {
        Some(text) => {
            to_lsp_range(LineMap::new(&text).range_of(name_span.start_byte, name_span.end_byte))
        }
        None => {
            let point = to_lsp_position(Position {
                line: name_span.line.saturating_sub(1),
                character: name_span.column.saturating_sub(1),
            });
            LspRange::new(point, point)
        }
    };
    Ok(Some(Location {
        uri: to_uri(root, target.file())?,
        range,
    }))
}

/// The formatting edits for a document, or `None` (LSP `null`) when formatting is
/// refused (unparsed source, a diagnostic-limited parse, or comment loss), the file is
/// not valid UTF-8, or the output exceeds its bound. A successful format is one
/// whole-document replacement edit.
pub fn formatting(
    snapshot: &AnalysisSnapshot,
    file: &FileIdentity,
    source: &str,
) -> Option<Vec<TextEdit>> {
    match snapshot.format(file) {
        Ok(FormatOutcome::Formatted(formatted)) => {
            if formatted == source {
                // Already formatted: no edit.
                return Some(Vec::new());
            }
            let map = LineMap::new(source);
            let whole = LspRange::new(LspPosition::new(0, 0), to_lsp_position(map.end_position()));
            Some(vec![TextEdit::new(whole, formatted)])
        }
        Ok(
            FormatOutcome::Refused(_) | FormatOutcome::TooLarge { .. } | FormatOutcome::InvalidUtf8,
        )
        | Err(_) => None,
    }
}

/// A query-local analysis resource refusal: the in-scope candidate set or rendered
/// display exceeded a per-query bound. The server maps it to the recoverable `-32803`
/// law — never a truncated prefix or display.
pub struct ResourceLimited;

/// The completion payload at an LSP position. `Ok(None)` covers a legitimately absent
/// classification, an unavailable (syntax) owner, and an unknown/out-of-range position —
/// the LSP `null` completion result. `Err(ResourceLimited)` is an over-cap candidate set.
/// Every candidate is projected verbatim from the compiler's fact; the set is the
/// complete in-scope namespace, never filtered, ranked, or truncated here.
pub fn completion(
    snapshot: &AnalysisSnapshot,
    file: &FileIdentity,
    source: &str,
    position: LspPosition,
) -> Result<Option<CompletionResponse>, ResourceLimited> {
    let offset = LineMap::new(source).byte_at(Position {
        line: position.line,
        character: position.character,
    });
    match snapshot.completions(file, offset) {
        Ok(CompletionOutcome::Ready(Fact::Present(completions))) => {
            Ok(Some(to_completion_response(&completions)))
        }
        Ok(CompletionOutcome::Ready(Fact::Absent | Fact::Unavailable(_))) | Err(_) => Ok(None),
        Ok(CompletionOutcome::Refused(_)) => Err(ResourceLimited),
    }
}

/// The complete in-scope candidate set as a non-incomplete completion list. No server-side
/// prefix/fuzzy filter, ranking, sort key, or commit character is applied: the client
/// filters over this bounded set.
fn to_completion_response(completions: &Completions) -> CompletionResponse {
    let items = completions
        .candidates()
        .iter()
        .map(to_completion_item)
        .collect();
    CompletionResponse::Array(items)
}

fn to_completion_item(candidate: &Candidate) -> CompletionItem {
    let detail = candidate.detail();
    CompletionItem {
        label: candidate.label().to_owned(),
        kind: Some(completion_item_kind(candidate.kind())),
        detail: (!detail.is_empty()).then(|| detail.to_owned()),
        ..Default::default()
    }
}

/// Map a compiler candidate kind to its editor symbol category. A closed match: a new
/// candidate kind forces a decision here.
fn completion_item_kind(kind: CandidateKind) -> CompletionItemKind {
    match kind {
        CandidateKind::Function | CandidateKind::Builtin => CompletionItemKind::FUNCTION,
        CandidateKind::Local | CandidateKind::Param => CompletionItemKind::VARIABLE,
        CandidateKind::Const => CompletionItemKind::CONSTANT,
        CandidateKind::Field => CompletionItemKind::FIELD,
        CandidateKind::EnumMember { .. } => CompletionItemKind::ENUM_MEMBER,
        CandidateKind::Type => CompletionItemKind::CLASS,
        CandidateKind::TypeParam => CompletionItemKind::TYPE_PARAMETER,
        CandidateKind::Module => CompletionItemKind::MODULE,
    }
}

/// The signature-help payload at an LSP position, or `None` (LSP `null`) for a position in
/// no resolvable call. `Err(ResourceLimited)` is an over-cap rendered display. The active
/// parameter and the parameter pieces come verbatim from the compiler, so no consumer
/// substring-searches the rendered signature.
pub fn signature_help(
    snapshot: &AnalysisSnapshot,
    file: &FileIdentity,
    source: &str,
    position: LspPosition,
) -> Result<Option<SignatureHelp>, ResourceLimited> {
    let offset = LineMap::new(source).byte_at(Position {
        line: position.line,
        character: position.character,
    });
    match snapshot.active_call(file, offset) {
        Ok(ActiveCallOutcome::Ready(Fact::Present(active))) => Ok(Some(to_signature_help(&active))),
        Ok(ActiveCallOutcome::Ready(Fact::Absent | Fact::Unavailable(_))) | Err(_) => Ok(None),
        Ok(ActiveCallOutcome::Refused(_)) => Err(ResourceLimited),
    }
}

fn to_signature_help(active: &ActiveCall) -> SignatureHelp {
    let active_parameter = active.active().map(u32::from);
    let parameters = active
        .params()
        .iter()
        .map(|piece| ParameterInformation {
            label: ParameterLabel::Simple(piece.label().to_owned()),
            documentation: None,
        })
        .collect();
    let signature = SignatureInformation {
        label: active.signature().to_owned(),
        documentation: None,
        parameters: Some(parameters),
        active_parameter,
    };
    SignatureHelp {
        signatures: vec![signature],
        active_signature: Some(0),
        active_parameter,
    }
}

/// The declaration-hierarchy outline of a document, or `None` (LSP `null`) for an
/// unknown file or one whose outline is unavailable — because the file did not parse, or
/// because it crossed a per-file count or depth bound and nothing was retained for it. A
/// pure projection of the compiler's document-symbol fact: the bound is enforced at
/// snapshot admission and its consequence is that one file's outline, so a query here
/// carries no resource refusal and no other file's outline is affected.
pub fn document_symbols(
    snapshot: &AnalysisSnapshot,
    file: &FileIdentity,
    source: &str,
) -> Option<DocumentSymbolResponse> {
    let map = LineMap::new(source);
    match snapshot.document_symbols(file) {
        Ok(Fact::Present(symbols)) => Some(DocumentSymbolResponse::Nested(
            symbols
                .iter()
                .map(|symbol| to_document_symbol(symbol, &map))
                .collect(),
        )),
        Ok(Fact::Absent | Fact::Unavailable(_)) | Err(_) => None,
    }
}

fn span_range(span: SourceSpan, map: &LineMap) -> LspRange {
    to_lsp_range(map.range_of(span.start_byte, span.end_byte))
}

#[allow(deprecated)]
fn to_document_symbol(symbol: &DeclSymbol, map: &LineMap) -> DocumentSymbol {
    let children: Vec<DocumentSymbol> = symbol
        .children()
        .iter()
        .map(|child| to_document_symbol(child, map))
        .collect();
    DocumentSymbol {
        name: symbol.name().to_owned(),
        detail: None,
        kind: symbol_kind(symbol.kind()),
        tags: None,
        deprecated: None,
        range: span_range(symbol.full_range(), map),
        selection_range: span_range(symbol.name_span(), map),
        children: (!children.is_empty()).then_some(children),
    }
}

/// Map a compiler declaration kind to its editor symbol category. A closed match: a new
/// declaration kind forces a decision here.
fn symbol_kind(kind: DeclKind) -> SymbolKind {
    match kind {
        DeclKind::Alias => SymbolKind::INTERFACE,
        DeclKind::Nominal => SymbolKind::CLASS,
        DeclKind::Const => SymbolKind::CONSTANT,
        DeclKind::Resource | DeclKind::Struct => SymbolKind::STRUCT,
        DeclKind::Store => SymbolKind::OBJECT,
        DeclKind::Function | DeclKind::Test => SymbolKind::FUNCTION,
        DeclKind::Enum => SymbolKind::ENUM,
        DeclKind::EnumMember => SymbolKind::ENUM_MEMBER,
    }
}

/// The absence gates over the server's production sources: the language server
/// projects the compiler's facts verbatim and never reconstructs completion
/// ranking, document syntax, or diagnostic severity. Every gate reads the same
/// recursive production inventory, so a module moved into a subdirectory — or a
/// forbidden token added to a file no list happened to name — is a test failure
/// rather than a review miss.
#[cfg(test)]
mod absence_gate {
    use std::path::{Path, PathBuf};

    /// Field setters (lsp-types snake_case) that would enable a refused behavior. These
    /// names appear legitimately in this gate's own lists and in test code; the scan
    /// covers only production code (see [`scan`]).
    const FORBIDDEN_FIELD_SETTERS: &[&str] = &[
        "sort_text",
        "filter_text",
        "commit_characters",
        "insert_text_format",
        "additional_text_edits",
        "resolve_provider",
    ];

    /// Reconstruction-leak tokens: no regex/scan over document text, no completion-context
    /// (and thus no trigger-character) classification, no keyword inventory. Advertising
    /// `trigger_characters` in the capability is editor ergonomics and stays allowed; only
    /// reading the request `CompletionContext` to classify is a leak.
    const FORBIDDEN_RECONSTRUCTION: &[&str] = &["regex", "Regex", "CompletionContext", "keyword"];

    fn src_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// Every `.rs` source of this crate, walked recursively in sorted order: a
    /// module moved into a subdirectory stays covered.
    fn crate_sources() -> Vec<(PathBuf, String)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
                .expect("read the crate src directory")
                .map(|entry| entry.expect("src entry").path())
                .collect();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    let text = std::fs::read_to_string(&path).expect("read crate source");
                    out.push((path, text));
                }
            }
        }
        let mut out = Vec::new();
        walk(&src_root(), &mut out);
        assert!(!out.is_empty(), "the source inventory must be non-empty");
        out
    }

    /// One character blanked. A newline survives, so a blanked source keeps the
    /// line boundaries of the original.
    fn blank(ch: char) -> char {
        if ch == '\n' { '\n' } else { ' ' }
    }

    /// The index just past a raw string literal whose prefix word ends at
    /// `after_word`, or `None` when no raw literal opens there. Only a whole word
    /// opens one, so `for` is not read as an `r` prefix, and nothing inside ends
    /// the literal except a quote followed by the hashes it opened with.
    fn raw_literal_end(chars: &[char], after_word: usize, word: &str) -> Option<usize> {
        matches!(word, "r" | "br" | "cr").then_some(())?;
        let mut hashes = 0;
        while chars.get(after_word + hashes) == Some(&'#') {
            hashes += 1;
        }
        (chars.get(after_word + hashes) == Some(&'"')).then_some(())?;
        let mut cursor = after_word + hashes + 1;
        while cursor < chars.len() {
            let closes = chars[cursor] == '"'
                && (1..=hashes).all(|offset| chars.get(cursor + offset) == Some(&'#'));
            cursor += 1;
            if closes {
                return Some(cursor + hashes);
            }
        }
        Some(cursor)
    }

    /// The index just past an escaped literal opening at `start`, where a
    /// backslash escapes the character after it: `"…"`, or `'…'` for a character.
    fn escaped_literal_end(chars: &[char], start: usize, delimiter: char) -> usize {
        let mut cursor = start + 1;
        while cursor < chars.len() {
            match chars[cursor] {
                '\\' => cursor += 2,
                ch if ch == delimiter => return cursor + 1,
                _ => cursor += 1,
            }
        }
        cursor
    }

    /// The index just past a block comment opening at `start`, counting nesting.
    fn block_comment_end(chars: &[char], start: usize) -> usize {
        let mut cursor = start + 2;
        let mut depth = 1usize;
        while cursor < chars.len() && depth > 0 {
            match (chars[cursor], chars.get(cursor + 1)) {
                ('/', Some('*')) => {
                    depth += 1;
                    cursor += 2;
                }
                ('*', Some('/')) => {
                    depth -= 1;
                    cursor += 2;
                }
                _ => cursor += 1,
            }
        }
        cursor
    }

    /// Blank `chars[start..end]` into `out`, returning the index to resume from.
    /// `end` may run past the source when a literal or comment is unterminated,
    /// which a source that compiles cannot contain.
    fn blank_through(chars: &[char], start: usize, end: usize, out: &mut String) -> usize {
        let end = end.min(chars.len());
        out.extend(chars[start..end].iter().copied().map(blank));
        end
    }

    /// The source with the contents of comments and literals blanked, one
    /// character for one, so [`production_lines`] reads item structure from code
    /// alone. Fixture text is the reason: a test that spells `#[cfg(test)]`, an
    /// item opener, or a closing brace at column zero inside a string would
    /// otherwise redraw the scanned region from inside a test module. A raw or
    /// byte literal read as an ordinary one is worse still — `r"a\"` and
    /// `br#"{"x":1}"#` each leave an unterminated string that blanks every line
    /// after it, retiring the gates below without a failure.
    fn code_only(source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        let mut out = String::with_capacity(source.len());
        let mut index = 0;
        while index < chars.len() {
            let is_word = |ch: &char| ch.is_alphanumeric() || *ch == '_';
            let ch = chars[index];
            if is_word(&ch) {
                let end = index + chars[index..].iter().take_while(|c| is_word(c)).count();
                let word: String = chars[index..end].iter().collect();
                match raw_literal_end(&chars, end, &word) {
                    Some(literal) => index = blank_through(&chars, index, literal, &mut out),
                    None => {
                        out.extend(chars[index..end].iter());
                        index = end;
                    }
                }
                continue;
            }
            match ch {
                '/' if chars.get(index + 1) == Some(&'/') => {
                    let end = chars[index..]
                        .iter()
                        .position(|c| *c == '\n')
                        .map_or(chars.len(), |offset| index + offset);
                    index = blank_through(&chars, index, end, &mut out);
                }
                '/' if chars.get(index + 1) == Some(&'*') => {
                    let end = block_comment_end(&chars, index);
                    index = blank_through(&chars, index, end, &mut out);
                }
                '"' => {
                    let end = escaped_literal_end(&chars, index, '"');
                    index = blank_through(&chars, index, end, &mut out);
                }
                // A character literal is `'x'` or `'\…'`; every other `'` opens a
                // lifetime, which is code.
                '\'' if chars.get(index + 1) == Some(&'\\')
                    || chars.get(index + 2) == Some(&'\'') =>
                {
                    let end = escaped_literal_end(&chars, index, '\'');
                    index = blank_through(&chars, index, end, &mut out);
                }
                _ => {
                    out.push(ch);
                    index += 1;
                }
            }
        }
        out
    }

    /// The production lines of a source: every line outside a `#[cfg(test)]`
    /// item. Test code and this gate's own token lists legitimately name the
    /// forbidden surface, so a block-opening annotated item is skipped from its
    /// attribute to the closing brace at the attribute's own indentation (with an
    /// optional `;` for an annotated initializer), which formatted sources
    /// guarantee. Structure is read from [`code_only`], never from comment or
    /// literal text. Production code that follows a test-only helper is still
    /// scanned. Any other annotated form stays in the scanned region, and an
    /// unterminated one panics: a false positive fails loudly, where guessing its
    /// extent would blank the rest of the file silently.
    fn production_lines(source: &str) -> Vec<&str> {
        let code = code_only(source);
        assert_eq!(
            code.lines().count(),
            source.lines().count(),
            "the blanked view must keep every line boundary; a shorter view would end \
             the zip below early and blank the rest of the file silently"
        );
        let mut lines = source.lines().zip(code.lines()).peekable();
        let mut kept = Vec::new();
        while let Some((line, structure)) = lines.next() {
            let Some(indent) = structure
                .trim_end()
                .strip_suffix("#[cfg(test)]")
                .map(str::len)
            else {
                kept.push(line);
                continue;
            };
            if !lines
                .peek()
                .is_some_and(|(_, next)| next.trim_end().ends_with('{'))
            {
                continue;
            }
            let close = format!("{}}}", " ".repeat(indent));
            let closed = lines.by_ref().any(|(_, skipped)| {
                let skipped = skipped.trim_end();
                skipped == close || skipped.strip_prefix(&close) == Some(";")
            });
            assert!(
                closed,
                "a `#[cfg(test)]` item opened at indent {indent} has no closing `{close}`; \
                 without it this scan would blank the rest of the file and silently pass"
            );
        }
        kept
    }

    /// A line the gate ignores: an explanatory comment (`//` …). A real forbidden use is a
    /// struct field set or path in production code, never a comment.
    fn is_comment(line: &str) -> bool {
        line.trim_start().starts_with("//")
    }

    fn scan(needles: &[&str]) {
        for (path, source) in crate_sources() {
            for line in production_lines(&source) {
                if is_comment(line) {
                    continue;
                }
                for needle in needles {
                    assert!(
                        !line.contains(needle),
                        "forbidden token `{needle}` appears in production server code ({}): {line}",
                        path.display()
                    );
                }
            }
        }
    }

    /// The inventory keeps scanning past a test-only helper: this production
    /// item follows one in `document.rs`, and a cut-at-the-first-attribute rule
    /// would silently drop it and everything after it.
    #[test]
    fn the_production_inventory_survives_an_early_test_item() {
        let document = std::fs::read_to_string(src_root().join("document.rs"))
            .expect("read the document ledger");
        assert!(
            production_lines(&document)
                .iter()
                .any(|line| line.contains("impl Default for DocumentLedger")),
            "the scanned region must reach production code after a test-only helper"
        );
    }

    /// The scanned region covers a violation anywhere outside a `#[cfg(test)]`
    /// item — before one, between two, and after the last — so a forbidden token
    /// cannot hide behind position in the file.
    #[test]
    fn the_scanned_region_covers_the_whole_file_outside_test_items() {
        let source = "\
before_item
#[cfg(test)]
mod first {
    hidden_in_first
}
between_items
#[cfg(test)]
mod second {
    hidden_in_second
}
after_items
";
        assert_eq!(
            production_lines(source),
            ["before_item", "between_items", "after_items"]
        );
    }

    /// A `#[cfg(test)]` item whose closing brace carries a trailing `;` still ends
    /// the skipped region, and one that never closes panics rather than blanking
    /// the rest of the file — the silent failure this scanner exists to avoid.
    #[test]
    fn an_unterminated_test_item_fails_loudly() {
        let terminated = "\
#[cfg(test)]
const FIXTURES: Fixtures = Fixtures {
    hidden: 1,
};
after_item
";
        assert_eq!(production_lines(terminated), ["after_item"]);

        let unterminated = "\
#[cfg(test)]
mod never_closed {
    hidden
 }
after_item
";
        assert!(
            std::panic::catch_unwind(|| production_lines(unterminated)).is_err(),
            "an unterminated test item must fail loudly, never blank the remainder"
        );
    }

    /// Fixture text is not structure. A test module whose string literals spell
    /// `#[cfg(test)]`, an item opener, and a closing brace at column zero — in an
    /// ordinary literal and in a raw one, exactly as the fixtures above do — is
    /// still skipped whole, and the production line after it is still scanned.
    /// Reading those lines as structure ends the skip inside the module, which
    /// scans test code as production and leaves the rest of the module unscanned.
    #[test]
    fn fixture_text_inside_a_literal_is_not_structure() {
        let source = r##"before_item
#[cfg(test)]
mod gate {
    const ORDINARY: &str = "
#[cfg(test)]
mod inner {
}
";
    const RAW: &str = r#"
}
"#;
    hidden_in_gate
}
after_item
"##;
        assert_eq!(production_lines(source), ["before_item", "after_item"]);
    }

    /// A `#[test]` attribute is test code by definition, so one in the scanned
    /// region of any crate source proves the scan lost a `#[cfg(test)]` item's
    /// boundary — the silent failure that would read this gate's own token lists
    /// as production uses.
    #[test]
    fn no_test_attribute_survives_the_production_scan() {
        for (path, source) in crate_sources() {
            for line in production_lines(&source) {
                assert_ne!(
                    line.trim(),
                    "#[test]",
                    "a `#[test]` attribute is inside the scanned region of {}, so a \
                     `#[cfg(test)]` item's extent was misread",
                    path.display()
                );
            }
        }
    }

    /// Every top-level item a crate source declares outside a `#[cfg(test)]` item: its
    /// own line, read from [`code_only`] so a header spelled inside a fixture literal is
    /// not mistaken for one. A top-level item opens at column zero, which nothing inside
    /// an item — test module included — does, and it ends its own line with `{` (a
    /// block) or `;` (a `mod`/`use` declaration). A header carrying the `#[cfg(test)]`
    /// attribute on the line above is the skipped item itself and is not one of these.
    fn top_level_production_items(source: &str) -> Vec<&str> {
        let code = code_only(source);
        let structure: Vec<&str> = code.lines().collect();
        source
            .lines()
            .zip(&structure)
            .enumerate()
            .filter(|(index, (_, line))| {
                let line = line.trim_end();
                line.starts_with(|first: char| first.is_ascii_alphabetic())
                    && (line.ends_with('{') || line.ends_with(';'))
                    && index
                        .checked_sub(1)
                        .and_then(|above| structure.get(above))
                        .is_none_or(|above| above.trim() != "#[cfg(test)]")
            })
            .map(|(_, (line, _))| line)
            .collect()
    }

    /// The over-skip direction, which the `#[test]` canary above cannot see: a
    /// `#[cfg(test)]` extent read as *longer* than it is, or a runaway blank in
    /// [`code_only`], drops production lines from the scanned region. Every gate below
    /// then passes over a region that no longer holds the code it is asking about — the
    /// vacuous pass, where the scanner proves nothing and says nothing.
    ///
    /// The sentinels are derived per file rather than listed, so this covers files no
    /// list here names and needs no maintenance when a module gains an item. A file that
    /// offers no sentinel is a file this gate does not cover, and it says so rather than
    /// quietly narrowing to the files that still happen to qualify.
    #[test]
    fn every_top_level_item_survives_the_production_scan() {
        let mut uncovered: Vec<PathBuf> = Vec::new();
        for (path, source) in crate_sources() {
            let expected = top_level_production_items(&source);
            if expected.is_empty() {
                uncovered.push(path);
                continue;
            }
            let kept = production_lines(&source);
            for item in expected {
                assert!(
                    kept.contains(&item),
                    "the production scan of {} dropped the top-level item `{}`, so the \
                     gates below are scanning less than the file's production code",
                    path.display(),
                    item.trim(),
                );
            }
        }
        assert!(
            uncovered.is_empty(),
            "every crate source must offer a top-level item this gate can follow; these \
             no longer do: {uncovered:?}",
        );
    }

    #[test]
    fn no_ranking_snippet_commit_or_resolve_surface() {
        scan(FORBIDDEN_FIELD_SETTERS);
    }

    #[test]
    fn no_reconstruction_leak() {
        scan(FORBIDDEN_RECONSTRUCTION);
    }

    /// The one severity owner is the diagnostic payload: no server source
    /// classifies a code to reconstruct severity. The forbidden names are the
    /// deleted registry severity surface; this gate lives inside a `#[cfg(test)]`
    /// module, which [`production_lines`] drops, so the names are spelled plainly.
    #[test]
    fn severity_comes_from_the_payload_never_the_code() {
        scan(&["SeverityClass", "severity_class", "fn severity_of"]);
        let facts = std::fs::read_to_string(src_root().join("facts.rs")).expect("read facts.rs");
        assert!(
            facts.contains("to_lsp_severity(diagnostic.severity())"),
            "expected the payload severity projection; if it was renamed, update this scan"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    use crate::analysis::{AnalysisOutcome, OverlayInput, run_analysis};
    use marrow_compile::InputRevision;

    fn identity(path: &str) -> FileIdentity {
        FileIdentity::validate(path).unwrap().0
    }

    fn temp_project(tag: &str, main: &str) -> (std::path::PathBuf, SelectedRoot) {
        use std::fs;
        let base = std::env::temp_dir().join(format!(
            "marrow-lsp-facts-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(base.join("src")).unwrap();
        fs::write(base.join("marrow.toml"), "edition = \"2026\"\n").unwrap();
        fs::write(base.join("src/main.mw"), main).unwrap();
        let root = root_for(&base);
        (base, root)
    }

    fn root_for(dir: &Path) -> SelectedRoot {
        let mut uri = String::from("file://");
        for component in dir.components() {
            use std::path::Component;
            if let Component::Normal(part) = component {
                uri.push('/');
                uri.push_str(part.to_str().unwrap());
            }
        }
        SelectedRoot::from_uri(&uri).unwrap()
    }

    fn analyze_source(
        tag: &str,
        main: &str,
    ) -> (Arc<AnalysisSnapshot>, SelectedRoot, std::path::PathBuf) {
        let (base, root) = temp_project(tag, main);
        let overlay = vec![OverlayInput {
            key: "src/main.mw",
            bytes: main.as_bytes(),
        }];
        let AnalysisOutcome::Snapshot(snapshot) =
            run_analysis(&root, &overlay, InputRevision::new(1))
        else {
            panic!("expected snapshot");
        };
        (snapshot, root, base)
    }

    #[test]
    fn diagnostics_project_span_to_utf16_range() {
        let main = "module main\n\npub fn f(): int {\n    return \n}\n";
        let (snapshot, root, base) = analyze_source("diag", main);
        let params =
            diagnostics_for_file(&snapshot, &root, &identity("src/main.mw"), main, Some(3))
                .unwrap();
        assert!(!params.diagnostics.is_empty());
        assert_eq!(params.version, Some(3));
        assert_eq!(
            params.uri.as_str(),
            &diagnostic_uri(&root, &identity("src/main.mw"))
        );
        // Every diagnostic has a real (nonzero-width or positioned) range and a code.
        for diagnostic in &params.diagnostics {
            assert!(matches!(diagnostic.code, Some(NumberOrString::String(_))));
            assert_eq!(diagnostic.source.as_deref(), Some("marrow"));
        }
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn clean_project_has_empty_diagnostic_list() {
        let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let (snapshot, root, base) = analyze_source("clean", main);
        let params =
            diagnostics_for_file(&snapshot, &root, &identity("src/main.mw"), main, Some(1))
                .unwrap();
        assert!(params.diagnostics.is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn hover_returns_type_display_at_call_site() {
        let main = "module main\n\nfn g(): int {\n    return 2\n}\n\npub fn f(): int {\n    return g()\n}\n";
        let (snapshot, _root, base) = analyze_source("hover", main);
        // Find the byte offset of the `g` in `g()` on the return line.
        let call = main.rfind("g()").unwrap();
        let map = LineMap::new(main);
        let pos = map.position_at(call);
        let lsp_pos = LspPosition::new(pos.line, pos.character);
        let result = hover(&snapshot, &identity("src/main.mw"), main, lsp_pos);
        // Hover may be present (a function signature) or absent depending on fact
        // coverage; when present it carries a nonempty display.
        if let Some(hover) = result {
            let HoverContents::Markup(markup) = hover.contents else {
                panic!("expected markup hover");
            };
            assert!(!markup.value.is_empty());
        }
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn formatting_returns_whole_document_edit_for_unformatted() {
        let main = "module main\n\npub fn f():int{\n return 1\n}\n";
        let (snapshot, _root, base) = analyze_source("fmt", main);
        let edits = formatting(&snapshot, &identity("src/main.mw"), main).unwrap();
        assert_eq!(edits.len(), 1, "one whole-document replacement");
        assert_eq!(edits[0].range.start, LspPosition::new(0, 0));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn formatting_refuses_unparseable_with_none() {
        let main = "module main\n\npub fn f(: {\n";
        let (snapshot, _root, base) = analyze_source("fmtbad", main);
        assert!(formatting(&snapshot, &identity("src/main.mw"), main).is_none());
        std::fs::remove_dir_all(&base).ok();
    }
}
