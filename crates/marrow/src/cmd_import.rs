//! `marrow import --store <dir> --jsonl <path> --root <name> [--keys <col,...>]`: populate a
//! native store from a flat-scalar JSONL corpus through the trusted importer.
//!
//! The terminal compiles and (via the companion) verifies the project at the working
//! directory, exactly like `marrow run --store`; it never opens the store itself. It writes the
//! compiled program image to a private temporary file and hands the actual provisioning and
//! import to the release-verified companion runner (`marrow-runner import`), the sole opener of
//! the store. Every imported row is created through the path kernel; no raw key, engine handle,
//! or transaction is ever exposed to the terminal.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use marrow_compile::{CompileFailure, compile};

use crate::project::capture_project;

struct Args {
    store: PathBuf,
    jsonl: PathBuf,
    root: String,
    keys: Option<String>,
}

pub(crate) fn import(rest: &[String]) -> ExitCode {
    let args = match parse_args(rest) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let project = match capture_project(&PathBuf::from(".")) {
        Ok(project) => project,
        Err(failure) => {
            crate::report_simple_error(failure.code, &failure.message);
            return ExitCode::FAILURE;
        }
    };

    // Compile without opening a store. A durable project must already carry its committed
    // `.marrow/ids`; import is not a mint path, so an identity or type error points the developer
    // at `marrow check` rather than auto-minting here.
    let compiled = match compile(&project) {
        Ok(compiled) => compiled,
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            for diagnostic in diagnostics.iter() {
                eprintln!("{}: {}", diagnostic.code, diagnostic.message);
            }
            eprintln!("the project does not compile; run `marrow check` before importing");
            return ExitCode::FAILURE;
        }
        Err(_) => {
            crate::report_simple_error(
                marrow_codes::Code::ConfigInvalid.as_str(),
                "the project could not be compiled; run `marrow check` before importing",
            );
            return ExitCode::FAILURE;
        }
    };

    let runner = match crate::companion::discover_companion() {
        Ok(runner) => runner,
        Err(damage) => {
            crate::report_simple_error(
                marrow_codes::Code::CliInstallationDamaged.as_str(),
                damage.message(),
            );
            return ExitCode::FAILURE;
        }
    };

    // Stage the image in a private temp file for the companion to read and independently
    // verify. The name carries the PID and a high-resolution timestamp so concurrent imports
    // do not collide; it is removed on every exit path, including this write failure.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let image_path = std::env::temp_dir().join(format!(
        "marrow-import-{}-{nonce}.image",
        std::process::id()
    ));
    if let Err(err) = std::fs::write(&image_path, &compiled.image.bytes) {
        let _ = std::fs::remove_file(&image_path);
        crate::report_simple_error(marrow_codes::Code::IoWrite.as_str(), &err.to_string());
        return ExitCode::FAILURE;
    }

    let mut command = Command::new(&runner);
    command
        .arg("import")
        .arg("--image")
        .arg(&image_path)
        .arg("--store")
        .arg(&args.store)
        .arg("--jsonl")
        .arg(&args.jsonl)
        .arg("--root")
        .arg(&args.root);
    if let Some(keys) = &args.keys {
        command.arg("--keys").arg(keys);
    }

    let status = command.status();
    let _ = std::fs::remove_file(&image_path);

    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            crate::report_simple_error(
                marrow_codes::Code::RunnerHandshake.as_str(),
                &err.to_string(),
            );
            ExitCode::FAILURE
        }
    }
}

fn parse_args(rest: &[String]) -> Result<Args, ExitCode> {
    let mut store: Option<PathBuf> = None;
    let mut jsonl: Option<PathBuf> = None;
    let mut root: Option<String> = None;
    let mut keys: Option<String> = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--store" => store = Some(PathBuf::from(next_value(&mut iter, "--store")?)),
            "--jsonl" => jsonl = Some(PathBuf::from(next_value(&mut iter, "--jsonl")?)),
            "--root" => root = Some(next_value(&mut iter, "--root")?),
            "--keys" => keys = Some(next_value(&mut iter, "--keys")?),
            other => return Err(usage(&format!("unexpected argument `{other}`"))),
        }
    }
    let Some(store) = store else {
        return Err(usage("`--store` names the native store directory"));
    };
    let Some(jsonl) = jsonl else {
        return Err(usage("`--jsonl` names the flat-scalar JSONL corpus file"));
    };
    let Some(root) = root else {
        return Err(usage("`--root` names the store root to populate"));
    };
    Ok(Args {
        store,
        jsonl,
        root,
        keys,
    })
}

fn next_value(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, ExitCode> {
    match iter.next() {
        Some(value) => Ok(value.clone()),
        None => Err(usage(&format!("`{flag}` needs a value"))),
    }
}

fn usage(message: &str) -> ExitCode {
    eprintln!(
        "{message}\nusage: marrow import --store <dir> --jsonl <path> --root <name> \
         [--keys <col,...>]"
    );
    ExitCode::from(2)
}
