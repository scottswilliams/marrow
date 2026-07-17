//! Absence gate: `durable/physical.rs` is the single owner of the durable cell
//! layout — the structural tags that discriminate roots, fields, branches, and
//! cursors, and the raw key construction that spells a cell key from a name and a
//! key value.
//!
//! Two invariants ride this gate. First, the durable modules widened the one
//! consequence planner and its node-parametric primitives to serve branch entries one
//! level down without introducing a second marker/leaf topology: no durable module
//! other than `physical.rs` may call the escaped-name key encoder that builds a cell
//! key. Second, a structural-tag byte literal (`0x20` root, `0x30` branch, `0xFF`
//! cursor) must not leak beyond the layout owner into any module that consumes the
//! store — not the other durable modules, and not the VM or compiler, which name typed
//! effects and sites rather than physical cell tags. A second owner — a hand-rolled
//! branch key, a duplicate tag constant in any of those trees — would trip this gate.
//! Consuming `physical.rs`'s own published `MARKER_VALUE` constant by name is not a
//! second owner and is allowed.
//!
//! The `0x20` literal names two positionally-disjoint roles the layout owner spells —
//! the root discriminator inside the entry-family prefix and the group tag that follows
//! a marker terminator — so the existing `0x20` scan already forbids either leaking to a
//! second owner; the group tag needs no new literal.

use std::path::{Path, PathBuf};

/// The one durable-module file allowed to spell the cell layout.
const LAYOUT_OWNER: &str = "physical.rs";

/// The structural-tag byte literals only the layout owner may spell: the root, branch,
/// and cursor discriminators. (`0x10` field and `0x00` marker terminator are omitted:
/// those byte values are too common in unrelated code to scan for without noise; the
/// branch and cursor tags are distinctive enough to make a second owner conspicuous.)
const STRUCTURAL_TAG_LITERALS: &[&str] = &["0x20", "0x30", "0xFF", "0xff"];

/// The raw cell-key name encoder. It appears only where a durable cell key is spelled,
/// so a call outside the layout owner is a second key-construction site.
const RAW_KEY_ENCODER: &str = "encode_escaped_bytes";

/// The `crates` directory, parent of every crate's source tree. CARGO_MANIFEST_DIR is
/// `<root>/crates/marrow-kernel`, so its parent is the crates root — the scan resolves
/// sibling crates from here rather than from the process working directory.
fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/marrow-kernel has a parent")
        .to_path_buf()
}

/// This kernel crate's `src/durable` directory, the durable module owner's home.
fn durable_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("durable")
}

/// Every `.rs` file at or below `dir`, recursively so a future nested module cannot
/// escape the scan, as `(crates-relative label, text)` in sorted order, skipping any
/// file whose name equals `skip` (the layout owner, where present).
fn rust_sources_under(dir: &Path, skip: Option<&str>) -> Vec<(String, String)> {
    let root = crates_root();
    let mut out = Vec::new();
    collect_rust_sources(dir, skip, &root, &mut out);
    out.sort();
    out
}

fn collect_rust_sources(
    dir: &Path,
    skip: Option<&str>,
    root: &Path,
    out: &mut Vec<(String, String)>,
) {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read {}: {err}", dir.display()))
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_rust_sources(&path, skip, root, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            if skip == path.file_name().and_then(|name| name.to_str()) {
                continue;
            }
            let label = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let text = std::fs::read_to_string(&path).expect("read rust source");
            out.push((label, text));
        }
    }
}

/// No module outside the layout owner spells a structural-tag byte literal: not the
/// durable modules, and not the VM or compiler that consume the store's typed effects
/// and sites. Widening the scan past the durable directory keeps the cell layout's one
/// owner from being duplicated in a downstream tree.
#[test]
fn structural_tag_literals_live_only_in_the_layout_owner() {
    let mut files = rust_sources_under(&durable_dir(), Some(LAYOUT_OWNER));
    files.extend(rust_sources_under(
        &crates_root().join("marrow-vm").join("src"),
        None,
    ));
    files.extend(rust_sources_under(
        &crates_root().join("marrow-compile").join("src"),
        None,
    ));
    for (label, text) in files {
        for line in text.lines() {
            // A comment may legitimately mention a tag value (the layout is documented
            // across the module); only code is a second owner.
            let code = line.split("//").next().unwrap_or("");
            for tag in STRUCTURAL_TAG_LITERALS {
                assert!(
                    !code.contains(tag),
                    "{label} spells the structural-tag literal `{tag}`; only \
                     durable/{LAYOUT_OWNER} owns the cell layout: `{}`",
                    line.trim()
                );
            }
        }
    }
}

/// No durable module other than `physical.rs` calls the escaped-name key encoder, so
/// raw durable cell keys are built in exactly one place.
#[test]
fn raw_cell_key_construction_lives_only_in_the_layout_owner() {
    for (label, text) in rust_sources_under(&durable_dir(), Some(LAYOUT_OWNER)) {
        assert!(
            !text.contains(RAW_KEY_ENCODER),
            "{label} calls `{RAW_KEY_ENCODER}` to build a cell key; only \
             durable/{LAYOUT_OWNER} constructs durable cell keys"
        );
    }
}
