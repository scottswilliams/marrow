//! `marrow init`: create a new project via a one-winner directory claim.
//!
//! The target directory is claimed by an exclusive `create_dir`: two concurrent
//! inits cannot both win, and an existing directory fails closed. Into the fresh
//! directory the command writes a bounded, fixed set of files — a concise
//! manifest and a contained `src` tree — each with `create_new` so nothing is
//! overwritten. No store is created.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_codes::Code;
use marrow_project::Edition;

use crate::project::MANIFEST_FILE;
use crate::report_simple_error;

const HELP: &str = "\
Usage:
  marrow init <projectdir>

Create a new Marrow project directory. The target directory must not already
exist. `marrow init` writes a `marrow.toml` manifest and a contained `src` tree
with a starter module; it creates no store.
";

pub(crate) fn init(args: &[String]) -> ExitCode {
    let mut target = None;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => return crate::unknown_option("init", value),
            value => {
                if let Err(code) =
                    crate::take_single_target(&mut target, value, "init", "project directory")
                {
                    return code;
                }
            }
        }
    }

    let Some(target) = target else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    let root = PathBuf::from(&target);

    match claim_and_scaffold(&root) {
        Ok(()) => {
            println!("created {}", root.display());
            println!("next steps:");
            println!("  cd {}", root.display());
            println!("  marrow fmt --check {}", root.display());
            ExitCode::SUCCESS
        }
        Err(ClaimError::AlreadyExists) => {
            report_simple_error(
                Code::ConfigInvalid.as_str(),
                &format!("cannot create {}: it already exists", root.display()),
            );
            ExitCode::FAILURE
        }
        Err(ClaimError::Io(error)) => {
            report_simple_error(
                Code::IoWrite.as_str(),
                &format!("failed to create {}: {error}", root.display()),
            );
            ExitCode::FAILURE
        }
    }
}

enum ClaimError {
    AlreadyExists,
    Io(io::Error),
}

/// Claim `root` with an exclusive directory create, then write the fixed
/// scaffold. A failure after the claim unwinds it — the claimed directory is
/// removed — so a transient failure leaves nothing behind and a retry is not
/// blocked by a partial project.
fn claim_and_scaffold(root: &Path) -> Result<(), ClaimError> {
    match fs::create_dir(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(ClaimError::AlreadyExists);
        }
        Err(error) => return Err(ClaimError::Io(error)),
    }
    if let Err(error) = scaffold(root) {
        let _ = fs::remove_dir_all(root);
        return Err(ClaimError::Io(error));
    }
    Ok(())
}

fn scaffold(root: &Path) -> io::Result<()> {
    #[cfg(debug_assertions)]
    if std::env::var_os("MARROW_TEST_INIT_FAIL_SCAFFOLD").is_some() {
        return Err(io::Error::other("injected init scaffold failure"));
    }
    fs::create_dir(root.join("src"))?;
    write_new(root.join(MANIFEST_FILE), &manifest_source())?;
    write_new(root.join("src").join("main.mw"), STARTER_MODULE)?;
    Ok(())
}

fn manifest_source() -> String {
    format!("edition = \"{}\"\n", Edition::CURRENT.as_str())
}

/// The starter module a fresh project contains. It parses cleanly and is already
/// formatted, so `marrow fmt --check` on a fresh project passes.
const STARTER_MODULE: &str = "pub fn main() {\n    return\n}\n";

fn write_new(path: PathBuf, contents: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents.as_bytes())
}
