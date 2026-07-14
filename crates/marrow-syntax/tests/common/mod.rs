//! Shared helpers for the integration tests: one reader over the fenced `mw`
//! code blocks in the language reference, so the lexer, parser, and formatter
//! suites all filter the same source of documented examples, plus the reusable
//! bounded [`oracle`] the source-bytes drivers adapt.
pub mod oracle;

use std::path::Path;

use marrow_syntax::{Diagnostic, DiagnosticReason, LexerDiagnosticReason, ParseDiagnosticReason};

/// Wrap a parser-stage reason in the unified diagnostic-reason enum, so the
/// parse suites can assert on the typed reason rather than rendered prose.
pub fn parse_reason(reason: ParseDiagnosticReason) -> DiagnosticReason {
    DiagnosticReason::Parser(reason)
}

/// Wrap a lexer-stage reason in the unified diagnostic-reason enum.
pub fn lexer_reason(reason: LexerDiagnosticReason) -> DiagnosticReason {
    DiagnosticReason::Lexer(reason)
}

/// Whether any diagnostic carries the given typed reason.
pub fn has_reason(diagnostics: &[Diagnostic], reason: DiagnosticReason) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.reason == reason)
}

/// How many diagnostics carry the given typed reason.
pub fn reason_count(diagnostics: &[Diagnostic], reason: DiagnosticReason) -> usize {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.reason == reason)
        .count()
}

/// One fenced ```mw``` block from a language-reference markdown file.
pub struct MwBlock {
    /// The markdown file name (such as `resources.md`).
    pub path: String,
    /// The 1-based index of this block within its file.
    pub index: usize,
    /// The block's source text, with a trailing newline on each line.
    pub source: String,
    /// Whether the block opens with a `module ` declaration, i.e. it is a
    /// complete library file rather than a signature-only or fragment example.
    pub starts_with_module: bool,
}

/// Read every fenced ```mw``` block from `docs/language/*.md`, in file then
/// block order.
pub fn mw_blocks() -> Vec<MwBlock> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("language");
    let mut entries = std::fs::read_dir(&dir)
        .expect("read docs/language")
        .map(|entry| entry.expect("language doc entry").path())
        .collect::<Vec<_>>();
    entries.sort();

    let mut blocks = Vec::new();
    for path in entries {
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).expect("read language doc");
        let mut in_block = false;
        let mut index = 0usize;
        let mut source = String::new();
        for line in text.lines() {
            if line.trim() == "```mw" {
                in_block = true;
                index += 1;
                source.clear();
                continue;
            }
            if line.trim() == "```" && in_block {
                blocks.push(MwBlock {
                    path: file_name.clone(),
                    index,
                    starts_with_module: source.trim_start().starts_with("module "),
                    source: source.clone(),
                });
                in_block = false;
                continue;
            }
            if in_block {
                source.push_str(line);
                source.push('\n');
            }
        }
    }
    blocks
}

/// The blocks that open with a `module ` declaration: complete library files
/// that must parse and format without diagnostics.
pub fn documented_module_blocks() -> Vec<MwBlock> {
    mw_blocks()
        .into_iter()
        .filter(|block| block.starts_with_module)
        .collect()
}

/// Every tracked `.mw` fixture under `fixtures/v01/`, in sorted path order. These
/// are the preserved-semantics corpus: shared-syntax programs the beta parser
/// structures, fed to the oracle as valid parse subjects.
pub fn tracked_mw_fixtures() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("v01");
    let mut out = Vec::new();
    collect_mw(&root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn collect_mw(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths = entries
        .map(|entry| entry.expect("fixture entry").path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_mw(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("mw") {
            let text = std::fs::read_to_string(&path).expect("read mw fixture");
            out.push((path.display().to_string(), text));
        }
    }
}

/// The reference library: the single `mw` block in `sample.md`, the canonical
/// end-to-end example the parser structure tests assert against.
pub fn reference_sample() -> String {
    mw_blocks()
        .into_iter()
        .find(|block| block.path == "sample.md")
        .expect("sample.md should contain a Marrow code block")
        .source
}
