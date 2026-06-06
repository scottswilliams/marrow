//! `marrow fmt`: format a Marrow source file or every file under a project.

use std::path::Path;
use std::process::ExitCode;

use crate::{CheckFormat, report_check, report_io_error, report_simple_error};

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

Format Marrow source. With a single `.mw` file and no flag, print the formatted
source to stdout. With a project directory (one that contains marrow.json),
format every `.mw` file under its source roots; a directory requires --check or
--write, since printing many files to stdout is meaningless. --check exits
non-zero if any file is not already formatted; --write rewrites changed files in
place. `marrow fmt` does not read from stdin.
"
                );
                return ExitCode::SUCCESS;
            }
            // A stdin pipe has no path to --write and no project to discover, so
            // reject it explicitly rather than mislabel `-` as an unknown option.
            "-" => {
                eprintln!("marrow fmt does not read from stdin; pass a file or project directory");
                return ExitCode::from(2);
            }
            value if value.starts_with('-') => return crate::unknown_option("fmt", value),
            value => {
                if let Err(code) = crate::take_single_target(
                    &mut target,
                    value,
                    "fmt",
                    "source file or project directory",
                ) {
                    return code;
                }
            }
        }
        index += 1;
    }

    let mode = mode.unwrap_or(FmtMode::Print);
    let Some(target) = target else {
        eprintln!("missing source file or project directory");
        return ExitCode::from(2);
    };
    if Path::new(&target).is_dir() {
        return fmt_project_dir(&target, mode);
    }
    let source = match std::fs::read_to_string(&target) {
        Ok(source) => source,
        Err(error) => {
            report_io_error(&target, &error, CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    match fmt_one(&target, &source, mode) {
        Ok(FmtOutcome::Formatted) | Ok(FmtOutcome::Unchanged) => ExitCode::SUCCESS,
        Ok(FmtOutcome::NeedsFormatting) | Err(()) => ExitCode::FAILURE,
    }
}

/// Format every `.mw` file under a project's source roots. A directory requires a
/// mode: printing many files to stdout is meaningless, so bare `fmt <dir>` is a
/// usage error. A missing/invalid `marrow.json` is a typed `config.*` error
/// through `load_config`, not a raw OS "Is a directory".
fn fmt_project_dir(dir: &str, mode: FmtMode) -> ExitCode {
    if matches!(mode, FmtMode::Print) {
        eprintln!("marrow fmt on a directory requires --check or --write");
        return ExitCode::from(2);
    }
    let config = match crate::load_config(dir) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let modules = match marrow_project::discover_modules(Path::new(dir), &config) {
        Ok(modules) => modules,
        Err(error) => {
            crate::report_simple_error(error.code, &error.to_string(), CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    let mut failed = false;
    for module in &modules {
        let path = module.path.display().to_string();
        let source = match std::fs::read_to_string(&module.path) {
            Ok(source) => source,
            Err(error) => {
                report_io_error(&path, &error, CheckFormat::Text);
                failed = true;
                continue;
            }
        };
        match fmt_one(&path, &source, mode) {
            // A whole-project run reports every problem, then fails overall, so
            // the operator sees all unformatted or unparseable files at once.
            Ok(FmtOutcome::Formatted) | Ok(FmtOutcome::Unchanged) => {}
            Ok(FmtOutcome::NeedsFormatting) | Err(()) => failed = true,
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
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
    // Do not reformat source that does not parse; report its diagnostics and
    // leave the file untouched.
    let parsed = marrow_syntax::parse_source(source);
    if parsed.has_errors() {
        report_check(file, &parsed, CheckFormat::Text);
        return Err(());
    }
    let formatted = marrow_syntax::format_source(source);
    match mode {
        FmtMode::Print => {
            print!("{formatted}");
            Ok(FmtOutcome::Unchanged)
        }
        FmtMode::Check => {
            if source == formatted {
                Ok(FmtOutcome::Unchanged)
            } else {
                eprintln!("{file}: not formatted");
                Ok(FmtOutcome::NeedsFormatting)
            }
        }
        FmtMode::Write => {
            if source == formatted {
                Ok(FmtOutcome::Unchanged)
            } else if let Err(error) = std::fs::write(file, &formatted) {
                report_simple_error(
                    "io.write",
                    &format!("failed to write {file}: {error}"),
                    CheckFormat::Text,
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
