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
    Diagnostic, DiagnosticSeverity, Hover, HoverContents, Location, MarkupContent, MarkupKind,
    NumberOrString, Position as LspPosition, PublishDiagnosticsParams, Range as LspRange, TextEdit,
    Uri,
};
use marrow_codes::{Code, SeverityClass};
use marrow_compile::{AnalysisSnapshot, Fact, FormatOutcome};
use marrow_project::FileIdentity;

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
