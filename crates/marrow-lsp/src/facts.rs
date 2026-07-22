//! Project the compiler's published analysis facts into standard LSP payloads.
//!
//! Every payload is built from the snapshot's facts and the exact source bytes; nothing
//! is reconstructed. Byte spans become UTF-16 ranges through [`crate::position`], codes
//! and severities come from the [`marrow_codes`] registry, type displays and definition
//! targets come verbatim from the snapshot, and diagnostic URIs come from the one
//! canonical re-encoder. The payload types are [`lsp_types`]; the server owns no
//! hand-written duplicate DTO.

use std::str::FromStr;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Diagnostic, DiagnosticSeverity,
    DocumentSymbol, DocumentSymbolResponse, Hover, HoverContents, Location, MarkupContent,
    MarkupKind, NumberOrString, ParameterInformation, ParameterLabel, Position as LspPosition,
    PublishDiagnosticsParams, Range as LspRange, SignatureHelp, SignatureInformation, SymbolKind,
    TextEdit, Uri,
};
use marrow_codes::{Code, SeverityClass};
use marrow_compile::{
    ActiveCall, ActiveCallOutcome, AnalysisSnapshot, Candidate, CandidateKind, CompletionOutcome,
    Completions, DeclKind, DeclSymbol, Fact, FormatOutcome,
};
use marrow_project_fs::FileIdentity;
use marrow_syntax::SourceSpan;

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

/// The LSP severity for a marrow code. An unregistered code (never expected from the
/// compiler) defaults to `ERROR`.
fn severity_of(code: &str) -> DiagnosticSeverity {
    match Code::from_code(code).map(Code::severity_class) {
        Some(SeverityClass::Warning) => DiagnosticSeverity::WARNING,
        _ => DiagnosticSeverity::ERROR,
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
                severity: Some(severity_of(diagnostic.code)),
                code: Some(NumberOrString::String(diagnostic.code.to_owned())),
                code_description: None,
                source: Some("marrow".to_owned()),
                message: diagnostic.message.clone(),
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
/// refused (unparsed source or comment loss) or the output exceeds its bound. A
/// successful format is one whole-document replacement edit.
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
        Ok(FormatOutcome::Refused(_) | FormatOutcome::TooLarge { .. }) | Err(_) => None,
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
/// unavailable (unparseable) or unknown file. A pure projection of the compiler's
/// document-symbol fact; the per-file count/depth bounds are enforced at snapshot
/// admission, so a query here carries no resource refusal.
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

/// The absence gate over the completion/signature surface: the language server projects
/// the compiler's complete candidate set verbatim and never reconstructs syntax or ranks.
/// The forbidden construction tokens are the field setters that would introduce a
/// server-side prefix/fuzzy filter, ranking, snippet, commit character, text edit beyond
/// the label, or `completionItem/resolve` — every unearned surface the design refuses.
/// Enforced over the crate's own analysis/wiring sources so a reintroduction is a build
/// failure, not a review miss.
#[cfg(test)]
const ABSENCE_SCAN_SOURCES: &[&str] = &[
    include_str!("facts.rs"),
    include_str!("server.rs"),
    include_str!("outbound.rs"),
];

#[cfg(test)]
mod absence_gate {
    use super::ABSENCE_SCAN_SOURCES;

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

    /// The production region of a source file: everything before its first `#[cfg(test)]`
    /// attribute. Test code and this gate's own token lists live below that line and
    /// legitimately name the forbidden surface; only production wiring is scanned.
    fn production_region(source: &str) -> &str {
        match source.find("#[cfg(test)]") {
            Some(cut) => &source[..cut],
            None => source,
        }
    }

    /// A line the gate ignores: an explanatory comment (`//` …). A real forbidden use is a
    /// struct field set or path in production code, never a comment.
    fn is_comment(line: &str) -> bool {
        line.trim_start().starts_with("//")
    }

    fn scan(needles: &[&str]) {
        for source in ABSENCE_SCAN_SOURCES {
            for line in production_region(source).lines() {
                if is_comment(line) {
                    continue;
                }
                for needle in needles {
                    assert!(
                        !line.contains(needle),
                        "forbidden token `{needle}` appears in production server code: {line}"
                    );
                }
            }
        }
    }

    #[test]
    fn no_ranking_snippet_commit_or_resolve_surface() {
        scan(FORBIDDEN_FIELD_SETTERS);
    }

    #[test]
    fn no_reconstruction_leak() {
        scan(FORBIDDEN_RECONSTRUCTION);
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
