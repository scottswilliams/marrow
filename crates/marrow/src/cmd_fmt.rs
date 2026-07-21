//! `marrow fmt`: format a single Marrow source file through the retained formatter.

use marrow_codes::Code;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{report_io_error, report_parse, report_simple_error};

const FMT_SYMLINK_HOP_LIMIT: usize = 40;

pub(crate) fn fmt(args: &[String]) -> ExitCode {
    let mut mode = None;
    let mut target = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--check" => {
                if mode.replace(FmtMode::Check).is_some() {
                    eprintln!("marrow fmt accepts only one of --check or --write");
                    return ExitCode::from(2);
                }
            }
            "--write" => {
                if mode.replace(FmtMode::Write).is_some() {
                    eprintln!("marrow fmt accepts only one of --check or --write");
                    return ExitCode::from(2);
                }
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow fmt [--check | --write] <file.mw | projectdir>

Format a Marrow source file or every captured source file of a project directory.
For a single file with no flag, print the formatted source to stdout. --check exits non-zero
if a file is not already formatted; --write rewrites it in place. For a project
directory, no flag checks without writing. `marrow fmt` does not read from stdin.
"
                );
                return ExitCode::SUCCESS;
            }
            // A stdin pipe has no path to --write and no project to discover, so
            // reject it explicitly rather than mislabel `-` as an unknown option.
            "-" => {
                eprintln!("marrow fmt does not read from stdin; pass a single .mw file");
                return ExitCode::from(2);
            }
            value if value.starts_with('-') => return crate::unknown_option("fmt", value),
            value => {
                if let Err(code) =
                    crate::take_single_target(&mut target, value, "fmt", "source file")
                {
                    return code;
                }
            }
        }
        index += 1;
    }

    let mode = mode.unwrap_or(FmtMode::Print);
    let Some(target) = target else {
        eprintln!("missing source file");
        return ExitCode::from(2);
    };
    let target_path = Path::new(&target);
    // A directory target formats every captured source file through the
    // `ProjectInput`, so file discovery and identity have exactly one owner.
    if target_path.is_dir() {
        return fmt_project(target_path, mode);
    }
    if let Err(error) = guard_regular_source_file(Path::new(&target)) {
        report_io_error(&target, &error);
        return ExitCode::FAILURE;
    }
    let source = match std::fs::read_to_string(&target) {
        Ok(source) => source,
        Err(error) => {
            report_io_error(&target, &error);
            return ExitCode::FAILURE;
        }
    };
    match fmt_one(&target, &source, mode) {
        Ok(FmtOutcome::Formatted) | Ok(FmtOutcome::Unchanged) => ExitCode::SUCCESS,
        Ok(FmtOutcome::NeedsFormatting) | Err(()) => ExitCode::FAILURE,
    }
}

/// Format every captured source file of the project rooted at `dir` through the
/// `ProjectInput`. Print mode has no single output stream for a whole project, so
/// it degrades to the non-destructive `--check` behavior; `--write` rewrites each
/// unformatted source file in place. The command fails if any source does not parse or,
/// under check, is not already formatted.
fn fmt_project(dir: &Path, mode: FmtMode) -> ExitCode {
    let input = match crate::project::capture_project(dir) {
        Ok(input) => input,
        Err(failure) => {
            render_capture_failure(&failure);
            return ExitCode::FAILURE;
        }
    };
    let mode = match mode {
        FmtMode::Print => FmtMode::Check,
        other => other,
    };

    let mut any_error = false;
    let mut any_needs_formatting = false;
    for module in input.modules() {
        let file = dir.join(module.identity().as_str());
        let label = file.display().to_string();
        let Ok(source) = std::str::from_utf8(module.source()) else {
            report_simple_error(
                Code::IoRead.as_str(),
                &format!("{label}: source is not valid UTF-8"),
            );
            any_error = true;
            continue;
        };
        match fmt_one(&label, source, mode) {
            Ok(FmtOutcome::Formatted | FmtOutcome::Unchanged) => {}
            Ok(FmtOutcome::NeedsFormatting) => any_needs_formatting = true,
            Err(()) => any_error = true,
        }
    }

    if any_error || any_needs_formatting {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Render a project-capture failure as a typed line: a located manifest fault
/// prints its `file:line:column`, and any other fault prints `code: message`.
fn render_capture_failure(failure: &crate::project::CaptureFailure) {
    use crate::term_style::{Stream, code_message};
    match &failure.location {
        Some(location) => eprintln!(
            "{}:{}:{}: {}",
            location.file,
            location.line,
            location.column,
            code_message(Stream::Stderr, failure.code, &failure.message)
        ),
        None => report_simple_error(failure.code, &failure.message),
    }
}

/// Reject an explicit single-file argument that resolves to an existing non-regular
/// file before the unbounded blocking read. A FIFO with no writer never returns, and
/// a socket or device cannot be a source body, so a non-regular target fails closed
/// promptly with an `io.read` error located at the path. A missing target never reaches
/// here: the caller classifies it as `config.missing` first.
fn guard_regular_source_file(path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        )),
        _ => Ok(()),
    }
}

/// The result of formatting one file in `--check`/`--write` mode.
enum FmtOutcome {
    /// `--write`: the file was rewritten with new formatting.
    Formatted,
    /// `--check`/`--write`: already formatted, nothing to do.
    Unchanged,
    /// `--check`: the file is not formatted (a finding, not an error).
    NeedsFormatting,
}

/// Format one file's `source` in `mode`, reporting parse errors, `--check`
/// findings, and `--write` I/O failures. Source that does not parse is left
/// untouched and reported (`Err`). The `Print` mode writes to stdout (only valid
/// for a single file).
fn fmt_one(file: &str, source: &str, mode: FmtMode) -> Result<FmtOutcome, ()> {
    // The checked-format policy (parse, format, refuse on parse failure or comment
    // loss) is owned once by the syntax crate; this command only routes its outcome to
    // the terminal and, in `--write`, to disk.
    let formatted = match marrow_syntax::check_format(source) {
        Ok(formatted) => formatted,
        Err(marrow_syntax::FormatRefusal::ParseInvalid(diagnostics)) => {
            report_parse(file, &diagnostics);
            return Err(());
        }
        Err(marrow_syntax::FormatRefusal::CommentLoss) => {
            report_simple_error(
                Code::FmtCommentLoss.as_str(),
                &format!("refusing to format {file}: formatting would discard retained comments"),
            );
            return Err(());
        }
    };
    match mode {
        FmtMode::Print => {
            print!("{formatted}");
            Ok(FmtOutcome::Unchanged)
        }
        FmtMode::Check => {
            if source == formatted {
                Ok(FmtOutcome::Unchanged)
            } else {
                eprintln!("{file}: not formatted; run marrow fmt --write {file} to format it");
                Ok(FmtOutcome::NeedsFormatting)
            }
        }
        FmtMode::Write => {
            if source == formatted {
                Ok(FmtOutcome::Unchanged)
            } else if let Err(error) = write_formatted_source(file, &formatted) {
                report_simple_error(
                    Code::IoWrite.as_str(),
                    &format!("failed to write {file}: {error}"),
                );
                Err(())
            } else {
                Ok(FmtOutcome::Formatted)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FmtMode {
    Print,
    Check,
    Write,
}

fn write_formatted_source(file: &str, formatted: &str) -> io::Result<()> {
    let target = resolve_format_target(Path::new(file))?;
    ensure_target_writable(&target)?;
    let permissions = fs::metadata(&target)?.permissions();
    let (temp_path, temp_file) = create_temp_source_file(&target)?;
    let mut writer = FmtWriter::new(temp_file);
    if let Err(error) = writer
        .write_all(formatted.as_bytes())
        .and_then(|()| writer.finish())
    {
        drop(writer);
        cleanup_temp_source(&temp_path);
        return Err(error);
    }
    drop(writer);
    if let Err(error) = fs::set_permissions(&temp_path, permissions) {
        cleanup_temp_source(&temp_path);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp_path, &target) {
        cleanup_temp_source(&temp_path);
        return Err(error);
    }
    Ok(())
}

fn ensure_target_writable(target: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).open(target).map(|_| ())
}

fn resolve_format_target(target: &Path) -> io::Result<PathBuf> {
    let mut path = target.to_path_buf();
    let mut visited = Vec::new();
    for _ in 0..FMT_SYMLINK_HOP_LIMIT {
        if visited.iter().any(|visited| visited == &path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "format target symlink cycle",
            ));
        }
        visited.push(path.clone());
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_symlink() {
            return Ok(path);
        }
        let target = fs::read_link(&path)?;
        path = resolve_link_target(&path, target);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "format target symlink chain is too deep",
    ))
}

fn resolve_link_target(link_path: &Path, target: PathBuf) -> PathBuf {
    if target.is_absolute() {
        target
    } else {
        link_path
            .parent()
            .map_or_else(|| target.clone(), |parent| parent.join(&target))
    }
}

fn create_temp_source_file(target: &Path) -> io::Result<(PathBuf, File)> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = target.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "format target path must name a file",
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
        "could not allocate a unique format temp path",
    ))
}

fn create_owner_only_new_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

fn cleanup_temp_source(path: &Path) {
    let _ = fs::remove_file(path);
}

struct FmtWriter {
    inner: BufWriter<File>,
    #[cfg(debug_assertions)]
    fail_after: Option<FailAfter>,
}

impl FmtWriter {
    fn new(file: File) -> Self {
        Self {
            inner: BufWriter::new(file),
            #[cfg(debug_assertions)]
            fail_after: injected_write_limit("MARROW_TEST_FMT_FAIL_AFTER_BYTES"),
        }
    }

    fn finish(&mut self) -> io::Result<()> {
        self.inner.flush()?;
        self.inner.get_ref().sync_all()
    }
}

impl Write for FmtWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        #[cfg(debug_assertions)]
        if let Some(fail_after) = &mut self.fail_after {
            return fail_after.write(&mut self.inner, buf, "injected fmt write failure");
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
