//! Absence gate: `durable/physical.rs` is the single owner of the durable cell
//! layout — the structural tags that discriminate roots, fields, branches, and
//! cursors, and the raw key construction that spells a cell key from a name and a
//! key value.
//!
//! E03 widened the one consequence planner and its node-parametric primitives to
//! serve branch entries one level down; it did not introduce a second marker/leaf
//! topology. This gate keeps it that way: no durable module other than `physical.rs`
//! may spell a structural-tag byte literal (`0x20` root, `0x30` branch, `0xFF`
//! cursor) or call the escaped-name key encoder that builds a cell key. A second
//! owner — a hand-rolled branch key, a duplicate tag constant — would trip this test.
//! Consuming `physical.rs`'s own published `MARKER_VALUE` constant by name is not a
//! second owner and is allowed.

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

fn durable_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow-kernel`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("durable")
}

/// Every `.rs` file directly under `src/durable/`, with its file name, except the
/// layout owner.
fn durable_files_except_owner() -> Vec<(String, String)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(durable_dir())
        .expect("read src/durable")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            if name == LAYOUT_OWNER {
                return None;
            }
            let text = std::fs::read_to_string(&path).expect("read durable source");
            Some((name, text))
        })
        .collect()
}

/// No durable module other than `physical.rs` spells a structural-tag byte literal, so
/// the marker/field/branch/cursor discriminators have exactly one owner.
#[test]
fn structural_tag_literals_live_only_in_the_layout_owner() {
    for (name, text) in durable_files_except_owner() {
        for line in text.lines() {
            // A comment may legitimately mention a tag value (the layout is documented
            // across the module); only code is a second owner.
            let code = line.split("//").next().unwrap_or("");
            for tag in STRUCTURAL_TAG_LITERALS {
                assert!(
                    !code.contains(tag),
                    "durable/{name} spells the structural-tag literal `{tag}`; only \
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
    for (name, text) in durable_files_except_owner() {
        assert!(
            !text.contains(RAW_KEY_ENCODER),
            "durable/{name} calls `{RAW_KEY_ENCODER}` to build a cell key; only \
             durable/{LAYOUT_OWNER} constructs durable cell keys"
        );
    }
}
