//! Complete logical backup under one retained source owner and read view.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use marrow_fs_journal::OpenedFile;
use marrow_image::StoreBackupDigest;
use marrow_kernel::durable::{ExportError, NativeOpenAccess};
use marrow_verify::VerifyRejection;

use crate::actor::{BindingStrictness, ImageAdmission};
use crate::audit::{self, ChainDigest, Names};
use crate::backup_stream::{Encoder, StreamError};
use crate::durable_fs::{Publication, custody_io};
use crate::provision::{open_admitted, temp_sibling};
use crate::seam::{Event, Seam};
use crate::{AuditError, FormatError, StoreAudit, prepare};
use marrow_codes::Code;

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
    pub fn code(&self) -> Code {
        match &self.fault {
            BackupFault::Image(error) => error.code(),
            BackupFault::Audit(error) => error.code(),
            BackupFault::Invalid(_) => Code::StoreCorruption,
            BackupFault::Format(error) => error.code(),
            BackupFault::Io(_) => Code::StoreIo,
            BackupFault::PublicationUncertain { .. } => Code::StorePublicationUncertain,
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
    backup_observed(source, image_bytes, destination, Seam::NONE)
}

/// [`backup`] over `seam`: the production seam observes nothing; a test's cuts or mutates
/// the sequence at a named point.
pub(crate) fn backup_observed(
    source: &Path,
    image_bytes: &[u8],
    destination: &Path,
    seam: Seam,
) -> Result<StoreBackup, BackupError> {
    let image = marrow_verify::verify(image_bytes).map_err(BackupFault::Image)?;
    let (image, projection) = prepare(image).into_parts();
    let projection = projection.ok_or(BackupFault::Audit(AuditError::NotExecutable))?;
    let names = Names::new(&projection);
    let admission = ImageAdmission::derive(&image, projection);
    let opened = open_admitted(source, NativeOpenAccess::ReadOnly, seam.clone(), |head| {
        admission.admit(head, BindingStrictness::Exact)
    })
    .map_err(|error| BackupFault::Audit(audit::open_error(error)))?;
    let stage = temp_sibling(destination);
    let publication =
        Publication::admit(&stage, destination, seam.clone()).map_err(BackupFault::Io)?;
    let file = publication.create_file().map_err(|error| BackupError {
        fault: BackupFault::Io(custody_io(error)),
        unpublished: Some(stage.clone()),
        cleanup: None,
    })?;
    let identity = file.identity();
    let mut output = Output(file);
    let result = write_backup(&opened, &names, image.image_id(), image_bytes, &mut output);
    let result = result.and_then(|backup| {
        seam.at(Event::OutputSync { stage: &stage })
            .and_then(|()| output.0.sync())
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
    let audit = StoreAudit::from_walk(
        opened.envelope.instance,
        image_id,
        &report,
        content.finish(),
        names,
    );
    if !audit.is_clean() {
        return Err(BackupFault::Invalid(Box::new(audit)));
    }
    let digest = encoder.finish().map_err(BackupFault::Io)?;
    Ok(StoreBackup { audit, digest })
}

#[cfg(test)]
#[path = "backup_tests.rs"]
pub(crate) mod tests;
