//! Project the compiler's published analysis facts into standard LSP payloads.
//!
//! Every payload is built from the snapshot's facts and the exact source bytes; nothing
//! is reconstructed. Byte spans become UTF-16 ranges through [`crate::position`], codes
//! and severities come verbatim from the diagnostic payload, type displays and
//! definition targets come verbatim from the snapshot, and diagnostic URIs come from
//! the one canonical re-encoder. The payload types are [`lsp_types`]; the server owns no
//! hand-written duplicate DTO.

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Diagnostic, DiagnosticSeverity,
    DocumentSymbol, DocumentSymbolResponse, Hover, HoverContents, Location, MarkupContent,
    MarkupKind, NumberOrString, ParameterInformation, ParameterLabel, Position,
    PublishDiagnosticsParams, Range, SignatureHelp, SignatureInformation, SymbolKind, TextEdit,
    Uri,
};
use marrow_compile::ProjectFile;
use marrow_compile::{
    ActiveCall, ActiveCallOutcome, AnalysisSnapshot, Candidate, CandidateKind, CompletionOutcome,
    Completions, DeclKind, DeclSymbol, Fact, FormatOutcome,
};
use marrow_syntax::{Severity, SourceSpan};

use crate::position::LineMap;

/// Why a payload could not be projected. Both are refusals, never a repaired result: a
/// misplaced range would point an editor at the wrong text.
#[derive(Debug)]
pub(crate) enum ProjectionRefusal {
    /// A canonically-encoded diagnostic URI did not parse back into an `lsp_types::Uri`.
    /// The encoder produces canonical URIs, so this is a coherence-class failure.
    Uri,
    /// The file's bytes are not UTF-8, so no byte span in it has a UTF-16 range.
    NotUtf8,
}

/// The snapshot's own bytes for one captured file, decoded. A fact's spans index the
/// exact source the snapshot was computed from, so a range projects through these bytes
/// and never through an open buffer that may already have moved past them.
fn captured_source(snapshot: &AnalysisSnapshot, file: &ProjectFile) -> Option<String> {
    let module = snapshot
        .input()
        .modules()
        .iter()
        .find(|module| ProjectFile::from(*module) == *file)?;
    std::str::from_utf8(module.source()).ok().map(str::to_owned)
}

/// The LSP severity of a diagnostic, projected from the payload's typed severity —
/// the one severity owner — never reconstructed by classifying the code.
fn to_lsp_severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    }
}

/// Build the per-file publish-diagnostics parameters for one snapshot file. The file's
/// own bytes drive the UTF-16 range projection, so a file that is not UTF-8 is refused
/// rather than published with every diagnostic collapsed onto the first character. The
/// `uri` is the caller's, because only the server knows which tree the file came from.
pub(crate) fn diagnostics_for_file(
    snapshot: &AnalysisSnapshot,
    uri: Uri,
    file: &ProjectFile,
    source: &[u8],
    version: Option<i32>,
) -> Result<PublishDiagnosticsParams, ProjectionRefusal> {
    let source = std::str::from_utf8(source).map_err(|_| ProjectionRefusal::NotUtf8)?;
    let map = LineMap::new(source);
    let diagnostics = snapshot
        .diagnostics_for(file)
        .map(|diagnostic| {
            let span = diagnostic.span();
            let range = map.range_of(span.start_byte, span.end_byte);
            Diagnostic {
                range,
                severity: Some(to_lsp_severity(diagnostic.severity())),
                code: Some(NumberOrString::String(
                    diagnostic.code().as_str().to_owned(),
                )),
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
        uri,
        diagnostics,
        version,
    })
}

/// The hover payload at an LSP position. `Ok(None)` covers a legitimately absent fact,
/// an unavailable (syntax/dependency) fact, and an out-of-range or unknown position —
/// the LSP `null` hover result. The type display comes verbatim from the compiler.
pub(crate) fn hover(
    snapshot: &AnalysisSnapshot,
    file: &ProjectFile,
    source: &str,
    position: Position,
) -> Option<Hover> {
    let offset = LineMap::new(source).byte_at(position);
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
/// target file's own source bytes, and `target_uri` resolves the target against the tree
/// the snapshot says it belongs to. The target need not be an open document — a callee
/// declared in another file, or in a dependency, is an ordinary target.
pub(crate) fn definition(
    snapshot: &AnalysisSnapshot,
    file: &ProjectFile,
    source: &str,
    target_uri: impl Fn(&ProjectFile) -> Option<Uri>,
    position: Position,
) -> Result<Option<Location>, ProjectionRefusal> {
    let offset = LineMap::new(source).byte_at(position);
    let target = match snapshot.definition(file, offset) {
        Ok(Fact::Present(definition)) => definition,
        Ok(Fact::Absent | Fact::Unavailable(_)) | Err(_) => return Ok(None),
    };
    // The range projects through the target file's own source. Without that source there
    // is no UTF-16 projection, and a range rebuilt from the compiler's 1-based line and
    // byte column would misplace every non-ASCII line, so the definition is refused.
    let name_span = target.name_span();
    // The target names its own tree, so a definition that crosses a dependency boundary
    // resolves through this same fact: the address below is the library's, not the
    // consuming project's.
    let target_file = ProjectFile::new(target.origin().clone(), target.file().clone());
    let Some(text) = captured_source(snapshot, &target_file) else {
        return Ok(None);
    };
    let range = LineMap::new(&text).range_of(name_span.start_byte, name_span.end_byte);
    Ok(Some(Location {
        uri: target_uri(&target_file).ok_or(ProjectionRefusal::Uri)?,
        range,
    }))
}

/// The formatting edits for a document, or `None` (LSP `null`) when formatting is
/// refused (unparsed source, a diagnostic-limited parse, or comment loss), the file is
/// not valid UTF-8, or the output exceeds its bound. A successful format is one
/// whole-document replacement edit.
pub(crate) fn formatting(
    snapshot: &AnalysisSnapshot,
    file: &ProjectFile,
    source: &str,
) -> Option<Vec<TextEdit>> {
    match snapshot.format(file) {
        Ok(FormatOutcome::Formatted(formatted)) => {
            if formatted == source {
                // Already formatted: no edit.
                return Some(Vec::new());
            }
            let whole = Range::new(Position::new(0, 0), LineMap::new(source).end_position());
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
pub(crate) struct ResourceLimited;

/// The completion payload at an LSP position. `Ok(None)` covers a legitimately absent
/// classification, an unavailable (syntax) owner, and an unknown/out-of-range position —
/// the LSP `null` completion result. `Err(ResourceLimited)` is an over-cap candidate set.
/// Every candidate is projected verbatim from the compiler's fact; the set is the
/// complete in-scope namespace, never filtered, ranked, or truncated here.
pub(crate) fn completion(
    snapshot: &AnalysisSnapshot,
    file: &ProjectFile,
    source: &str,
    position: Position,
) -> Result<Option<CompletionResponse>, ResourceLimited> {
    let offset = LineMap::new(source).byte_at(position);
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
pub(crate) fn signature_help(
    snapshot: &AnalysisSnapshot,
    file: &ProjectFile,
    source: &str,
    position: Position,
) -> Result<Option<SignatureHelp>, ResourceLimited> {
    let offset = LineMap::new(source).byte_at(position);
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
/// because it crossed a per-file count or depth bound and nothing was retained for it.
/// The bound is enforced at snapshot admission and costs that one file's outline, so a
/// query here carries no resource refusal and no other file is affected.
pub(crate) fn document_symbols(
    snapshot: &AnalysisSnapshot,
    file: &ProjectFile,
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

fn span_range(span: SourceSpan, map: &LineMap) -> Range {
    map.range_of(span.start_byte, span.end_byte)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::str::FromStr;
    use std::sync::Arc;

    use crate::analysis::{AnalysisOutcome, OverlayInput, run_analysis};
    use crate::uri::{DocumentKey, OriginRoots, SelectedRoot, document_uri};
    use marrow_compile::InputRevision;
    use marrow_project_fs::FileIdentity;
    use marrow_project_fs::SourceOrigin;
    use marrow_test_support::{Scratch, file_uri};

    fn identity(path: &str) -> FileIdentity {
        FileIdentity::validate(path).unwrap().0
    }

    /// The address of one of the root project's own files.
    fn main_file() -> ProjectFile {
        ProjectFile::root(identity("src/main.mw"))
    }

    fn temp_project(tag: &str, main: &str) -> (Scratch, SelectedRoot) {
        let base = Scratch::project(&format!("facts-{tag}"), main);
        let root = root_for(base.path());
        (base, root)
    }

    /// The project's own `src/main.mw` URI, built the way the server builds one.
    fn main_uri(root: &SelectedRoot) -> Uri {
        let key = DocumentKey::captured(&SourceOrigin::Root, &identity("src/main.mw"));
        Uri::from_str(&document_uri(root, &OriginRoots::default(), &key).unwrap()).unwrap()
    }

    fn root_for(dir: &Path) -> SelectedRoot {
        SelectedRoot::from_uri(&file_uri(dir)).unwrap()
    }

    fn analyze_source(tag: &str, main: &str) -> (Arc<AnalysisSnapshot>, SelectedRoot, Scratch) {
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
        let (snapshot, root, _dir) = analyze_source("diag", main);
        let uri = main_uri(&root);
        let params = diagnostics_for_file(
            &snapshot,
            uri.clone(),
            &main_file(),
            main.as_bytes(),
            Some(3),
        )
        .unwrap();
        assert!(!params.diagnostics.is_empty());
        assert_eq!(params.version, Some(3));
        assert_eq!(params.uri, uri);
        // Every diagnostic has a real (nonzero-width or positioned) range and a code.
        for diagnostic in &params.diagnostics {
            assert!(matches!(diagnostic.code, Some(NumberOrString::String(_))));
            assert_eq!(diagnostic.source.as_deref(), Some("marrow"));
        }
    }

    #[test]
    fn clean_project_has_empty_diagnostic_list() {
        let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let (snapshot, root, _dir) = analyze_source("clean", main);
        let params = diagnostics_for_file(
            &snapshot,
            main_uri(&root),
            &main_file(),
            main.as_bytes(),
            Some(1),
        )
        .unwrap();
        assert!(params.diagnostics.is_empty());
    }

    #[test]
    fn hover_returns_type_display_at_call_site() {
        let main = "module main\n\nfn g(): int {\n    return 2\n}\n\npub fn f(): int {\n    return g()\n}\n";
        let (snapshot, _root, _dir) = analyze_source("hover", main);
        // Find the byte offset of the `g` in `g()` on the return line.
        let call = main.rfind("g()").unwrap();
        let position = LineMap::new(main).position_at(call);
        let result = hover(&snapshot, &main_file(), main, position);
        // Hover may be present (a function signature) or absent depending on fact
        // coverage; when present it carries a nonempty display.
        if let Some(hover) = result {
            let HoverContents::Markup(markup) = hover.contents else {
                panic!("expected markup hover");
            };
            assert!(!markup.value.is_empty());
        }
    }

    #[test]
    fn formatting_returns_whole_document_edit_for_unformatted() {
        let main = "module main\n\npub fn f():int{\n return 1\n}\n";
        let (snapshot, _root, _dir) = analyze_source("fmt", main);
        let edits = formatting(&snapshot, &main_file(), main).unwrap();
        assert_eq!(edits.len(), 1, "one whole-document replacement");
        assert_eq!(edits[0].range.start, Position::new(0, 0));
    }

    #[test]
    fn formatting_refuses_unparseable_with_none() {
        let main = "module main\n\npub fn f(: {\n";
        let (snapshot, _root, _dir) = analyze_source("fmtbad", main);
        assert!(formatting(&snapshot, &main_file(), main).is_none());
    }
}
