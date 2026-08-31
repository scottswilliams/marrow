//! Cargo-graph boundary for the compiler CLI and the language server.
//!
//! The real stdio protocol probes live with the `marrow-lsp` binary. This test remains
//! in the `marrow` package so `cargo test -p marrow` itself proves that neither the
//! server nor its protocol dependency is in the CLI's declared or resolved graph.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root is two levels above the marrow manifest")
        .to_path_buf()
}

fn cargo_metadata() -> Value {
    let root = workspace_root();
    let mut command = Command::new(env!("CARGO"));
    command
        .arg("metadata")
        .args(["--format-version", "1", "--locked", "--all-features"])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"));
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        command.env("CARGO_TARGET_DIR", target_dir);
    }
    let output = command.output().expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON")
}

fn package<'a>(metadata: &'a Value, name: &str) -> &'a Value {
    metadata["packages"]
        .as_array()
        .expect("metadata packages are an array")
        .iter()
        .find(|package| package["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("workspace metadata contains {name}"))
}

#[test]
fn marrow_cannot_reach_the_language_server_or_lsp_types() {
    const FORBIDDEN: &[&str] = &["marrow-lsp", "lsp-types"];

    let metadata = cargo_metadata();
    let marrow = package(&metadata, "marrow");

    let declared: Vec<_> = marrow["dependencies"]
        .as_array()
        .expect("package dependencies are an array")
        .iter()
        .filter_map(|dependency| dependency["name"].as_str())
        .collect();
    assert!(
        FORBIDDEN.iter().all(|name| !declared.contains(name)),
        "marrow declares a forbidden dependency: {declared:?}"
    );

    let resolve = metadata["resolve"]
        .as_object()
        .expect("cargo metadata includes a resolve graph");
    let nodes = resolve["nodes"]
        .as_array()
        .expect("resolve nodes are an array");
    let by_id: HashMap<_, _> = nodes
        .iter()
        .map(|node| (node["id"].as_str().expect("resolve node has an id"), node))
        .collect();

    let forbidden_ids: HashSet<_> = metadata["packages"]
        .as_array()
        .expect("metadata packages are an array")
        .iter()
        .filter(|package| {
            package["name"]
                .as_str()
                .is_some_and(|name| FORBIDDEN.contains(&name))
        })
        .map(|package| package["id"].as_str().expect("package has an id"))
        .collect();

    let marrow_id = marrow["id"].as_str().expect("marrow package has an id");
    let mut pending = VecDeque::from([marrow_id]);
    let mut reachable = HashSet::new();
    while let Some(id) = pending.pop_front() {
        if !reachable.insert(id) {
            continue;
        }
        let node = by_id.get(id).expect("reachable package has a resolve node");
        for dependency in node["deps"]
            .as_array()
            .expect("resolve dependencies are an array")
        {
            pending.push_back(
                dependency["pkg"]
                    .as_str()
                    .expect("resolved dependency has a package id"),
            );
        }
    }

    let reached_forbidden: Vec<_> = reachable.intersection(&forbidden_ids).copied().collect();
    assert!(
        reached_forbidden.is_empty(),
        "marrow reaches the language-server graph: {reached_forbidden:?}"
    );

    let lsp = package(&metadata, "marrow-lsp");
    let targets = lsp["targets"]
        .as_array()
        .expect("marrow-lsp targets are an array");
    assert_eq!(
        targets
            .iter()
            .filter(|target| {
                target["name"].as_str() == Some("marrow_lsp")
                    && target["kind"]
                        .as_array()
                        .expect("target kind is an array")
                        .iter()
                        .any(|kind| kind.as_str() == Some("lib"))
            })
            .count(),
        1,
        "marrow-lsp must retain exactly one library target"
    );
    assert_eq!(
        targets
            .iter()
            .filter(|target| {
                target["name"].as_str() == Some("marrow-lsp")
                    && target["kind"]
                        .as_array()
                        .expect("target kind is an array")
                        .iter()
                        .any(|kind| kind.as_str() == Some("bin"))
            })
            .count(),
        1,
        "marrow-lsp must expose exactly one named server binary"
    );
}

#[test]
fn the_language_server_is_not_a_marrow_subcommand() {
    let old_spelling = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(["lsp", "--help"])
        .output()
        .expect("run the marrow CLI with the removed spelling");
    assert_eq!(
        old_spelling.status.code(),
        Some(2),
        "the removed command must be an unknown-command usage failure"
    );

    let help = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("--help")
        .output()
        .expect("run marrow --help");
    assert!(help.status.success());
    let stdout = String::from_utf8(help.stdout).expect("help is UTF-8");
    assert!(
        !stdout.lines().any(|line| line.trim() == "marrow lsp"),
        "marrow --help must not advertise the removed subcommand"
    );
}
