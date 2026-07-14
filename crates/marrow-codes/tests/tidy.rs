//! Standing anti-legacy tidy gate for the beta workspace.
//!
//! Two invariants, both enforced from source and the Cargo DAG rather than prose:
//!
//! 1. The workspace members are exactly the retained beta set.
//! 2. No tracked file under `crates/` names a forbidden legacy family — the
//!    deleted crates, the `surface` construct, `ProjectSession`, `Value::Absent`,
//!    or the tree-walking interpreter — as a Rust identifier or crate reference.
//!
//! The scan matches concrete Rust identifiers (crate paths and type/enum names),
//! not the ordinary English word "surface", so it stays precise as the retained
//! crates keep using words like "diagnostic surface" in prose. This test file is
//! the one place the forbidden strings are spelled, so it excludes itself.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The exact set of workspace packages the beta line retains after B00.
const RETAINED_MEMBERS: &[&str] = &["marrow", "marrow-codes", "marrow-store", "marrow-syntax"];

/// Forbidden legacy families, spelled as the concrete identifiers or crate
/// references that would appear in retained source if a deleted owner leaked
/// back in. Each is a real Rust token, never an English word, so the scan has no
/// false positives against ordinary prose.
const FORBIDDEN_FAMILIES: &[&str] = &[
    // Deleted crate references (any `use marrow_x` / `marrow_x::` edge).
    "marrow_check",
    "marrow_run",
    "marrow_schema",
    "marrow_catalog",
    "marrow_json",
    "marrow_project",
    // The surface construct: AST nodes, the keyword variant, the codes family,
    // and the wire ABI types all share the `Surface` identifier prefix.
    "Surface",
    // The composed prototype session owner.
    "ProjectSession",
    // The deleted structural-optional value variant.
    "Value::Absent",
    // The tree-walking interpreter's owning type.
    "Interpreter",
];

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow-codes`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .to_path_buf()
}

fn tracked_crate_files(root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "crates"])
        .output()
        .expect("run git ls-files");
    assert!(output.status.success(), "git ls-files failed");
    String::from_utf8(output.stdout)
        .expect("git output is utf-8")
        .lines()
        .map(|line| root.join(line))
        .collect()
}

#[test]
fn workspace_members_are_exactly_the_retained_set() {
    let root = workspace_root();
    let output = Command::new(env!("CARGO"))
        .arg("metadata")
        .args(["--format-version", "1", "--no-deps"])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .output()
        .expect("run cargo metadata");
    assert!(output.status.success(), "cargo metadata failed");
    let text = String::from_utf8(output.stdout).expect("metadata is utf-8");

    // Minimal, dependency-free extraction of package names from the metadata
    // JSON: the `--no-deps` package list carries only workspace members. Package
    // ids are the unambiguous carrier — `path+file://.../crates/<dir>#<version>`,
    // or `...#<name>@<version>` when the name differs from the directory. Bare
    // `"name"` fields also match lib-target names, which use underscores.
    let mut members: Vec<String> = text
        .split("\"id\":\"")
        .skip(1)
        .filter_map(|rest| {
            let id = rest.split('"').next()?;
            let (path, fragment) = id.split_once('#')?;
            let name = match fragment.split_once('@') {
                Some((name, _version)) => name,
                None => path.rsplit('/').next()?,
            };
            Some(name.to_string())
        })
        .filter(|name| name.starts_with("marrow"))
        .collect();
    members.sort();
    members.dedup();

    let mut expected: Vec<String> = RETAINED_MEMBERS.iter().map(|s| s.to_string()).collect();
    expected.sort();

    assert_eq!(
        members, expected,
        "workspace members must be exactly the retained beta set"
    );
}

#[test]
fn no_tracked_crate_file_names_a_forbidden_family() {
    let root = workspace_root();
    let this_file = Path::new(file!())
        .file_name()
        .expect("this test file has a name")
        .to_owned();

    let mut violations: Vec<String> = Vec::new();
    for path in tracked_crate_files(&root) {
        if path.file_name() == Some(this_file.as_os_str()) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue; // binary/non-utf8 tracked asset
        };
        for family in FORBIDDEN_FAMILIES {
            if contents.contains(family) {
                violations.push(format!("{}: {family}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "forbidden legacy families still present:\n{}",
        violations.join("\n")
    );
}
