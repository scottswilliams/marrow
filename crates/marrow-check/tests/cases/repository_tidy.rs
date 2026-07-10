use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Recursively collect the repo-relative paths of files under `dir`, skipping build
/// output, version-control state, and any hidden entry, so the scan sees the tracked
/// source tree and never a stray artifact dir.
fn tracked_files(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("read repo dir");
    for entry in entries {
        let path = entry.expect("repo dir entry").path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            tracked_files(&path, root, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .expect("path under root")
                    .to_path_buf(),
            );
        }
    }
}

/// Whole-repo absence gate: the removed `marrow.catalog.json` artifact name must not survive
/// anywhere in the source tree except the three places that legitimately spell it — this gate,
/// the forbidden-vocabulary token in `language_reference_docs.rs`, and the run-path negative
/// assertion that proves the run projects a lock and never the removed artifact
/// (`run_cli_fence.rs`). The store is the saved-data identity authority and `marrow.lock` its
/// committed projection; reintroducing the file name fails this gate.
#[test]
fn marrow_catalog_json_appears_only_in_allowed_places() {
    const ARTIFACT: &str = "marrow.catalog.json";
    let allowed: [&Path; 3] = [
        Path::new("crates/marrow-check/tests/cases/language_reference_docs.rs"),
        Path::new("crates/marrow-check/tests/cases/repository_tidy.rs"),
        Path::new("crates/marrow-run/tests/cases/run_cli_fence.rs"),
    ];

    let root = repo_root();
    let mut files = Vec::new();
    tracked_files(&root, &root, &mut files);
    files.sort();

    let mut violations: Vec<String> = Vec::new();
    for relative in &files {
        if allowed.contains(&relative.as_path()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(relative)) else {
            continue;
        };
        for (line_index, line) in text.lines().enumerate() {
            if line.contains(ARTIFACT) {
                violations.push(format!("{}:{}", relative.display(), line_index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "the removed `{ARTIFACT}` artifact name reappeared outside its allowlist: {violations:#?}"
    );
}

/// Absence gate: the throwaway root fixtures a checker session once left behind must not reappear
/// anywhere in the tracked tree. They are scratch parser inputs, not source, examples, or part of
/// the conformance corpus.
#[test]
fn stray_root_scratch_fixtures_are_absent() {
    const STRAY: [&str; 3] = ["bare_param.mw", "colon_no_ret.mw", "no_annot.mw"];

    let root = repo_root();
    let mut files = Vec::new();
    tracked_files(&root, &root, &mut files);

    let found: Vec<String> = files
        .iter()
        .filter(|relative| {
            relative
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| STRAY.contains(&name))
        })
        .map(|relative| relative.display().to_string())
        .collect();

    assert!(
        found.is_empty(),
        "stray scratch fixtures must stay deleted, found: {found:#?}"
    );
}
