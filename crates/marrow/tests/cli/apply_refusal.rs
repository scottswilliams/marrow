//! A change that would reinterpret stored cells, through the built binaries.
//!
//! A stored struct is written into one cell by position. Swapping its two leaves in the
//! source would make every existing cell read `x` as `y`, so both ways of moving a store
//! to the edited program refuse: `run --store` as a durable-contract change, and
//! `marrow apply` with a typed reason. The store is left byte-for-byte as it was, and the
//! old program still reads what it wrote.

use std::fs;
use std::path::{Path, PathBuf};

use crate::common::{CliOutcome, stage_toolchain, staged_marrow_in, unaccepted_ceiling_id, write};
use marrow_test_support::Scratch;

const SOURCE: &str = r#"struct Pos {
    x: int
    y: int
}

resource Marker {
    required at: Pos
}

store ^markers[id: int]: Marker

pub fn put(id: int, x: int) {
    transaction {
        ^markers[id] = Marker(at: Pos(x: x, y: 2))
    }
}

pub fn read(id: int): int {
    if const m = ^markers[id] {
        return m.at.x
    }
    return -1
}
"#;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id product Marker 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Marker.at 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root markers 2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d\n\
     id key markers.id 2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e\n\
     high-water 0\n\
     end\n";

/// A project at `dir` holding `source` under the fixed ledger.
fn project(dir: &Path, source: &str) -> PathBuf {
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    write(&dir.join("src/main.mw"), source);
    write(&dir.join(".marrow/ids"), IDS);
    dir.to_path_buf()
}

fn ok(outcome: CliOutcome, what: &str) -> CliOutcome {
    assert!(
        outcome.success(),
        "{what}: {}{}",
        outcome.stdout_text(),
        outcome.stderr_text()
    );
    outcome
}

/// The deployment image of `project`, accepting the ceiling `marrow image` proposes.
fn image(toolchain: &Path, project: &Path) -> PathBuf {
    let preview = staged_marrow_in(toolchain, project, &["image", "--out", "img"]);
    let ceiling = unaccepted_ceiling_id(&preview.stderr_text());
    ok(
        staged_marrow_in(
            toolchain,
            project,
            &["image", "--out", "img", "--accept-ceiling", &ceiling],
        ),
        "image",
    );
    project.join("img/program.image")
}

/// Every file in the store directory with its bytes, sorted by name.
fn snapshot(store: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut files: Vec<_> = fs::read_dir(store)
        .expect("list store")
        .map(|entry| {
            let entry = entry.expect("store entry");
            (
                entry.file_name(),
                fs::read(entry.path()).expect("store file"),
            )
        })
        .collect();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[test]
fn a_positional_leaf_swap_is_refused_and_the_old_binding_reads_back() {
    let toolchain = stage_toolchain();
    let toolchain = toolchain.path();
    let temp = Scratch::new("apply-refusal");
    let old = project(&temp.path().join("old"), SOURCE);
    let new = project(
        &temp.path().join("new"),
        &SOURCE.replace("    x: int\n    y: int\n", "    y: int\n    x: int\n"),
    );
    let store = temp.path().join("store");
    let store_arg = store.to_str().expect("store path");

    let old_image = image(toolchain, &old);
    let new_image = image(toolchain, &new);
    let provision = std::process::Command::new(toolchain.join("marrow-runner"))
        .args(["provision", "--image"])
        .arg(&old_image)
        .arg("--store")
        .arg(&store)
        .arg("--yes")
        .output()
        .expect("provision");
    assert!(
        provision.status.success(),
        "{}",
        String::from_utf8_lossy(&provision.stderr)
    );
    ok(
        staged_marrow_in(
            toolchain,
            &old,
            &["run", "main.put", "--store", store_arg, "--", "1", "7"],
        ),
        "put",
    );
    let before = snapshot(&store);

    let swapped = staged_marrow_in(
        toolchain,
        &new,
        &["run", "main.read", "--store", store_arg, "--", "1"],
    );
    let said = format!("{}{}", swapped.stdout_text(), swapped.stderr_text());
    assert_eq!(swapped.code(), Some(1), "{said}");
    assert!(said.contains("store.contract_changed"), "{said}");
    assert_eq!(
        snapshot(&store),
        before,
        "a refused attach changes no store file"
    );

    let applied = staged_marrow_in(
        toolchain,
        temp.path(),
        &[
            "apply",
            "--store",
            store_arg,
            "--old-image",
            old_image.to_str().expect("old image"),
            "--new-image",
            new_image.to_str().expect("new image"),
            "--format",
            "jsonl",
        ],
    );
    assert_eq!(applied.code(), Some(1), "{}", applied.stderr_text());
    let receipt: serde_json::Value =
        serde_json::from_str(&applied.stdout_text()).expect("apply receipt");
    assert_eq!(receipt["kind"], "apply");
    assert_eq!(receipt["outcome"], "refused");
    assert_eq!(receipt["code"], "store.apply_unsupported");
    assert_eq!(receipt["reason"], "stored_value");
    assert_eq!(
        snapshot(&store),
        before,
        "a refused apply changes no store file"
    );

    let read = ok(
        staged_marrow_in(
            toolchain,
            &old,
            &["run", "main.read", "--store", store_arg, "--", "1"],
        ),
        "old read",
    );
    assert_eq!(read.stdout_text(), "7\n");
}
