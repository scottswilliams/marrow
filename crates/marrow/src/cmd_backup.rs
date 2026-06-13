//! `marrow backup`: write a typed portable backup of a project's saved data.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_run::SystemNondeterminism;
use marrow_store::tree::TreeStore;

use crate::backup::{create_backup, ensure_store_uid};
use crate::{CheckFormat, load_checked_project, open_store_for_inspection, report_simple_error};

pub(crate) fn backup(args: &[String]) -> ExitCode {
    let (dir, output) = match backup_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let format = CheckFormat::Text;
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let mut nondeterminism = SystemNondeterminism::new();
    // A project with no saved data on disk yields a valid empty backup.
    let store = match open_store_for_inspection(&dir, &config, format) {
        Ok(Some(store)) => store,
        Ok(None) => {
            let store = TreeStore::memory();
            if let Err(error) = ensure_store_uid(&store, &mut nondeterminism) {
                report_simple_error(error.code(), &error.to_string(), format);
                return ExitCode::FAILURE;
            }
            store
        }
        Err(code) => return code,
    };

    let output_path = Path::new(&output);
    let (temp_path, file) = match create_temp_artifact(output_path) {
        Ok(created) => created,
        Err(error) => {
            report_simple_error(
                "io.write",
                &format!("could not create temporary backup for {output}: {error}"),
                format,
            );
            return ExitCode::FAILURE;
        }
    };
    let mut writer = BackupWriter::new(file);
    match create_backup(&program, &store, &mut writer) {
        Ok(report) => {
            if let Err(error) = writer.finish() {
                drop(writer);
                cleanup_temp_artifact(&temp_path);
                report_simple_error(
                    "io.write",
                    &format!("failed to finish writing {output}: {error}"),
                    format,
                );
                return ExitCode::FAILURE;
            }
            drop(writer);
            if let Err(error) = fs::rename(&temp_path, output_path) {
                cleanup_temp_artifact(&temp_path);
                report_simple_error(
                    "io.write",
                    &format!("failed to replace {output}: {error}"),
                    format,
                );
                return ExitCode::FAILURE;
            }
            println!(
                "ok: backed up {} record(s) to {output}",
                report.record_count
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            drop(writer);
            cleanup_temp_artifact(&temp_path);
            report_simple_error(error.code(), &error.to_string(), format);
            ExitCode::FAILURE
        }
    }
}

fn backup_args(args: &[String]) -> Result<(String, String), ExitCode> {
    let mut positionals = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("Usage:\n  marrow backup <projectdir> <output-file>\n");
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => return Err(crate::unknown_option("backup", value)),
            value => positionals.push(value.to_string()),
        }
    }
    match positionals.as_slice() {
        [dir, output] => Ok((dir.clone(), output.clone())),
        [] | [_] => {
            eprintln!("marrow backup requires a project directory and an output-file");
            Err(ExitCode::from(2))
        }
        _ => {
            eprintln!("marrow backup accepts one project directory and one output-file");
            Err(ExitCode::from(2))
        }
    }
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
