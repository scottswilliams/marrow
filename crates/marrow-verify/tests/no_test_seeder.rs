//! Absence gate: the sealed [`marrow_verify::VerifiedImage`] the VM accepts has
//! exactly one constructor, the phased verifier, and no test-only or fixture-seeder
//! bypass may be added.
//!
//! `VerifiedImage`'s fields are `pub(crate)`, so a struct literal can only appear
//! inside `marrow-verify`; this gate proves it appears only where the verifier seals
//! it, and that no function anywhere in the workspace returns a `VerifiedImage`
//! except the verifier's own `verify`. A raw fixture seeder that mints a trusted
//! image without verification would trip this test, keeping the
//! `bytes → verify → VerifiedImage` trust path the sole way to obtain a runnable
//! image — for test images (the TEST-ENTRY table) exactly as for run images.

use std::path::{Path, PathBuf};

/// The one source file allowed to construct a `VerifiedImage`: the verifier's
/// sealing pass.
const SEALING_FILE: &str = "crates/marrow-verify/src/verify.rs";

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow-verify`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .to_path_buf()
}

/// Every `crates/*/src/**/*.rs` file, with its workspace-relative path.
fn workspace_source_files(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let crates = root.join("crates");
    for crate_dir in read_dir_sorted(&crates) {
        let src = crate_dir.join("src");
        if src.is_dir() {
            collect_rs(&src, root, &mut out);
        }
    }
    out
}

fn collect_rs(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
    for entry in read_dir_sorted(dir) {
        if entry.is_dir() {
            collect_rs(&entry, root, out);
        } else if entry.extension().is_some_and(|ext| ext == "rs") {
            let rel = entry
                .strip_prefix(root)
                .expect("path under root")
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&entry).expect("read source file");
            out.push((rel, text));
        }
    }
}

fn read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    paths.sort();
    paths
}

/// A `VerifiedImage { ... }` struct literal appears only in the sealing file, so no
/// other module — production or `#[cfg(test)]` — constructs a trusted image directly.
/// The `struct VerifiedImage { ... }` definition itself is not a construction.
#[test]
fn verified_image_is_constructed_only_by_the_verifier() {
    let root = workspace_root();
    for (path, text) in workspace_source_files(&root) {
        if path == SEALING_FILE {
            continue;
        }
        for line in text.lines() {
            let constructs = line.contains("VerifiedImage {")
                && !line.contains("struct VerifiedImage")
                && !line.contains("impl VerifiedImage");
            assert!(
                !constructs,
                "{path} constructs a VerifiedImage directly; only {SEALING_FILE} may seal one \
                 (no fixture seeder or test-only constructor may bypass verification): `{}`",
                line.trim()
            );
        }
    }
}

/// No function in the workspace returns a `VerifiedImage` except the verifier's
/// `verify`, so there is no alternate factory a fixture or test could call to obtain
/// a trusted image without running the phased verification.
#[test]
fn verify_is_the_only_function_returning_a_verified_image() {
    let root = workspace_root();
    for (path, text) in workspace_source_files(&root) {
        for line in text.lines() {
            let trimmed = line.trim_start();
            let returns_image = trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ");
            if returns_image && line.contains("-> VerifiedImage") {
                let is_verify = path == SEALING_FILE
                    && (trimmed.starts_with("pub fn verify(") || trimmed.starts_with("fn seal("));
                assert!(
                    is_verify,
                    "{path} declares a function returning VerifiedImage outside the verifier: \
                     `{}`",
                    line.trim()
                );
            }
        }
    }
}
