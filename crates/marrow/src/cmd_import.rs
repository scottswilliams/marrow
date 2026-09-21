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

use crate::Command;
use crate::command_output::{flag_value, once, unknown_option, usage};
use crate::companion::{companion_command, run_companion, stage_image};
use crate::project::compile_project;

pub(crate) const HELP: &str = "\
Usage:
  marrow import --store <dir> --jsonl <path> --root <name> [--keys <col,...>]

Compile and verify the project at the working directory, then fill the native store
at <dir> from a file of JSON objects, one entry per line, through the companion
runner. Every member is a scalar: a key component of the root, named in --keys, or a
field of the stored resource. A fresh store is provisioned on first use; an existing
store is filled only when the project is its active program. `import` mints no
identity: a missing one is `check.durable_identity`.
";

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
        Ok(compiled) => compiled,
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
    const COMMAND: Command = Command::Import;
    let mut store: Option<PathBuf> = None;
    let mut jsonl: Option<PathBuf> = None;
    let mut root: Option<String> = None;
    let mut keys: Option<String> = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--store" => once(
                &mut store,
                PathBuf::from(flag_value(&mut iter, COMMAND, "--store")?),
                COMMAND,
                "`--store` directory",
            )?,
            "--jsonl" => once(
                &mut jsonl,
                PathBuf::from(flag_value(&mut iter, COMMAND, "--jsonl")?),
                COMMAND,
                "`--jsonl` file",
            )?,
            "--root" => once(
                &mut root,
                flag_value(&mut iter, COMMAND, "--root")?.to_string(),
                COMMAND,
                "`--root` name",
            )?,
            "--keys" => once(
                &mut keys,
                flag_value(&mut iter, COMMAND, "--keys")?.to_string(),
                COMMAND,
                "`--keys` list",
            )?,
            other => return Err(unknown_option(COMMAND, other)),
        }
    }
    Ok(Args {
        store: store.ok_or_else(|| usage(COMMAND, "`--store` must name the store directory"))?,
        jsonl: jsonl.ok_or_else(|| usage(COMMAND, "`--jsonl` must name the JSONL file"))?,
        root: root
            .ok_or_else(|| usage(COMMAND, "`--root` must name the store root to populate"))?,
        keys,
    })
}
