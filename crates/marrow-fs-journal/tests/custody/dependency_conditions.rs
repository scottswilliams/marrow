//! `rustix` dependency-policy gates, enforced from the manifests, the lockfile
//! and the build configuration:
//!
//! 1. `rustix` appears in exactly one workspace manifest (this crate's), with
//!    the exact `=1.1.4` pin and default features off.
//! 2. No second package depends on `rustix`, so nothing unions extra features
//!    into the resolved set — any new feature word is a new maintainer decision.
//! 3. The Linux qualification leg pins the `linux_raw` backend.

use std::path::{Path, PathBuf};

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
fn rustix_has_exactly_one_consumer_in_the_lockfile() {
    // With one rustix package, one workspace manifest naming it, and that
    // manifest's exact `default-features = false` plus `{std, fs}` feature list,
    // the only way the resolved feature set could widen is a second package
    // depending on rustix and unioning its features in. The lockfile records
    // every such edge, so this reads them rather than shelling out to cargo.
    let lock = std::fs::read_to_string(workspace_root().join("Cargo.lock")).expect("read lockfile");
    let consumers: Vec<&str> = lock
        .split("[[package]]")
        .skip(1)
        .filter(|block| {
            block
                .split_once("dependencies = [")
                .is_some_and(|(_, list)| {
                    list.split(']')
                        .next()
                        .expect("the dependency array terminates")
                        .contains("\"rustix\"")
                })
        })
        .map(|block| {
            block
                .split_once("name = \"")
                .expect("a package block names its package")
                .1
                .split('"')
                .next()
                .expect("the name terminates")
        })
        .collect();
    assert_eq!(
        consumers,
        ["marrow-fs-journal"],
        "a second rustix consumer would union its features into the resolved set; \
         a new feature word is a new maintainer decision"
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
