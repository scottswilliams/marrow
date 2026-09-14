//! Complete logical backup under one retained source owner and read view.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use marrow_fs_journal::OpenedFile;
use marrow_image::StoreBackupDigest;
use marrow_kernel::durable::{ExportError, NativeOpenAccess};
use marrow_verify::VerifyRejection;

use crate::actor::ImageAdmission;
use crate::audit::{self, ChainDigest, Names};
use crate::backup_stream::{Encoder, StreamError};
use crate::durable_fs::{Publication, custody_io};
use crate::provision::{open_admitted, temp_sibling};
use crate::{AuditError, FormatError, StoreAudit, prepare};

/// A complete published backup and its source's logical audit. Physical source
/// checksums are not verified by the logical export.
#[derive(Debug)]
pub struct StoreBackup {
    pub audit: StoreAudit,
    pub digest: StoreBackupDigest,
}

#[derive(Debug)]
pub enum BackupFault {
    Image(VerifyRejection),
    Audit(AuditError),
    Invalid(Box<StoreAudit>),
    Format(FormatError),
    Io(io::Error),
    PublicationUncertain {
        destination: PathBuf,
        source: io::Error,
    },
}

/// Primary failure and independent custody of a possible unpublished file.
/// A creation failure can leave a file before returning its owner; that path is
/// retained without attempting deletion. Published output is never cleaned up.
#[derive(Debug)]
pub struct BackupError {
    pub fault: BackupFault,
    pub unpublished: Option<PathBuf>,
    pub cleanup: Option<io::Error>,
}

impl From<BackupFault> for BackupError {
    fn from(fault: BackupFault) -> Self {
        Self {
            fault,
            unpublished: None,
            cleanup: None,
        }
    }
}

impl BackupError {
    pub fn code(&self) -> &'static str {
        use marrow_codes::Code;
        match &self.fault {
            BackupFault::Image(error) => error.code(),
            BackupFault::Audit(error) => error.code(),
            BackupFault::Invalid(_) => Code::StoreCorruption.as_str(),
            BackupFault::Format(error) => error.code(),
            BackupFault::Io(_) => Code::StoreIo.as_str(),
            BackupFault::PublicationUncertain { .. } => Code::StorePublicationUncertain.as_str(),
        }
    }
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.fault {
            BackupFault::Image(error) => write!(f, "{error}"),
            BackupFault::Audit(error) => write!(f, "{error}"),
            BackupFault::Invalid(report) => write!(
                f,
                "source audit found {} inconsistencies",
                report.summary.findings
            ),
            BackupFault::Format(error) => write!(f, "backup output {error}"),
            BackupFault::Io(error) => write!(f, "backup output failed: {error}"),
            BackupFault::PublicationUncertain {
                destination,
                source,
            } => write!(
                f,
                "backup was published at {}, but publication durability is unconfirmed: {source}",
                destination.display()
            ),
        }?;
        if let Some(path) = &self.unpublished {
            write!(
                f,
                "; possible unpublished backup retained at {}",
                path.display()
            )?;
        }
        if let Some(error) = &self.cleanup {
            write!(f, "; cleanup failed: {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for BackupError {}

fn stream_fault(error: StreamError) -> BackupFault {
    match error {
        StreamError::Io(error) => BackupFault::Io(error),
        StreamError::Format(error) => BackupFault::Format(error),
    }
}

struct Output(OpenedFile);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.append(bytes).map_err(custody_io)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Appends are unbuffered. The owner performs the durable sync separately.
        Ok(())
    }
}

/// Verify the exact immutable image bytes, audit/export their active store in
/// one read view, and publish a complete backup without replacing any entry.
/// This operation neither compiles source nor executes an image export.
pub fn backup(
    source: &Path,
    image_bytes: &[u8],
    destination: &Path,
) -> Result<StoreBackup, BackupError> {
    let image = marrow_verify::verify(image_bytes).map_err(BackupFault::Image)?;
    let (image, projection) = prepare(image).into_parts();
    let projection = projection.ok_or(BackupFault::Audit(AuditError::NotExecutable))?;
    let names = Names::new(&projection);
    let admission = ImageAdmission::derive(&image, projection);
    let opened = open_admitted(source, NativeOpenAccess::ReadOnly, |head| {
        admission.admit_exact(head)
    })
    .map_err(|error| BackupFault::Audit(audit::open_error(error)))?;
    let stage = temp_sibling(destination);
    let publication = Publication::admit(&stage, destination).map_err(BackupFault::Io)?;
    let file = publication.create_file().map_err(|error| BackupError {
        fault: BackupFault::Io(custody_io(error)),
        unpublished: Some(stage.clone()),
        cleanup: None,
    })?;
    let identity = file.identity();
    let mut output = Output(file);
    let result = write_backup(&opened, &names, image.image_id(), image_bytes, &mut output);
    let result = result.and_then(|backup| {
        #[cfg(test)]
        tests::before_file_sync(&stage).map_err(BackupFault::Io)?;
        output
            .0
            .sync()
            .map_err(|error| BackupFault::Io(custody_io(error)))?;
        Ok(backup)
    });
    drop(output);
    let result = result.and_then(|backup| {
        publication
            .publish(identity)
            .map_err(|error| BackupFault::Io(custody_io(error)))?;
        Ok(backup)
    });
    let backup = match result {
        Ok(backup) => backup,
        Err(fault) => {
            return Err(match publication.remove_file(identity) {
                Ok(()) => fault.into(),
                Err(error) => BackupError {
                    fault,
                    unpublished: Some(stage),
                    cleanup: Some(custody_io(error)),
                },
            });
        }
    };
    publication.verify_destination(identity).map_err(|error| {
        BackupFault::PublicationUncertain {
            destination: destination.to_path_buf(),
            source: custody_io(error),
        }
    })?;
    #[cfg(test)]
    tests::before_parent_sync().map_err(|source| BackupFault::PublicationUncertain {
        destination: destination.to_path_buf(),
        source,
    })?;
    publication
        .sync()
        .map_err(|source| BackupFault::PublicationUncertain {
            destination: destination.to_path_buf(),
            source,
        })?;
    Ok(backup)
}

fn write_backup(
    opened: &crate::OpenStore,
    names: &Names,
    image_id: marrow_image::ImageId,
    image_bytes: &[u8],
    output: &mut dyn Write,
) -> Result<StoreBackup, BackupFault> {
    let (head, _) = opened.head.encode_with_digest();
    let mut encoder = Encoder::new(output, image_bytes, &head).map_err(stream_fault)?;
    let mut content = ChainDigest::new();
    let report = opened
        .export_cells(&mut content, &mut encoder)
        .map_err(|error| match error {
            ExportError::Read(error) => BackupFault::Audit(AuditError::Read(error)),
            ExportError::Output(error) => BackupFault::Io(error),
        })?;
    let audit = StoreAudit {
        instance: opened.envelope.instance,
        image_id,
        summary: report.summary,
        findings: report
            .findings
            .iter()
            .map(|finding| names.finding(finding))
            .collect(),
        digest: content.finish(),
    };
    if !audit.is_clean() {
        return Err(BackupFault::Invalid(Box::new(audit)));
    }
    let digest = encoder.finish().map_err(BackupFault::Io)?;
    Ok(StoreBackup { audit, digest })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{EngineKind, LogicalHead, ProvisionRequest, StoreEnvelope, StoreInstanceId};

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Failure {
        FileSync,
        ReplacedStage,
        ParentSync,
    }
    thread_local! {
        static FAILURE: std::cell::Cell<Option<Failure>> = const { std::cell::Cell::new(None) };
    }

    pub(super) fn before_file_sync(stage: &Path) -> io::Result<()> {
        match FAILURE.get() {
            Some(Failure::FileSync) => Err(io::ErrorKind::StorageFull.into()),
            Some(Failure::ReplacedStage) => {
                std::fs::rename(stage, stage.with_extension("preserved")).unwrap();
                std::fs::write(stage, b"replacement must survive").unwrap();
                Err(io::ErrorKind::StorageFull.into())
            }
            _ => Ok(()),
        }
    }

    pub(super) fn before_parent_sync() -> io::Result<()> {
        if FAILURE.get() == Some(Failure::ParentSync) {
            Err(io::ErrorKind::StorageFull.into())
        } else {
            Ok(())
        }
    }

    pub(crate) const SOURCE: &str = "resource Item { required value: int }\nstore ^items[key: int]: Item { index byValue[value] unique }\npub fn read(key: int): int { return ^items[key].value ?? 0 }\npub fn seed() { transaction { ^items[7] = Item(value: 42) } }\n";

    pub(crate) fn image_bytes() -> &'static [u8] {
        static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        BYTES.get_or_init(|| compile_image(SOURCE))
    }

    pub(crate) fn compile_image(source: &str) -> Vec<u8> {
        let ids = "marrow ids v0\nmachine-written by marrow; do not edit\nid application . 01010101010101010101010101010101\nid product Item 02020202020202020202020202020202\nid field Item.value 03030303030303030303030303030303\nid root items 04040404040404040404040404040404\nid key items.key 05050505050505050505050505050505\nid index items.byValue 06060606060606060606060606060606\nhigh-water 0\nend\n";
        let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").unwrap();
        let project = marrow_project::capture(
            &manifest,
            vec![marrow_project::CapturedFile::new(
                "src/main.mw".into(),
                source.as_bytes().to_vec(),
            )],
            Some(ids.as_bytes()),
            &marrow_project::CaptureLimits::DEFAULT,
        )
        .unwrap();
        marrow_compile::compile(&project).unwrap().image.bytes
    }

    pub(crate) struct Scratch(pub(crate) PathBuf);
    impl Scratch {
        pub(crate) fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "marrow-logical-backup-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        pub(crate) fn source(&self) -> PathBuf {
            self.0.join("store")
        }
        pub(crate) fn provision(&self) {
            let image = marrow_verify::verify(image_bytes()).unwrap();
            crate::provision(
                &self.source(),
                ProvisionRequest {
                    envelope: StoreEnvelope {
                        instance: StoreInstanceId::draw().unwrap(),
                        writer_toolchain: env!("CARGO_PKG_VERSION").into(),
                        engine_kind: EngineKind::Redb,
                        engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
                    },
                    head: LogicalHead::provision(
                        crate::active_binding(&image),
                        crate::accepted_ceiling(&image),
                        crate::head_map(&image).unwrap(),
                    ),
                },
            )
            .unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn published_empty_backup_contains_exact_image_and_accepted_head() {
        let scratch = Scratch::new();
        scratch.provision();
        let before_head = std::fs::read(scratch.source().join(crate::HEAD_FILE)).unwrap();
        let before_envelope = std::fs::read(scratch.source().join(crate::ENVELOPE_FILE)).unwrap();
        let destination = scratch.0.join("backup");
        let result = backup(&scratch.source(), image_bytes(), &destination).unwrap();
        assert!(result.audit.is_clean());
        assert_eq!(result.audit.summary.cells, 0);
        let bytes = std::fs::read(&destination).unwrap();
        let mut cursor = io::Cursor::new(&bytes);
        let (mut decoder, header) = crate::backup_stream::Decoder::new(&mut cursor).unwrap();
        assert_eq!(header.image, image_bytes());
        assert_eq!(header.head, before_head);
        assert!(decoder.next_cell().unwrap().is_none());
        assert_eq!(&bytes[bytes.len() - 32..], result.digest.bytes());
        assert_eq!(
            std::fs::read(scratch.source().join(crate::HEAD_FILE)).unwrap(),
            before_head
        );
        assert_eq!(
            std::fs::read(scratch.source().join(crate::ENVELOPE_FILE)).unwrap(),
            before_envelope
        );
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 2);
    }

    #[test]
    fn occupied_output_is_unchanged_and_its_private_candidate_is_removed() {
        let scratch = Scratch::new();
        scratch.provision();
        let destination = scratch.0.join("backup");
        std::fs::write(&destination, b"existing backup").unwrap();
        let error = backup(&scratch.source(), image_bytes(), &destination).unwrap_err();
        assert!(
            matches!(error.fault, BackupFault::Io(source) if source.kind() == io::ErrorKind::AlreadyExists)
        );
        assert!(error.unpublished.is_none());
        assert!(error.cleanup.is_none());
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing backup");
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 2);
    }

    #[test]
    fn held_source_and_invalid_image_refuse_before_output_creation() {
        let scratch = Scratch::new();
        scratch.provision();
        let destination = scratch.0.join("backup");
        let image = marrow_verify::verify(image_bytes()).unwrap();
        let held = crate::attach(&scratch.source(), prepare(image)).unwrap();
        let error = backup(&scratch.source(), image_bytes(), &destination).unwrap_err();
        assert!(matches!(error.fault, BackupFault::Audit(_)));
        assert!(error.unpublished.is_none());
        drop(held);
        assert!(matches!(
            backup(&scratch.source(), b"invalid", &destination)
                .unwrap_err()
                .fault,
            BackupFault::Image(_)
        ));
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_barriers_and_cleanup_identity_preserve_the_actual_outcome() {
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                FAILURE.set(None);
            }
        }
        for failure in [
            Failure::FileSync,
            Failure::ReplacedStage,
            Failure::ParentSync,
        ] {
            let scratch = std::mem::ManuallyDrop::new(Scratch::new());
            eprintln!("preserved backup failure fixture: {}", scratch.0.display());
            scratch.provision();
            let destination = scratch.0.join("backup");
            let _clear = Clear;
            FAILURE.set(Some(failure));
            let error = backup(&scratch.source(), image_bytes(), &destination).unwrap_err();
            match failure {
                Failure::FileSync => {
                    assert!(
                        matches!(error.fault, BackupFault::Io(source) if source.kind() == io::ErrorKind::StorageFull)
                    );
                    assert!(error.unpublished.is_none());
                    assert!(error.cleanup.is_none());
                    assert!(!destination.exists());
                    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);
                }
                Failure::ReplacedStage => {
                    assert!(
                        matches!(error.fault, BackupFault::Io(source) if source.kind() == io::ErrorKind::StorageFull)
                    );
                    assert!(error.cleanup.is_some());
                    let stage = error.unpublished.unwrap();
                    assert_eq!(std::fs::read(&stage).unwrap(), b"replacement must survive");
                    assert!(stage.with_extension("preserved").is_file());
                    assert!(!destination.exists());
                }
                Failure::ParentSync => {
                    assert!(
                        matches!(error.fault, BackupFault::PublicationUncertain { destination: ref found, ref source } if found == &destination && source.kind() == io::ErrorKind::StorageFull)
                    );
                    assert!(error.unpublished.is_none());
                    assert!(error.cleanup.is_none());
                    let bytes = std::fs::read(&destination).unwrap();
                    let mut input = io::Cursor::new(bytes);
                    let (mut decoder, _) = crate::backup_stream::Decoder::new(&mut input).unwrap();
                    assert!(decoder.next_cell().unwrap().is_none());
                    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 2);
                }
            }
        }
    }
}
