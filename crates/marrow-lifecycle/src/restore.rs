//! Fresh-store construction from complete, verified logical backup input.
//! Head remains absent until the private body passes all transfer and audit gates.

use std::convert::Infallible;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use marrow_kernel::durable::{
    NativeOwnerOpenError, NativeRestoreError, NativeStore, StoreProjection,
};
use marrow_verify::VerifyRejection;

use crate::actor::ImageAdmission;
use crate::audit::{self, ChainDigest, Names};
use crate::backup_stream::Decoder;
use crate::durable_fs::{Publication, custody_io};
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::provision::{AdmitError, complete_publication, create_private_dir, temp_sibling};
use crate::store_dir::{AdmittedStoreDir, Artifact};
use crate::{
    AuditError, BackupReadError, EngineKind, EntropyUnavailable, LogicalHead, ProvisionFault,
    StoreAudit, StoreEnvelope, StoreInstanceId, prepare,
};

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

    pub fn code(&self) -> &'static str {
        use marrow_codes::Code;
        use marrow_kernel::durable::RestoreError as Body;
        match &self.fault {
            RestoreFault::Input(error) | RestoreFault::Body(Body::Input(error)) => error.code(),
            RestoreFault::Image(error) => error.code(),
            RestoreFault::Admission(error) => error.code(),
            RestoreFault::Entropy(_) => Code::IoRead.as_str(),
            RestoreFault::Io(_) => Code::StoreIo.as_str(),
            RestoreFault::Provision(error) => error.code(),
            RestoreFault::Open(NativeOwnerOpenError::Lock(error)) => error.code(),
            RestoreFault::Open(NativeOwnerOpenError::Store(error))
            | RestoreFault::Body(Body::Store(error)) => error.code(),
            RestoreFault::Open(NativeOwnerOpenError::Refused(never)) => match *never {},
            RestoreFault::Body(Body::CellLimit) => Code::StoreLimit.as_str(),
            RestoreFault::Body(Body::Aborted | Body::Indeterminate) => {
                Code::StoreRestoreCommit.as_str()
            }
            RestoreFault::Body(
                Body::NotEmpty | Body::Unordered | Body::OutsideNamespace | Body::Invalid(_),
            ) => Code::StoreCorruption.as_str(),
            RestoreFault::Completion { .. } => Code::StoreActivationUncertain.as_str(),
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
    let (mut decoder, header) = Decoder::new(input).map_err(RestoreFault::Input)?;
    let image = marrow_verify::verify(&header.image).map_err(RestoreFault::Image)?;
    let (image, projection) = prepare(image).into_parts();
    let projection = projection.ok_or(RestoreFault::Admission(AuditError::NotExecutable))?;
    let (head, head_digest) = LogicalHead::decode_with_digest(&header.head)
        .map_err(|error| RestoreFault::Input(BackupReadError::Format(error)))?;
    let admission = ImageAdmission::derive(&image, &projection);
    admission
        .admit_exact(&head)
        .map_err(|error| RestoreFault::Admission(audit::open_error(AdmitError::Refused(error))))?;
    let names = Names::new(&projection);
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
    let publication = Publication::admit(&stage, destination).map_err(RestoreFault::Io)?;
    create_private_dir(&stage).map_err(|source| RestoreError {
        fault: RestoreFault::Io(source),
        stage: Some(stage.clone()),
    })?;
    let mut content = ChainDigest::new();
    let built = build_body(
        &stage,
        &pending,
        instance,
        projection,
        &mut decoder,
        &mut content,
    );
    let (owner, directory, report) = built.map_err(|fault| RestoreError {
        fault,
        stage: Some(stage.clone()),
    })?;

    // Every byte and body commit is validated before Head can become visible.
    let installed = directory
        .write_new(Artifact::Head, &header.head)
        .and_then(|()| directory.sync());
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
    #[cfg(test)]
    tests::before_final_admission(&directory);
    audit::admit_published(
        &directory,
        destination,
        &EnvelopeRecord {
            metadata: envelope,
            state: EnvelopeState::Active,
        },
        head_digest,
        &admission,
    )
    .map_err(|source| RestoreFault::Completion { instance, source })?;
    let audit = StoreAudit {
        instance,
        image_id: image.image_id(),
        summary: report.summary,
        findings: report
            .findings
            .iter()
            .map(|finding| names.finding(finding))
            .collect(),
        digest: content.finish(),
    };
    drop(owner);
    Ok(RestoredStore { audit })
}

fn build_body(
    stage: &Path,
    pending: &[u8],
    instance: StoreInstanceId,
    projection: StoreProjection,
    decoder: &mut Decoder<'_>,
    content: &mut ChainDigest,
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
    let directory = AdmittedStoreDir::admit_under_owner(&owner)
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
            projection,
            || Ok::<_, Infallible>(()),
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
mod tests {
    use super::*;
    use crate::backup::tests::{Scratch, image_bytes};

    #[test]
    fn batch_failure_preserves_completion_fact_without_claiming_publication() {
        use marrow_kernel::durable::RestoreError as Body;
        for (body, outcome) in [
            (Body::Aborted, RestoreBatchOutcome::Aborted),
            (Body::Indeterminate, RestoreBatchOutcome::Indeterminate),
        ] {
            let error = RestoreError {
                fault: RestoreFault::Body(body),
                stage: Some(PathBuf::from("private-stage")),
            };
            assert_eq!(
                error.code(),
                marrow_codes::Code::StoreRestoreCommit.as_str()
            );
            assert_eq!(error.batch_outcome(), Some(outcome));
            assert_eq!(error.published_instance(), None);
        }
    }

    thread_local! {
        static FINAL_CHANGE: std::cell::Cell<Option<Artifact>> = const { std::cell::Cell::new(None) };
    }

    pub(super) fn before_final_admission(directory: &AdmittedStoreDir) {
        let Some(artifact) = FINAL_CHANGE.take() else {
            return;
        };
        let bytes = match artifact {
            Artifact::Envelope => {
                let mut record = crate::provision::decode_record(directory).unwrap();
                record.metadata.writer_toolchain.push_str("-changed");
                record.encode().unwrap()
            }
            Artifact::Head => {
                let (mut head, _) = crate::provision::decode_head(directory).unwrap();
                head.binding.image_id[0] ^= 1;
                head.encode()
            }
        };
        directory.replace(artifact, &bytes).unwrap();
    }

    #[test]
    fn final_admission_rejects_changed_metadata_and_retains_published_store() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                FINAL_CHANGE.set(None);
            }
        }
        for artifact in [Artifact::Envelope, Artifact::Head] {
            let scratch = std::mem::ManuallyDrop::new(Scratch::new());
            eprintln!(
                "preserved final-admission restore fixture: {}",
                scratch.0.display()
            );
            scratch.provision();
            let backup = scratch.0.join("backup");
            crate::backup(&scratch.source(), image_bytes(), &backup).unwrap();
            let destination = scratch.0.join("restored");
            FINAL_CHANGE.set(Some(artifact));
            let _reset = Reset;
            let error =
                restore(&mut std::fs::File::open(backup).unwrap(), &destination).unwrap_err();
            let record = EnvelopeRecord::decode(
                &std::fs::read(destination.join(crate::ENVELOPE_FILE)).unwrap(),
            )
            .unwrap();
            assert!(matches!(error.fault,
                RestoreFault::Completion {
                    instance,
                    source: AuditError::Open(crate::OpenError::Corruption { .. }),
                } if instance == record.metadata.instance));
            assert!(error.stage.is_none());
            assert_eq!(record.state, EnvelopeState::Active);
            assert!(destination.join(crate::HEAD_FILE).is_file());
            assert!(FINAL_CHANGE.get().is_none());
        }
    }

    #[test]
    fn restoring_empty_content_does_not_execute_embedded_seed() {
        let scratch = Scratch::new();
        scratch.provision();
        let artifact = scratch.0.join("backup");
        crate::backup(&scratch.source(), image_bytes(), &artifact).unwrap();
        let destination = scratch.0.join("restored");
        let restored = restore(&mut std::fs::File::open(artifact).unwrap(), &destination).unwrap();
        assert_eq!(restored.audit.summary.entries, 0);
        let image = marrow_verify::verify(image_bytes()).unwrap();
        assert_eq!(
            crate::audit(&destination, prepare(image))
                .unwrap()
                .summary
                .entries,
            0
        );
    }

    #[test]
    fn occupied_destination_retains_completed_unpublished_stage() {
        let scratch = std::mem::ManuallyDrop::new(Scratch::new());
        eprintln!(
            "preserved restore collision fixture: {}",
            scratch.0.display()
        );
        scratch.provision();
        let artifact = scratch.0.join("backup");
        crate::backup(&scratch.source(), image_bytes(), &artifact).unwrap();
        let destination = scratch.0.join("occupied");
        std::fs::create_dir(&destination).unwrap();
        let metadata = std::fs::metadata(&destination).unwrap();
        let error = restore(&mut std::fs::File::open(artifact).unwrap(), &destination).unwrap_err();
        assert!(
            matches!(error.fault, RestoreFault::Io(ref error) if error.kind() == io::ErrorKind::AlreadyExists)
        );
        let stage = error.stage.unwrap();
        assert!(destination.is_dir());
        assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                metadata.ino(),
                std::fs::metadata(&destination).unwrap().ino()
            );
        }
        #[cfg(not(unix))]
        let _ = metadata;
        let record =
            EnvelopeRecord::decode(&std::fs::read(stage.join(crate::ENVELOPE_FILE)).unwrap())
                .unwrap();
        let (_, digest) =
            LogicalHead::decode_with_digest(&std::fs::read(stage.join(crate::HEAD_FILE)).unwrap())
                .unwrap();
        assert_eq!(record.state, EnvelopeState::Provision { head: digest });
    }

    #[test]
    fn publication_and_activation_barrier_failures_retain_destination_and_instance() {
        use crate::provision::publication_sync_fault::{Point, with_failure};
        for point in [Point::Publication, Point::Activation] {
            let scratch = std::mem::ManuallyDrop::new(Scratch::new());
            eprintln!("preserved restore barrier fixture: {}", scratch.0.display());
            scratch.provision();
            let artifact = scratch.0.join("backup");
            crate::backup(&scratch.source(), image_bytes(), &artifact).unwrap();
            let destination = scratch.0.join("restored");
            let error = with_failure(&destination, point, || {
                restore(&mut std::fs::File::open(artifact).unwrap(), &destination)
            })
            .unwrap_err();
            assert!(error.stage.is_none());
            let record = EnvelopeRecord::decode(
                &std::fs::read(destination.join(crate::ENVELOPE_FILE)).unwrap(),
            )
            .unwrap();
            let (_, digest) = LogicalHead::decode_with_digest(
                &std::fs::read(destination.join(crate::HEAD_FILE)).unwrap(),
            )
            .unwrap();
            let instance = match (point, error.fault) {
                (
                    Point::Publication,
                    RestoreFault::Provision(ProvisionFault::PublicationUncertain {
                        instance, ..
                    }),
                ) => {
                    assert_eq!(record.state, EnvelopeState::Provision { head: digest });
                    instance
                }
                (
                    Point::Activation,
                    RestoreFault::Provision(ProvisionFault::ActivationUncertain {
                        instance, ..
                    }),
                ) => {
                    assert_eq!(record.state, EnvelopeState::Active);
                    instance
                }
                (_, fault) => panic!("unexpected barrier result: {fault:?}"),
            };
            assert_eq!(record.metadata.instance, instance);
        }
    }

    #[test]
    fn truncated_transfer_has_no_head_and_no_ordinary_or_recovery_admission() {
        let scratch = std::mem::ManuallyDrop::new(Scratch::new());
        eprintln!(
            "preserved truncated restore fixture: {}",
            scratch.0.display()
        );
        scratch.provision();
        let artifact = scratch.0.join("backup");
        crate::backup(&scratch.source(), image_bytes(), &artifact).unwrap();
        let mut bytes = std::fs::read(artifact).unwrap();
        bytes.pop();
        let destination = scratch.0.join("restored");
        let error = restore(&mut io::Cursor::new(bytes), &destination).unwrap_err();
        assert!(matches!(
            error.fault,
            RestoreFault::Body(marrow_kernel::durable::RestoreError::Input(
                BackupReadError::Format(crate::FormatError::Truncated)
            ))
        ));
        let stage = error.stage.unwrap();
        assert!(!stage.join(crate::HEAD_FILE).exists());
        assert!(!destination.exists());
        let image = marrow_verify::verify(image_bytes()).unwrap();
        assert!(matches!(
            crate::attach(&stage, prepare(image.clone())),
            Err(crate::LifecycleError::Open(crate::OpenError::Incomplete))
        ));
        let error = crate::recover(&stage, prepare(image)).unwrap_err();
        assert!(matches!(
            error.fault,
            crate::RecoveryFault::Validation(AuditError::Open(crate::OpenError::Incomplete))
        ));
    }
}
