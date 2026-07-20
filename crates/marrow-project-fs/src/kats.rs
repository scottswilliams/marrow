//! Presentation-facade behavior tests (Commit B baseline).
//!
//! These pin every facade arm, the exact current CLI operating-system `Display`,
//! bounded-sink rejection, the facade-owned `Debug` redaction, and the absence of
//! any presentation cap. They construct failures through the crate-internal
//! constructors, so they observe the same values the physical producer emits.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_project::{CaptureLimits, CapturedFile, Manifest};

use crate::failure::{
    CaptureFailure, LinkPosition, PhysicalBound, PhysicalFailure, PhysicalIoError,
    PhysicalOperation, PhysicalRefusal, PhysicalRole,
};
use crate::overlay::{OverlayBound, OverlayFailure, OverlayReason};
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
