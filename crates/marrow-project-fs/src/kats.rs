//! Crate-internal behavior tests over the capture seams and presentation facade.
//!
//! The presentation group pins every facade arm, the exact current CLI
//! operating-system `Display`, bounded-sink rejection, the facade-owned `Debug`
//! redaction, and the absence of any presentation cap, constructing failures
//! through the crate-internal constructors so they observe the values the physical
//! producer emits.
//!
//! The behavior group observes the adapter's laws through the production seams:
//! the real limit-parameterized capture seam driven with tight per-field policies,
//! and the overlay constructor. Each assertion names the bound or classification it
//! requires — which role refuses, which typed refusal it carries, and which entry it
//! indicts — and names no owner, counter, lease, frame, or index type.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_project::{CaptureLimits, CapturedFile, Manifest};

use crate::capture::capture_project_with_limits;
use crate::failure::{
    CaptureFailure, CaptureFailureKind, LedgerHome, LinkPosition, PhysicalBound, PhysicalFailure,
    PhysicalIoError, PhysicalKind, PhysicalRefusal, PhysicalRole,
};
use crate::limits::AdapterLimits;
use crate::overlay::{OverlayBound, OverlayEntry, OverlayFailure, OverlayReason, OverlaySnapshot};
use crate::path::native_units;
use crate::scratch::TempDir;

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

fn physical(role: PhysicalRole, spelling: &str, refusal: PhysicalRefusal) -> CaptureFailure {
    CaptureFailure::from_physical(PhysicalFailure {
        role,
        path: Some(PathBuf::from(spelling)),
        refusal,
    })
}

fn pathless(role: PhysicalRole, refusal: PhysicalRefusal) -> CaptureFailure {
    CaptureFailure::from_physical(PhysicalFailure {
        role,
        path: None,
        refusal,
    })
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

/// Assert each pinned `(code, body)` the facade renders for the caller root
/// `/proj`. Both writers agree on every terse physical body: only a raw I/O
/// refusal carries operating-system prose, and that one is pinned above.
fn assert_pins<'a>(pins: impl IntoIterator<Item = (&'a str, CaptureFailure, Code, &'a str)>) {
    for (pin, failure, code, body) in pins {
        let presentation = present(&failure, Path::new(ROOT));
        assert_eq!(presentation.code(), code, "{pin}: code");
        assert!(
            presentation.position().is_none(),
            "{pin}: a physical refusal is never located"
        );
        assert_eq!(both_messages(&failure), body, "{pin}: body");
    }
}

fn bound(bound: PhysicalBound, limit: usize, actual: usize) -> PhysicalRefusal {
    PhysicalRefusal::Bound {
        bound,
        limit,
        actual,
    }
}

fn link(position: LinkPosition) -> PhysicalRefusal {
    PhysicalRefusal::Link { position }
}

/// The physical refusals that keep a pure source-family code.
#[test]
fn project_family_physical_refusals_render_their_pinned_code_and_body() {
    use LedgerHome::{Occupied, Vacant};
    use PhysicalBound::*;
    use PhysicalRole::*;
    assert_pins([
        (
            "identity ledger symlink",
            physical(IdentityLedger, ".marrow/ids", link(LinkPosition::Terminal)),
            Code::ProjectIdsCorrupt,
            "/proj/.marrow/ids is a symlink; the identity artifact must be a real file inside the project",
        ),
        (
            "ledger at the retired root path, home vacant",
            physical(
                IdentityLedger,
                "marrow.ids",
                PhysicalRefusal::LegacyLedgerPath { home: Vacant },
            ),
            Code::ProjectIdsLocation,
            "/proj/marrow.ids is at the ledger's retired root location; its home is `.marrow/ids` — \
             move it (`git mv marrow.ids .marrow/ids`) and commit the move",
        ),
        (
            "ledger at both paths",
            physical(
                IdentityLedger,
                "marrow.ids",
                PhysicalRefusal::LegacyLedgerPath { home: Occupied },
            ),
            Code::ProjectIdsLocation,
            "/proj/marrow.ids also exists beside `.marrow/ids`; a project has exactly one ledger — \
             keep the correct `.marrow/ids` and delete the root `marrow.ids`",
        ),
        (
            "identity ledger byte bound",
            physical(
                IdentityLedger,
                ".marrow/ids",
                bound(IdentityLedgerBytes, 1_048_576, 1_048_577),
            ),
            Code::ProjectIdsCorrupt,
            "/proj/.marrow/ids is 1048577 bytes, over the 1048576-byte identity-artifact bound",
        ),
        (
            "source root symlink",
            physical(SourceRoot, "src", link(LinkPosition::Terminal)),
            Code::ProjectSourcePath,
            "source root /proj/src is a symlink; a project's `src` must be a real directory inside the project",
        ),
        (
            "per-file byte bound renders the forward-slash spelling directly",
            physical(
                SourceFile,
                "src/big.mw",
                bound(SourceFileBytes, 1_048_576, 1_048_577),
            ),
            Code::ProjectCaptureLimit,
            "`src/big.mw` capture is 1048577, over the per-file byte limit (1048576)",
        ),
        (
            "total byte bound renders the forward-slash spelling directly",
            physical(SourceFile, "src/big.mw", bound(SourceTotalBytes, 6, 7)),
            Code::ProjectCaptureLimit,
            "`src/big.mw` capture is 7, over the project byte limit (6)",
        ),
        (
            "source-file count bound joins the caller root",
            physical(SourceFile, "src/d.mw", bound(SourceFiles, 3, 4)),
            Code::ProjectCaptureLimit,
            "`/proj/src/d.mw` capture is 4, over the source-file limit (3)",
        ),
        (
            "invalid path encoding",
            physical(
                SourceFile,
                "src/bad.mw",
                PhysicalRefusal::InvalidPathEncoding,
            ),
            Code::ProjectSourcePath,
            "source path /proj/src/bad.mw is not valid UTF-8",
        ),
    ]);
}

/// The payload-free physical refusals: each renders a terse typed body under the
/// operational `io.read` code, with a role noun standing in for an absent path.
#[test]
fn io_read_physical_refusals_render_a_terse_typed_body() {
    use LinkPosition::{Intermediate, Terminal};
    use PhysicalBound::*;
    use PhysicalRole::*;
    let not_a = |expected| PhysicalRefusal::UnexpectedKind { expected };
    let pins = [
        (
            "hardlink",
            physical(Manifest, "marrow.toml", PhysicalRefusal::Hardlink),
            "/proj/marrow.toml is hard-linked",
        ),
        (
            "terminal link",
            physical(Manifest, "marrow.toml", link(Terminal)),
            "/proj/marrow.toml is a symbolic link",
        ),
        (
            "intermediate link",
            physical(SourceFile, "src/a.mw", link(Intermediate)),
            "/proj/src/a.mw lies below a symbolic link",
        ),
        (
            "changed",
            physical(SourceFile, "src/main.mw", PhysicalRefusal::Changed),
            "/proj/src/main.mw changed during capture",
        ),
        (
            "unexpected kind with a path",
            physical(SourceFile, "src/x", not_a(PhysicalKind::Directory)),
            "/proj/src/x is not a directory",
        ),
        (
            "pathless unexpected root kind renders a role subject",
            pathless(Root, not_a(PhysicalKind::Directory)),
            "the project root is not a directory",
        ),
        (
            "pathless unexpected kind of a regular-file role",
            pathless(Manifest, not_a(PhysicalKind::RegularFile)),
            "the manifest is not a regular file",
        ),
        (
            "manifest byte bound",
            physical(Manifest, "marrow.toml", bound(ManifestBytes, 6, 7)),
            "/proj/marrow.toml is 7 bytes, over the 6-byte manifest bound",
        ),
        (
            "traversal depth bound",
            physical(SourceDirectory, "src/deep", bound(TraversalDepth, 1, 2)),
            "/proj/src/deep is at depth 2, over the 1-directory traversal-depth bound",
        ),
        (
            "visited entry bound is pathless",
            pathless(SourceDirectory, bound(VisitedEntries, 3, 4)),
            "capture visited 4 directory entries, over the 3-entry bound",
        ),
        (
            "retained path bound is pathless",
            pathless(SourceDirectory, bound(RetainedPathUnits, 1, 2)),
            "capture retains 2 path units, over the 1-unit bound",
        ),
        (
            "path work bound is pathless",
            pathless(SourceDirectory, bound(PathWorkUnits, 1, 2)),
            "capture works over 2 path units, over the 1-unit bound",
        ),
    ];
    assert_pins(
        pins.into_iter()
            .map(|(pin, failure, body)| (pin, failure, Code::IoRead, body)),
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
        "src/secret-path.mw",
        PhysicalRefusal::Io {
            error: io(io::ErrorKind::PermissionDenied, "secret-os-detail"),
        },
    );
    let opaque = format!("{failure:?}");
    assert_eq!(opaque, "CaptureFailure { .. }");
    assert!(!opaque.contains("secret"));

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

// ===== Behavior tests against the production seams =============================

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
        _ => panic!("this refusal must classify as a physical failure"),
    }
}

fn as_overlay(failure: &CaptureFailure) -> &OverlayFailure {
    match failure.kind() {
        CaptureFailureKind::OverlayInput(overlay) => overlay,
        _ => panic!("this refusal must classify as an overlay-input failure"),
    }
}

fn valid_project(temp: &TempDir) {
    temp.write("marrow.toml", b"edition = \"2026\"\n");
}

// --- Row: physical root producers ---------------------------------------------

#[test]
fn missing_root_is_a_canonicalize_failure_not_a_manifest_failure() {
    let root = Path::new("/marrow-cap01-red-missing-root-zzz");
    let failure = capture_project_with_limits(root, OverlaySnapshot::empty(), &base_limits())
        .expect_err("a missing root refuses");
    let physical = as_physical(&failure);
    assert_eq!(
        physical.role,
        PhysicalRole::Root,
        "a missing root refuses in the root role, before the manifest is read"
    );
    assert!(physical.path.is_none(), "a root failure is pathless");
}

#[test]
fn a_file_root_is_an_unexpected_kind_failure() {
    let temp = TempDir::new("file-root");
    let file_root = temp.path().join("not-a-directory");
    fs::write(&file_root, b"x").expect("write file root");
    let failure = capture_project_with_limits(&file_root, OverlaySnapshot::empty(), &base_limits())
        .expect_err("a file root refuses");
    let physical = as_physical(&failure);
    assert_eq!(
        physical.role,
        PhysicalRole::Root,
        "a non-directory root refuses in the root role, before the manifest is read"
    );
    assert!(matches!(
        &physical.refusal,
        PhysicalRefusal::UnexpectedKind { .. }
    ));
}

// --- Row: physical role/read seams --------------------------------------------

#[test]
fn a_symlinked_manifest_is_refused_as_a_link() {
    let temp = TempDir::new("symlink-manifest");
    temp.write("real.toml", b"edition = \"2026\"\n");
    std::os::unix::fs::symlink(
        temp.path().join("real.toml"),
        temp.path().join("marrow.toml"),
    )
    .expect("symlink manifest");
    let failure =
        capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
            .expect_err("a symlinked manifest refuses; it is never followed and parsed");
    assert!(matches!(
        &as_physical(&failure).refusal,
        PhysicalRefusal::Link { .. }
    ));
}

#[test]
fn a_hardlinked_manifest_is_refused_as_a_hardlink() {
    let temp = TempDir::new("hardlink-manifest");
    temp.write("real.toml", b"edition = \"2026\"\n");
    fs::hard_link(
        temp.path().join("real.toml"),
        temp.path().join("marrow.toml"),
    )
    .expect("hardlink manifest");
    let failure =
        capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
            .expect_err(
                "a hardlinked manifest refuses; a second link to it is never read as the manifest",
            );
    assert!(matches!(
        &as_physical(&failure).refusal,
        PhysicalRefusal::Hardlink
    ));
}

#[test]
fn a_hardlinked_source_file_is_refused_as_a_hardlink() {
    let temp = TempDir::new("hardlink-source");
    valid_project(&temp);
    temp.write("src/real.mw", b"pub fn f()\n");
    fs::hard_link(
        temp.path().join("src/real.mw"),
        temp.path().join("src/main.mw"),
    )
    .expect("hardlink source");
    let failure =
        capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
            .expect_err("a hardlinked source file refuses; a second link to it is never captured");
    assert!(matches!(
        &as_physical(&failure).refusal,
        PhysicalRefusal::Hardlink
    ));
}

// --- Row: symbolic links and special files below the source root ---------------

/// Create a special file — a FIFO — at a project-relative path. Nothing in the
/// crate opens it: capture classifies a terminal object's kind before it opens
/// one, so a FIFO fixture cannot block a test on a missing writer.
fn write_special_file(temp: &TempDir, relative: &str) {
    let path = temp.path().join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    let status = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo created the fixture special file");
}

/// Capture the project and require the exact typed terminal-link refusal naming
/// `spelling`. A link below `src` must refuse with a cause; it is never skipped,
/// which would leave the module silently absent from the capture.
fn expect_link_below_src(temp: &TempDir, spelling: &str) -> CaptureFailure {
    let failure =
        match capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits()) {
            Err(failure) => failure,
            Ok(input) => panic!(
                "a symbolic link below `src` must refuse; capture admitted {:?} instead",
                input
                    .modules()
                    .iter()
                    .map(|module| module.identity().as_str())
                    .collect::<Vec<_>>()
            ),
        };
    let physical = as_physical(&failure);
    assert_eq!(physical.role, PhysicalRole::SourceDirectory);
    assert!(
        matches!(
            &physical.refusal,
            PhysicalRefusal::Link {
                position: LinkPosition::Terminal,
            }
        ),
        "a link below `src` is a terminal link, never followed to classify its target"
    );
    assert_eq!(
        physical
            .path
            .as_deref()
            .expect("a link refusal carries the charged entry path"),
        Path::new(spelling),
        "the refusal names the link itself, not whatever it points at"
    );
    failure
}

/// The reproduction: a module reachable only through a symlinked directory. The
/// skipping walk captured a project that silently lacked it, which surfaces
/// downstream as an unexplained missing module; the link now carries the cause.
#[test]
fn a_module_behind_a_symlinked_directory_refuses_instead_of_vanishing() {
    let temp = TempDir::new("symlink-module");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    temp.write("outside/shelf.mw", b"module link::shelf\n");
    std::os::unix::fs::symlink(temp.path().join("outside"), temp.path().join("src/link"))
        .expect("symlink a directory into src");
    let failure = expect_link_below_src(&temp, "src/link");
    assert_eq!(present(&failure, Path::new(ROOT)).code(), Code::IoRead);
    assert_eq!(both_messages(&failure), "/proj/src/link is a symbolic link");
}

/// A symlink to a regular `.mw` file: the alias would admit bytes capture never
/// opened at that name, so it refuses like every other aliased role.
#[test]
fn a_symlinked_source_file_below_src_is_refused_as_a_link() {
    let temp = TempDir::new("symlink-source");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    std::os::unix::fs::symlink(
        temp.path().join("src/main.mw"),
        temp.path().join("src/alias.mw"),
    )
    .expect("symlink a source file");
    expect_link_below_src(&temp, "src/alias.mw");
}

/// A broken link resolves to nothing, so following it is impossible and skipping
/// it is the same causeless absence. It refuses on the link itself, never on its
/// missing target.
#[test]
fn a_broken_symlink_below_src_is_refused_as_a_link() {
    let temp = TempDir::new("symlink-broken");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    std::os::unix::fs::symlink(
        temp.path().join("no-such-target.mw"),
        temp.path().join("src/dangling.mw"),
    )
    .expect("symlink a missing target");
    expect_link_below_src(&temp, "src/dangling.mw");
}

/// A link escaping the project root: refusing makes the escape unrepresentable
/// rather than merely unreached.
#[test]
fn a_symlink_escaping_the_project_root_is_refused_as_a_link() {
    let temp = TempDir::new("symlink-escape");
    let root = temp.path().join("project");
    fs::create_dir_all(root.join("src")).expect("create the project");
    fs::write(root.join("marrow.toml"), b"edition = \"2026\"\n").expect("write manifest");
    fs::write(root.join("src/main.mw"), b"pub fn main()\n").expect("write source");
    let outside = temp.path().join("elsewhere");
    fs::create_dir_all(&outside).expect("create the outside tree");
    fs::write(outside.join("stray.mw"), b"module away::stray\n").expect("write outside source");
    std::os::unix::fs::symlink(&outside, root.join("src/away")).expect("symlink out of the root");
    let failure = capture_project_with_limits(&root, OverlaySnapshot::empty(), &base_limits())
        .expect_err("a link that escapes the project root refuses");
    assert!(matches!(
        &as_physical(&failure).refusal,
        PhysicalRefusal::Link {
            position: LinkPosition::Terminal,
        }
    ));
}

/// A link cycle: refusing makes an unbounded walk unrepresentable by construction,
/// with no depth bound or visited set standing in for the policy.
#[test]
fn a_symlink_cycle_below_src_is_refused_as_a_link() {
    let temp = TempDir::new("symlink-cycle");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    std::os::unix::fs::symlink(temp.path().join("src"), temp.path().join("src/self"))
        .expect("symlink src into itself");
    expect_link_below_src(&temp, "src/self");
}

/// A special file occupying a module identity is the same causeless absence one
/// node kind over: it is admitted through the one source owner, which refuses the
/// kind before opening it, rather than being ignored like a non-source entry.
#[test]
fn a_special_file_named_mw_below_src_is_refused_as_a_wrong_kind() {
    let temp = TempDir::new("special-source");
    valid_project(&temp);
    write_special_file(&temp, "src/main.mw");
    let failure =
        match capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits()) {
            Err(failure) => failure,
            Ok(input) => panic!(
                "a special file at a module identity must refuse; capture admitted {:?} instead",
                input
                    .modules()
                    .iter()
                    .map(|module| module.identity().as_str())
                    .collect::<Vec<_>>()
            ),
        };
    let physical = as_physical(&failure);
    assert_eq!(physical.role, PhysicalRole::SourceFile);
    assert!(matches!(
        &physical.refusal,
        PhysicalRefusal::UnexpectedKind {
            expected: PhysicalKind::RegularFile,
        }
    ));
    assert_eq!(
        both_messages(&failure),
        "/proj/src/main.mw is not a regular file"
    );
}

/// The boundary of that widening: a special file that names no module is still an
/// ignored entry, exactly like a non-`.mw` regular file.
#[test]
fn a_special_file_below_src_naming_no_module_is_still_ignored() {
    let temp = TempDir::new("special-ignored");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    write_special_file(&temp, "src/notes.txt");
    let input = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &base_limits())
        .expect("a special file that names no module is ignored");
    assert_eq!(
        input
            .modules()
            .iter()
            .map(|module| module.identity().as_str())
            .collect::<Vec<_>>(),
        vec!["src/main.mw"]
    );
}

// --- Row: source spelling, retained native paths, aggregate path work ----------

#[test]
fn over_bound_aggregate_path_work_is_refused() {
    let temp = TempDir::new("path-work-bound");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    let mut limits = base_limits();
    limits.max_path_work_units = 1;
    let failure = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits)
        .expect_err("path work is charged and refuses once the aggregate allowance is spent");
    assert!(matches!(
        &as_physical(&failure).refusal,
        PhysicalRefusal::Bound {
            bound: PhysicalBound::PathWorkUnits,
            ..
        }
    ));
}

#[test]
fn over_bound_retained_path_units_is_refused() {
    let temp = TempDir::new("retained-path-bound");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    let mut limits = base_limits();
    limits.max_retained_path_units = 1;
    let failure = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits)
        .expect_err("live retained native paths are charged and refuse past their allowance");
    assert!(matches!(
        &as_physical(&failure).refusal,
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
fn visiting_over_the_entry_bound_is_refused() {
    let temp = TempDir::new("visited-bound");
    valid_project(&temp);
    for name in ["a", "b", "c", "d"] {
        temp.write(&format!("src/{name}.mw"), b"");
    }
    let mut limits = base_limits();
    limits.visited_entries = 3;
    let failure = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits)
        .expect_err("a visit past the entry bound refuses");
    assert!(matches!(
        &as_physical(&failure).refusal,
        PhysicalRefusal::Bound {
            bound: PhysicalBound::VisitedEntries,
            ..
        }
    ));
}

#[test]
fn descending_past_the_depth_bound_is_refused() {
    let temp = TempDir::new("depth-bound");
    valid_project(&temp);
    temp.write("src/a/b/c/deep.mw", b"");
    let mut limits = base_limits();
    limits.traversal_depth = 1;
    let failure = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits)
        .expect_err("the traversal refuses before descending past the depth bound");
    assert!(matches!(
        &as_physical(&failure).refusal,
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
fn an_over_count_overlay_is_rejected() {
    let keys: Vec<String> = (0..4097).map(|index| format!("src/f{index}.mw")).collect();
    let entries: Vec<OverlayEntry> = keys
        .iter()
        .map(|key| OverlayEntry::new(key, b"x"))
        .collect();
    let failure = OverlaySnapshot::try_new(&entries)
        .expect_err("an overlay one entry over the count bound is rejected at construction");
    match failure.reason() {
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
fn an_over_long_key_is_rejected() {
    let key = "s".repeat(4097);
    let entries = [OverlayEntry::new(&key, b"x")];
    let failure = OverlaySnapshot::try_new(&entries)
        .expect_err("a key one byte over the key bound is rejected at construction");
    match failure.reason() {
        OverlayReason::Bound {
            bound: OverlayBound::KeyBytes,
            entry: Some(index),
            ..
        } => assert_eq!(index.0, 0),
        other => panic!("expected a KeyBytes bound at entry 0, got {other:?}"),
    }
}

#[test]
fn an_over_large_body_is_rejected() {
    let body = vec![0u8; (1 << 20) + 1];
    let entries = [OverlayEntry::new("src/main.mw", &body)];
    let failure = OverlaySnapshot::try_new(&entries)
        .expect_err("a body one byte over the per-file bound is rejected at construction");
    assert!(matches!(
        failure.reason(),
        OverlayReason::Bound {
            bound: OverlayBound::FileBytes,
            entry: Some(_),
            ..
        }
    ));
}

#[test]
fn over_aggregate_body_bytes_are_rejected() {
    // Sixty-five 1 MiB bodies total 65 MiB, over the 64 MiB aggregate, while each
    // stays within the per-body bound.
    let chunk = vec![0u8; 1 << 20];
    let keys: Vec<String> = (0..65).map(|index| format!("src/f{index}.mw")).collect();
    let entries: Vec<OverlayEntry> = keys
        .iter()
        .map(|key| OverlayEntry::new(key, &chunk))
        .collect();
    let failure = OverlaySnapshot::try_new(&entries)
        .expect_err("bodies within the per-file bound still reject once they exceed the aggregate");
    assert!(matches!(
        failure.reason(),
        OverlayReason::Bound {
            bound: OverlayBound::TotalBytes,
            ..
        }
    ));
}

#[test]
fn lexically_invalid_keys_are_rejected() {
    for key in [
        "../escape.mw",
        "/absolute.mw",
        "a//b.mw",
        "a/./b.mw",
        "a\\b.mw",
        ".",
        "trailing/",
        "",
    ] {
        let entries = [OverlayEntry::new(key, b"x")];
        let failure = OverlaySnapshot::try_new(&entries)
            .expect_err("a lexically invalid key is rejected at construction");
        assert!(
            matches!(
                failure.reason(),
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
fn duplicate_overlay_keys_report_both_original_indices() {
    let entries = [
        OverlayEntry::new("src/main.mw", b"x"),
        OverlayEntry::new("src/main.mw", b"y"),
    ];
    let failure = OverlaySnapshot::try_new(&entries)
        .expect_err("two entries under one key are rejected at construction");
    match failure.reason() {
        OverlayReason::Duplicate { first, second } => {
            assert_eq!((first.0, second.0), (0, 1));
        }
        other => panic!("expected a Duplicate with the two original indices, got {other:?}"),
    }
}

#[test]
fn an_exact_member_overlay_replaces_the_disk_body() {
    let temp = TempDir::new("overlay-replace");
    valid_project(&temp);
    temp.write("src/main.mw", b"disk-body");
    let entries = [OverlayEntry::new("src/main.mw", b"overlay-body")];
    let snapshot = OverlaySnapshot::try_new(&entries).expect("a valid single-entry overlay");
    let result = capture_project_with_limits(temp.path(), snapshot, &base_limits());
    assert!(
        result.is_ok(),
        "an overlay keyed on a captured member is admitted, not refused as nonmember"
    );
    assert_eq!(result.unwrap().modules()[0].source(), b"overlay-body");
}

/// A key no admitted source carries constructs, is refused as a nonmember at
/// its original index after a successful pure capture, and presents under the
/// source-path code without a location. A drive-prefixed spelling is such a
/// key: the identity owner reads `C:` as a directory outside `src`.
#[test]
fn a_nonmember_overlay_is_refused_at_its_index_and_presented() {
    let temp = TempDir::new("overlay-nonmember");
    valid_project(&temp);
    temp.write("src/main.mw", b"pub fn main()\n");
    for key in ["src/ghost.mw", "C:/x"] {
        let entries = [OverlayEntry::new(key, b"x")];
        let snapshot = OverlaySnapshot::try_new(&entries).expect("a canonical key constructs");
        let failure = capture_project_with_limits(temp.path(), snapshot, &base_limits())
            .expect_err("a nonmember overlay refuses");
        match as_overlay(&failure).reason() {
            OverlayReason::Nonmember { entry } => assert_eq!(entry.0, 0, "{key}"),
            other => panic!("{key}: a nonmember overlay key must report Nonmember, got {other:?}"),
        }
        let presentation = present(&failure, Path::new(ROOT));
        assert_eq!(presentation.code(), Code::ProjectSourcePath, "{key}");
        assert_eq!(
            both_messages(&failure),
            "overlay key is not a captured source",
            "{key}"
        );
        assert!(presentation.position().is_none(), "{key}");
    }
}

/// A source's native-path lease is released once its bytes are captured, so the
/// live retained set during an admission is the tree root, `src`, one absolute
/// path per entry of the directory being walked, and the file's own evidence
/// path — never every source captured so far. The limit here admits that set
/// with half the sources' total to spare, far below what holding every source
/// lease to the end would need.
#[test]
fn source_leases_release_before_the_next_admission() {
    let temp = TempDir::new("source-lease-release");
    valid_project(&temp);
    let names: Vec<String> = (0..8)
        .map(|index| format!("{}{index}.mw", "f".repeat(200)))
        .collect();
    for name in &names {
        temp.write(&format!("src/{name}"), b"");
    }
    let root =
        native_units(&fs::canonicalize(temp.path()).expect("the fixture root canonicalizes"));
    let evidence = "src/".len() + names[0].len();
    let live_during_admission =
        root + "src".len() + names.len() * (root + "/src/".len() + names[0].len()) + evidence;
    let mut limits = base_limits();
    limits.max_retained_path_units = live_during_admission + names.len() * evidence / 2;
    let input = capture_project_with_limits(temp.path(), OverlaySnapshot::empty(), &limits)
        .expect("a lease released per source keeps every admission under the live bound");
    assert_eq!(input.modules().len(), names.len());
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
    assert_eq!(physical.role, PhysicalRole::Manifest);
    // An absent required manifest is an I/O refusal, which may also carry the
    // dedicated `Missing` classification.
    assert!(matches!(
        &physical.refusal,
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
    assert_eq!(physical.role, PhysicalRole::IdentityLedger);
    assert!(matches!(
        &physical.refusal,
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
    assert_eq!(physical.role, PhysicalRole::IdentityLedger);
    assert!(matches!(
        &physical.refusal,
        PhysicalRefusal::LegacyLedgerPath {
            home: LedgerHome::Occupied
        }
    ));
}

#[test]
fn stage_c_pure_refusal_precedes_unmatched_overlay_settlement() {
    // A colliding project plus a nonmember overlay: the pure collision is reported
    // first, before overlay settlement runs at all.
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

mod directory_admission {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::base_limits;
    use crate::capture::unix::{DirectoryAdmission, Tree};
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
            CaptureFailureKind::Physical(physical) => match &physical.refusal {
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
                if matches!(&physical.refusal, PhysicalRefusal::Io { .. } | PhysicalRefusal::Missing { .. })
        )
    }

    /// Settle a synthetic all-success batch from a fresh budget, observing the sorted
    /// relatives and the committed work/retained/visited totals.
    fn observe(order: &[&str], limits: &AdapterLimits) -> (Vec<String>, usize, usize, usize) {
        let mut budget = PathBudget::new();
        let mut visited = 0usize;
        let children = DirectoryAdmission::settle(
            ok_entries(order).into_iter(),
            &Tree::root(PathBuf::from("/root")),
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
            &Tree::root(PathBuf::from("/root")),
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
            &Tree::root(PathBuf::from("/root")),
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
            &Tree::root(PathBuf::from("/root")),
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
            &Tree::root(PathBuf::from("/root")),
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
            &Tree::root(PathBuf::from("/root")),
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
            &Tree::root(PathBuf::from("/root")),
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
            CaptureFailureKind::Physical(physical) => match &physical.refusal {
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
            &Tree::root(PathBuf::from("/root")),
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
            &Tree::root(PathBuf::from("/root")),
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
                &Tree::root(PathBuf::from("/root")),
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
