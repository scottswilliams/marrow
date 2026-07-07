//! Keeps the agent-instruction layer from drifting: every crate carries both
//! stub files, the implementation map's crate table matches the crates on disk,
//! and the crates overview states no crate count that a new crate would falsify.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root sits two levels above crates/marrow")
        .to_path_buf()
}

fn crates_on_disk(root: &Path) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for entry in fs::read_dir(root.join("crates")).expect("read crates/") {
        let path = entry.expect("crates/ entry").path();
        if path.join("Cargo.toml").is_file() {
            set.insert(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    set
}

#[test]
fn every_crate_carries_both_stub_files() {
    let root = repo_root();
    for name in crates_on_disk(&root) {
        let dir = root.join("crates").join(&name);
        for stub in ["AGENTS.md", "CLAUDE.md"] {
            assert!(dir.join(stub).is_file(), "crates/{name} is missing {stub}");
        }
    }
}

#[test]
fn implementation_map_table_matches_crates_on_disk() {
    let root = repo_root();
    let readme = fs::read_to_string(root.join("docs/implementation/README.md"))
        .expect("read implementation README");
    let mut in_table = false;
    let mut listed = BTreeSet::new();
    for line in readme.lines().map(str::trim) {
        if line.starts_with("| Crate |") {
            in_table = true;
        } else if in_table && !line.starts_with('|') {
            break;
        } else if in_table {
            let cell = line
                .trim_start_matches('|')
                .split('|')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('`');
            if cell.starts_with("marrow") {
                listed.insert(cell.to_string());
            }
        }
    }
    assert_eq!(
        listed,
        crates_on_disk(&root),
        "docs/implementation/README.md crate table disagrees with crates/ on disk"
    );
}

#[test]
fn crates_overview_states_no_crate_count() {
    let root = repo_root();
    let overview =
        fs::read_to_string(root.join("crates/AGENTS.md")).expect("read crates/AGENTS.md");
    let counts: BTreeSet<&str> = [
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
        "twelve",
    ]
    .into_iter()
    .collect();
    let words: Vec<String> = overview
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    for pair in words.windows(2) {
        let is_count =
            counts.contains(pair[0].as_str()) || pair[0].chars().all(|c| c.is_ascii_digit());
        assert!(
            !(is_count && pair[1].starts_with("crate")),
            "crates/AGENTS.md states a crate count (\"{} {}\"); let the count live \
             only in the implementation map table",
            pair[0],
            pair[1]
        );
    }
}
