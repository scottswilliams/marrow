//! `rustix` dependency-policy gates, enforced from the manifests, the lockfile
//! and the build configuration:
//!
//! 1. `rustix` appears in exactly one workspace manifest (this crate's), with
//!    the exact `=1.1.4` pin and default features off.
//! 2. The resolved feature set of `rustix` is exactly `{alloc, fs, std}`
//!    (`alloc` implied by `std`) — any new feature word is a new maintainer
//!    decision.
//! 3. The Linux qualification leg pins the `linux_raw` backend.

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

/// The Linux qualification leg: the `linux_raw` backend must actually be in
/// effect. rustix's build script selects `linux_raw` exactly when the target
/// is a supported Linux architecture and neither the `rustix_use_libc` nor the
/// `rustix_no_linux_raw` cfg is present; those cfgs arrive via `RUSTFLAGS`,
/// which cargo applies to every crate in the build — including this test — so
/// their absence here proves their absence for rustix in the same build.
#[cfg(target_os = "linux")]
#[test]
// The assertions are constant on purpose: the test's subject is the build
// configuration itself, so each condition folds to a constant in that build.
#[allow(unexpected_cfgs, clippy::assertions_on_constants)]
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
