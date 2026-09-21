//! Fresh-store construction from complete, verified logical backup input.
//! Head remains absent until the private body passes all transfer and audit gates.

use std::convert::Infallible;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use marrow_kernel::durable::{
    NativeOwnerOpenError, NativeRestoreError, NativeStore, NumberedProjection,
};
use marrow_verify::VerifyRejection;

use crate::actor::{BindingStrictness, ImageAdmission};
use crate::audit::{self, ChainDigest, Names};
use crate::backup_stream::Decoder;
use crate::durable_fs::{Publication, custody_io};
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::provision::{AdmitError, complete_publication, create_private_dir, temp_sibling};
use crate::seam::{Seam, Step};
use crate::store_dir::{AdmittedStoreDir, Artifact};
use crate::{
    AuditError, BackupReadError, EngineKind, EntropyUnavailable, LogicalHead, ProvisionFault,
    StoreAudit, StoreEnvelope, StoreInstanceId, prepare,
};
use marrow_codes::Code;

#[derive(Debug)]
pub struct RestoredStore {
    pub audit: StoreAudit,
}

#[derive(Debug)]
pub enum RestoreFault {
    Input(BackupReadError),
    Image(VerifyRejection),
    Admission(AuditError),
    Entropy(EntropyUnavailable),
    Io(io::Error),
    Provision(ProvisionFault),
    Open(NativeOwnerOpenError<Infallible>),
    Body(marrow_kernel::durable::RestoreError<BackupReadError>),
    Completion {
        instance: StoreInstanceId,
        source: AuditError,
    },
}

/// A failed restore preserves a possible unpublished stage for inspection.
/// It never retries commits or removes a stage under an unresolved native owner.
/// After publication, uncertainty is carried by the fault and stage is None.
#[derive(Debug)]
pub struct RestoreError {
    pub fault: RestoreFault,
    pub stage: Option<PathBuf>,
}

/// The failed construction batch, not the state of earlier confirmed batches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreBatchOutcome {
    Aborted,
    Indeterminate,
}

impl From<RestoreFault> for RestoreError {
    fn from(fault: RestoreFault) -> Self {
        Self { fault, stage: None }
    }
}

impl RestoreError {
    pub fn batch_outcome(&self) -> Option<RestoreBatchOutcome> {
        use marrow_kernel::durable::RestoreError as Body;
        match &self.fault {
            RestoreFault::Body(Body::Aborted) => Some(RestoreBatchOutcome::Aborted),
            RestoreFault::Body(Body::Indeterminate) => Some(RestoreBatchOutcome::Indeterminate),
            _ => None,
        }
    }

    pub fn code(&self) -> Code {
        use marrow_kernel::durable::RestoreError as Body;
        match &self.fault {
            RestoreFault::Input(error) | RestoreFault::Body(Body::Input(error)) => error.code(),
            RestoreFault::Image(error) => error.code(),
            RestoreFault::Admission(error) => error.code(),
            RestoreFault::Entropy(_) => Code::IoRead,
            RestoreFault::Io(_) => Code::StoreIo,
            RestoreFault::Provision(error) => error.code(),
            RestoreFault::Open(NativeOwnerOpenError::Lock(error)) => error.code(),
            RestoreFault::Open(NativeOwnerOpenError::Store(error))
            | RestoreFault::Body(Body::Store(error)) => error.code(),
            RestoreFault::Open(NativeOwnerOpenError::Refused(never)) => match *never {},
            RestoreFault::Body(Body::CellLimit) => Code::StoreLimit,
            RestoreFault::Body(Body::Aborted | Body::Indeterminate) => Code::StoreRestoreCommit,
            RestoreFault::Body(
                Body::NotEmpty | Body::Unordered | Body::OutsideNamespace | Body::Invalid(_),
            ) => Code::StoreCorruption,
            RestoreFault::Completion { .. } => Code::StoreActivationUncertain,
        }
    }

    /// The published store identity, only when this failure knows publication occurred.
    pub fn published_instance(&self) -> Option<StoreInstanceId> {
        match &self.fault {
            RestoreFault::Provision(
                ProvisionFault::PublicationUncertain { instance, .. }
                | ProvisionFault::ActivationUncertain { instance, .. },
            )
            | RestoreFault::Completion { instance, .. } => Some(*instance),
            _ => None,
        }
    }
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use marrow_kernel::durable::RestoreError as Body;
        match &self.fault {
            RestoreFault::Input(error) | RestoreFault::Body(Body::Input(error)) => {
                write!(f, "{error}")
            }
            RestoreFault::Image(error) => write!(f, "{error}"),
            RestoreFault::Admission(error) => write!(f, "{error}"),
            RestoreFault::Entropy(error) => write!(f, "{error}"),
            RestoreFault::Io(error) => write!(f, "restore failed: {error}"),
            RestoreFault::Provision(error) => write!(f, "{error}"),
            RestoreFault::Open(NativeOwnerOpenError::Lock(error)) => write!(f, "{error}"),
            RestoreFault::Open(NativeOwnerOpenError::Store(error))
            | RestoreFault::Body(Body::Store(error)) => write!(f, "{error}"),
            RestoreFault::Open(NativeOwnerOpenError::Refused(never)) => match *never {},
            RestoreFault::Body(Body::NotEmpty) => {
                write!(f, "restore construction body is not empty")
            }
            RestoreFault::Body(Body::CellLimit) => {
                write!(f, "backup cell exceeds its representation bound")
            }
            RestoreFault::Body(Body::Unordered) => {
                write!(f, "backup cells are not strictly ordered")
            }
            RestoreFault::Body(Body::OutsideNamespace) => write!(
                f,
                "backup contains a cell outside the admitted entry and index families"
            ),
            RestoreFault::Body(Body::Aborted) => write!(
                f,
                "restore batch aborted; earlier confirmed batches remain in the unpublished stage"
            ),
            RestoreFault::Body(Body::Indeterminate) => write!(
                f,
                "restore batch completion is indeterminate; preserve the unpublished stage without retrying"
            ),
            RestoreFault::Body(Body::Invalid(report)) => write!(
                f,
                "restored body audit found {} inconsistencies",
                report.summary.findings
            ),
            RestoreFault::Completion { instance, source } => write!(
                f,
                "store {} was published, but final active admission failed: {source}",
                instance.to_hex()
            ),
        }?;
        if let Some(path) = &self.stage {
            write!(
                f,
                "; possible unpublished stage retained at {}",
                path.display()
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for RestoreError {}

/// Restore into a fresh directory from the backup's own verified image and
/// accepted head. No current source is compiled and no embedded export executes.
pub fn restore(input: &mut dyn Read, destination: &Path) -> Result<RestoredStore, RestoreError> {
    restore_observed(input, destination, Seam::NONE)
}

/// [`restore`] over `seam`: the production seam observes nothing; a test's cuts or mutates
/// the sequence at a named step.
pub(crate) fn restore_observed(
    input: &mut dyn Read,
    destination: &Path,
    seam: Seam,
) -> Result<RestoredStore, RestoreError> {
    let (mut decoder, header) = Decoder::new(input).map_err(RestoreFault::Input)?;
    let image = marrow_verify::verify(&header.image).map_err(RestoreFault::Image)?;
    let (image, projection) = prepare(image).into_parts();
    let projection = projection.ok_or(RestoreFault::Admission(AuditError::NotExecutable))?;
    let (head, head_digest) = LogicalHead::decode_with_digest(&header.head)
        .map_err(|error| RestoreFault::Input(BackupReadError::Format(error)))?;
    let names = Names::new(&projection);
    let admission = ImageAdmission::derive(&image, projection);
    let layout = admission
        .admit(&head, BindingStrictness::Exact)
        .map_err(|error| RestoreFault::Admission(audit::open_error(AdmitError::Refused(error))))?;
    let instance = StoreInstanceId::draw().map_err(RestoreFault::Entropy)?;
    let envelope = StoreEnvelope {
        instance,
        writer_toolchain: env!("CARGO_PKG_VERSION").into(),
        engine_kind: EngineKind::Redb,
        engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
    };
    let pending = EnvelopeRecord {
        metadata: envelope.clone(),
        state: EnvelopeState::Provision { head: head_digest },
    }
    .encode()
    .map_err(|error| RestoreFault::Input(BackupReadError::Format(error)))?;
    let stage = temp_sibling(destination);
    let publication =
        Publication::admit(&stage, destination, seam.clone()).map_err(RestoreFault::Io)?;
    create_private_dir(&stage).map_err(|source| RestoreError {
        fault: RestoreFault::Io(source),
        stage: Some(stage.clone()),
    })?;
    let mut content = ChainDigest::new();
    let built = build_body(
        &stage,
        &pending,
        instance,
        layout,
        &mut decoder,
        &mut content,
        seam,
    );
    let (owner, directory, report) = built.map_err(|fault| RestoreError {
        fault,
        stage: Some(stage.clone()),
    })?;

    // Every byte and body commit is validated before Head can become visible.
    let installed = directory
        .write_new(Artifact::Head, &header.head)
        .and_then(|()| directory.sync(Step::ConstructionStage));
    installed.map_err(|error| RestoreError {
        fault: RestoreFault::Provision(ProvisionFault::Admission(error)),
        stage: Some(stage.clone()),
    })?;
    publication
        .publish(directory.identity())
        .map_err(|error| RestoreError {
            fault: RestoreFault::Io(custody_io(error)),
            stage: Some(stage.clone()),
        })?;
    complete_publication(destination, &publication, &directory, envelope.clone())
        .map_err(RestoreFault::Provision)?;
    directory
        .at(Step::FinalRead)
        .map_err(|error| RestoreFault::Completion {
            instance,
            source: AuditError::Open(crate::OpenError::Admission(error)),
        })?;
    audit::verify_published(
        &directory,
        destination,
        &EnvelopeRecord {
            metadata: envelope,
            state: EnvelopeState::Active,
        },
        head_digest,
    )
    .map_err(|source| RestoreFault::Completion { instance, source })?;
    let audit = StoreAudit::from_walk(
        instance,
        image.image_id(),
        &report,
        content.finish(),
        &names,
    );
    drop(owner);
    Ok(RestoredStore { audit })
}

fn build_body(
    stage: &Path,
    pending: &[u8],
    instance: StoreInstanceId,
    layout: NumberedProjection,
    decoder: &mut Decoder<'_>,
    content: &mut ChainDigest,
    seam: Seam,
) -> Result<
    (
        NativeStore,
        AdmittedStoreDir,
        marrow_kernel::durable::AuditReport,
    ),
    RestoreFault,
> {
    let owner = NativeStore::acquire_existing(stage)
        .map_err(|error| RestoreFault::Io(io::Error::other(error)))?;
    let directory = AdmittedStoreDir::admit_under_owner(&owner, seam)
        .map_err(|error| RestoreFault::Provision(ProvisionFault::Admission(error)))?;
    directory
        .write_new(Artifact::Envelope, pending)
        .map_err(|error| RestoreFault::Provision(ProvisionFault::Admission(error)))?;
    NativeStore::provision(stage)
        .map_err(|error| RestoreFault::Provision(ProvisionFault::Store(error)))?;
    directory
        .verify_location(stage)
        .map_err(|error| RestoreFault::Provision(ProvisionFault::Admission(error)))?;
    // Exact in-memory admission already passed, and these inputs are immutable.
    let (owner, report) = owner
        .restore(
            *instance.bytes(),
            || Ok::<_, Infallible>(layout),
            || decoder.next_cell(),
            content,
        )
        .map_err(|error| match error {
            NativeRestoreError::Open(error) => RestoreFault::Open(error),
            NativeRestoreError::Restore(error) => RestoreFault::Body(error),
        })?;
    Ok((owner, directory, report))
}

#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;
