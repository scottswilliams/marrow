//! Crate-internal behavior tests over the capture seams and presentation facade.
//!
//! The presentation group pins every facade arm, the exact current CLI
//! operating-system `Display`, bounded-sink rejection, the facade-owned `Debug`
//! redaction, and the absence of any presentation cap, constructing failures
//! through the crate-internal constructors so they observe the values the physical
//! producer emits.
//!
//! The behavior-red group observes the target adapter's laws through the baseline
//! production seams: the real limit-parameterized capture seam driven with tight
//! per-field policies, and the overlay constructor. The baseline is deliberately
//! insufficient — it enforces no visited-entry, depth, spelling, or path-budget
//! bound, follows links and hardlinks, and refuses every nonempty overlay with one
//! coarse bound — so those assertions fail against it. They name no final owner,
//! counter, lease, frame, or index type. The preclassified controls stay green
//! because their property already holds at the baseline and survives to the target.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_project::{CaptureLimits, CapturedFile, Manifest};

use crate::capture::capture_project_with_limits;
use crate::failure::{
    CaptureFailure, CaptureFailureKind, LinkPosition, PhysicalBound, PhysicalFailure,
    PhysicalIoError, PhysicalOperation, PhysicalRefusal, PhysicalRole,
};
use crate::limits::AdapterLimits;
use crate::overlay::{OverlayBound, OverlayEntry, OverlayFailure, OverlayReason, OverlaySnapshot};
use crate::path::OperationalPath;

const ROOT: &str = "/proj";

fn present<'a>(failure: &'a CaptureFailure, root: &'a Path) -> crate::CapturePresentation<'a> {
    failure.presentation(root)
}

fn cli_message(failure: &CaptureFailure) -> String {
    let root = Path::new(ROOT);
    let mut sink = String::new();
    present(failure, root)
        .write_cli_message(&mut sink)
        .expect("string sink");
    sink
}

fn operational_message(failure: &CaptureFailure) -> String {
    let root = Path::new(ROOT);
    let mut sink = String::new();
    present(failure, root)
        .write_operational_message(&mut sink)
        .expect("string sink");
    sink
}

fn physical(
    role: PhysicalRole,
    operation: PhysicalOperation,
    spelling: &str,
    refusal: PhysicalRefusal,
) -> CaptureFailure {
    CaptureFailure::from_physical(PhysicalFailure::new(
        role,
        operation,
        Some(OperationalPath::new(PathBuf::from(spelling))),
        refusal,
    ))
}

fn io(kind: io::ErrorKind, message: &str) -> PhysicalIoError {
    PhysicalIoError::new(io::Error::new(kind, message))
}

#[test]
fn manifest_read_failure_renders_io_read_with_and_without_os_prose() {
    let error = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
    let display = error.to_string();
    let failure = physical(
        PhysicalRole::Manifest,
        PhysicalOperation::Read,
        "marrow.toml",
        PhysicalRefusal::Io {
            error: PhysicalIoError::new(error),
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        cli_message(&failure),
        format!("failed to read /proj/marrow.toml: {display}")
    );
    assert_eq!(
        operational_message(&failure),
        "failed to read /proj/marrow.toml"
    );
    assert!(present(&failure, Path::new(ROOT)).position().is_none());
}

#[test]
fn identity_ledger_symlink_renders_ids_corrupt() {
    let failure = physical(
        PhysicalRole::IdentityLedger,
        PhysicalOperation::Inspect,
        "marrow.ids",
        PhysicalRefusal::Link {
            position: LinkPosition::Terminal,
        },
    );
    assert_eq!(
        present(&failure, Path::new(ROOT)).code(),
        Code::ProjectIdsCorrupt
    );
    assert_eq!(
        cli_message(&failure),
        "/proj/marrow.ids is a symlink; the identity artifact must be a real file inside the project"
    );
}

#[test]
fn identity_ledger_byte_bound_renders_ids_corrupt() {
    let failure = physical(
        PhysicalRole::IdentityLedger,
        PhysicalOperation::Retain,
        "marrow.ids",
        PhysicalRefusal::Bound {
            bound: PhysicalBound::IdentityLedgerBytes,
            limit: 1_048_576,
            actual: 1_048_577,
        },
    );
    assert_eq!(
        present(&failure, Path::new(ROOT)).code(),
        Code::ProjectIdsCorrupt
    );
    assert_eq!(
        cli_message(&failure),
        "/proj/marrow.ids is 1048577 bytes, over the 1048576-byte identity-artifact bound"
    );
}

#[test]
fn source_root_symlink_renders_source_path() {
    let failure = physical(
        PhysicalRole::SourceRoot,
        PhysicalOperation::Inspect,
        "src",
        PhysicalRefusal::Link {
            position: LinkPosition::Terminal,
        },
    );
    assert_eq!(
        present(&failure, Path::new(ROOT)).code(),
        Code::ProjectSourcePath
    );
    assert_eq!(
        cli_message(&failure),
        "source root /proj/src is a symlink; a project's `src` must be a real directory inside the project"
    );
}

#[test]
fn per_file_byte_bound_renders_the_forward_slash_spelling_directly() {
    let failure = physical(
        PhysicalRole::SourceFile,
        PhysicalOperation::Retain,
        "src/big.mw",
        PhysicalRefusal::Bound {
            bound: PhysicalBound::SourceFileBytes,
            limit: 1_048_576,
            actual: 1_048_577,
        },
    );
    assert_eq!(
        present(&failure, Path::new(ROOT)).code(),
        Code::ProjectCaptureLimit
    );
    // The per-file bound renders the root-relative spelling directly, not joined.
    assert_eq!(
        cli_message(&failure),
        "`src/big.mw` capture is 1048577, over the per-file byte limit (1048576)"
    );
}

#[test]
fn total_byte_bound_renders_the_forward_slash_spelling_directly() {
    let failure = physical(
        PhysicalRole::SourceFile,
        PhysicalOperation::Retain,
        "src/big.mw",
        PhysicalRefusal::Bound {
            bound: PhysicalBound::SourceTotalBytes,
            limit: 6,
            actual: 7,
        },
    );
    assert_eq!(
        cli_message(&failure),
        "`src/big.mw` capture is 7, over the project byte limit (6)"
    );
}

#[test]
fn source_file_count_bound_joins_the_caller_root() {
    let failure = physical(
        PhysicalRole::SourceFile,
        PhysicalOperation::Retain,
        "src/d.mw",
        PhysicalRefusal::Bound {
            bound: PhysicalBound::SourceFiles,
            limit: 3,
            actual: 4,
        },
    );
    // The count bound joins the caller root to the offending path.
    assert_eq!(
        cli_message(&failure),
        "`/proj/src/d.mw` capture is 4, over the source-file limit (3)"
    );
}

#[test]
fn invalid_path_encoding_renders_source_path() {
    let failure = physical(
        PhysicalRole::SourceFile,
        PhysicalOperation::Inspect,
        "src/bad.mw",
        PhysicalRefusal::InvalidPathEncoding,
    );
    assert_eq!(
        present(&failure, Path::new(ROOT)).code(),
        Code::ProjectSourcePath
    );
    assert_eq!(
        cli_message(&failure),
        "source path /proj/src/bad.mw is not valid UTF-8"
    );
}

#[test]
fn manifest_arm_forwards_pure_facts_and_locates_only_malformed() {
    let error = Manifest::parse("edition = [\n").expect_err("malformed");
    let code = error.code();
    let message = error.message().to_string();
    let position = error.position().expect("malformed is located");
    let failure = CaptureFailure::from_manifest(error);

    let root = Path::new(ROOT);
    let presentation = present(&failure, root);
    assert_eq!(presentation.code(), code);
    assert_eq!(cli_message(&failure), message);
    assert_eq!(presentation.position(), Some(position));

    let mut file = String::new();
    presentation
        .write_position_file(&mut file)
        .expect("string sink");
    assert_eq!(file, "/proj/marrow.toml");
}

#[test]
fn an_unlocated_manifest_fault_writes_no_position_file() {
    let error = Manifest::parse("").expect_err("missing edition");
    let failure = CaptureFailure::from_manifest(error);
    let root = Path::new(ROOT);
    let presentation = present(&failure, root);
    assert!(presentation.position().is_none());

    let mut file = String::new();
    presentation
        .write_position_file(&mut file)
        .expect("string sink");
    assert!(
        file.is_empty(),
        "an unlocated fault writes no position file"
    );
}

#[test]
fn project_arm_forwards_the_pure_capture_message_and_code() {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid");
    let error = marrow_project::capture(
        &manifest,
        vec![CapturedFile::new("outside.mw".to_string(), Vec::new())],
        None,
        &CaptureLimits::DEFAULT,
    )
    .expect_err("a path outside src rejects");
    let code = error.code();
    let message = error.message().to_string();
    let failure = CaptureFailure::from_project(error);

    assert_eq!(present(&failure, Path::new(ROOT)).code(), code);
    assert_eq!(cli_message(&failure), message);
}

#[test]
fn overlay_input_is_wrapped_and_presented_without_a_location() {
    let failure = CaptureFailure::from_overlay_input(OverlayFailure::new(OverlayReason::Bound {
        bound: OverlayBound::Entries,
        limit: 0,
        actual: 3,
        entry: None,
    }));
    let presentation = present(&failure, Path::new(ROOT));
    assert_eq!(presentation.code(), Code::ProjectSourcePath);
    assert_eq!(
        operational_message(&failure),
        "overlay entries 3 exceed the 0 bound"
    );
    assert!(presentation.position().is_none());
}

/// A sink that rejects once a fixed byte budget is exceeded, leaving its accepted
/// prefix in place.
struct BoundedSink {
    budget: usize,
    written: String,
}

impl fmt::Write for BoundedSink {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.written.len() + text.len() > self.budget {
            return Err(fmt::Error);
        }
        self.written.push_str(text);
        Ok(())
    }
}

#[test]
fn a_rejecting_sink_propagates_the_error_and_leaves_a_partial_prefix() {
    let failure = physical(
        PhysicalRole::Manifest,
        PhysicalOperation::Read,
        "marrow.toml",
        PhysicalRefusal::Io {
            error: io(io::ErrorKind::NotFound, "absent"),
        },
    );
    let mut sink = BoundedSink {
        budget: 5,
        written: String::new(),
    };
    let result = present(&failure, Path::new(ROOT)).write_operational_message(&mut sink);
    assert!(
        result.is_err(),
        "the rejecting sink must surface fmt::Error"
    );
    // Some caller-owned prefix may remain; the caller discards it. It is never the
    // whole message.
    assert!(sink.written.len() <= 5);
}

#[test]
fn direct_debug_redacts_every_private_evidence() {
    let failure = physical(
        PhysicalRole::SourceFile,
        PhysicalOperation::Read,
        "src/secret-path.mw",
        PhysicalRefusal::Io {
            error: io(io::ErrorKind::PermissionDenied, "secret-os-detail"),
        },
    );
    let opaque = format!("{failure:?}");
    assert_eq!(opaque, "CaptureFailure { .. }");
    assert!(!opaque.contains("secret"));

    let inner = PhysicalFailure::new(
        PhysicalRole::SourceFile,
        PhysicalOperation::Read,
        Some(OperationalPath::new(PathBuf::from("src/secret-path.mw"))),
        PhysicalRefusal::Io {
            error: io(io::ErrorKind::PermissionDenied, "secret-os-detail"),
        },
    );
    let physical_debug = format!("{inner:?}");
    assert!(physical_debug.contains("SourceFile"));
    assert!(!physical_debug.contains("secret-path"));
    assert!(!physical_debug.contains("secret-os-detail"));

    let opaque_io = io(io::ErrorKind::PermissionDenied, "secret-os-detail");
    let io_debug = format!("{opaque_io:?}");
    assert!(io_debug.contains("PermissionDenied"));
    assert!(!io_debug.contains("secret-os-detail"));
}

#[test]
fn a_large_manifest_message_is_never_capped() {
    let edition = "x".repeat((1 << 20) - 64);
    let error =
        Manifest::parse(&format!("edition = \"{edition}\"\n")).expect_err("unsupported edition");
    let expected = error.message().len();
    assert!(expected > 900_000, "the fixture message is nearly 1 MiB");
    let failure = CaptureFailure::from_manifest(error);
    assert_eq!(
        cli_message(&failure).len(),
        expected,
        "the facade streams the whole message with no cap"
    );
}

// ===== Behavior reds against the baseline production seams =====================

/// A temporary directory removed on drop.
struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "marrow-cap01-red-{tag}-{}-{nanos}",
            std::process::id()
        ));
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

/// Production limits with the frozen production values; a test tightens exactly one
/// field to isolate the bound it drives.
fn base_limits() -> AdapterLimits {
    AdapterLimits {
        manifest_bytes: 1 << 20,
        identity_ledger_bytes: 1 << 20,
        visited_entries: 65_536,
        traversal_depth: 64,
        source: CaptureLimits::new(4096, 1 << 20, 64 << 20),
        overlay_entries: 4096,
        overlay_key_bytes: 4096,
        overlay_file_bytes: 1 << 20,
        overlay_total_bytes: 64 << 20,
        max_source_spelling_bytes: 4096,
        max_retained_path_units: 64 << 20,
        max_path_work_units: 64 << 20,
    }
}

fn as_physical(failure: &CaptureFailure) -> &PhysicalFailure {
    match failure.kind() {
        CaptureFailureKind::Physical(physical) => physical,
        _ => panic!("target produces a physical failure here"),
    }
}

fn as_overlay(failure: &CaptureFailure) -> &OverlayFailure {
    match failure.kind() {
        CaptureFailureKind::OverlayInput(overlay) => overlay,
        _ => panic!("target produces an overlay-input failure here"),
    }
}

fn valid_project(temp: &TempDir) {
    temp.write("marrow.toml", b"edition = \"2026\"\n");
}

// --- Row: physical root producers ---------------------------------------------

#[test]
fn red_missing_root_is_a_canonicalize_failure_not_a_manifest_failure() {
    let root = Path::new("/marrow-cap01-red-missing-root-zzz");
    let failure = capture_project_with_limits(root, OverlaySnapshot::empty(), &base_limits())
        .expect_err("a missing root refuses");
    let physical = as_physical(&failure);
    assert_eq!(
        physical.role(),
        PhysicalRole::Root,
        "target admits the root first; the baseline refuses at the manifest"
    );
    assert_eq!(physical.operation(), PhysicalOperation::Canonicalize);
    assert!(physical.path().is_none(), "a root failure is pathless");
}

#[test]
fn red_a_file_root_is_an_unexpected_kind_failure() {
    let temp = TempDir::new("file-root");
    let file_root = temp.path().join("not-a-directory");
    fs::write(&file_root, b"x").expect("write file root");
    let failure = capture_project_with_limits(&file_root, OverlaySnapshot::empty(), &base_limits())
        .expect_err("a file root refuses");
    let physical = as_physical(&failure);
    assert_eq!(
        physical.role(),
        PhysicalRole::Root,
        "target rejects a non-directory root; the baseline refuses at the manifest"
    );
    assert!(matches!(
        physical.refusal(),
        PhysicalRefusal::UnexpectedKind { .. }
    ));
}

// --- Row: physical role/read seams --------------------------------------------

#[cfg(unix)]
#[test]
fn red_a_symlinked_manifest_is_refused_as_a_link() {
    let temp = TempDir::new("symlink-manifest");
    temp.write("real.toml", b"edition = \"2026\"\n");
    std::os::unix::fs::symlink(
        temp.path().join("real.toml"),
        temp.path().join("marrow.toml"),
    )
    .expect("symlink manifest");
    let result = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits());
    assert!(
        result.is_err(),
        "target refuses a symlinked manifest; the baseline follows and parses it"
    );
    let failure = result.unwrap_err();
    assert!(matches!(
        as_physical(&failure).refusal(),
        PhysicalRefusal::Link { .. }
    ));
}

#[cfg(unix)]
#[test]
fn red_a_hardlinked_manifest_is_refused_as_a_hardlink() {
    let temp = TempDir::new("hardlink-manifest");
    temp.write("real.toml", b"edition = \"2026\"\n");
    fs::hard_link(
        temp.path().join("real.toml"),
        temp.path().join("marrow.toml"),
    )
    .expect("hardlink manifest");
    let result = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits());
    assert!(
        result.is_err(),
        "target refuses a hardlinked manifest; the baseline reads it transparently"
    );
    assert!(matches!(
        as_physical(&result.unwrap_err()).refusal(),
        PhysicalRefusal::Hardlink
    ));
}

#[cfg(unix)]
#[test]
fn red_a_hardlinked_source_file_is_refused_as_a_hardlink() {
    let temp = TempDir::new("hardlink-source");
    valid_project(&temp);
    temp.write("src/real.mw", b"pub fn f()\n");
    fs::hard_link(
        temp.path().join("src/real.mw"),
        temp.path().join("src/main.mw"),
    )
    .expect("hardlink source");
    let result = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits());
    assert!(
        result.is_err(),
        "target refuses a hardlinked source; the baseline accepts it"
    );
    assert!(matches!(
        as_physical(&result.unwrap_err()).refusal(),
        PhysicalRefusal::Hardlink
    ));
}

// --- Row: source spelling, retained native paths, aggregate path work ----------

#[test]
fn red_an_over_bound_source_spelling_is_refused() {
    let temp = TempDir::new("spelling-bound");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    let mut limits = base_limits();
    // `src/main.mw` is 11 bytes, over a 4-byte spelling policy.
    limits.max_source_spelling_bytes = 4;
    let result = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits);
    assert!(
        result.is_err(),
        "target refuses an over-bound valid spelling before materialization"
    );
    assert!(matches!(
        as_physical(&result.unwrap_err()).refusal(),
        PhysicalRefusal::Bound {
            bound: PhysicalBound::SourceSpellingBytes,
            ..
        }
    ));
}

#[test]
fn red_over_bound_aggregate_path_work_is_refused() {
    let temp = TempDir::new("path-work-bound");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    let mut limits = base_limits();
    limits.max_path_work_units = 1;
    let result = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits);
    assert!(
        result.is_err(),
        "target charges and refuses aggregate path work; the baseline charges nothing"
    );
    assert!(matches!(
        as_physical(&result.unwrap_err()).refusal(),
        PhysicalRefusal::Bound {
            bound: PhysicalBound::PathWorkUnits,
            ..
        }
    ));
}

#[test]
fn red_over_bound_retained_path_units_is_refused() {
    let temp = TempDir::new("retained-path-bound");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    let mut limits = base_limits();
    limits.max_retained_path_units = 1;
    let result = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits);
    assert!(
        result.is_err(),
        "target charges and refuses live retained native paths; the baseline retains freely"
    );
    assert!(matches!(
        as_physical(&result.unwrap_err()).refusal(),
        PhysicalRefusal::Bound {
            bound: PhysicalBound::RetainedPathUnits,
            ..
        }
    ));
}

#[test]
fn control_an_under_bound_project_captures_its_modules() {
    let temp = TempDir::new("under-bound-source");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    let input = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
        .expect("an under-bound project captures");
    let modules: Vec<&str> = input
        .modules()
        .iter()
        .map(|m| m.module().as_str())
        .collect();
    assert_eq!(modules, ["main"]);
}

// --- Row: atomic directory admission ------------------------------------------

#[test]
fn red_visiting_over_the_entry_bound_is_refused() {
    let temp = TempDir::new("visited-bound");
    valid_project(&temp);
    for name in ["a", "b", "c", "d"] {
        temp.write(&format!("src/{name}.mw"), b"");
    }
    let mut limits = base_limits();
    limits.visited_entries = 3;
    let result = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits);
    assert!(
        result.is_err(),
        "target refuses the fourth visit; the baseline visits without a bound"
    );
    assert!(matches!(
        as_physical(&result.unwrap_err()).refusal(),
        PhysicalRefusal::Bound {
            bound: PhysicalBound::VisitedEntries,
            ..
        }
    ));
}

#[test]
fn red_descending_past_the_depth_bound_is_refused() {
    let temp = TempDir::new("depth-bound");
    valid_project(&temp);
    temp.write("src/a/b/c/deep.mw", b"");
    let mut limits = base_limits();
    limits.traversal_depth = 1;
    let result = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits);
    assert!(
        result.is_err(),
        "target refuses before descending past the depth bound; the baseline recurses freely"
    );
    assert!(matches!(
        as_physical(&result.unwrap_err()).refusal(),
        PhysicalRefusal::Bound {
            bound: PhysicalBound::TraversalDepth,
            ..
        }
    ));
}

#[test]
fn control_source_capture_order_is_deterministic() {
    let temp = TempDir::new("deterministic-order");
    valid_project(&temp);
    for name in ["zeta", "alpha", "mid"] {
        temp.write(&format!("src/{name}.mw"), b"");
    }
    let input = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
        .expect("captures");
    let modules: Vec<&str> = input
        .modules()
        .iter()
        .map(|m| m.module().as_str())
        .collect();
    assert_eq!(
        modules,
        ["alpha", "mid", "zeta"],
        "capture order is canonical regardless of directory yield order"
    );
}

// --- Row: raw overlay constructor ---------------------------------------------

#[test]
fn red_an_over_count_overlay_is_rejected() {
    let keys: Vec<String> = (0..4097).map(|index| format!("src/f{index}.mw")).collect();
    let entries: Vec<OverlayEntry> = keys
        .iter()
        .map(|key| OverlayEntry::new(key, b"x"))
        .collect();
    let result = OverlaySnapshot::try_new(&entries);
    assert!(
        result.is_err(),
        "target rejects a 4097-entry overlay; the baseline accepts without validation"
    );
    match result.unwrap_err().reason() {
        OverlayReason::Bound {
            bound: OverlayBound::Entries,
            limit,
            actual,
            entry,
        } => {
            assert_eq!(*limit, 4096);
            assert_eq!(*actual, 4097);
            assert!(entry.is_none(), "a whole-slice count failure has no entry");
        }
        other => panic!("expected an Entries bound, got {other:?}"),
    }
}

#[test]
fn red_an_over_long_key_is_rejected() {
    let key = "s".repeat(4097);
    let entries = [OverlayEntry::new(&key, b"x")];
    let result = OverlaySnapshot::try_new(&entries);
    assert!(result.is_err(), "target rejects a 4097-byte key");
    match result.unwrap_err().reason() {
        OverlayReason::Bound {
            bound: OverlayBound::KeyBytes,
            entry: Some(index),
            ..
        } => assert_eq!(index.get(), 0),
        other => panic!("expected a KeyBytes bound at entry 0, got {other:?}"),
    }
}

#[test]
fn red_an_over_large_body_is_rejected() {
    let body = vec![0u8; (1 << 20) + 1];
    let entries = [OverlayEntry::new("src/main.mw", &body)];
    let result = OverlaySnapshot::try_new(&entries);
    assert!(result.is_err(), "target rejects a 1 MiB + 1 body");
    assert!(matches!(
        result.unwrap_err().reason(),
        OverlayReason::Bound {
            bound: OverlayBound::FileBytes,
            entry: Some(_),
            ..
        }
    ));
}

#[test]
fn red_over_aggregate_body_bytes_are_rejected() {
    // Sixty-five 1 MiB bodies total 65 MiB, over the 64 MiB aggregate, while each
    // stays within the per-body bound.
    let chunk = vec![0u8; 1 << 20];
    let keys: Vec<String> = (0..65).map(|index| format!("src/f{index}.mw")).collect();
    let entries: Vec<OverlayEntry> = keys
        .iter()
        .map(|key| OverlayEntry::new(key, &chunk))
        .collect();
    let result = OverlaySnapshot::try_new(&entries);
    assert!(
        result.is_err(),
        "target rejects over-aggregate overlay bodies"
    );
    assert!(matches!(
        result.unwrap_err().reason(),
        OverlayReason::Bound {
            bound: OverlayBound::TotalBytes,
            ..
        }
    ));
}

#[test]
fn red_lexically_invalid_keys_are_rejected() {
    for key in [
        "../escape.mw",
        "/absolute.mw",
        "a//b.mw",
        "a/./b.mw",
        "a\\b.mw",
        "C:/drive.mw",
        ".",
        "trailing/",
        "",
    ] {
        let entries = [OverlayEntry::new(key, b"x")];
        let result = OverlaySnapshot::try_new(&entries);
        assert!(
            result.is_err(),
            "target rejects the lexically invalid key {key:?}; the baseline accepts it"
        );
        assert!(
            matches!(
                result.unwrap_err().reason(),
                OverlayReason::Bound { .. } | OverlayReason::Noncanonical { .. }
            ),
            "key {key:?} rejects lexically"
        );
    }
}

#[test]
fn control_case_distinct_overlay_keys_are_accepted() {
    let entries = [
        OverlayEntry::new("src/Books.mw", b"x"),
        OverlayEntry::new("src/books.mw", b"y"),
    ];
    // Case-distinct keys are not duplicates; both are admitted through construction.
    assert!(OverlaySnapshot::try_new(&entries).is_ok());
}

#[test]
fn control_the_empty_overlay_constructs_infallibly() {
    let entries: [OverlayEntry; 0] = [];
    assert!(OverlaySnapshot::try_new(&entries).is_ok());
    // `empty()` is a distinct allocation-free constructor with the same meaning.
    let _empty = OverlaySnapshot::empty();
}

// --- Row: overlay provenance and settlement -----------------------------------

#[test]
fn red_duplicate_overlay_keys_report_both_original_indices() {
    let entries = [
        OverlayEntry::new("src/main.mw", b"x"),
        OverlayEntry::new("src/main.mw", b"y"),
    ];
    let result = OverlaySnapshot::try_new(&entries);
    assert!(result.is_err(), "target rejects duplicate keys");
    match result.unwrap_err().reason() {
        OverlayReason::Duplicate { first, second } => {
            assert_eq!((first.get(), second.get()), (0, 1));
        }
        other => panic!("expected a Duplicate with the two original indices, got {other:?}"),
    }
}

#[test]
fn red_an_exact_member_overlay_replaces_the_disk_body() {
    let temp = TempDir::new("overlay-replace");
    valid_project(&temp);
    temp.write("src/main.mw", b"disk-body");
    let entries = [OverlayEntry::new("src/main.mw", b"overlay-body")];
    let snapshot = OverlaySnapshot::try_new(&entries).expect("baseline try_new is infallible");
    let result = capture_project_with_limits(temp.path(), snapshot, &base_limits());
    assert!(
        result.is_ok(),
        "target admits an exact member overlay; the baseline coarsely refuses every nonempty overlay"
    );
    assert_eq!(result.unwrap().modules()[0].source(), b"overlay-body");
}

#[test]
fn red_a_nonmember_overlay_reports_its_original_index() {
    let temp = TempDir::new("overlay-nonmember");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    let entries = [OverlayEntry::new("src/ghost.mw", b"x")];
    let snapshot = OverlaySnapshot::try_new(&entries).expect("infallible");
    let failure = capture_project_with_limits(temp.path(), snapshot, &base_limits())
        .expect_err("a nonmember overlay refuses");
    match as_overlay(&failure).reason() {
        OverlayReason::Nonmember { entry } => assert_eq!(entry.get(), 0),
        other => panic!("target reports Nonmember; the baseline coarsely refuses, got {other:?}"),
    }
}

// --- Row: high-level stages ---------------------------------------------------

#[test]
fn stage_a_missing_manifest_is_the_only_reported_role() {
    // Control: no source or ledger role is inspected after the manifest refuses.
    let temp = TempDir::new("stage-a-order");
    temp.write("src/main.mw", b"pub fn main()\n");
    temp.write("marrow.ids", b"garbage");
    let failure =
        capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
            .expect_err("a missing manifest refuses");
    let physical = as_physical(&failure);
    assert_eq!(physical.role(), PhysicalRole::Manifest);
    assert!(matches!(physical.refusal(), PhysicalRefusal::Io { .. }));
}

#[test]
fn stage_b_bounded_traversal_is_enforced() {
    // The bounded depth-first traversal refuses an over-deep tree; the baseline does
    // not. This is the stage-B checkpoint red.
    let temp = TempDir::new("stage-b-traversal");
    valid_project(&temp);
    temp.write("src/one/two/three.mw", b"");
    let mut limits = base_limits();
    limits.traversal_depth = 1;
    let result = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits);
    assert!(
        result.is_err(),
        "stage B enforces the depth checkpoint before descent"
    );
}

#[test]
fn stage_c_pure_refusal_precedes_unmatched_overlay_settlement() {
    // A colliding project plus a nonmember overlay: the target reports the pure
    // collision first, before overlay settlement. The baseline refuses the nonempty
    // overlay before running pure capture at all.
    let temp = TempDir::new("stage-c-precedence");
    valid_project(&temp);
    temp.write("src/a/b.mw", b"");
    temp.write("src/a.b.mw", b"");
    let entries = [OverlayEntry::new("src/ghost.mw", b"x")];
    let snapshot = OverlaySnapshot::try_new(&entries).expect("infallible");
    let failure = capture_project_with_limits(temp.path(), snapshot, &base_limits())
        .expect_err("a colliding project refuses");
    assert!(
        matches!(failure.kind(), CaptureFailureKind::Project(_)),
        "the pure collision precedes unmatched-overlay settlement"
    );
}

#[test]
fn control_empty_overlay_capture_is_byte_stable() {
    // Empty-overlay capture returns exactly the disk bytes: a retained control.
    let temp = TempDir::new("stage-c-empty");
    valid_project(&temp);
    temp.write("src/main.mw", b"disk");
    let input = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
        .expect("captures");
    assert_eq!(input.modules()[0].source(), b"disk");
}
