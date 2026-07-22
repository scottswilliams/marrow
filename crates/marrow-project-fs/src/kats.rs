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
    CaptureFailure, CaptureFailureKind, LedgerHome, LinkPosition, PhysicalBound, PhysicalFailure,
    PhysicalIoError, PhysicalKind, PhysicalOperation, PhysicalRefusal, PhysicalRole,
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

fn pathless(
    role: PhysicalRole,
    operation: PhysicalOperation,
    refusal: PhysicalRefusal,
) -> CaptureFailure {
    CaptureFailure::from_physical(PhysicalFailure::new(role, operation, None, refusal))
}

fn io(kind: io::ErrorKind, message: &str) -> PhysicalIoError {
    PhysicalIoError::new(io::Error::new(kind, message))
}

/// The identical CLI and operational message body a refusal renders. Every terse
/// physical body is operating-system-prose-free, so both writers agree.
fn both_messages(failure: &CaptureFailure) -> String {
    let cli = cli_message(failure);
    let operational = operational_message(failure);
    assert_eq!(
        cli, operational,
        "a terse physical body is identical in both writers"
    );
    assert!(
        !cli.is_empty(),
        "a payload-free refusal renders a nonempty body"
    );
    cli
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
        ".marrow/ids",
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
        "/proj/.marrow/ids is a symlink; the identity artifact must be a real file inside the project"
    );
}

#[test]
fn ledger_at_retired_root_path_renders_ids_location_with_a_move_steer() {
    let failure = physical(
        PhysicalRole::IdentityLedger,
        PhysicalOperation::Inspect,
        "marrow.ids",
        PhysicalRefusal::LegacyLedgerPath {
            home: LedgerHome::Vacant,
        },
    );
    assert_eq!(
        present(&failure, Path::new(ROOT)).code(),
        Code::ProjectIdsLocation
    );
    assert_eq!(
        cli_message(&failure),
        "/proj/marrow.ids is at the ledger's retired root location; its home is `.marrow/ids` — \
         move it (`git mv marrow.ids .marrow/ids`) and commit the move"
    );
}

#[test]
fn ledger_at_both_paths_renders_ids_location_with_a_reconcile_steer() {
    let failure = physical(
        PhysicalRole::IdentityLedger,
        PhysicalOperation::Inspect,
        "marrow.ids",
        PhysicalRefusal::LegacyLedgerPath {
            home: LedgerHome::Occupied,
        },
    );
    assert_eq!(
        present(&failure, Path::new(ROOT)).code(),
        Code::ProjectIdsLocation
    );
    assert_eq!(
        cli_message(&failure),
        "/proj/marrow.ids also exists beside `.marrow/ids`; a project has exactly one ledger — \
         keep the correct `.marrow/ids` and delete the root `marrow.ids`"
    );
}

#[test]
fn identity_ledger_byte_bound_renders_ids_corrupt() {
    let failure = physical(
        PhysicalRole::IdentityLedger,
        PhysicalOperation::Retain,
        ".marrow/ids",
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
        "/proj/.marrow/ids is 1048577 bytes, over the 1048576-byte identity-artifact bound"
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
    // The same body renders through the operational writer.
    assert_eq!(
        operational_message(&failure),
        "source path /proj/src/bad.mw is not valid UTF-8"
    );
}

// ===== Payload-free physical refusals render a terse typed body in both writers =

#[test]
fn hardlink_renders_a_terse_typed_body() {
    let failure = physical(
        PhysicalRole::Manifest,
        PhysicalOperation::Inspect,
        "marrow.toml",
        PhysicalRefusal::Hardlink,
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(both_messages(&failure), "/proj/marrow.toml is hard-linked");
}

#[test]
fn terminal_link_renders_a_terse_typed_body() {
    let failure = physical(
        PhysicalRole::Manifest,
        PhysicalOperation::Inspect,
        "marrow.toml",
        PhysicalRefusal::Link {
            position: LinkPosition::Terminal,
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        both_messages(&failure),
        "/proj/marrow.toml is a symbolic link"
    );
}

#[test]
fn intermediate_link_renders_a_terse_typed_body() {
    let failure = physical(
        PhysicalRole::SourceFile,
        PhysicalOperation::Inspect,
        "src/a.mw",
        PhysicalRefusal::Link {
            position: LinkPosition::Intermediate,
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        both_messages(&failure),
        "/proj/src/a.mw lies below a symbolic link"
    );
}

#[test]
fn changed_renders_a_terse_typed_body() {
    let failure = physical(
        PhysicalRole::SourceFile,
        PhysicalOperation::Open,
        "src/main.mw",
        PhysicalRefusal::Changed,
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        both_messages(&failure),
        "/proj/src/main.mw changed during capture"
    );
}

#[test]
fn unexpected_kind_with_a_path_renders_a_terse_typed_body() {
    let failure = physical(
        PhysicalRole::SourceFile,
        PhysicalOperation::Inspect,
        "src/x",
        PhysicalRefusal::UnexpectedKind {
            expected: PhysicalKind::Directory,
            actual: PhysicalKind::RegularFile,
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(both_messages(&failure), "/proj/src/x is not a directory");
}

#[test]
fn a_pathless_unexpected_root_kind_renders_a_role_subject() {
    let failure = pathless(
        PhysicalRole::Root,
        PhysicalOperation::Inspect,
        PhysicalRefusal::UnexpectedKind {
            expected: PhysicalKind::Directory,
            actual: PhysicalKind::RegularFile,
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        both_messages(&failure),
        "the project root is not a directory"
    );
}

#[test]
fn a_manifest_byte_bound_renders_a_terse_typed_body() {
    let failure = physical(
        PhysicalRole::Manifest,
        PhysicalOperation::Read,
        "marrow.toml",
        PhysicalRefusal::Bound {
            bound: PhysicalBound::ManifestBytes,
            limit: 6,
            actual: 7,
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        both_messages(&failure),
        "/proj/marrow.toml is 7 bytes, over the 6-byte manifest bound"
    );
}

#[test]
fn a_traversal_depth_bound_renders_a_terse_typed_body() {
    let failure = physical(
        PhysicalRole::SourceDirectory,
        PhysicalOperation::Enumerate,
        "src/deep",
        PhysicalRefusal::Bound {
            bound: PhysicalBound::TraversalDepth,
            limit: 1,
            actual: 2,
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        both_messages(&failure),
        "/proj/src/deep is at depth 2, over the 1-directory traversal-depth bound"
    );
}

#[test]
fn an_over_long_identity_forwards_the_pathless_pure_source_path_family() {
    // The pure projection maps a valid over-long spelling to the sealed pathless
    // pure Capture family. The adapter forwards it unmatched: presentation renders
    // the pure code and message, and the message retains no raw path.
    let overbound = format!(
        "src/{}.mw",
        "a".repeat(marrow_project::MAX_FILE_IDENTITY_BYTES)
    );
    let error = CapturedFile::check_identity_bound(&overbound)
        .expect_err("a valid over-long identity refuses");
    assert_eq!(error.code(), Code::ProjectSourcePath);
    let message = error.message().to_string();
    assert!(
        !message.contains("aaaa"),
        "the pure message retains no raw path"
    );
    let failure = CaptureFailure::from_project(error);
    assert_eq!(
        present(&failure, Path::new(ROOT)).code(),
        Code::ProjectSourcePath
    );
    assert_eq!(cli_message(&failure), message);
    // A valid in-bound identity passes the projection with no refusal.
    assert!(CapturedFile::check_identity_bound("src/main.mw").is_ok());
}

#[test]
fn a_visited_entry_bound_renders_a_pathless_terse_body() {
    let failure = pathless(
        PhysicalRole::SourceDirectory,
        PhysicalOperation::Enumerate,
        PhysicalRefusal::Bound {
            bound: PhysicalBound::VisitedEntries,
            limit: 3,
            actual: 4,
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        both_messages(&failure),
        "capture visited 4 directory entries, over the 3-entry bound"
    );
}

#[test]
fn a_retained_path_bound_renders_a_pathless_terse_body() {
    let failure = pathless(
        PhysicalRole::SourceDirectory,
        PhysicalOperation::Retain,
        PhysicalRefusal::Bound {
            bound: PhysicalBound::RetainedPathUnits,
            limit: 1,
            actual: 2,
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        both_messages(&failure),
        "capture retains 2 path units, over the 1-unit bound"
    );
}

#[test]
fn a_path_work_bound_renders_a_pathless_terse_body() {
    let failure = pathless(
        PhysicalRole::SourceDirectory,
        PhysicalOperation::Retain,
        PhysicalRefusal::Bound {
            bound: PhysicalBound::PathWorkUnits,
            limit: 1,
            actual: 2,
        },
    );
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(
        both_messages(&failure),
        "capture works over 2 path units, over the 1-unit bound"
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
    // Copy the frozen production defaults field by field (every field is `Copy`),
    // so the test base can never drift from `AdapterLimits::DEFAULT`; a test then
    // tightens exactly one field to isolate the bound it drives.
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
    temp.write(".marrow/ids", b"garbage");
    let failure =
        capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
            .expect_err("a missing manifest refuses");
    let physical = as_physical(&failure);
    assert_eq!(physical.role(), PhysicalRole::Manifest);
    // An absent required manifest is an I/O refusal; the target may classify it as
    // the dedicated `Missing` variant, so this control does not exclude it.
    assert!(matches!(
        physical.refusal(),
        PhysicalRefusal::Io { .. } | PhysicalRefusal::Missing { .. }
    ));
}

#[test]
fn a_ledger_at_the_retired_root_path_fails_closed_before_any_ledger_read() {
    // The ledger has one home. A file at the retired root path refuses with the
    // typed location fault and is never read — even valid artifact bytes there
    // change nothing.
    let temp = TempDir::new("legacy-ledger-vacant");
    valid_project(&temp);
    temp.write("marrow.ids", b"garbage never read");
    let failure =
        capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
            .expect_err("a root-path ledger refuses");
    let physical = as_physical(&failure);
    assert_eq!(physical.role(), PhysicalRole::IdentityLedger);
    assert!(matches!(
        physical.refusal(),
        PhysicalRefusal::LegacyLedgerPath {
            home: LedgerHome::Vacant
        }
    ));
}

#[test]
fn a_ledger_at_both_paths_fails_closed_as_a_reconcile_fault() {
    let temp = TempDir::new("legacy-ledger-occupied");
    valid_project(&temp);
    temp.write("marrow.ids", b"stale copy");
    temp.write(".marrow/ids", b"home copy");
    let failure =
        capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
            .expect_err("two ledger locations refuse");
    let physical = as_physical(&failure);
    assert_eq!(physical.role(), PhysicalRole::IdentityLedger);
    assert!(matches!(
        physical.refusal(),
        PhysicalRefusal::LegacyLedgerPath {
            home: LedgerHome::Occupied
        }
    ));
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

// ===== Target-owner KATs: directory admission and path budget =================

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod directory_admission {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::base_limits;
    use crate::capture::unix::DirectoryAdmission;
    use crate::failure::{CaptureFailure, CaptureFailureKind, PhysicalBound, PhysicalRefusal};
    use crate::limits::AdapterLimits;
    use crate::path::PathBudget;

    fn ok_entries(order: &[&str]) -> Vec<io::Result<PathBuf>> {
        order.iter().map(|path| Ok(PathBuf::from(*path))).collect()
    }

    fn refusal<T>(result: Result<T, CaptureFailure>) -> CaptureFailure {
        match result {
            Ok(_) => panic!("expected a refusal"),
            Err(failure) => failure,
        }
    }

    fn bound_of(failure: &CaptureFailure) -> PhysicalBound {
        match failure.kind() {
            CaptureFailureKind::Physical(physical) => match physical.refusal() {
                PhysicalRefusal::Bound { bound, .. } => *bound,
                other => panic!("expected a bound refusal, got {other:?}"),
            },
            _ => panic!("expected a physical failure"),
        }
    }

    fn is_io(failure: &CaptureFailure) -> bool {
        matches!(
            failure.kind(),
            CaptureFailureKind::Physical(physical)
                if matches!(physical.refusal(), PhysicalRefusal::Io { .. } | PhysicalRefusal::Missing { .. })
        )
    }

    /// Settle a synthetic all-success batch from a fresh budget, observing the sorted
    /// relatives and the committed work/retained/visited totals.
    fn observe(order: &[&str], limits: &AdapterLimits) -> (Vec<String>, usize, usize, usize) {
        let mut budget = PathBudget::new();
        let mut visited = 0usize;
        let children = DirectoryAdmission::settle(
            ok_entries(order).into_iter(),
            Path::new("src"),
            &mut budget,
            limits,
            &mut visited,
        )
        .expect("an under-bound batch settles");
        let relatives = children
            .iter()
            .map(|child| child.relative().to_string_lossy().into_owned())
            .collect();
        (relatives, budget.work(), budget.retained(), visited)
    }

    #[test]
    fn settlement_is_commutative_over_yield_order() {
        let limits = base_limits();
        let forward = observe(&["/root/a", "/root/b", "/root/c"], &limits);
        let reverse = observe(&["/root/c", "/root/b", "/root/a"], &limits);
        let zigzag = observe(&["/root/b", "/root/a", "/root/c"], &limits);
        assert_eq!(
            forward, reverse,
            "reverse yield order gives byte-identical results"
        );
        assert_eq!(
            forward, zigzag,
            "zigzag yield order gives byte-identical results"
        );
        assert_eq!(
            forward.0,
            ["src/a", "src/b", "src/c"],
            "children sort in native order"
        );
    }

    #[test]
    fn a_successful_batch_commits_visited_and_work_once() {
        let limits = base_limits();
        let (_, work, retained, visited) = observe(&["/root/aa", "/root/bb"], &limits);
        // Two 8-byte native paths: work and retained each advance by the exact
        // aggregate once, and visited by the exact count.
        assert_eq!(visited, 2);
        assert_eq!(work, 16);
        assert_eq!(retained, 16);
    }

    #[test]
    fn a_visit_over_the_remaining_allowance_leaves_counters_at_baseline() {
        let mut limits = base_limits();
        limits.visited_entries = 3;
        let mut budget = PathBudget::new();
        let mut visited = 2usize;
        let ok = DirectoryAdmission::settle(
            ok_entries(&["/root/a"]).into_iter(),
            Path::new("src"),
            &mut budget,
            &limits,
            &mut visited,
        );
        assert!(ok.is_ok(), "the N-th visit settles");
        assert_eq!(visited, 3);

        let baseline_work = budget.work();
        let baseline_retained = budget.retained();
        let failure = refusal(DirectoryAdmission::settle(
            ok_entries(&["/root/b", "/root/c"]).into_iter(),
            Path::new("src"),
            &mut budget,
            &limits,
            &mut visited,
        ));
        assert_eq!(bound_of(&failure), PhysicalBound::VisitedEntries);
        assert_eq!(visited, 3, "a refused batch leaves visited at the baseline");
        assert_eq!(budget.work(), baseline_work, "no work commits on refusal");
        assert_eq!(
            budget.retained(),
            baseline_retained,
            "no live charge commits on refusal"
        );
    }

    #[test]
    fn count_first_wins_when_the_extra_entry_is_a_success() {
        let mut limits = base_limits();
        limits.visited_entries = 1;
        let mut budget = PathBudget::new();
        let mut visited = 0usize;
        let failure = refusal(DirectoryAdmission::settle(
            ok_entries(&["/root/a", "/root/b"]).into_iter(),
            Path::new("src"),
            &mut budget,
            &limits,
            &mut visited,
        ));
        assert_eq!(bound_of(&failure), PhysicalBound::VisitedEntries);
    }

    #[test]
    fn the_first_iterator_error_wins_without_an_extra_success() {
        let mut limits = base_limits();
        limits.visited_entries = 1;
        let mut budget = PathBudget::new();
        let mut visited = 0usize;
        // `[Ok, Err]` with a one-entry allowance: the error at the second position
        // wins because no extra success was observed.
        let entries: Vec<io::Result<PathBuf>> = vec![
            Ok(PathBuf::from("/root/a")),
            Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        ];
        let failure = refusal(DirectoryAdmission::settle(
            entries.into_iter(),
            Path::new("src"),
            &mut budget,
            &limits,
            &mut visited,
        ));
        assert!(
            is_io(&failure),
            "an iterator error is an I/O refusal, not a visit bound"
        );
    }

    #[test]
    fn an_extra_success_before_an_error_still_wins_by_count() {
        let mut limits = base_limits();
        limits.visited_entries = 1;
        let mut budget = PathBudget::new();
        let mut visited = 0usize;
        // `[Ok, Ok, Err]`: the extra success at the second position establishes N+1
        // before the error is ever polled.
        let entries: Vec<io::Result<PathBuf>> = vec![
            Ok(PathBuf::from("/root/a")),
            Ok(PathBuf::from("/root/b")),
            Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        ];
        let failure = refusal(DirectoryAdmission::settle(
            entries.into_iter(),
            Path::new("src"),
            &mut budget,
            &limits,
            &mut visited,
        ));
        assert_eq!(bound_of(&failure), PhysicalBound::VisitedEntries);
    }

    #[test]
    fn retained_wins_a_simultaneous_aggregate_bound() {
        let mut limits = base_limits();
        limits.max_retained_path_units = 1;
        limits.max_path_work_units = 1;
        let mut budget = PathBudget::new();
        let mut visited = 0usize;
        let failure = refusal(DirectoryAdmission::settle(
            ok_entries(&["/root/a"]).into_iter(),
            Path::new("src"),
            &mut budget,
            &limits,
            &mut visited,
        ));
        assert_eq!(
            bound_of(&failure),
            PhysicalBound::RetainedPathUnits,
            "retained wins when both aggregate bounds would be exceeded"
        );
        assert_eq!(
            visited, 0,
            "a refused aggregate leaves visited at the baseline"
        );
    }

    /// The exact `(bound, limit, actual)` tuple of a bound refusal.
    fn bound_tuple(failure: &CaptureFailure) -> (PhysicalBound, usize, usize) {
        match failure.kind() {
            CaptureFailureKind::Physical(physical) => match physical.refusal() {
                PhysicalRefusal::Bound {
                    bound,
                    limit,
                    actual,
                } => (*bound, *limit, *actual),
                other => panic!("expected a bound refusal, got {other:?}"),
            },
            _ => panic!("expected a physical failure"),
        }
    }

    /// Settle a synthetic multiset, drop the staged carriers to release all live
    /// charge, and observe only the committed work — the work-only calibration.
    fn work_after_release(order: &[&str], limits: &AdapterLimits) -> (usize, usize) {
        let mut budget = PathBudget::new();
        let mut visited = 0usize;
        let children = DirectoryAdmission::settle(
            ok_entries(order).into_iter(),
            Path::new("src"),
            &mut budget,
            limits,
            &mut visited,
        )
        .expect("an under-bound batch settles");
        drop(children);
        (budget.work(), budget.retained())
    }

    #[test]
    fn the_work_only_calibration_is_commutative_and_monotone_after_release() {
        let limits = base_limits();
        let forward = work_after_release(&["/root/a", "/root/bb", "/root/ccc"], &limits);
        let reverse = work_after_release(&["/root/ccc", "/root/bb", "/root/a"], &limits);
        let zigzag = work_after_release(&["/root/bb", "/root/ccc", "/root/a"], &limits);
        assert_eq!(forward, reverse, "committed work is order-independent");
        assert_eq!(forward, zigzag, "committed work is order-independent");
        assert_eq!(
            forward.1, 0,
            "dropping the staged carriers releases all live charge"
        );
        assert!(
            forward.0 > 0,
            "work is committed and monotone across yield orders"
        );
    }

    #[test]
    fn a_wide_directory_settles_at_the_visit_limit_and_refuses_at_the_limit_plus_one() {
        let limits = base_limits();
        let limit = limits.visited_entries;

        // Exactly the limit settles: 65,536 entries.
        let full: Vec<io::Result<PathBuf>> = (0..limit)
            .map(|index| Ok(PathBuf::from(format!("/root/{index:06}"))))
            .collect();
        let mut budget = PathBudget::new();
        let mut visited = 0usize;
        let children = DirectoryAdmission::settle(
            full.into_iter(),
            Path::new("src"),
            &mut budget,
            &limits,
            &mut visited,
        )
        .expect("exactly the visit limit settles");
        assert_eq!(children.len(), limit);
        assert_eq!(visited, limit);

        // One extra entry refuses with the exact tuple and baseline counters,
        // whether the extra entry is yielded last or first.
        for extra_first in [false, true] {
            let mut order: Vec<io::Result<PathBuf>> = (0..limit)
                .map(|index| Ok(PathBuf::from(format!("/root/{index:06}"))))
                .collect();
            let extra = Ok(PathBuf::from("/root/extra"));
            if extra_first {
                order.insert(0, extra);
            } else {
                order.push(extra);
            }
            let mut budget = PathBudget::new();
            let mut visited = 0usize;
            let failure = refusal(DirectoryAdmission::settle(
                order.into_iter(),
                Path::new("src"),
                &mut budget,
                &limits,
                &mut visited,
            ));
            assert_eq!(
                bound_tuple(&failure),
                (PhysicalBound::VisitedEntries, limit, limit + 1),
                "the {}-first N+1 batch refuses with the exact visit tuple",
                if extra_first { "extra" } else { "wide" }
            );
            assert_eq!(visited, 0, "a refused batch leaves visited at the baseline");
            assert_eq!(budget.work(), 0, "a refused batch commits no work");
            assert_eq!(
                budget.retained(),
                0,
                "a refused batch commits no live charge"
            );
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod path_budget {
    use crate::path::{PathBudget, ReserveError};

    #[test]
    fn a_checked_add_overflow_is_reported_without_wrapping() {
        let mut budget = PathBudget::new();
        let _lease = budget
            .reserve(usize::MAX, usize::MAX, usize::MAX)
            .expect("the first reserve fits the range");
        let overflow = budget.reserve(1, usize::MAX, usize::MAX);
        assert!(matches!(overflow, Err(ReserveError::Overflow)));
    }

    #[test]
    fn a_released_lease_returns_the_live_charge_but_never_refunds_work() {
        let mut budget = PathBudget::new();
        {
            let _lease = budget.reserve(10, 100, 100).expect("reserve fits");
            assert_eq!(budget.retained(), 10);
            assert_eq!(budget.work(), 10);
        }
        assert_eq!(
            budget.retained(),
            0,
            "a dropped lease releases its live charge"
        );
        assert_eq!(budget.work(), 10, "work is monotone and never refunds");
    }
}
