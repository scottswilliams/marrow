use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Diagnostic prose is render-only: semantic code asserts codes, spans, typed
/// payloads, facts, store effects, or runtime values, and render contracts are
/// pinned by golden files. Matching on `message` text is therefore never the
/// right oracle for new work. The checked-in baseline inventories the
/// occurrences that predate this gate; it only shrinks, and it ends at zero.
const BASELINE: &str = include_str!("../prose_assertion_baseline.txt");

/// The scanned pattern, assembled at runtime so this file never matches itself.
fn needle() -> String {
    ["message", ".contains("].concat()
}

fn workspace_crates_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

fn rust_sources(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("read source dir") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_sources(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn observed_counts() -> BTreeMap<String, usize> {
    let root = workspace_crates_root();
    let needle = needle();
    let mut files = Vec::new();
    rust_sources(&root, &mut files);
    let mut counts = BTreeMap::new();
    for file in files {
        let text = fs::read_to_string(&file).expect("read rust source");
        let count = text.match_indices(&needle).count();
        if count > 0 {
            let relative = file
                .strip_prefix(&root)
                .expect("source under crates root")
                .to_string_lossy()
                .replace('\\', "/");
            counts.insert(format!("crates/{relative}"), count);
        }
    }
    counts
}

fn baseline_counts() -> BTreeMap<String, usize> {
    BASELINE
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (path, count) = line
                .rsplit_once(' ')
                .expect("baseline line: <path> <count>");
            (path.to_string(), count.parse().expect("baseline count"))
        })
        .collect()
}

#[test]
fn message_prose_assertions_only_shrink() {
    let observed = observed_counts();
    let baseline = baseline_counts();
    let mut drift = Vec::new();
    for (path, count) in &observed {
        match baseline.get(path) {
            None => drift.push(format!(
                "{path}: {count} new (assert the typed code, payload, span, or a golden instead)"
            )),
            Some(allowed) if count > allowed => drift.push(format!(
                "{path}: {count} exceeds the baseline {allowed} (assert the typed code, payload, \
                 span, or a golden instead)"
            )),
            Some(allowed) if count < allowed => drift.push(format!(
                "{path}: {count} is below the baseline {allowed} (ratchet the baseline down in \
                 this change)"
            )),
            Some(_) => {}
        }
    }
    for path in baseline.keys() {
        if !observed.contains_key(path) {
            drift.push(format!(
                "{path}: 0 but still listed (remove its baseline line in this change)"
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "message-prose assertion drift against crates/marrow/tests/prose_assertion_baseline.txt:\n{}",
        drift.join("\n")
    );
}
