//! DEP-RUSTIX-FSJOURNAL condition gates, enforced from the manifests, the
//! lockfile, the resolved dependency graph, and the crate source rather than
//! prose:
//!
//! 1. `rustix` appears in exactly one workspace manifest (this crate's), with
//!    the exact `=1.1.4` pin and default features off.
//! 2. The resolved feature set of `rustix` is exactly `{alloc, fs, std}`
//!    (`alloc` implied by `std`) — any new feature word is a new maintainer
//!    decision.
//! 3. No `rustix` type escapes this crate's public API: the token appears in
//!    exactly one private module, and that module exports nothing `pub`.
//! 4. Darwin sync discipline: every sync is a plain `fsync`; the
//!    full-flush and `sync_all`/`sync_data` spellings are absent because no
//!    current envelope claims power-loss durability, and the crate docs keep
//!    the macOS disclaimer.
//! 5. The Linux qualification leg pins the `linux_raw` backend.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow-fs-journal`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .to_path_buf()
}

fn crate_sources() -> Vec<(PathBuf, String)> {
    let src = workspace_root().join("crates/marrow-fs-journal/src");
    let mut files: Vec<(PathBuf, String)> = std::fs::read_dir(&src)
        .expect("read crate src")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| {
            let contents = std::fs::read_to_string(&path).expect("read crate source");
            (path, contents)
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!files.is_empty(), "the crate source scan found no files");
    files
}

#[test]
fn rustix_is_pinned_in_exactly_one_workspace_manifest() {
    let root = workspace_root();
    let mut naming: Vec<PathBuf> = Vec::new();

    let mut manifests = vec![root.join("Cargo.toml")];
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("read crates dir")
        .flatten()
    {
        let manifest = entry.path().join("Cargo.toml");
        if manifest.is_file() {
            manifests.push(manifest);
        }
    }
    for manifest in manifests {
        let contents = std::fs::read_to_string(&manifest).expect("read manifest");
        if contents.contains("rustix") {
            naming.push(manifest);
        }
    }

    assert_eq!(
        naming,
        [root.join("crates/marrow-fs-journal/Cargo.toml")],
        "rustix must appear in exactly this crate's manifest"
    );

    let own = std::fs::read_to_string(root.join("crates/marrow-fs-journal/Cargo.toml"))
        .expect("read this crate's manifest");
    assert!(
        own.contains(r#"version = "=1.1.4""#),
        "the rustix edge must carry the exact =1.1.4 pin"
    );
    assert!(
        own.contains("default-features = false"),
        "the rustix edge must disable default features"
    );

    // The lockfile carries exactly one rustix package at the pinned version.
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).expect("read lockfile");
    assert_eq!(
        lock.matches("name = \"rustix\"").count(),
        1,
        "exactly one rustix major/minor line may exist in the lock"
    );
    assert!(
        lock.contains("name = \"rustix\"\nversion = \"1.1.4\""),
        "the locked rustix version must be 1.1.4"
    );
}

#[test]
fn the_resolved_rustix_feature_set_is_exactly_std_fs() {
    let root = workspace_root();
    let output = Command::new(env!("CARGO"))
        .arg("metadata")
        .args(["--format-version", "1"])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .output()
        .expect("run cargo metadata");
    assert!(output.status.success(), "cargo metadata failed");
    let text = String::from_utf8(output.stdout).expect("metadata is utf-8");

    // Minimal dependency-free extraction over the resolve graph only (package
    // objects also carry `"features"` arrays inside their dependency lists,
    // so the scan starts at the resolve section).
    let resolve_start = text
        .find("\"resolve\":")
        .expect("metadata has a resolve graph");
    let resolve = &text[resolve_start..];
    let mut feature_lists: Vec<Vec<String>> = Vec::new();
    for chunk in resolve.split("\"id\":\"").skip(1) {
        let id = chunk.split('"').next().expect("id terminates");
        if !id.contains("#rustix@1.1.4") {
            continue;
        }
        let scope = chunk.split("\"id\":\"").next().expect("chunk head");
        let Some((_, rest)) = scope.split_once("\"features\":[") else {
            continue;
        };
        let body = rest.split(']').next().expect("feature array terminates");
        let mut features: Vec<String> = body
            .split(',')
            .map(|item| item.trim().trim_matches('"').to_string())
            .filter(|item| !item.is_empty())
            .collect();
        features.sort();
        feature_lists.push(features);
    }

    assert_eq!(
        feature_lists,
        [["alloc", "fs", "std"]],
        "the resolved rustix feature set must be exactly {{alloc, fs, std}} \
         with default features off; a new feature word is a new maintainer decision"
    );
}

#[test]
fn no_rustix_type_escapes_the_public_api() {
    let sources = crate_sources();

    // The token is confined to the one private adapter module.
    let naming: Vec<&Path> = sources
        .iter()
        .filter(|(_, contents)| contents.contains("rustix"))
        .map(|(path, _)| path.as_path())
        .collect();
    assert_eq!(
        naming.len(),
        1,
        "rustix must be named by exactly one source file: {naming:?}"
    );
    assert!(
        naming[0].ends_with("sys.rs"),
        "the rustix consumer must be the private sys module: {naming:?}"
    );

    // The adapter module is private and exports nothing `pub`.
    let (_, lib) = sources
        .iter()
        .find(|(path, _)| path.ends_with("lib.rs"))
        .expect("lib.rs exists");
    assert!(
        lib.contains("mod sys;") && !lib.contains("pub mod sys"),
        "the sys module must be private"
    );
    assert!(
        !lib.contains("pub use sys"),
        "the sys module must not be re-exported"
    );
    let (_, sys) = sources
        .iter()
        .find(|(path, _)| path.ends_with("sys.rs"))
        .expect("sys.rs exists");
    for declaration in [
        "pub fn",
        "pub struct",
        "pub enum",
        "pub type",
        "pub use",
        "pub mod",
        "pub const",
        "pub trait",
        "pub(super)",
    ] {
        assert!(
            !sys.contains(declaration),
            "sys.rs must export nothing beyond pub(crate): found `{declaration}`"
        );
    }
}

#[test]
fn every_sync_is_a_plain_fsync_and_the_darwin_disclaimer_stands() {
    // No current envelope claims power-loss durability, so the full-flush
    // fcntl and the std sync wrappers (whose Darwin implementation issues that
    // fcntl) must be absent. The patterns are concatenated so this test file
    // does not match itself if the scan ever widens.
    let forbidden = [
        ["fcntl_", "fullfsync"].concat(),
        ["F_", "FULLFSYNC"].concat(),
        ["sync", "_all"].concat(),
        ["sync", "_data"].concat(),
        ["fdata", "sync"].concat(),
    ];
    let mut violations: Vec<String> = Vec::new();
    for (path, contents) in crate_sources() {
        for pattern in &forbidden {
            if contents.contains(pattern.as_str()) {
                violations.push(format!("{}: {pattern}", path.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "a sync stronger or weaker than plain fsync entered the crate:\n{}",
        violations.join("\n")
    );

    let (_, lib) = crate_sources()
        .into_iter()
        .find(|(path, _)| path.ends_with("lib.rs"))
        .expect("lib.rs exists");
    assert!(
        lib.contains("Sudden-power-loss") && lib.contains("on macOS is not established"),
        "the macOS sudden-power-loss disclaimer must remain in the crate docs"
    );
}

#[test]
fn the_crate_contains_no_unsafe_code() {
    // The workspace forbids `unsafe_code` by lint; this scan keeps the
    // absence conspicuous from the conformance suite as well.
    for (path, contents) in crate_sources() {
        assert!(
            !contents.contains("unsafe "),
            "{} contains unsafe code",
            path.display()
        );
    }
}

/// The Linux qualification leg: the `linux_raw` backend must actually be in
/// effect. rustix's build script selects `linux_raw` exactly when the target
/// is a supported Linux architecture and neither the `rustix_use_libc` nor the
/// `rustix_no_linux_raw` cfg is present; those cfgs arrive via `RUSTFLAGS`,
/// which cargo applies to every crate in the build — including this test — so
/// their absence here proves their absence for rustix in the same build.
#[cfg(target_os = "linux")]
#[test]
#[allow(unexpected_cfgs)]
fn the_linux_backend_is_linux_raw() {
    assert!(
        cfg!(any(target_arch = "x86_64", target_arch = "aarch64")),
        "Linux qualification covers x86_64 and aarch64 only; this architecture is unqualified"
    );
    assert!(
        !cfg!(rustix_use_libc),
        "RUSTFLAGS carries --cfg rustix_use_libc: the qualified linux_raw backend is not in effect"
    );
    assert!(
        !cfg!(rustix_no_linux_raw),
        "RUSTFLAGS carries --cfg rustix_no_linux_raw: the qualified linux_raw backend is not in effect"
    );
    // The `use-libc` feature is the remaining flip; the resolved-feature gate
    // above pins the feature set to exactly {alloc, fs, std} on every leg.
}
