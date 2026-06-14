use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use marrow_check::CheckedProgram;
use marrow_store::tree::TreeStore;

use super::{BackupError, create_backup, validate_backup_archive};

pub(crate) struct BackupArtifactReport {
    pub(crate) record_count: u64,
}

pub(crate) fn create_backup_artifact(
    program: &CheckedProgram,
    store: &TreeStore,
    output_path: &Path,
) -> Result<BackupArtifactReport, BackupError> {
    let (temp_path, file) = create_temp_artifact(output_path).map_err(|error| {
        backup_io(
            error.kind(),
            format!(
                "could not create temporary backup for {}: {error}",
                output_path.display()
            ),
        )
    })?;
    match write_and_validate_temp_backup(program, store, output_path, &temp_path, file) {
        Ok(report) => {
            if let Err(error) = fs::rename(&temp_path, output_path) {
                cleanup_temp_artifact(&temp_path);
                return Err(backup_io(
                    error.kind(),
                    format!("failed to replace {}: {error}", output_path.display()),
                ));
            }
            Ok(report)
        }
        Err(error) => {
            cleanup_temp_artifact(&temp_path);
            Err(error)
        }
    }
}

fn write_and_validate_temp_backup(
    program: &CheckedProgram,
    store: &TreeStore,
    output_path: &Path,
    temp_path: &Path,
    file: File,
) -> Result<BackupArtifactReport, BackupError> {
    let mut writer = BackupWriter::new(file);
    let report = create_backup(program, store, &mut writer)?;
    if let Err(error) = writer.finish() {
        return Err(backup_io(
            error.kind(),
            format!(
                "failed to finish writing {}: {error}",
                output_path.display()
            ),
        ));
    }
    drop(writer);
    let file = File::open(temp_path).map_err(|error| {
        backup_io(
            error.kind(),
            format!(
                "failed to reopen temporary backup {}: {error}",
                temp_path.display()
            ),
        )
    })?;
    validate_backup_archive(&mut BufReader::new(file))?;
    Ok(BackupArtifactReport {
        record_count: report.record_count,
    })
}

fn backup_io(kind: io::ErrorKind, message: String) -> BackupError {
    BackupError::Io(io::Error::new(kind, message))
}

fn create_temp_artifact(target: &Path) -> io::Result<(PathBuf, File)> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = target.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "backup output path must name a file",
        )
    })?;
    let file_name = file_name.to_string_lossy();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    for attempt in 0..16 {
        let path = parent.join(format!(
            ".{file_name}.{}.{}.{}.tmp",
            std::process::id(),
            nanos,
            attempt
        ));
        match create_owner_only_new_file(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique backup temp path",
    ))
}

fn create_owner_only_new_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

fn cleanup_temp_artifact(path: &Path) {
    let _ = fs::remove_file(path);
}

struct BackupWriter {
    inner: BufWriter<File>,
    #[cfg(debug_assertions)]
    fail_after: Option<FailAfter>,
}

impl BackupWriter {
    fn new(file: File) -> Self {
        Self {
            inner: BufWriter::new(file),
            #[cfg(debug_assertions)]
            fail_after: injected_write_limit("MARROW_TEST_BACKUP_FAIL_AFTER_BYTES"),
        }
    }

    fn finish(&mut self) -> io::Result<()> {
        self.inner.flush()?;
        self.inner.get_ref().sync_all()
    }
}

impl Write for BackupWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        #[cfg(debug_assertions)]
        if let Some(fail_after) = &mut self.fail_after {
            return fail_after.write(&mut self.inner, buf, "injected backup write failure");
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(debug_assertions)]
fn injected_write_limit(name: &str) -> Option<FailAfter> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(FailAfter::new)
}

#[cfg(debug_assertions)]
struct FailAfter {
    remaining: usize,
}

#[cfg(debug_assertions)]
impl FailAfter {
    fn new(remaining: usize) -> Self {
        Self { remaining }
    }

    fn write<W: Write>(
        &mut self,
        inner: &mut W,
        buf: &[u8],
        message: &'static str,
    ) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::other(message));
        }
        let allowed = self.remaining.min(buf.len());
        let written = inner.write(&buf[..allowed])?;
        self.remaining = self.remaining.saturating_sub(written);
        Ok(written)
    }
}
