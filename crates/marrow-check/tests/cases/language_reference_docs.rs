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
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("language")
        .join(file_name);
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

fn block_containing(file_name: &str, needle: &str) -> String {
    mw_blocks(file_name)
        .into_iter()
        .find(|block| block.source.contains(needle))
        .map(|block| block.source)
        .unwrap_or_else(|| panic!("{file_name} has no mw block containing {needle:?}"))
}

fn source_path_for_module(source: &str) -> String {
    let module_line = source
        .lines()
        .find(|line| line.starts_with("module "))
        .expect("documented example must be a complete module");
    let module = module_line.trim_start_matches("module ").trim();
    format!("src/{}.mw", module.replace("::", "/"))
}

#[test]
fn resources_unique_index_lookup_example_checks_clean() {
    let source = block_containing("resources-and-storage.md", "^books.byIsbn(isbn)");
    assert!(
        source.trim_start().starts_with("module "),
        "the unique index lookup example must be documented as a complete module"
    );
    let relative_path = source_path_for_module(&source);
    let root = temp_project("docs-unique-index-lookup", |root| {
        write(root, &relative_path, &source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
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
