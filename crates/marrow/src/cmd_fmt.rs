//! `marrow fmt`: format a single Marrow source file through the retained formatter.

use marrow_codes::Code;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::Command;
use crate::command_output::{once, unknown_option, usage};
use crate::{report_io_error, report_parse, report_simple_error};

pub(crate) const HELP: &str = "\
Usage:
  marrow fmt [--check | --write] <file.mw | projectdir>

Format a Marrow source file or every captured source file of a project directory.
For a single file with no flag, print the formatted source to stdout. --check exits non-zero
if a file is not already formatted; --write rewrites it in place. For a project
directory, no flag checks without writing. `marrow fmt` does not read from stdin.
";

const FMT_SYMLINK_HOP_LIMIT: usize = 40;

pub(crate) fn fmt(args: &[String]) -> ExitCode {
    let mut mode = None;
    let mut target = None;
    for arg in args {
        let result = match arg.as_str() {
            "--check" => once(
                &mut mode,
                FmtMode::Check,
                Command::Fmt,
                "of `--check` or `--write`",
            ),
            "--write" => once(
                &mut mode,
                FmtMode::Write,
                Command::Fmt,
                "of `--check` or `--write`",
            ),
            // A stdin pipe has no path to --write and no project to discover, so
            // reject it explicitly rather than mislabel `-` as an unknown option.
            "-" => Err(usage(
                Command::Fmt,
                "marrow fmt does not read from stdin; pass a single .mw file",
            )),
            value if value.starts_with('-') => Err(unknown_option(Command::Fmt, value)),
            value => once(
                &mut target,
                value.to_string(),
                Command::Fmt,
                "source file or project directory",
            ),
        };
        if let Err(code) = result {
            return code;
        }
    }

    let mode = mode.unwrap_or(FmtMode::Print);
    let Some(target) = target else {
        return usage(
            Command::Fmt,
            "marrow fmt takes a source file or project directory",
        );
    };
    let target_path = Path::new(&target);
    // A directory target formats every captured source file through the
    // `ProjectInput`, so file discovery and identity have exactly one owner.
    if target_path.is_dir() {
        return fmt_project(target_path, mode);
    }
    match admit_single_source_file(target_path) {
        Ok(()) => {}
        Err(SingleFileRefusal::NotRegular(error)) => {
            report_io_error(&target, &error);
            return ExitCode::FAILURE;
        }
        Err(SingleFileRefusal::OverModuleLimit { actual, limit }) => {
            // `actual` is the stat's true file size, which this owner can report
            // exactly because it never opens the file. The project target reaches the
            // same 1 MiB bound through capture's bounded read, which stops one byte
            // past the limit and so can only ever report `limit + 1`. The two owners
            // name one bound under their own typed codes; their byte figures are not
            // interchangeable.
            report_simple_error(
                Code::CliCompilerResourceLimit,
                &format!("`{target}` is {actual} bytes, over the per-file byte limit ({limit})"),
            );
            return ExitCode::FAILURE;
        }
    }
    let source = match std::fs::read_to_string(&target) {
        Ok(source) => source,
        Err(error) => {
            report_io_error(&target, &error);
            return ExitCode::FAILURE;
        }
    };
    match fmt_one(&target, &source, mode, FileAuthority::Owned) {
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
        let (label, authority) = match module.origin().alias() {
            None => (
                captured_module_path(dir, module.identity().as_str())
                    .display()
                    .to_string(),
                FileAuthority::Owned,
            ),
            // A dependency's file is named by the compiler's one address spelling, so a
            // formatting finding and a diagnostic agree on how to name it.
            Some(_) => (
                marrow_compile::ProjectFile::from(module).spelling(),
                FileAuthority::Dependency,
            ),
        };
        let Ok(source) = std::str::from_utf8(module.source()) else {
            report_simple_error(Code::IoRead, &format!("{label}: source is not valid UTF-8"));
            any_error = true;
            continue;
        };
        match fmt_one(&label, source, mode.under(authority), authority) {
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

/// Who owns a captured file on disk, and therefore whether `--write` may rewrite it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FileAuthority {
    /// A source file of the root project `fmt` was invoked on.
    Owned,
    /// A source file of a declared dependency. It is reported and never rewritten: the
    /// project that declares a file is the project that formats and commits it, so a
    /// consumer that rewrote one would change a tree it does not own and leave that
    /// tree's own `fmt --check` disagreeing with its committed source.
    Dependency,
}

/// The path a captured module is reported and written under: the capture root joined
/// to the module's project-relative identity, with `.` components dropped. The join is
/// what keeps `--write` and the `--write` hint correct for a root named from elsewhere
/// (`marrow fmt --check app` reports `app/src/main.mw`); dropping `.` is what keeps the
/// common in-project spelling identical to the `src/main.mw` that capture and `check`
/// report. A dependency file is reported under its alias instead
/// (`graphtext:src/text.mw`), which names the tree that owns it and is deliberately not
/// a path in the consuming tree.
///
/// This governs formatting findings only. A capture refusal is spelled root-relative
/// by the capture presentation facade and printed verbatim, so one run can report
/// `app/src/main.mw` for a finding and `src/big.mw` for a refusal.
fn captured_module_path(root: &Path, identity: &str) -> PathBuf {
    root.join(identity)
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect()
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

/// Why the single-file admission refused a target before any read.
enum SingleFileRefusal {
    /// An existing non-regular target, reported as a located `io.read` error.
    NotRegular(io::Error),
    /// A regular target larger than the compiler's module byte limit, reported
    /// under the exact typed code the `ProjectFileBytes` admission uses.
    OverModuleLimit { actual: u64, limit: u64 },
}

/// Admit an explicit single-file argument from one stat, before the blocking read
/// or any allocation. A FIFO with no writer never returns, and a socket or device
/// cannot be a source body, so a non-regular target fails closed promptly; a
/// regular target over the compiler's module byte limit is refused with the
/// module-size admission's typed code rather than materialized only to be rejected
/// at compile. The limit is that owner's constant rather than a copy: it is the
/// longest file whose parse fits the compiler's heap ceiling, and this path is about
/// to parse the file. A missing or unstatable target passes through:
/// `read_to_string` reports it as the located `io.read` error.
fn admit_single_source_file(path: &Path) -> Result<(), SingleFileRefusal> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    if !metadata.file_type().is_file() {
        return Err(SingleFileRefusal::NotRegular(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        )));
    }
    let limit = marrow_compile::MAX_PARSED_FILE_BYTES as u64;
    if metadata.len() > limit {
        return Err(SingleFileRefusal::OverModuleLimit {
            actual: metadata.len(),
            limit,
        });
    }
    Ok(())
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
/// for a single file). `authority` selects the steer a `--check` finding carries; a
/// dependency file never reaches `FmtMode::Write`, because [`FmtMode::under`] demotes
/// it first.
fn fmt_one(
    file: &str,
    source: &str,
    mode: FmtMode,
    authority: FileAuthority,
) -> Result<FmtOutcome, ()> {
    // The checked-format policy (parse, format, refuse on parse failure or comment
    // loss) is owned once by the syntax crate; this command only routes its outcome to
    // the terminal and, in `--write`, to disk.
    let formatted = match marrow_syntax::check_format(source) {
        Ok(formatted) => formatted,
        Err(marrow_syntax::FormatRefusal::ParseInvalid(diagnostics)) => {
            report_parse(file, diagnostics.as_slice());
            return Err(());
        }
        Err(marrow_syntax::FormatRefusal::DiagnosticLimit(limit)) => {
            let bound = match limit {
                marrow_syntax::SyntaxDiagnosticLimit::Count { limit } => {
                    format!("{limit} retained rows")
                }
                marrow_syntax::SyntaxDiagnosticLimit::OwnedBytes { limit } => {
                    format!("{limit} retained bytes")
                }
            };
            report_simple_error(
                Code::FmtDiagnosticLimit,
                &format!(
                    "refusing to format {file}: its parse diagnostics exceeded the {bound} \
                     bound, so no complete parse exists; repair the source's parse errors first"
                ),
            );
            return Err(());
        }
        Err(marrow_syntax::FormatRefusal::CommentLoss) => {
            report_simple_error(
                Code::FmtCommentLoss,
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
                match authority {
                    FileAuthority::Owned => eprintln!(
                        "{file}: not formatted; run marrow fmt --write {file} to format it"
                    ),
                    FileAuthority::Dependency => eprintln!(
                        "{file}: not formatted; format it in the project that declares it"
                    ),
                }
                Ok(FmtOutcome::NeedsFormatting)
            }
        }
        FmtMode::Write => {
            if source == formatted {
                Ok(FmtOutcome::Unchanged)
            } else if let Err(error) = write_formatted_source(file, &formatted) {
                report_simple_error(Code::IoWrite, &format!("failed to write {file}: {error}"));
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

impl FmtMode {
    /// The mode one file is actually formatted under. A dependency file is only ever
    /// reported: `--write` demotes to `--check` there, so no write path exists for a
    /// tree this project does not own.
    fn under(self, authority: FileAuthority) -> FmtMode {
        match authority {
            FileAuthority::Owned => self,
            FileAuthority::Dependency => FmtMode::Check,
        }
    }
}

fn write_formatted_source(file: &str, formatted: &str) -> io::Result<()> {
    let target = resolve_format_target(Path::new(file))?;
    ensure_target_writable(&target)?;
    let permissions = fs::metadata(&target)?.permissions();
    let (temp_path, temp_file) = create_temp_source_file(&target)?;
    let mut writer = BufWriter::new(temp_file);
    let written = writer
        .write_all(formatted.as_bytes())
        .and_then(|()| writer.flush())
        .and_then(|()| writer.get_ref().sync_all());
    drop(writer);
    let staged = written
        .and_then(|()| fs::set_permissions(&temp_path, permissions))
        .and_then(|()| fs::rename(&temp_path, &target));
    if staged.is_err() {
        cleanup_temp_source(&temp_path);
    }
    staged
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
