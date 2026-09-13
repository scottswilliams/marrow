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

impl From<RestoreFault> for RestoreError {
    fn from(fault: RestoreFault) -> Self {
        Self { fault, stage: None }
    }
}

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
