//! Crate-internal behavior tests over multi-origin capture: resolving a declared
//! dependency path from the admitted root, the one shared limit accumulator
//! carried across roots, and the read-only treatment of a dependency tree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use marrow_codes::Code;
use marrow_project::{CaptureLimits, DependencyAlias, SourceOrigin};

use crate::capture::capture_project_with_limits;
use crate::failure::{
    CaptureFailure, CaptureFailureKind, DependencyRefusal, PhysicalBound, PhysicalFailure,
    PhysicalRefusal, PhysicalRole,
};
use crate::limits::AdapterLimits;
use crate::overlay::{OverlayEntry, OverlaySnapshot};

/// A temporary directory holding one or more project trees, removed on drop.
struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("marrow-dep01-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp dir");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative: &str, contents: &[u8]) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write fixture");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn base_limits() -> AdapterLimits {
    let default = &AdapterLimits::DEFAULT;
    AdapterLimits {
        manifest_bytes: default.manifest_bytes,
        identity_ledger_bytes: default.identity_ledger_bytes,
        visited_entries: default.visited_entries,
        traversal_depth: default.traversal_depth,
        source: default.source,
        overlay_entries: default.overlay_entries,
        overlay_key_bytes: default.overlay_key_bytes,
        overlay_file_bytes: default.overlay_file_bytes,
        overlay_total_bytes: default.overlay_total_bytes,
        max_retained_path_units: default.max_retained_path_units,
        max_path_work_units: default.max_path_work_units,
    }
}

const LIBRARY_LEDGER: &[u8] = b"marrow ids v0\nmachine-written by marrow; do not edit\n\
                                id product Pair 03030303030303030303030303030303\n\
                                high-water 0\nend\n";

fn app_manifest(path: &str) -> Vec<u8> {
    format!("edition = \"2026\"\n\n[dependencies]\ngraphtext = {{ path = \"{path}\" }}\n")
        .into_bytes()
}

/// Two sibling trees: an application declaring one local-path dependency, and the
/// library it names.
fn two_trees(tag: &str) -> TempDir {
    let temp = TempDir::new(tag);
    temp.write("app/marrow.toml", &app_manifest("../graphtext"));
    temp.write("app/src/main.mw", b"pub fn main()\n");
    temp.write("graphtext/marrow.toml", b"edition = \"2026\"\n");
    temp.write("graphtext/src/text.mw", b"module text\n");
    temp
}

fn capture(root: &Path) -> Result<marrow_project::ProjectInput, CaptureFailure> {
    capture_project_with_limits(root, OverlaySnapshot::empty(), &base_limits())
}

fn refusal(root: &Path) -> CaptureFailure {
    capture(root).expect_err("this capture refuses")
}

fn code(root: &Path, failure: &CaptureFailure) -> Code {
    failure.presentation(root).code()
}

fn as_physical(failure: &CaptureFailure) -> &PhysicalFailure {
    match failure.kind() {
        CaptureFailureKind::Physical(physical) => physical,
        _ => panic!("this refusal must classify as a physical failure"),
    }
}

fn message(root: &Path, failure: &CaptureFailure) -> String {
    let mut sink = String::new();
    failure
        .presentation(root)
        .write_operational_message(&mut sink)
        .expect("string sink");
    sink
}

/// Every regular file under `root`, by relative path, with its bytes and modified
/// time, so a tree can be proved untouched.
fn snapshot(root: &Path) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
    let mut entries = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("read fixture directory") {
            let path = entry.expect("fixture entry").path();
            let metadata = fs::symlink_metadata(&path).expect("fixture metadata");
            if metadata.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("entry under root")
                    .to_string_lossy()
                    .into_owned();
                entries.insert(
                    relative,
                    (
                        fs::read(&path).expect("fixture bytes"),
                        metadata.modified().expect("fixture mtime"),
                    ),
                );
            }
        }
    }
    entries
}

// --- Row: a dependency joins the capture --------------------------------------

#[test]
fn a_declared_dependency_joins_the_capture_under_its_alias() {
    let temp = two_trees("alias-prefix");
    let root = temp.path().join("app");
    let input = capture(&root).expect("a two-tree project captures");

    let modules: Vec<(&str, &str)> = input
        .modules()
        .iter()
        .map(|module| (module.module().as_str(), module.identity().as_str()))
        .collect();
    assert_eq!(
        modules,
        vec![("main", "src/main.mw"), ("graphtext.text", "src/text.mw")],
        "the root's modules come first, then the dependency's under its alias, \
         each identity relative to its own tree"
    );
    assert_eq!(
        input.origins(),
        [
            SourceOrigin::Root,
            SourceOrigin::Dependency(DependencyAlias::parse("graphtext").expect("valid alias"))
        ]
    );
}

#[test]
fn an_undeclared_sibling_tree_is_not_captured() {
    let temp = TempDir::new("no-dependencies");
    temp.write("app/marrow.toml", b"edition = \"2026\"\n");
    temp.write("app/src/main.mw", b"pub fn main()\n");
    temp.write("graphtext/marrow.toml", b"edition = \"2026\"\n");
    temp.write("graphtext/src/text.mw", b"module text\n");
    let input = capture(&temp.path().join("app")).expect("a single-tree project captures");
    assert_eq!(input.modules().len(), 1);
    assert_eq!(input.origins(), [SourceOrigin::Root]);
}

#[test]
fn the_same_two_trees_capture_identically_wherever_they_sit_on_disk() {
    let here = two_trees("location-a");
    let there = two_trees("location-b");
    assert_eq!(
        capture(&here.path().join("app")).expect("captures"),
        capture(&there.path().join("app")).expect("captures"),
        "capture is location-independent across every tree it admits"
    );
}

// --- Row: dependency-path refusals --------------------------------------------

#[test]
fn a_dependency_path_that_names_no_usable_project_refuses() {
    let cases: Vec<(&str, DependencyRefusal)> = vec![
        ("no manifest", DependencyRefusal::NotAProject),
        ("no source root", DependencyRefusal::NotAProject),
        ("itself", DependencyRefusal::SelfReference),
        ("a transitive dependency", DependencyRefusal::Transitive),
        ("an invalid manifest", DependencyRefusal::InvalidManifest),
    ];
    for (label, reason) in cases {
        let temp = TempDir::new("unusable");
        let declared = match label {
            "itself" => "../app",
            _ => "../graphtext",
        };
        temp.write("app/marrow.toml", &app_manifest(declared));
        temp.write("app/src/main.mw", b"pub fn main()\n");
        match label {
            "no manifest" => temp.write("graphtext/src/text.mw", b"module text\n"),
            "no source root" => temp.write("graphtext/marrow.toml", b"edition = \"2026\"\n"),
            "itself" => {}
            "a transitive dependency" => {
                temp.write(
                    "graphtext/marrow.toml",
                    b"edition = \"2026\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
                );
                temp.write("graphtext/src/text.mw", b"module text\n");
                temp.write("core/marrow.toml", b"edition = \"2026\"\n");
                temp.write("core/src/core.mw", b"module core\n");
            }
            _ => {
                temp.write("graphtext/marrow.toml", b"name = \"graphtext\"\n");
                temp.write("graphtext/src/text.mw", b"module text\n");
            }
        }

        let root = temp.path().join("app");
        let failure = refusal(&root);
        assert_eq!(
            code(&root, &failure),
            Code::ProjectDependencyPath,
            "a dependency that is {label} refuses as a dependency-path fault"
        );
        let physical = as_physical(&failure);
        assert_eq!(physical.role(), PhysicalRole::Dependency, "{label}");
        assert!(
            matches!(
                physical.refusal(),
                PhysicalRefusal::Dependency { reason: actual } if *actual == reason
            ),
            "{label}: got {:?}",
            physical.refusal()
        );
        assert!(
            message(&root, &failure)
                .starts_with(&format!("dependency {}", root.join(declared).display())),
            "{label}: the message names the declared path as the consumer spelled it"
        );
    }
}

#[test]
fn a_dependency_reached_through_a_symlink_refuses() {
    let temp = two_trees("symlinked");
    std::os::unix::fs::symlink(temp.path().join("graphtext"), temp.path().join("link"))
        .expect("create symlink");
    temp.write("app/marrow.toml", &app_manifest("../link"));

    let root = temp.path().join("app");
    let failure = refusal(&root);
    assert_eq!(code(&root, &failure), Code::ProjectDependencyPath);
    assert!(matches!(
        as_physical(&failure).refusal(),
        PhysicalRefusal::Link { .. }
    ));
}

#[test]
fn a_dependency_path_that_escapes_to_nothing_refuses() {
    let temp = two_trees("escaping");
    temp.write(
        "app/marrow.toml",
        &app_manifest("../../marrow-dep01-absent"),
    );
    let root = temp.path().join("app");
    let failure = refusal(&root);
    assert_eq!(code(&root, &failure), Code::ProjectDependencyPath);
    assert!(matches!(
        as_physical(&failure).refusal(),
        PhysicalRefusal::Missing { .. }
    ));
}

#[test]
fn an_absolute_dependency_path_refuses_in_the_manifest() {
    let temp = two_trees("absolute");
    let absolute = temp.path().join("graphtext");
    temp.write(
        "app/marrow.toml",
        &app_manifest(&absolute.to_string_lossy()),
    );
    let root = temp.path().join("app");
    // The pure manifest owner refuses a location spelling before any filesystem
    // operation, so no admission is attempted.
    assert_eq!(code(&root, &refusal(&root)), Code::ProjectDependencyPath);
}

#[test]
fn an_alias_that_collides_with_a_root_module_refuses() {
    let temp = two_trees("alias-collision");
    temp.write("app/src/graphtext/text.mw", b"module graphtext::text\n");
    let root = temp.path().join("app");
    assert_eq!(code(&root, &refusal(&root)), Code::ProjectDependencyAlias);
}

// --- Row: one budget across every tree ----------------------------------------

#[test]
fn a_source_file_bound_crossed_only_in_the_sum_refuses_once() {
    let temp = two_trees("file-budget");
    temp.write("app/src/extra.mw", b"module extra\n");
    let root = temp.path().join("app");

    let mut limits = base_limits();
    limits.source = CaptureLimits::new(2, 1 << 20, 64 << 20);
    let failure = capture_project_with_limits(&root, OverlaySnapshot::empty(), &limits)
        .expect_err("three files across two trees refuse against a two-file bound");
    assert_eq!(
        failure.presentation(&root).code(),
        Code::ProjectCaptureLimit,
        "neither tree alone reaches the bound; their sum does"
    );
    assert!(matches!(
        as_physical(&failure).refusal(),
        PhysicalRefusal::Bound {
            bound: PhysicalBound::SourceFiles,
            limit: 2,
            actual: 3,
        }
    ));
}

#[test]
fn the_visited_entry_counter_is_not_reinitialised_per_tree() {
    let temp = two_trees("visit-budget");
    temp.write("app/src/extra.mw", b"module extra\n");
    temp.write("graphtext/src/more.mw", b"module more\n");
    let root = temp.path().join("app");

    // Two entries under each `src`: within the bound per tree, over it in the sum.
    let mut limits = base_limits();
    limits.visited_entries = 3;
    let failure = capture_project_with_limits(&root, OverlaySnapshot::empty(), &limits)
        .expect_err("four visited entries refuse against a three-entry bound");
    assert!(matches!(
        as_physical(&failure).refusal(),
        PhysicalRefusal::Bound {
            bound: PhysicalBound::VisitedEntries,
            ..
        }
    ));

    limits.visited_entries = 4;
    capture_project_with_limits(&root, OverlaySnapshot::empty(), &limits)
        .expect("the exact sum of both trees' entries is admitted");
}

#[test]
fn a_source_byte_bound_crossed_only_in_the_sum_refuses_once() {
    let temp = two_trees("byte-budget");
    let root = temp.path().join("app");
    let mut limits = base_limits();
    // `pub fn main()\n` is 14 bytes and `module text\n` is 12; either fits alone.
    limits.source = CaptureLimits::new(4096, 1 << 20, 20);
    let failure = capture_project_with_limits(&root, OverlaySnapshot::empty(), &limits)
        .expect_err("the two trees together pass the project byte bound");
    assert!(matches!(
        as_physical(&failure).refusal(),
        PhysicalRefusal::Bound {
            bound: PhysicalBound::SourceTotalBytes,
            ..
        }
    ));
}

// --- Row: a dependency is read, never written ---------------------------------

#[test]
fn a_dependency_ledger_is_captured_and_left_byte_identical() {
    let temp = two_trees("dependency-ledger");
    temp.write("graphtext/.marrow/ids", LIBRARY_LEDGER);
    let library = temp.path().join("graphtext");
    let before = snapshot(&library);

    let root = temp.path().join("app");
    let input = capture(&root).expect("a two-tree project captures");
    let alias = DependencyAlias::parse("graphtext").expect("valid alias");
    assert!(
        input
            .identity_ledger_for(&SourceOrigin::Dependency(alias))
            .is_some(),
        "the dependency's committed ledger is read"
    );
    assert!(
        input.identity_ledger().is_none(),
        "the root committed none of its own"
    );

    // The write owner is only ever handed the root path: capture opens a
    // dependency read-only, so every byte and timestamp under it survives, and no
    // publication artifact appears there.
    assert_eq!(before, snapshot(&library));
    assert!(!library.join(".marrow/ids.pending").exists());
    assert!(!library.join(".marrow/lock").exists());
}

#[test]
fn an_overlay_never_replaces_a_dependency_body() {
    let temp = two_trees("dependency-overlay");
    let root = temp.path().join("app");
    // `src/text.mw` names a captured source in the dependency, not in the root, so
    // the overlay finds no member to replace.
    let entries = [OverlayEntry::new(
        "src/text.mw",
        b"module text\n// edited\n",
    )];
    let snapshot = OverlaySnapshot::try_new(&entries).expect("a valid overlay constructs");
    let failure = capture_project_with_limits(&root, snapshot, &base_limits())
        .expect_err("an overlay key that names no root source is refused");
    assert!(matches!(
        failure.kind(),
        CaptureFailureKind::OverlayInput(_)
    ));
}

#[test]
fn a_fault_inside_a_dependency_names_the_declared_path() {
    let temp = two_trees("dependency-evidence");
    std::os::unix::fs::symlink("elsewhere.mw", temp.path().join("graphtext/src/link.mw"))
        .expect("create symlink");
    let root = temp.path().join("app");
    let failure = refusal(&root);
    let physical = as_physical(&failure);
    assert_eq!(
        physical.role(),
        PhysicalRole::SourceDirectory,
        "a dependency's own source discipline is the root project's discipline"
    );
    assert_eq!(
        message(&root, &failure),
        format!(
            "{} is a symbolic link",
            root.join("../graphtext/src/link.mw").display()
        ),
        "the evidence path stays relative to the caller's root"
    );
}
