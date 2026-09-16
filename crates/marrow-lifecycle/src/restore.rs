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
        layout,
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
    layout: NumberedProjection,
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
mod tests {
    use super::*;
    use crate::backup::tests::{Scratch, image_bytes, provision_fixture};
    use crate::backup_stream::{Encoder, Header};
    use marrow_kernel::durable::{Cell, ExportSink};

    fn populated_transfer(scratch: &Scratch, count: i64) -> (Header, Vec<Cell>) {
        use marrow_kernel::codec::{key::KeyScalar, value::RuntimeScalar};
        use marrow_kernel::durable::{DemandCoverage, Durable, EntryValue, InvocationGrant};
        use marrow_kernel::equality::ValueDomain;
        provision_fixture(scratch);
        let image = marrow_verify::verify(image_bytes()).unwrap();
        let write_site = image
            .sites()
            .iter()
            .position(|site| {
                matches!(
                    site,
                    marrow_verify::SealedSite::Flat {
                        root: 0,
                        target: marrow_verify::SealedSiteTarget::WholePayload
                    }
                )
            })
            .unwrap() as u16;
        let crate::AttachOutcome::AlreadyActive(mut attachment) =
            crate::attach(&scratch.store(), prepare(image)).unwrap()
        else {
            panic!("active fixture")
        };
        let (_, host) = attachment.bridge();
        let mut txn = host
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .unwrap();
        let site = txn.site(write_site);
        for key in 0..count {
            txn.create_entry(
                &site,
                &[KeyScalar::Int(key)],
                EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(key + 1000)))],
                    groups: Vec::new(),
                },
            )
            .unwrap();
        }
        assert!(matches!(
            txn.commit(),
            marrow_kernel::durable::CommitResult::Committed
        ));
        drop(txn);
        drop(attachment);
        let path = scratch.base().join("source.backup");
        let backup = crate::backup(&scratch.store(), image_bytes(), &path).unwrap();
        assert_eq!(backup.audit.summary.entries, count as u64);
        assert_eq!(backup.audit.summary.index_cells, count as u64);
        decode_transfer(&std::fs::read(path).unwrap())
    }

    fn decode_transfer(bytes: &[u8]) -> (Header, Vec<Cell>) {
        let mut input = io::Cursor::new(bytes);
        let (mut decoder, header) = Decoder::new(&mut input).unwrap();
        let mut cells = Vec::new();
        while let Some(cell) = decoder.next_cell().unwrap() {
            cells.push(cell);
        }
        (header, cells)
    }

    fn encode_transfer(header: &Header, cells: &[Cell]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = Encoder::new(&mut bytes, &header.image, &header.head).unwrap();
        for (key, value) in cells {
            encoder.cell(key, value).unwrap();
        }
        encoder.finish().unwrap();
        // A semantic refusal must not be masked by bad framing or a stale digest.
        let (decoded, actual) = decode_transfer(&bytes);
        assert_eq!(decoded.image, header.image);
        assert_eq!(decoded.head, header.head);
        assert_eq!(actual, cells);
        bytes
    }

    fn assert_headless_refusal(stage: &Path) {
        assert!(!stage.join(crate::HEAD_FILE).exists());
        let image = marrow_verify::verify(image_bytes()).unwrap();
        assert!(matches!(
            crate::attach(stage, prepare(image.clone())),
            Err(crate::LifecycleError::Open(crate::OpenError::Incomplete))
        ));
        assert!(matches!(
            crate::recover(stage, prepare(image)).unwrap_err().fault,
            crate::RecoveryFault::Validation(AuditError::Open(crate::OpenError::Incomplete))
        ));
    }

    #[test]
    fn populated_truncation_retains_headless_stage_and_refuses_service() {
        let scratch = std::mem::ManuallyDrop::new(Scratch::new("restore"));
        eprintln!(
            "preserved populated truncated transfer: {}",
            scratch.base().display()
        );
        let (header, cells) = populated_transfer(&scratch, 130);
        assert_eq!(cells.len(), 390);
        let mut bytes = encode_transfer(&header, &cells);
        bytes.pop();
        let destination = scratch.base().join("restored");
        let error = restore(&mut io::Cursor::new(bytes), &destination).unwrap_err();
        assert!(matches!(
            error.fault,
            RestoreFault::Body(marrow_kernel::durable::RestoreError::Input(
                BackupReadError::Format(crate::FormatError::Truncated)
            ))
        ));
        assert!(!destination.exists());
        assert_headless_refusal(&error.stage.unwrap());
    }

    #[test]
    fn rehashed_witness_and_missing_or_stale_index_refuse_before_head() {
        use marrow_kernel::durable::{AuditFault, RestoreError as Body};
        let scratch = std::mem::ManuallyDrop::new(Scratch::new("restore"));
        eprintln!(
            "preserved semantic transfer refusals: {}",
            scratch.base().display()
        );
        let (header, original) = populated_transfer(&scratch, 3);
        let mut witness = original.clone();
        // Canonical v1 generation-zero witness from the kernel's witness codec.
        // This fixed hostile artifact is valid metadata, never application content.
        witness.push((b"\x10witness\0\0".to_vec(), [vec![1], vec![0; 16]].concat()));
        let mut missing = original.clone();
        let last = missing.pop().unwrap();
        assert_eq!(last.0[0], 2, "fixture's final cell is a managed index");
        let mut stale = original.clone();
        let first_index_value = original
            .iter()
            .find(|(key, _)| key[0] == 2)
            .unwrap()
            .1
            .clone();
        stale.last_mut().unwrap().1 = first_index_value;
        for (name, cells, finding) in [
            ("witness", witness, None),
            ("missing", missing, Some(AuditFault::IndexMissing)),
            ("stale", stale, Some(AuditFault::IndexStale)),
        ] {
            let bytes = encode_transfer(&header, &cells);
            let destination = scratch.base().join(name);
            let error = restore(&mut io::Cursor::new(bytes), &destination).unwrap_err();
            match (error.fault, finding) {
                (RestoreFault::Body(Body::OutsideNamespace), None) => {}
                (RestoreFault::Body(Body::Invalid(report)), Some(finding)) => {
                    assert!(report.findings.iter().any(|actual| actual.fault == finding))
                }
                (fault, expected) => panic!("{name}: {fault:?}, expected {expected:?}"),
            }
            assert!(!destination.exists());
            assert_headless_refusal(&error.stage.unwrap());
        }
    }

    #[test]
    fn rehashed_incompatible_headers_refuse_before_private_construction() {
        use crate::backup::tests::{SOURCE, compile_image};
        let scratch = Scratch::new("restore");
        let (header, cells) = populated_transfer(&scratch, 0);
        let other = compile_image(&SOURCE.replace("42", "43"));
        assert_ne!(
            marrow_verify::verify(&other).unwrap().image_id(),
            marrow_verify::verify(&header.image).unwrap().image_id()
        );
        let mut old_head = header.head.clone();
        old_head[4] = 1;
        let body_len = old_head.len() - 32;
        let digest = marrow_image::StoreHeadDigest::compute(&old_head[..body_len]);
        old_head[body_len..].copy_from_slice(digest.bytes());
        let before: std::collections::BTreeSet<_> = std::fs::read_dir(scratch.base())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        for (name, image, head) in [
            ("binding", other, header.head.clone()),
            ("generation", header.image.clone(), old_head),
            ("image", b"invalid image".to_vec(), header.head),
        ] {
            let bytes = encode_transfer(&Header { image, head }, &cells);
            let destination = scratch.base().join(name);
            let error = restore(&mut io::Cursor::new(bytes), &destination).unwrap_err();
            match (name, &error.fault) {
                ("binding", RestoreFault::Admission(AuditError::ImageNotActive))
                | (
                    "generation",
                    RestoreFault::Input(BackupReadError::Format(
                        crate::FormatError::UnknownVersion { found: 1 },
                    )),
                )
                | ("image", RestoreFault::Image(_)) => {}
                _ => panic!("{name}: {error:?}"),
            }
            assert!(error.stage.is_none());
            assert!(!destination.exists());
            let after: std::collections::BTreeSet<_> = std::fs::read_dir(scratch.base())
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(after, before);
        }
    }

    #[test]
    fn fresh_populated_publication_prefix_recovers_without_replaying_construction() {
        let scratch = Scratch::new("restore");
        let (header, cells) = populated_transfer(&scratch, 70);
        let bytes = encode_transfer(&header, &cells);
        let image = marrow_verify::verify(&header.image).unwrap();
        let (image, projection) = prepare(image).into_parts();
        let projection = projection.unwrap();
        let (head, digest) = LogicalHead::decode_with_digest(&header.head).unwrap();
        let layout = ImageAdmission::derive(&image, projection)
            .admit(&head, BindingStrictness::Exact)
            .unwrap_or_else(|_| panic!("fixture head admitted"));
        let instance = StoreInstanceId::draw().unwrap();
        let envelope = StoreEnvelope {
            instance,
            writer_toolchain: env!("CARGO_PKG_VERSION").into(),
            engine_kind: EngineKind::Redb,
            engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
        };
        let pending = EnvelopeRecord {
            metadata: envelope,
            state: EnvelopeState::Provision { head: digest },
        }
        .encode()
        .unwrap();
        let stage = scratch.base().join("complete-stage");
        let destination = scratch.base().join("published");
        let publication = Publication::admit(&stage, &destination).unwrap();
        create_private_dir(&stage).unwrap();
        let mut input = io::Cursor::new(&bytes);
        let (mut decoder, _) = Decoder::new(&mut input).unwrap();
        let (owner, directory, report) = build_body(
            &stage,
            &pending,
            instance,
            layout,
            &mut decoder,
            &mut ChainDigest::new(),
        )
        .unwrap();
        assert_eq!(report.summary.entries, 70);
        assert_eq!(report.summary.index_cells, 70);
        assert!(!stage.join(crate::HEAD_FILE).exists());
        directory.write_new(Artifact::Head, &header.head).unwrap();
        directory.sync().unwrap();
        publication.publish(directory.identity()).unwrap();
        publication.sync().unwrap();
        // This fresh positive fixture deliberately stops at the supported
        // published Provision prefix. It is not a reopened archived failure,
        // and orderly owner release is not an OS-crash durability experiment.
        drop(directory);
        drop(owner);
        drop(publication);
        assert!(matches!(
            crate::attach(&destination, prepare((*image).clone())),
            Err(crate::LifecycleError::Open(
                crate::OpenError::ActivationRequired { .. }
            ))
        ));
        let recovered = crate::recover(&destination, prepare((*image).clone())).unwrap();
        assert_eq!(recovered.instance, instance);
        assert_eq!(recovered.image_id, image.image_id());
        assert_eq!(
            std::fs::read(destination.join(crate::HEAD_FILE)).unwrap(),
            header.head
        );
        assert_eq!(
            crate::provision::decode_record(&AdmittedStoreDir::admit(&destination).unwrap())
                .unwrap()
                .state,
            EnvelopeState::Active
        );
        let artifact = scratch.base().join("recovered.backup");
        let backup = crate::backup(&destination, &header.image, &artifact).unwrap();
        assert_eq!(backup.audit.instance, instance);
        assert_eq!(std::fs::read(artifact).unwrap(), bytes);
        assert!(matches!(
            crate::attach(&destination, prepare((*image).clone())).unwrap(),
            crate::AttachOutcome::AlreadyActive(_)
        ));
    }

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
            assert_eq!(error.code(), marrow_codes::Code::StoreRestoreCommit);
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
            let scratch = std::mem::ManuallyDrop::new(Scratch::new("restore"));
            eprintln!(
                "preserved final-admission restore fixture: {}",
                scratch.base().display()
            );
            provision_fixture(&scratch);
            let backup = scratch.base().join("backup");
            crate::backup(&scratch.store(), image_bytes(), &backup).unwrap();
            let destination = scratch.base().join("restored");
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
        let scratch = Scratch::new("restore");
        provision_fixture(&scratch);
        let artifact = scratch.base().join("backup");
        crate::backup(&scratch.store(), image_bytes(), &artifact).unwrap();
        let destination = scratch.base().join("restored");
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
        let scratch = std::mem::ManuallyDrop::new(Scratch::new("restore"));
        eprintln!(
            "preserved restore collision fixture: {}",
            scratch.base().display()
        );
        provision_fixture(&scratch);
        let artifact = scratch.base().join("backup");
        crate::backup(&scratch.store(), image_bytes(), &artifact).unwrap();
        let destination = scratch.base().join("occupied");
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
            let scratch = std::mem::ManuallyDrop::new(Scratch::new("restore"));
            eprintln!(
                "preserved restore barrier fixture: {}",
                scratch.base().display()
            );
            provision_fixture(&scratch);
            let artifact = scratch.base().join("backup");
            crate::backup(&scratch.store(), image_bytes(), &artifact).unwrap();
            let destination = scratch.base().join("restored");
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
        let scratch = std::mem::ManuallyDrop::new(Scratch::new("restore"));
        eprintln!(
            "preserved truncated restore fixture: {}",
            scratch.base().display()
        );
        provision_fixture(&scratch);
        let artifact = scratch.base().join("backup");
        crate::backup(&scratch.store(), image_bytes(), &artifact).unwrap();
        let mut bytes = std::fs::read(artifact).unwrap();
        bytes.pop();
        let destination = scratch.base().join("restored");
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
