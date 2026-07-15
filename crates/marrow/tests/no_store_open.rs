//! Absence gate: the CLI opens no store in process.
//!
//! T01's in-process `--store` open died at D00, where the durable-run trough
//! begins. The CLI still compiles, verifies, and completes the identity of a
//! durable image, and it still renders durable values (its outcome owner uses the
//! path kernel's logical key codec, `marrow_kernel::codec::key`), so the Cargo DAG
//! keeps a `marrow-kernel` edge that no Cargo-graph gate can forbid. This finer
//! source-level gate is what the DAG boundary test cannot express: no file under
//! `crates/marrow/src` names the store-open surface — the kernel's `durable`
//! session module, a `DurableStore`, a native engine, or a session open — so the
//! CLI cannot reacquire an in-process store without tripping this test. Durable
//! execution returns as the ephemeral-memory preview (E01) and the persistent
//! companion path (F02b), neither of which reopens the store from the CLI process.

use std::path::{Path, PathBuf};

/// Store-open surface the CLI must not name. Each is a concrete Rust token that
/// would appear in CLI source only if an in-process store open leaked back in:
/// the kernel's durable session module path, the durable store owner, a native
/// engine, and the two session-opening methods.
const FORBIDDEN_STORE_OPEN: &[&str] = &[
    "marrow_kernel::durable",
    "DurableStore",
    "NativeEngine",
    ".txn_session",
    ".read_session",
];

fn cli_src_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow`.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn collect_rs(dir: &Path, out: &mut Vec<(String, String)>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    entries.sort();
    for entry in entries {
        if entry.is_dir() {
            collect_rs(&entry, out);
        } else if entry.extension().is_some_and(|ext| ext == "rs") {
            let name = entry.to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&entry).expect("read CLI source file");
            out.push((name, text));
        }
    }
}

#[test]
fn the_cli_names_no_store_open_surface() {
    let mut files = Vec::new();
    collect_rs(&cli_src_dir(), &mut files);
    assert!(!files.is_empty(), "found no CLI source files to scan");

    let mut violations: Vec<String> = Vec::new();
    for (path, text) in &files {
        for token in FORBIDDEN_STORE_OPEN {
            if text.contains(token) {
                violations.push(format!("{path}: {token}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "the CLI must open no store in process (T01's in-process open died at D00); \
         these files name the store-open surface:\n{}",
        violations.join("\n")
    );
}
