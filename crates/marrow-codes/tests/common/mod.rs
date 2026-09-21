//! The tracked-file corpus the repository gates read.
//!
//! `git ls-files` decides membership so a gate never reports on build output, a
//! stray editor file, or an untracked scratch copy. The listing is taken once
//! per test binary.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The workspace root, two levels above this crate's manifest.
pub fn workspace_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root two levels above the crate manifest")
            .to_path_buf()
    })
}

/// Every tracked path, repository-relative and slash-separated.
pub fn tracked_paths() -> &'static BTreeSet<String> {
    static PATHS: OnceLock<BTreeSet<String>> = OnceLock::new();
    PATHS.get_or_init(|| {
        let output = Command::new("git")
            .arg("-C")
            .arg(workspace_root())
            .arg("ls-files")
            .output()
            .expect("run git ls-files");
        assert!(output.status.success(), "git ls-files failed");
        let paths: BTreeSet<String> = String::from_utf8(output.stdout)
            .expect("git output is utf-8")
            .lines()
            .map(str::to_owned)
            .collect();
        assert!(!paths.is_empty(), "the tracked-file listing is empty");
        paths
    })
}
