//! `marrow import --store <dir> --jsonl <path> --root <name> [--keys <col,...>]`: populate a
//! native store from a flat-scalar JSONL corpus through the trusted importer.
//!
//! The terminal compiles and (via the companion) verifies the project at the working
//! directory, exactly like `marrow run --store`; it never opens the store itself. It writes the
//! compiled program image to a private temporary file and hands the actual provisioning and
//! import to the release-verified companion runner (`marrow-runner import`), the sole opener of
//! the store. Every imported row is created through the path kernel; no raw key, engine handle,
//! or transaction is ever exposed to the terminal.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_compile::compile;

use crate::companion::{companion_command, run_companion, stage_image};
use crate::project::compile_project;

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

    // Compile without opening a store. A durable project must already carry its committed
    // `.marrow/ids`; import is not a mint path, so an identity or type error points the developer
    // at `marrow check` rather than auto-minting here.
    let compiled = match compile_project(
        Path::new("."),
        compile,
        Some("the project does not compile; run `marrow check` before importing"),
    ) {
        Ok((compiled, _)) => compiled,
        Err(code) => return code,
    };

    let mut command = match companion_command("import") {
        Ok(command) => command,
        Err(code) => return code,
    };
    let image = match stage_image(&compiled.image.bytes) {
        Ok(image) => image,
        Err(code) => return code,
    };
    command
        .arg("--image")
        .arg(image.path())
        .arg("--store")
        .arg(&args.store)
        .arg("--jsonl")
        .arg(&args.jsonl)
        .arg("--root")
        .arg(&args.root);
    if let Some(keys) = &args.keys {
        command.arg("--keys").arg(keys);
    }
    run_companion(command)
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
