//! Exact-symbol absence gates over the workspace: the ledger-ID-only occurrence and
//! effect lookups this row deletes must not reappear, in production code or in a test.
//!
//! Each scan runs over the literal-stripped projection of the source, so a shape spelled
//! inside a comment or a string is not mistaken for the real thing. That projection has
//! exactly one owner in the workspace and is included here rather than copied.

use std::fs;
use std::path::{Path, PathBuf};

#[path = "../../marrow-compile/tests/common/source_projection.rs"]
mod source_projection;
use source_projection::without_literals;

/// Every `.rs` file under `crates/`, production and test tier alike. A deleted lookup
/// that survives in a test is still a live lookup: it keeps the shape compiling and lets
/// a later caller reach for it.
fn workspace_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read directory") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                walk(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .to_path_buf();
    let mut files = Vec::new();
    walk(&crates, &mut files);
    files.sort();
    assert!(
        files.len() > 100,
        "the whole workspace is scanned, not one crate: {} files",
        files.len()
    );
    files
}

/// Every `(file, line)` at which `needle` appears in code, comments and string literals
/// blanked.
fn occurrences(needle: &str) -> Vec<(PathBuf, usize)> {
    let mut found = Vec::new();
    for path in workspace_files() {
        let code = without_literals(&fs::read_to_string(&path).expect("read source file"));
        for (index, line) in code.lines().enumerate() {
            if line.contains(needle) {
                found.push((path.clone(), index + 1));
            }
        }
    }
    found
}

/// A managed index belongs to one root occurrence; a stored field belongs to a Product
/// declaration, which several roots may project. So a bare field ledger identity names
/// no occurrence, and an index-maintenance answer keyed on one alone unions the indexes
/// of every root that projects that Product — telling a write through one root that it
/// must keep another root's index coherent.
///
/// The replacement is [`marrow_verify::VerifiedRootOccurrence`]: the question is posed
/// through a verified occurrence handle, so it cannot be asked without naming the root.
#[test]
fn no_ledger_id_only_index_incidence_lookup_exists() {
    for needle in ["field_incidence", "root_incidence"] {
        let found = occurrences(needle);
        assert!(
            found.is_empty(),
            "`{needle}` answers an occurrence-scoped effect question from a declaration \
             identity alone; ask it through `VerifiedRootOccurrence`: {found:?}",
        );
    }
}

/// Within the verifier the surviving maintenance answers are methods of the occurrence
/// handle and of nothing else, so no sibling can grow back an image-wide form beside
/// them. (The kernel owns its own positional per-root maintenance query below this
/// boundary; it is consumed here, not restated.)
#[test]
fn every_verifier_index_maintenance_answer_is_owned_by_the_occurrence_handle() {
    let owner = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sealed.rs");
    for needle in [
        "fn entry_maintenance",
        "fn field_maintenance",
        "fn unique_collision_outcomes",
    ] {
        let defined: Vec<(PathBuf, usize)> = occurrences(needle)
            .into_iter()
            .filter(|(path, _)| path.starts_with(Path::new(env!("CARGO_MANIFEST_DIR"))))
            .collect();
        assert_eq!(
            defined.len(),
            1,
            "`{needle}` has exactly one definition in the verifier: {defined:?}",
        );
        assert_eq!(
            defined[0].0, owner,
            "`{needle}` is defined by the sealed-image owner: {defined:?}",
        );
    }
}

/// A gate that cannot see its own subject passes for the wrong reason.
#[test]
fn the_scan_sees_code_and_not_prose() {
    let planted = without_literals(
        r##"
        // fn field_incidence(&self, field: LedgerIdBytes) {}
        const DOC: &str = "field_incidence";
        const RAW: &str = r#"field_incidence"#;
        fn field_incidence() {}
        "##,
    );
    let hits: Vec<&str> = planted
        .lines()
        .filter(|line| line.contains("field_incidence"))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly the code occurrence is visible to the scan: {hits:?}",
    );
    assert!(
        hits[0].trim().starts_with("fn field_incidence"),
        "the visible occurrence is the code one: {hits:?}",
    );
}
