//! Fresh validation and activation of the caller-selected current store binding.
//! No missing head write is replayed, no old acknowledgment is reconstructed,
//! and no application attachment escapes this operation.

use std::path::Path;

use marrow_codes::Code;
use marrow_image::{ImageId, StoreHeadDigest};
use marrow_kernel::durable::NativeOpenAccess;

use crate::actor::{BindingStrictness, ImageAdmission};
use crate::audit::{self, AuditError, Names, StoreAudit};
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::provision::{AdmitError, LockedStore, OpenError};
use crate::seam::{Seam, Step};
use crate::store_dir::{AdmissionError, Artifact, StoreEntry};
use crate::{PreparedImage, StoreInstanceId};

/// The binding actually validated and activated at the current directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredStore {
    pub instance: StoreInstanceId,
    pub image_id: ImageId,
    /// Relative names moved by this attempt, at most one per metadata slot.
    pub preserved: Vec<String>,
}

/// Failure and the preservation moves already known to this attempt.
/// A lost reply cannot be reconstructed by a later invocation.
#[derive(Debug)]
pub struct RecoveryError {
    pub fault: RecoveryFault,
    pub preserved: Vec<String>,
}

impl RecoveryError {
    pub fn code(&self) -> Code {
        self.fault.code()
    }
}
impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fault.fmt(f)
    }
}
impl std::error::Error for RecoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.fault)
    }
}

/// One OS entropy draw for a filename, not a store identity or replay token.
fn preservation_nonce() -> std::io::Result<[u8; 16]> {
    use std::io::Read;
    let mut bytes = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[derive(Debug)]
pub enum RecoveryFault {
    Validation(AuditError),
    /// The present head is not one of the exact heads recorded by Pending.
    HeadMismatch,
    Logical(Box<StoreAudit>),
    Metadata(AdmissionError),
    Io(std::io::Error),
    /// Required earlier barriers passed, but final activation was not confirmed.
    Completion {
        instance: StoreInstanceId,
        source: AuditError,
    },
}

impl RecoveryFault {
    pub fn code(&self) -> Code {
        match self {
            Self::Validation(error) => error.code(),
            Self::HeadMismatch | Self::Logical(_) => Code::StoreCorruption,
            Self::Metadata(error) => error.code(),
            Self::Io(_) => Code::StoreIo,
            Self::Completion { .. } => Code::StoreActivationUncertain,
        }
    }
}

impl std::fmt::Display for RecoveryFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(error) => write!(f, "{error}"),
            Self::HeadMismatch => write!(
                f,
                "the current head is not recorded by the pending transition"
            ),
            Self::Logical(report) => write!(
                f,
                "the store has {} logical integrity findings",
                report.summary.findings
            ),
            Self::Metadata(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "recovery failed: {error}"),
            Self::Completion { instance, source } => write!(
                f,
                "store {} activation is unconfirmed: {source}",
                instance.to_hex()
            ),
        }
    }
}

impl std::error::Error for RecoveryFault {}

fn validation(error: OpenError) -> RecoveryFault {
    RecoveryFault::Validation(AuditError::Open(error))
}

fn accepts_head(state: EnvelopeState, digest: StoreHeadDigest) -> bool {
    match state {
        EnvelopeState::Active => true,
        EnvelopeState::Provision { head } => head == digest,
        EnvelopeState::Rebind { old, new } => digest == old || digest == new,
    }
}

/// Validate physical and logical integrity under one owner, establish fresh
/// current-location barriers, and activate only the supplied exact stored image.
pub fn recover(dir: &Path, prepared: PreparedImage) -> Result<RecoveredStore, RecoveryError> {
    recover_observed(dir, prepared, Seam::NONE)
}

/// [`recover`] over `seam`: the production seam observes nothing; a test's cuts or mutates
/// the sequence at a named step.
pub(crate) fn recover_observed(
    dir: &Path,
    prepared: PreparedImage,
    seam: Seam,
) -> Result<RecoveredStore, RecoveryError> {
    let mut preserved = Vec::new();
    match recover_inner(dir, prepared, seam, &mut preserved) {
        Ok((instance, image_id)) => Ok(RecoveredStore {
            instance,
            image_id,
            preserved,
        }),
        Err(fault) => Err(RecoveryError { fault, preserved }),
    }
}

fn recover_inner(
    dir: &Path,
    prepared: PreparedImage,
    seam: Seam,
    preserved: &mut Vec<String>,
) -> Result<(StoreInstanceId, ImageId), RecoveryFault> {
    let (image, projection) = prepared.into_parts();
    let projection = projection.ok_or(RecoveryFault::Validation(AuditError::NotExecutable))?;
    let names = Names::new(&projection);
    let admission = ImageAdmission::derive(&image, projection);
    let locked = LockedStore::acquire(dir, seam).map_err(validation)?;
    let location = locked.directory_path().to_path_buf();
    let state = locked.envelope.state;
    let opened = locked
        .open(NativeOpenAccess::Recovery, |head, digest| {
            if !accepts_head(state, digest) {
                return Err(RecoveryFault::HeadMismatch);
            }
            admission
                .admit(head, BindingStrictness::Exact)
                .map_err(|error| {
                    RecoveryFault::Validation(audit::open_error(AdmitError::Refused(error)))
                })
        })
        .map_err(|error| match error {
            AdmitError::Open(error) => validation(error),
            AdmitError::Refused(error) => error,
        })?;
    let inspected =
        audit::inspect(&opened, &names, image.image_id()).map_err(RecoveryFault::Validation)?;
    if !inspected.is_clean() {
        return Err(RecoveryFault::Logical(Box::new(inspected)));
    }
    for artifact in [Artifact::Envelope, Artifact::Head] {
        opened
            .directory
            .preserve_replacement(artifact, preservation_nonce, preserved)
            .map_err(RecoveryFault::Metadata)?;
    }
    let instance = opened.envelope.instance;
    let record = EnvelopeRecord {
        metadata: crate::StoreEnvelope {
            writer_toolchain: env!("CARGO_PKG_VERSION").to_owned(),
            ..opened.envelope.clone()
        },
        state: EnvelopeState::Active,
    };
    opened
        .directory
        .sync_artifacts()
        .map_err(RecoveryFault::Metadata)?;
    opened
        .directory
        .verify_location(&location)
        .map_err(RecoveryFault::Metadata)?;
    let parent = location
        .parent()
        .ok_or_else(|| RecoveryFault::Io(std::io::Error::from(std::io::ErrorKind::InvalidInput)))?;
    opened
        .directory
        .at(Step::RecoveryParent)
        .map_err(|error| RecoveryFault::Io(std::io::Error::other(error)))?;
    crate::durable_fs::sync_dir(parent).map_err(RecoveryFault::Io)?;
    opened
        .directory
        .verify_location(&location)
        .map_err(RecoveryFault::Metadata)?;

    let finish = || -> Result<(), AuditError> {
        let metadata_error = |error| AuditError::Open(OpenError::Admission(error));
        let bytes = record
            .encode()
            .map_err(|error| metadata_error(AdmissionError::format(StoreEntry::Envelope, error)))?;
        opened
            .directory
            .replace(Artifact::Envelope, &bytes)
            .map_err(metadata_error)?;
        opened
            .directory
            .sync(Step::RecoveryActive)
            .map_err(metadata_error)?;
        opened.directory.at(Step::FinalRead).map_err(metadata_error)?;
        audit::verify_published(&opened.directory, &location, &record, opened.head_digest)
    };
    finish().map_err(|source| RecoveryFault::Completion { instance, source })?;
    Ok((instance, image.image_id()))
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
