//! The one borrowed capture-presentation facade.
//!
//! A [`CapturePresentation`] borrows the caller's root spelling and a
//! [`CaptureFailure`]. It forwards the pure owner's canonical `code`, `message`,
//! and manifest `position` unchanged, and it is the single owner of Physical and
//! Overlay code classification, path joining, and message selection. It is neither
//! `Clone` nor an owned message, and has no `Debug`. The two message writers and
//! the location writer stream into a caller-supplied sink without a shared
//! presentation cap or truncation.

use std::fmt;
use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_project::Position;

use crate::failure::{
    CaptureFailure, CaptureFailureKind, PhysicalBound, PhysicalFailure, PhysicalRefusal,
    PhysicalRole,
};
use crate::overlay::{OverlayBound, OverlayReason};
use crate::path::OperationalPath;

/// The required manifest file, joined to the caller root for a located fault.
const MANIFEST_FILE: &str = "marrow.toml";

/// A borrowed capture-presentation facade over a caller root spelling and a
/// [`CaptureFailure`].
pub struct CapturePresentation<'a> {
    root: &'a Path,
    failure: &'a CaptureFailure,
}

impl<'a> CapturePresentation<'a> {
    /// Borrow a facade over a caller root spelling and a failure.
    pub(crate) fn new(root: &'a Path, failure: &'a CaptureFailure) -> Self {
        Self { root, failure }
    }

    /// The stable diagnostic code for this failure. Pure Manifest/Project codes are
    /// forwarded; Physical/Overlay codes are classified here.
    pub fn code(&self) -> Code {
        match self.failure.kind() {
            CaptureFailureKind::Manifest(error) => error.code(),
            CaptureFailureKind::Project(error) => error.code(),
            CaptureFailureKind::Physical(failure) => physical_code(failure),
            CaptureFailureKind::OverlayInput(_) => Code::ProjectSourcePath,
        }
    }

    /// The 1-based manifest position of a located fault, or `None`. Only a
    /// malformed-manifest fault is located.
    pub fn position(&self) -> Option<Position> {
        match self.failure.kind() {
            CaptureFailureKind::Manifest(error) => error.position(),
            _ => None,
        }
    }

    /// Write the operating-system-prose-free operational message. Pure messages are
    /// rendered exactly; a Physical I/O failure omits its raw `io::Error` display.
    pub fn write_operational_message(&self, sink: &mut impl fmt::Write) -> fmt::Result {
        self.write_message(sink, false)
    }

    /// Write the message body for the CLI. Identical to the operational message
    /// except that a Physical I/O failure additionally appends its exact
    /// `io::Error` display for current CLI byte compatibility.
    pub fn write_cli_message(&self, sink: &mut impl fmt::Write) -> fmt::Result {
        self.write_message(sink, true)
    }

    /// Write the caller-spelled root joined to fixed `marrow.toml`, but only when
    /// [`position`](Self::position) is `Some`; otherwise write no bytes.
    pub fn write_position_file(&self, sink: &mut impl fmt::Write) -> fmt::Result {
        if self.position().is_some() {
            write!(sink, "{}", self.root.join(MANIFEST_FILE).display())?;
        }
        Ok(())
    }

    fn write_message(&self, sink: &mut impl fmt::Write, os_prose: bool) -> fmt::Result {
        match self.failure.kind() {
            CaptureFailureKind::Manifest(error) => sink.write_str(error.message()),
            CaptureFailureKind::Project(error) => sink.write_str(error.message()),
            CaptureFailureKind::Physical(failure) => self.write_physical(sink, failure, os_prose),
            CaptureFailureKind::OverlayInput(failure) => write_overlay(sink, failure.reason()),
        }
    }

    fn write_physical(
        &self,
        sink: &mut impl fmt::Write,
        failure: &PhysicalFailure,
        os_prose: bool,
    ) -> fmt::Result {
        match failure.refusal() {
            PhysicalRefusal::Missing { error } | PhysicalRefusal::Io { error } => {
                sink.write_str("failed to read ")?;
                self.write_joined(sink, failure.path())?;
                if os_prose {
                    write!(sink, ": {}", error.as_io_error())?;
                }
                Ok(())
            }
            PhysicalRefusal::Link { .. } => match failure.role() {
                PhysicalRole::SourceRoot => {
                    sink.write_str("source root ")?;
                    self.write_joined(sink, failure.path())?;
                    sink.write_str(
                        " is a symlink; a project's `src` must be a real directory inside the project",
                    )
                }
                PhysicalRole::IdentityLedger => {
                    self.write_joined(sink, failure.path())?;
                    sink.write_str(
                        " is a symlink; the identity artifact must be a real file inside the project",
                    )
                }
                _ => self.write_joined(sink, failure.path()),
            },
            PhysicalRefusal::Bound {
                bound,
                limit,
                actual,
            } => self.write_bound(sink, failure.path(), *bound, *limit, *actual),
            PhysicalRefusal::InvalidPathEncoding => {
                sink.write_str("source path ")?;
                self.write_joined(sink, failure.path())?;
                sink.write_str(" is not valid UTF-8")
            }
            PhysicalRefusal::UnexpectedKind { .. }
            | PhysicalRefusal::Hardlink
            | PhysicalRefusal::Changed
            | PhysicalRefusal::UnsupportedPlatform => Ok(()),
        }
    }

    fn write_bound(
        &self,
        sink: &mut impl fmt::Write,
        path: Option<&OperationalPath>,
        bound: PhysicalBound,
        limit: usize,
        actual: usize,
    ) -> fmt::Result {
        match bound {
            PhysicalBound::IdentityLedgerBytes => {
                self.write_joined(sink, path)?;
                write!(
                    sink,
                    " is {actual} bytes, over the {limit}-byte identity-artifact bound"
                )
            }
            // Per-file and project-total source bounds render the forward-slash
            // root-relative spelling directly; the source-file-count bound joins the
            // caller root.
            PhysicalBound::SourceFileBytes => self.write_capture_limit(
                sink,
                path,
                actual,
                limit,
                "over the per-file byte limit",
                false,
            ),
            PhysicalBound::SourceTotalBytes => self.write_capture_limit(
                sink,
                path,
                actual,
                limit,
                "over the project byte limit",
                false,
            ),
            PhysicalBound::SourceFiles => self.write_capture_limit(
                sink,
                path,
                actual,
                limit,
                "over the source-file limit",
                true,
            ),
            PhysicalBound::ManifestBytes
            | PhysicalBound::VisitedEntries
            | PhysicalBound::TraversalDepth
            | PhysicalBound::SourceSpellingBytes
            | PhysicalBound::RetainedPathUnits
            | PhysicalBound::PathWorkUnits => Ok(()),
        }
    }

    fn write_capture_limit(
        &self,
        sink: &mut impl fmt::Write,
        path: Option<&OperationalPath>,
        actual: usize,
        limit: usize,
        explanation: &str,
        join: bool,
    ) -> fmt::Result {
        sink.write_str("`")?;
        if join {
            self.write_joined(sink, path)?;
        } else {
            write_direct(sink, path)?;
        }
        write!(sink, "` capture is {actual}, {explanation} ({limit})")
    }

    /// Write the caller root joined to a root-relative path.
    fn write_joined(
        &self,
        sink: &mut impl fmt::Write,
        path: Option<&OperationalPath>,
    ) -> fmt::Result {
        match path {
            Some(path) => write!(sink, "{}", self.joined(path).display()),
            None => Ok(()),
        }
    }

    fn joined(&self, path: &OperationalPath) -> PathBuf {
        self.root.join(path.as_path())
    }
}

/// Write a root-relative path directly, without joining the caller root.
fn write_direct(sink: &mut impl fmt::Write, path: Option<&OperationalPath>) -> fmt::Result {
    match path {
        Some(path) => write!(sink, "{}", path.as_path().display()),
        None => Ok(()),
    }
}

/// The Physical code classification: pure source families are preserved and the new
/// physical faults use the operational `io.read` family.
fn physical_code(failure: &PhysicalFailure) -> Code {
    match failure.refusal() {
        PhysicalRefusal::Missing { .. } | PhysicalRefusal::Io { .. } => Code::IoRead,
        PhysicalRefusal::Link { .. } => match failure.role() {
            PhysicalRole::IdentityLedger => Code::ProjectIdsCorrupt,
            PhysicalRole::SourceRoot => Code::ProjectSourcePath,
            _ => Code::IoRead,
        },
        PhysicalRefusal::Bound { bound, .. } => match bound {
            PhysicalBound::IdentityLedgerBytes => Code::ProjectIdsCorrupt,
            PhysicalBound::SourceFiles
            | PhysicalBound::SourceFileBytes
            | PhysicalBound::SourceTotalBytes => Code::ProjectCaptureLimit,
            PhysicalBound::ManifestBytes
            | PhysicalBound::VisitedEntries
            | PhysicalBound::TraversalDepth
            | PhysicalBound::SourceSpellingBytes
            | PhysicalBound::RetainedPathUnits
            | PhysicalBound::PathWorkUnits => Code::IoRead,
        },
        PhysicalRefusal::InvalidPathEncoding => Code::ProjectSourcePath,
        PhysicalRefusal::UnexpectedKind { .. }
        | PhysicalRefusal::Hardlink
        | PhysicalRefusal::Changed
        | PhysicalRefusal::UnsupportedPlatform => Code::IoRead,
    }
}

/// The Overlay message. Overlay input faults are consumer-neutral and are not
/// reachable through any current CLI capture; this rendering is for the later
/// language-server consumer.
fn write_overlay(sink: &mut impl fmt::Write, reason: &OverlayReason) -> fmt::Result {
    match reason {
        OverlayReason::Bound {
            bound,
            limit,
            actual,
            ..
        } => {
            let resource = match bound {
                OverlayBound::Entries => "entries",
                OverlayBound::KeyBytes => "key bytes",
                OverlayBound::FileBytes => "file bytes",
                OverlayBound::TotalBytes => "total body bytes",
            };
            write!(sink, "overlay {resource} {actual} exceed the {limit} bound")
        }
        OverlayReason::Allocation { .. } => sink.write_str("overlay reservation failed"),
        OverlayReason::Duplicate { .. } => sink.write_str("overlay contains a duplicate key"),
        OverlayReason::Noncanonical { .. } => {
            sink.write_str("overlay key is not a canonical root-relative path")
        }
        OverlayReason::Nonmember { .. } => sink.write_str("overlay key is not a captured source"),
        OverlayReason::WrongRole { .. } => {
            sink.write_str("overlay key names a non-source project role")
        }
    }
}
