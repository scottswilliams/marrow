//! Exact-symbol absence gates over `marrow-image/src`: shapes this crate has
//! deleted must not reappear, in production code or in its own test tier.
//!
//! Each scan runs over the literal-stripped projection of the source, so a shape
//! spelled inside a comment or a string is not mistaken for the real thing. That
//! projection has exactly one owner in the workspace and is included here rather
//! than copied.

use std::fs;
use std::path::{Path, PathBuf};

#[path = "../../marrow-compile/tests/common/source_projection.rs"]
mod source_projection;
use source_projection::without_literals;

fn src_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    assert!(!files.is_empty(), "the source tree is scanned");
    files
}

/// Every `(file, line)` at which `needle` appears in code — comments and string
/// literals blanked, `#[cfg(test)]` items deliberately retained, since a deleted
/// relationship may not survive in a test either.
fn occurrences(needle: &str) -> Vec<(PathBuf, usize)> {
    let mut found = Vec::new();
    for path in src_files() {
        let code = without_literals(&fs::read_to_string(&path).expect("read source file"));
        for (index, line) in code.lines().enumerate() {
            if line.contains(needle) {
                found.push((path.clone(), index + 1));
            }
        }
    }
    found
}

/// The root count and the record-type count are independent bounds, and neither may
/// be derived from the other again.
///
/// `MAX_ROOTS` bounds root *occurrences*; `MAX_TYPES` bounds the *type population*.
/// The deleted derivation read "each root's resource is a record type, so the type
/// table bounds the root count" — true only while every root occurrence carried its
/// own record type. Many roots may occur over one Product declaration, contributing
/// one record type between them, so the implication no longer holds; and it never
/// held downward, since declarations and monomorphization grow the type population
/// with no durable root at all. Restoring either the compile-time derivation or the
/// equality known-answer test would silently couple two independently justified
/// ceilings, so that a widening of one inherited the other's evidence.
#[test]
fn no_root_count_to_type_count_derivation_exists() {
    for needle in [
        "MAX_ROOTS <= MAX_TYPES",
        "MAX_TYPES >= MAX_ROOTS",
        "MAX_ROOTS, MAX_TYPES",
        "MAX_TYPES, MAX_ROOTS",
    ] {
        let found = occurrences(needle);
        assert!(
            found.is_empty(),
            "`{needle}` re-derives one bound from the other; each carries its own \
             evidence: {found:?}",
        );
    }
}

/// A gate that cannot see its own subject passes for the wrong reason.
#[test]
fn the_scan_sees_code_and_not_prose() {
    let planted = without_literals(
        r##"
        // assert!(MAX_ROOTS <= MAX_TYPES);
        const DOC: &str = "MAX_ROOTS <= MAX_TYPES";
        const RAW: &str = r#"MAX_ROOTS <= MAX_TYPES"#;
        const LIVE: bool = MAX_ROOTS <= MAX_TYPES;
        "##,
    );
    let hits: Vec<&str> = planted
        .lines()
        .filter(|line| line.contains("MAX_ROOTS <= MAX_TYPES"))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly the code occurrence is visible to the scan: {hits:?}",
    );
    assert!(
        hits[0].contains("const LIVE"),
        "the visible occurrence is the code one: {hits:?}",
    );
}

/// No site id is ever a narrowed table length, and no second site mint path exists.
///
/// The site table was appended to directly, its id taken as `self.sites.len() as u16`,
/// with the bound seen only at `encode()`. A producer could request past `u16::MAX`
/// distinct durable nodes, receive a wrapped id, and hand two distinct nodes one site
/// operand. The bounded plan mints only after checking vacant capacity, so restoring
/// either the raw append or a length-narrowing cast would reopen the aliasing.
#[test]
fn no_length_narrowing_site_mint_path_exists() {
    for needle in [
        "sites.len() as u16",
        "rows.len() as u16",
        "fn add_site",
        "fn alloc_site",
        ".add_site(",
        ".alloc_site(",
    ] {
        let hits = occurrences(needle);
        assert!(
            hits.is_empty(),
            "`{needle}` is deleted from the site path; found at {hits:?}",
        );
    }
}

/// The whole `as u16` family on the site path, in every spelling a length could take.
/// The plan's own conversions are `u16::try_from`, so any `as u16` reached from a length
/// is the deleted shape returning.
#[test]
fn the_site_plan_narrows_no_length_with_an_as_cast() {
    let plan =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/site_plan.rs"))
            .expect("read the site plan");
    let code = without_literals(&plan);
    assert!(
        !code.contains("as u16"),
        "the site plan converts with `u16::try_from`, never an `as` cast",
    );
    assert!(
        code.contains("u16::try_from"),
        "the plan's checked conversion is present, so this gate has a live subject",
    );
}

/// The planted-probe half of the two gates above: each needle must be visible to the
/// scan in code and invisible in prose, or the gates pass for the wrong reason.
#[test]
fn the_site_scan_sees_code_and_not_prose() {
    let planted = without_literals(
        r##"
        // let id = self.sites.len() as u16;
        const DOC: &str = "sites.len() as u16";
        const RAW: &str = r#"sites.len() as u16"#;
        let live = self.sites.len() as u16;
        "##,
    );
    let hits: Vec<&str> = planted
        .lines()
        .filter(|line| line.contains("sites.len() as u16"))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly the code occurrence is visible to the scan: {hits:?}",
    );
    assert!(
        hits[0].contains("let live"),
        "the visible occurrence is the code one: {hits:?}",
    );
}
