//! Recurrence gate over the physical adapter's package metadata, dependency DAG,
//! and lockfile.
//!
//! This pins, from full locked `cargo metadata` plus the manifest and lockfile
//! sources (no metadata parser dependency): the new package's inherited fields
//! and workspace lints; the exact one-member / three-present-internal-edge set;
//! the absence of the approved-future `marrow-lsp -> marrow-project-fs` edge and
//! the permanently forbidden direct `marrow-lsp -> marrow-project` edge; the exact
//! internal lockfile edges; and the sorted external inventory frozen from the
//! clean tree. Every external package's source is asserted to be the one registry,
//! and the frozen `(name, version, checksum, license)` rows are compared exactly.
//! A missing license, an added external node, a source or checksum change, or a
//! forbidden edge fails the ordinary gate.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The one registry every external dependency resolves from.
const REGISTRY_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// The frozen external dependency inventory: `(name, version, checksum, license)`,
/// sorted, captured from the clean stage-one tree. Commit S adds no external
/// dependency, so this is byte-identical before and after adding the member.
const EXTERNAL_INVENTORY: &[(&str, &str, &str, &str)] = &[
    (
        "block-buffer",
        "0.10.4",
        "3078c7629b62d3f0439517fa394996acacc5cbc91c5a20d8c658e77abd503a71",
        "MIT OR Apache-2.0",
    ),
    (
        "cfg-if",
        "1.0.4",
        "9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801",
        "MIT OR Apache-2.0",
    ),
    (
        "cpufeatures",
        "0.2.17",
        "59ed5838eebb26a2bb2e58f6d5b5316989ae9d08bab10e0e6d103e656d1b0280",
        "MIT OR Apache-2.0",
    ),
    (
        "crypto-common",
        "0.1.7",
        "78c8292055d1c1df0cce5d180393dc8cce0abec0a7102adb6c7b1eef6016d60a",
        "MIT OR Apache-2.0",
    ),
    (
        "digest",
        "0.10.7",
        "9ed9a281f7bc9b7576e61468ba615a66a5c8cfdff42420a70aa82701a3b1e292",
        "MIT OR Apache-2.0",
    ),
    (
        "generic-array",
        "0.14.7",
        "85649ca51fd72272d7821adaf274ad91c288277713d9c18820d8499a7ff69e9a",
        "MIT",
    ),
    (
        "libc",
        "0.2.186",
        "68ab91017fe16c622486840e4c83c9a37afeff978bd239b5293d61ece587de66",
        "MIT OR Apache-2.0",
    ),
    (
        "proc-macro2",
        "1.0.106",
        "8fd00f0bb2e90d81d1044c2b32617f68fcb9fa3bb7640c23e9c748e53fb30934",
        "MIT OR Apache-2.0",
    ),
    (
        "quote",
        "1.0.46",
        "dfbc457d0c7a0759a614551b11a6409e5951f6c7537be1f1b7682b9ae9230368",
        "MIT OR Apache-2.0",
    ),
    (
        "redb",
        "4.1.0",
        "8e925444704b5f17d32bf42f5b6e2df050bceebc3dcd6e71cc73dafe8092e839",
        "MIT OR Apache-2.0",
    ),
    (
        "serde_core",
        "1.0.228",
        "41d385c7d4ca58e59fc732af25c3983b67ac852c1a25000afe1175de458b67ad",
        "MIT OR Apache-2.0",
    ),
    (
        "serde_derive",
        "1.0.228",
        "d540f220d3187173da220f885ab66608367b6574e925011a9353e4badda91d79",
        "MIT OR Apache-2.0",
    ),
    (
        "serde_spanned",
        "1.1.1",
        "6662b5879511e06e8999a8a235d848113e942c9124f211511b16466ee2995f26",
        "MIT OR Apache-2.0",
    ),
    (
        "sha2",
        "0.10.9",
        "a7507d819769d01a365ab707794a4084392c824f54a7a6a7862f8c3d0892b283",
        "MIT OR Apache-2.0",
    ),
    (
        "syn",
        "2.0.118",
        "1b9ae57f904213ebb649ce6895b8a66c66f0203b9319718f69a5612a065b1422",
        "MIT OR Apache-2.0",
    ),
    (
        "toml",
        "1.1.3+spec-1.1.0",
        "53c96ecdfa941c8fc4fcaed14f99ada8ebed502eef533015095a07e3301d4c3c",
        "MIT OR Apache-2.0",
    ),
    (
        "toml_datetime",
        "1.1.1+spec-1.1.0",
        "3165f65f62e28e0115a00b2ebdd37eb6f3b641855f9d636d3cd4103767159ad7",
        "MIT OR Apache-2.0",
    ),
    (
        "toml_parser",
        "1.1.2+spec-1.1.0",
        "a2abe9b86193656635d2411dc43050282ca48aa31c2451210f4202550afb7526",
        "MIT OR Apache-2.0",
    ),
    (
        "typenum",
        "1.20.1",
        "b6f5e870be6c3b371b77fe0ee0bafb859fa4964b4404c27de1d380043c4dda20",
        "MIT OR Apache-2.0",
    ),
    (
        "unicode-ident",
        "1.0.24",
        "e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75",
        "(MIT OR Apache-2.0) AND Unicode-3.0",
    ),
    (
        "version_check",
        "0.9.5",
        "0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a",
        "MIT/Apache-2.0",
    ),
    (
        "winnow",
        "1.0.4",
        "23b97319f7b8343df12cc98938e5c3eb436064524c8d2b4e30a1d3a36eecdf81",
        "MIT",
    ),
];

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow-project-fs`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .to_path_buf()
}

fn read(root: &Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn full_metadata(root: &Path) -> String {
    let output = Command::new(env!("CARGO"))
        .arg("metadata")
        .args(["--format-version", "1", "--locked"])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .output()
        .expect("run cargo metadata");
    assert!(output.status.success(), "cargo metadata failed");
    String::from_utf8(output.stdout).expect("metadata is utf-8")
}

/// The `"key":"value"` string field nearest the start of `chunk`, if present.
fn json_string_field<'a>(chunk: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":\"");
    let start = chunk.find(&needle)? + needle.len();
    let rest = &chunk[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Every external package's `(name, version, source, license)`, parsed from the
/// `packages` array of full metadata. A package object begins with `{"name":"`
/// and carries its `version`, `source`, and `license` before its `dependencies`
/// array, so each object chunk holds exactly those fields; a workspace member has
/// a null `source` and a dependency entry has neither `version` nor `license`, so
/// both are excluded.
fn metadata_externals(metadata: &str) -> Vec<(String, String, String, String)> {
    let packages_region = metadata
        .split_once("],\"workspace_members\"")
        .map(|(head, _)| head)
        .expect("metadata has a workspace_members section after packages");

    let mut externals = Vec::new();
    for chunk in packages_region.split("{\"name\":\"").skip(1) {
        let name = chunk.split('"').next().expect("object name terminates");
        let (Some(version), Some(source), Some(license)) = (
            json_string_field(chunk, "version"),
            json_string_field(chunk, "source"),
            json_string_field(chunk, "license"),
        ) else {
            continue;
        };
        externals.push((
            name.to_string(),
            version.to_string(),
            source.to_string(),
            license.to_string(),
        ));
    }
    externals
}

/// One resolved package object's `version` and `license`, or `None` when the
/// package is absent. Both fields precede the object's `dependencies`, so the
/// split chunk carries them.
fn metadata_package_version_license(metadata: &str, name: &str) -> Option<(String, String)> {
    let packages_region = metadata
        .split_once("],\"workspace_members\"")
        .map(|(head, _)| head)?;
    let head = format!("{name}\",\"version\":\"");
    for chunk in packages_region.split("{\"name\":\"").skip(1) {
        if let Some(rest) = chunk.strip_prefix(&head) {
            let version = rest.split('"').next()?.to_string();
            let license = json_string_field(chunk, "license")?.to_string();
            return Some((version, license));
        }
    }
    None
}

/// Every `[[package]]` in the lockfile that has a `source` (an external crate),
/// as `(name, version, source, checksum)`.
fn lock_externals(lock: &str) -> Vec<(String, String, String, String)> {
    let mut externals = Vec::new();
    for block in lock.split("[[package]]").skip(1) {
        let name = toml_line(block, "name");
        let version = toml_line(block, "version");
        let source = toml_line(block, "source");
        let checksum = toml_line(block, "checksum");
        if let (Some(name), Some(version), Some(source), Some(checksum)) =
            (name, version, source, checksum)
        {
            externals.push((name, version, source, checksum));
        }
    }
    externals
}

/// The value of a `key = "value"` line inside one lockfile block.
fn toml_line(block: &str, key: &str) -> Option<String> {
    let needle = format!("\n{key} = \"");
    let start = block.find(&needle)? + needle.len();
    let rest = &block[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// One `[[package]]` block's `dependencies = [ ... ]` names, if present.
fn lock_dependencies(lock: &str, package: &str) -> Vec<String> {
    for block in lock.split("[[package]]").skip(1) {
        if toml_line(block, "name").as_deref() != Some(package) {
            continue;
        }
        let Some((_, after)) = block.split_once("dependencies = [") else {
            return Vec::new();
        };
        let body = after.split_once(']').map(|(body, _)| body).unwrap_or("");
        // Each dependency is a quoted string; the quoted pieces are the odd
        // fragments of a split on the quote character.
        return body
            .split('"')
            .enumerate()
            .filter(|(index, _)| index % 2 == 1)
            .map(|(_, name)| name.to_string())
            .collect();
    }
    panic!("lockfile has no package named {package}");
}

#[test]
fn the_new_member_inherits_the_workspace_package_fields_and_lints() {
    let root = workspace_root();
    let manifest = read(&root, "crates/marrow-project-fs/Cargo.toml");
    for declaration in [
        "version.workspace = true",
        "edition.workspace = true",
        "rust-version.workspace = true",
        "license.workspace = true",
        "repository.workspace = true",
        "authors.workspace = true",
    ] {
        assert!(
            manifest.contains(declaration),
            "marrow-project-fs must inherit `{declaration}`"
        );
    }
    assert!(
        manifest.contains("[lints]\nworkspace = true"),
        "marrow-project-fs must declare workspace lints"
    );

    // The workspace concretely declares the inherited values, pinned to Rust 1.89.
    let root_manifest = read(&root, "Cargo.toml");
    for declaration in [
        "version = \"0.1.0\"",
        "edition = \"2024\"",
        "rust-version = \"1.89\"",
        "license = \"Apache-2.0\"",
        "repository = \"https://github.com/scottswilliams/marrow\"",
        "authors = [\"Marrow contributors\"]",
    ] {
        assert!(
            root_manifest.contains(declaration),
            "workspace package must declare `{declaration}`"
        );
    }
    assert!(
        root_manifest.contains("\"crates/marrow-project-fs\","),
        "root manifest must add exactly the one new member"
    );

    // Full metadata resolves the inherited values.
    let metadata = full_metadata(&root);
    let (version, license) = metadata_package_version_license(&metadata, "marrow-project-fs")
        .expect("marrow-project-fs is present in metadata");
    assert_eq!(version, "0.1.0", "resolved version");
    assert_eq!(license, "Apache-2.0", "resolved license");
}

#[test]
fn the_internal_edges_are_exactly_the_three_present_edges() {
    let root = workspace_root();
    let lock = read(&root, "Cargo.lock");

    // marrow-project-fs depends on exactly the pure owner and the code registry.
    let mut pfs_deps = lock_dependencies(&lock, "marrow-project-fs");
    pfs_deps.sort();
    assert_eq!(
        pfs_deps,
        ["marrow-codes", "marrow-project"],
        "marrow-project-fs lock edges must be exactly marrow-codes and marrow-project"
    );

    // The CLI consumes the adapter.
    assert!(
        lock_dependencies(&lock, "marrow")
            .iter()
            .any(|d| d == "marrow-project-fs"),
        "marrow must consume marrow-project-fs"
    );

    // The future `marrow-lsp -> marrow-project-fs` edge and the forbidden direct
    // `marrow-lsp -> marrow-project` edge are both absent: no marrow-lsp package.
    assert!(
        !lock.contains("name = \"marrow-lsp\""),
        "marrow-lsp must not appear in the workspace lockfile in this lane"
    );
    let metadata = full_metadata(&root);
    assert!(
        !metadata.contains("\"name\":\"marrow-lsp\""),
        "marrow-lsp must not appear in workspace metadata in this lane"
    );
}

#[test]
fn the_external_inventory_is_unchanged_from_the_clean_tree() {
    let root = workspace_root();
    let lock = read(&root, "Cargo.lock");
    let metadata = full_metadata(&root);

    let lock_externals = lock_externals(&lock);
    for (_, _, source, _) in &lock_externals {
        assert_eq!(
            source, REGISTRY_SOURCE,
            "external source must be the registry"
        );
    }

    // Join lock (name, version, checksum) with metadata (name, version, license).
    let metadata_externals = metadata_externals(&metadata);
    for (_, _, source, _) in &metadata_externals {
        assert_eq!(
            source, REGISTRY_SOURCE,
            "external metadata source must be the registry"
        );
    }
    let license_of = |name: &str, version: &str| -> String {
        metadata_externals
            .iter()
            .find(|(n, v, _, _)| n == name && v == version)
            .map(|(_, _, _, license)| license.clone())
            .unwrap_or_else(|| panic!("metadata has no license for {name} {version}"))
    };

    let mut inventory: Vec<(String, String, String, String)> = lock_externals
        .iter()
        .map(|(name, version, _source, checksum)| {
            (
                name.clone(),
                version.clone(),
                checksum.clone(),
                license_of(name, version),
            )
        })
        .collect();
    inventory.sort();

    let expected: Vec<(String, String, String, String)> = EXTERNAL_INVENTORY
        .iter()
        .map(|(n, v, c, l)| (n.to_string(), v.to_string(), c.to_string(), l.to_string()))
        .collect();

    assert_eq!(
        inventory, expected,
        "the external (name, version, source, checksum, license) inventory drifted from the clean tree"
    );
}
