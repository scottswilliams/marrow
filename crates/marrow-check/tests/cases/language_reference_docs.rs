use crate::support;
use marrow_check::check_project;

use support::{config, temp_project, write};

const MIN_DOCUMENTED_MODULE_EXAMPLES: usize = 5;

struct MwBlock {
    file_name: String,
    index: usize,
    source: String,
}

fn language_docs_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("language")
}

fn mw_blocks(file_name: &str) -> Vec<MwBlock> {
    let path = language_docs_dir().join(file_name);
    let text = std::fs::read_to_string(path).expect("read language doc");
    let mut blocks = Vec::new();
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
                file_name: file_name.to_string(),
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

    blocks
}

fn all_mw_blocks() -> Vec<MwBlock> {
    let mut files = std::fs::read_dir(language_docs_dir())
        .expect("read language docs")
        .map(|entry| entry.expect("language doc entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();

    files
        .into_iter()
        .flat_map(|path| {
            let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
            mw_blocks(&file_name)
        })
        .collect()
}

fn source_path_for_module(source: &str) -> String {
    let module_line = source
        .lines()
        .find(|line| line.starts_with("module "))
        .expect("documented example must be a complete module");
    let module = module_line.trim_start_matches("module ").trim();
    format!("src/{}.mw", module.replace("::", "/"))
}

/// Implementation-only storage and identity vocabulary that must never surface in
/// the language reference. Each token is matched case-insensitively as a substring,
/// so spacing and hyphenation variants of the same engine concept are all caught.
const FORBIDDEN_VOCABULARY: &[&str] = &[
    "marrow.catalog.json",
    "catalog",
    "epoch",
    "structural signature",
    "shape signature",
    "source digest",
    "shape digest",
    "catalog digest",
    "engine-profile digest",
    "opaque id",
    "opaque stable id",
    "stable-id annotation",
    "never-reuse",
    "id ledger",
    "identity ledger",
    "ledger",
    "commit stamp",
    "id stamp",
    "engine-profile",
    "engine profile",
    "value-codec",
    "value codec",
    "tree-cell key",
    "fence",
];

/// Public saved-data vocabulary the storage reference must keep, so the rewrite
/// cannot pass by deleting the section instead of restating it for developers.
const REQUIRED_VOCABULARY: &[&str] = &[
    "marrow.lock",
    "stale lock",
    "pending evolution",
    "backup",
    "restore",
    "rename",
    "retire",
];

#[test]
fn language_docs_use_public_vocabulary_only() {
    let mut files = std::fs::read_dir(language_docs_dir())
        .expect("read language docs")
        .map(|entry| entry.expect("language doc entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();

    let mut violations: Vec<(String, &str, usize)> = Vec::new();
    for path in &files {
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).expect("read language doc");
        for (line_index, line) in text.to_lowercase().lines().enumerate() {
            for token in FORBIDDEN_VOCABULARY {
                if line.contains(token) {
                    violations.push((file_name.clone(), token, line_index + 1));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "language docs leak implementation-only vocabulary: {violations:#?}"
    );

    let storage_doc = std::fs::read_to_string(language_docs_dir().join("resources-and-storage.md"))
        .expect("read resources-and-storage doc")
        .to_lowercase();
    let missing: Vec<&str> = REQUIRED_VOCABULARY
        .iter()
        .copied()
        .filter(|term| !storage_doc.contains(term))
        .collect();
    assert!(
        missing.is_empty(),
        "resources-and-storage.md must state the public saved-data model, missing: {missing:#?}"
    );
}

#[test]
fn documented_module_examples_check_clean() {
    let mut checked = 0usize;

    for block in all_mw_blocks()
        .into_iter()
        .filter(|block| block.source.trim_start().starts_with("module "))
    {
        let relative_path = source_path_for_module(&block.source);
        let root = temp_project("docs-module-example", |root| {
            write(root, &relative_path, &block.source);
        });
        let (report, _program) = check_project(&root, &config()).expect("check");

        assert!(
            report.diagnostics.is_empty(),
            "{} block {} produced checker diagnostics: {:#?}",
            block.file_name,
            block.index,
            report.diagnostics
        );
        checked += 1;
    }

    assert!(
        checked >= MIN_DOCUMENTED_MODULE_EXAMPLES,
        "expected at least {MIN_DOCUMENTED_MODULE_EXAMPLES} documented module examples, found {checked}"
    );
}
