//! Exact-symbol absence gates over the workspace: the deleted ledger-ID-only occurrence
//! and effect lookups must not reappear, in production code or in a test.
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

/// No lookup recovers the durable node an operation addresses from a record `TypeId`, a
/// resource spelling, or a whole-payload entry site.
///
/// A branch's materialized entry record and a resource spelling are Product *declaration*
/// facts, and one declaration may be projected by several store roots, so none of them
/// names an occurrence. Each deleted lookup answered with whichever root happened to be
/// declared first: `branch_by_record` searched every executable root's branch tree,
/// `by_resource`/`root_by_resource` mapped a resource to one arbitrary placement, and
/// `root_by_entry_site` scanned for a site the addressed node already carries. The
/// occurrence comes from the address that was resolved; the declaration answers
/// record-shape and constructor questions and names no root.
#[test]
fn no_lookup_recovers_an_occurrence_from_a_declaration_fact() {
    for needle in [
        "root_by_entry_site",
        "branch_by_record",
        "root_by_resource",
        ".by_resource",
        "by_resource:",
    ] {
        let found = occurrences(needle);
        assert!(
            found.is_empty(),
            "`{needle}` recovers a root occurrence from a fact its Product declaration \
             owns, so it answers with the first root declared: {found:?}",
        );
    }
}

/// The container digest slot has one owner across the workspace.
///
/// Recomputing a forged image's digest means naming the slot and the payload it covers by
/// offset, which is the container header format. A hand copy that drifts from the real
/// header fails silently rather than loudly: it yields artifacts that stop at the envelope
/// gate, and every hostile test asserting a *rejection* still passes — at the wrong phase,
/// for the wrong reason. So the header layout is written once and included.
#[test]
fn the_forged_image_digest_has_one_owner() {
    let found = occurrences("fn rehash(");
    assert_eq!(
        found.len(),
        1,
        "the forged-image digest is recomputed in more than one place, so the container \
         header layout is hand-copied: {found:?}",
    );
    assert!(
        found[0]
            .0
            .ends_with("marrow-image/tests/common/image_forgery.rs"),
        "the one owner is the shared include beside the seam protocol: {found:?}",
    );
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
