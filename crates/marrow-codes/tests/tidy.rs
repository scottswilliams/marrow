//! Standing anti-legacy tidy gate for the beta workspace.
//!
//! Two invariants, both enforced from source and the Cargo DAG rather than prose:
//!
//! 1. The workspace members are exactly the retained beta set.
//! 2. No tracked file in the repository names a forbidden legacy family — the
//!    deleted crates (hyphen and underscore forms), the `surface` construct,
//!    `ProjectSession`, `Value::Absent`, or the tree-walking interpreter — as a
//!    Rust identifier, crate reference, or documented-current name.
//!
//! The scan matches concrete Rust identifiers (crate paths and type/enum names),
//! not the ordinary English word "surface", so it stays precise as the retained
//! crates keep using words like "diagnostic surface" in prose. This test file is
//! the one place the forbidden strings are spelled, so it excludes itself.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The exact set of workspace packages the beta line retains after B00.
const RETAINED_MEMBERS: &[&str] = &[
    "marrow",
    "marrow-codes",
    "marrow-compile",
    "marrow-image",
    "marrow-kernel",
    "marrow-local-wire",
    "marrow-project",
    "marrow-store",
    "marrow-syntax",
    "marrow-temporal",
    "marrow-verify",
    "marrow-vm",
];

/// Forbidden legacy families, spelled as the concrete identifiers or crate
/// references that would appear in retained source if a deleted owner leaked
/// back in. Each is a real Rust token, never an English word, so the scan has no
/// false positives against ordinary prose.
const FORBIDDEN_FAMILIES: &[&str] = &[
    // Deleted crate references: source edges (`use marrow_x` / `marrow_x::`)
    // and manifest/doc spellings (`marrow-x`).
    "marrow_check",
    "marrow_run",
    "marrow_schema",
    "marrow_catalog",
    "marrow_json",
    "marrow-check",
    "marrow-run",
    "marrow-schema",
    "marrow-catalog",
    "marrow-json",
    // The surface construct: AST nodes, the keyword variant, the codes family,
    // and the wire ABI types all share the `Surface` identifier prefix.
    "Surface",
    // The composed prototype session owner.
    "ProjectSession",
    // The deleted structural-optional value variant.
    "Value::Absent",
    // The tree-walking interpreter's owning type.
    "Interpreter",
    // Store-owned language vocabulary relocated to the path kernel at K.5: the
    // key/value scalar types and the deleted tree-cell/catalog-id key substrate.
    // The kernel now owns `KeyScalar`/`RuntimeScalar`; these old spellings must
    // not reappear in the store or anywhere else.
    "SavedKey",
    "SavedValue",
    "CatalogId",
    "DataPathSegment",
];

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow-codes`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .to_path_buf()
}

fn tracked_files(root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files"])
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

/// One workspace package's dependency edges, extracted from `cargo metadata`.
struct PackageEdges {
    name: String,
    /// `(dependency name, is_dev)` for every workspace-internal `marrow*` edge.
    edges: Vec<(String, bool)>,
}

/// Extract each workspace member's internal dependency edges from
/// `cargo metadata --no-deps`. The `--no-deps` package list carries every
/// member's `dependencies` array (name + kind), which is exactly the Cargo DAG
/// the trust-boundary gates below assert over. Parsing is the same minimal
/// dependency-free string extraction the membership test uses: the field order
/// within a package object is stable (`id` precedes `dependencies` precedes
/// `targets`), so splitting on `"id":"` yields one chunk per package.
fn workspace_edges() -> Vec<PackageEdges> {
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

    let mut packages = Vec::new();
    for chunk in text.split("\"id\":\"").skip(1) {
        let id = chunk.split('"').next().expect("id string terminates");
        let (path, fragment) = id.split_once('#').expect("package id has a fragment");
        let name = match fragment.split_once('@') {
            Some((name, _version)) => name.to_string(),
            None => path
                .rsplit('/')
                .next()
                .expect("package id path has segments")
                .to_string(),
        };
        let deps_body = chunk
            .split_once("\"dependencies\":[")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once("],\"targets\""))
            .map(|(body, _)| body)
            .unwrap_or("");
        let edges = deps_body
            .split("{\"name\":\"")
            .skip(1)
            .filter_map(|entry| {
                let dep = entry.split('"').next()?;
                if !dep.starts_with("marrow") {
                    return None;
                }
                let is_dev = entry
                    .split_once('}')
                    .is_some_and(|(fields, _)| fields.contains("\"kind\":\"dev\""));
                Some((dep.to_string(), is_dev))
            })
            .collect();
        packages.push(PackageEdges { name, edges });
    }
    assert_eq!(
        packages.len(),
        RETAINED_MEMBERS.len(),
        "metadata should list every workspace member"
    );
    packages
}

/// Trust-boundary Cargo-DAG gates (design §A): the VM never decodes the image
/// container, the compiler cannot reach the verifier/VM/kernel/store (it opens
/// no store and mints no VerifiedImage), and the raw byte engine is consumed
/// only through the path kernel. These edges are architecture, not convenience;
/// this test exists to make a regression conspicuous.
#[test]
fn cargo_dag_respects_the_trust_boundaries() {
    let packages = workspace_edges();
    let find = |name: &str| {
        packages
            .iter()
            .find(|package| package.name == name)
            .unwrap_or_else(|| panic!("workspace member {name} missing from metadata"))
    };

    // marrow-vm consumes only sealed images: no production edge to marrow-image
    // (a dev-dependency for building test artifacts is permitted).
    let vm = find("marrow-vm");
    assert!(
        !vm.edges
            .iter()
            .any(|(dep, is_dev)| dep == "marrow-image" && !is_dev),
        "marrow-vm must not have a production dependency on marrow-image"
    );

    // marrow-compile emits bytes only: no edge of any kind to the verifier, VM,
    // kernel, or store.
    let compile = find("marrow-compile");
    for forbidden in [
        "marrow-verify",
        "marrow-vm",
        "marrow-kernel",
        "marrow-store",
    ] {
        assert!(
            !compile.edges.iter().any(|(dep, _)| dep == forbidden),
            "marrow-compile must not depend on {forbidden}"
        );
    }

    // marrow-local-wire is the pure protocol owner: framing, limits, the closed
    // grammar, and canonical JSON with no execution, storage, or process edge. Its
    // only internal dependency is the diagnostic-code registry, so a regression that
    // reached the VM, verifier, kernel, image, or store from the wire crate — the
    // exact coupling the pure-crate boundary forbids — is conspicuous here.
    let wire = find("marrow-local-wire");
    for (dep, _) in &wire.edges {
        assert_eq!(
            dep, "marrow-codes",
            "marrow-local-wire must depend on marrow-codes alone; found an edge to {dep}"
        );
    }

    // The raw byte engine has exactly one consumer: the path kernel.
    for package in &packages {
        let depends_on_store = package.edges.iter().any(|(dep, _)| dep == "marrow-store");
        if package.name == "marrow-kernel" {
            assert!(
                depends_on_store,
                "marrow-kernel is the byte engine's consumer and must depend on marrow-store"
            );
        } else {
            assert!(
                !depends_on_store,
                "{} must not depend on marrow-store; the path kernel is the engine's only consumer",
                package.name
            );
        }
    }
}

#[test]
fn no_tracked_file_names_a_forbidden_family() {
    let root = workspace_root();
    let this_file = Path::new(file!())
        .file_name()
        .expect("this test file has a name")
        .to_owned();

    let mut violations: Vec<String> = Vec::new();
    for path in tracked_files(&root) {
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

/// Ambient-clock APIs that must not reach the temporal language path. A Marrow
/// temporal value is pure: it never derives from a wall or monotonic clock, a
/// timezone database, or a date/time crate. `Instant::now` (not the bare word
/// `Instant`, which is the temporal type) and `SystemTime` are the standard-library
/// clocks; the rest are the common third-party date/time crates.
const FORBIDDEN_CLOCK_APIS: &[&str] = &[
    "SystemTime",
    "UNIX_EPOCH",
    "Instant::now",
    "chrono",
    "OffsetDateTime",
    "PrimitiveDateTime",
];

/// The production source roots on the temporal language path: the temporal domain
/// owner, the compiler, the image container, the verifier, the VM, the parser, and
/// the kernel's logical codecs. The kernel's durable *store substrate* is excluded:
/// its witness-token nonce legitimately mixes the wall clock for cross-process
/// distinctness, which is a physical-substrate concern, not a temporal value.
const TEMPORAL_PATH_SRC: &[&str] = &[
    "crates/marrow-temporal/src",
    "crates/marrow-compile/src",
    "crates/marrow-image/src",
    "crates/marrow-verify/src",
    "crates/marrow-vm/src",
    "crates/marrow-syntax/src",
    "crates/marrow-kernel/src/codec",
];

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
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

/// The pure owners have no filesystem edge: `marrow-project` (the project-input
/// and identity-ledger owner) and `marrow-compile` (a read-only ledger
/// consumer) never touch `std::fs`. This is the D00 absence gate for compiler
/// ledger mutation — minting and publishing `marrow.ids` live only in the CLI's
/// `marrow run` convenience action (and in the accepted apply action when it
/// lands), so the compiler can never write identity. OS entropy is likewise a
/// CLI concern; these crates draw none.
#[test]
fn pure_owners_have_no_filesystem_edge() {
    let root = workspace_root();
    let mut files = Vec::new();
    for relative in ["crates/marrow-project/src", "crates/marrow-compile/src"] {
        rust_sources(&root.join(relative), &mut files);
    }
    assert!(
        !files.is_empty(),
        "the pure-owner source scan found no files; the roots moved"
    );

    let mut violations: Vec<String> = Vec::new();
    for path in files {
        let contents = std::fs::read_to_string(&path).expect("read a tracked rust source");
        for api in ["std::fs", "std::io::Read", "File::open", "File::create"] {
            if contents.contains(api) {
                violations.push(format!("{}: {api}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "a filesystem edge reached a pure owner:\n{}",
        violations.join("\n")
    );
}

/// No ambient clock feeds a temporal value: the temporal language path reads no wall
/// or monotonic clock and depends on no date/time crate. A clock is a later explicit
/// host effect; the temporal types are constructed only from literals and arguments.
#[test]
fn no_ambient_clock_on_the_temporal_path() {
    let root = workspace_root();
    let mut files = Vec::new();
    for relative in TEMPORAL_PATH_SRC {
        rust_sources(&root.join(relative), &mut files);
    }
    assert!(
        !files.is_empty(),
        "the temporal-path source scan found no files; the roots moved"
    );

    let mut violations: Vec<String> = Vec::new();
    for path in files {
        let contents = std::fs::read_to_string(&path).expect("read a tracked rust source");
        for api in FORBIDDEN_CLOCK_APIS {
            if contents.contains(api) {
                violations.push(format!("{}: {api}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "an ambient clock reached the temporal language path:\n{}",
        violations.join("\n")
    );
}
