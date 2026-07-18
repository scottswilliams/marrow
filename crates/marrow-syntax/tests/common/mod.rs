//! Shared helpers for the integration tests: one reader over the fenced `mw`
//! code blocks in current documentation, so the lexer, parser, and formatter
//! suites consume the same source inventory, plus the reusable bounded
//! [`oracle`] the source-bytes drivers adapt.
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

/// One fenced ```mw``` block from a current-documentation Markdown file.
pub struct MwBlock {
    /// The repository-relative Markdown path (such as `docs/language/resources.md`).
    pub path: String,
    /// The 1-based index of this block within its file.
    pub index: usize,
    /// The block's source text, with a trailing newline on each line.
    pub source: String,
}

/// Read every fenced ```mw``` block from current documentation and repository
/// front doors, in file then block order. `docs/future/` is intentionally absent;
/// the production-path docs gate separately enforces that future pages contain no
/// current-source fences.
pub fn mw_blocks() -> Vec<MwBlock> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let files = current_markdown_files(&root);

    let mut blocks = Vec::new();
    for path in files {
        let doc = path
            .strip_prefix(&root)
            .expect("documentation path beneath repository root")
            .to_string_lossy()
            .into_owned();
        let text = std::fs::read_to_string(&path).expect("read markdown doc");
        let mut in_block = false;
        let mut index = 0usize;
        let mut source = String::new();
        for line in text.lines() {
            if line.trim() == "```mw" {
                assert!(!in_block, "nested mw fence in {doc} block #{index}");
                in_block = true;
                index += 1;
                source.clear();
                continue;
            }
            if line.trim() == "```" && in_block {
                blocks.push(MwBlock {
                    path: doc.clone(),
                    index,
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
        assert!(!in_block, "unterminated mw fence in {doc} block #{index}");
    }
    blocks
}

/// The `*.md` files directly in `dir` (not recursive), in sorted path order.
fn markdown_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = std::fs::read_dir(dir)
        .expect("read markdown directory")
        .map(|entry| entry.expect("markdown entry").path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

/// The `.md` files recursively beneath `dir`, optionally excluding one complete
/// subtree.
fn markdown_files_recursively(dir: &Path, excluded: Option<&Path>) -> Vec<std::path::PathBuf> {
    fn collect(dir: &Path, excluded: Option<&Path>, files: &mut Vec<std::path::PathBuf>) {
        if excluded == Some(dir) {
            return;
        }
        let mut entries = std::fs::read_dir(dir)
            .expect("read markdown directory")
            .map(|entry| entry.expect("markdown entry").path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                collect(&path, excluded, files);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("md") {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    collect(dir, excluded, &mut files);
    files
}

fn current_markdown_files(root: &Path) -> Vec<std::path::PathBuf> {
    let docs = root.join("docs");
    let future = docs.join("future");
    let mut files = markdown_files_recursively(&docs, Some(&future));
    files.extend(markdown_files(root));
    files
}

/// Every current `mw` fence. Documentation uses `mw` only for complete source
/// files; contextual fragments use `text` or `ebnf`.
pub fn documented_source_blocks() -> Vec<MwBlock> {
    mw_blocks()
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
        .find(|block| block.path == "docs/language/sample.md")
        .expect("sample.md should contain a Marrow code block")
        .source
}
