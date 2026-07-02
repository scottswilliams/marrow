//! Enforcement artifact for the store-admission owner.
//!
//! `TreeStore`'s on-disk constructors are crate-private, so `SealedStore::open` is the only
//! source of a durable handle (compile-enforced). This scan adds the second half: `SealedStore`
//! itself may be consumed only inside the `marrow-run` admission module. Any other production
//! module reaching for `SealedStore` would be minting a handle around the admission stage, so
//! the scan fails closed on it.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_crates_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/marrow-run; its parent is the workspace crates/ dir.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn sealed_store_is_consumed_only_by_the_admission_module() {
    let crates_dir = workspace_crates_dir();
    // The store crate defines `SealedStore`; the admission module is its one production consumer.
    let allowed = [
        crates_dir.join("marrow-store/src/sealed.rs"),
        crates_dir.join("marrow-store/src/lib.rs"),
        crates_dir.join("marrow-run/src/admission.rs"),
    ];

    let mut offenders = Vec::new();
    let mut crate_dirs: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&crates_dir)
        .expect("read crates dir")
        .flatten()
    {
        let src = entry.path().join("src");
        if src.is_dir() {
            crate_dirs.push(src);
        }
    }
    let mut sources = Vec::new();
    for src in &crate_dirs {
        rust_sources(src, &mut sources);
    }

    for source in sources {
        if allowed.contains(&source) {
            continue;
        }
        let text = fs::read_to_string(&source).expect("read source");
        for (line_index, line) in text.lines().enumerate() {
            if line.contains("SealedStore") {
                offenders.push(format!(
                    "{}:{}",
                    source
                        .strip_prefix(&crates_dir)
                        .unwrap_or(&source)
                        .display(),
                    line_index + 1
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "SealedStore may be used only inside the marrow-run admission module; found:\n{}",
        offenders.join("\n")
    );
}
