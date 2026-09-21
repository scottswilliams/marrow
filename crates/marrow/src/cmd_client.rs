//! `marrow client typescript [--out <dir>]`.
//!
//! The client generator: capture the project at the working directory,
//! compile it to canonical image bytes, verify them, reconstruct the wire
//! interface from the verified image, and emit the deterministic strict
//! TypeScript client (`client.mts`) beside the pinned Node supervision module
//! (`marrow-supervisor.mjs` + its `.d.mts` declarations) into the output
//! directory (default `client`). Stable inputs yield byte-identical output.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_image::InterfaceError;
use marrow_verify::interface_of;

use crate::Command;
use crate::command_output::{flag_value, once, unknown_option, usage};
use crate::project::compile_project;
use crate::tsgen::{self, ExportName};

pub(crate) const HELP: &str = "\
Usage:
  marrow client typescript [--out <dir>]

Compile and verify the project at the working directory, then write a strict
TypeScript client for its exports into <dir> (default `client`): one `async` method
per export with exact types, beside the pinned Node module that starts and
supervises the runner. Stable inputs yield byte-identical output.
";

struct ClientArgs {
    out: PathBuf,
}

pub(crate) fn client(rest: &[String]) -> ExitCode {
    let Some((target, options)) = rest.split_first() else {
        return usage(Command::Client, "marrow client takes a target: typescript");
    };
    if target != "typescript" {
        return usage(
            Command::Client,
            &format!("unknown client target `{target}`; the supported target is typescript"),
        );
    }
    let args = match parse_options(options) {
        Ok(args) => args,
        Err(code) => return code,
    };

    // Family 1: source diagnostics. Unlike `run`, the generator never mints
    // identities — a project with unminted durable declarations fails precisely.
    let compiled = match compile_project(Path::new("."), marrow_compile::compile, None) {
        Ok(compiled) => compiled,
        Err(code) => return code,
    };

    // Family 2: artifact rejection (the compiler cannot mint a verified image).
    let image = match marrow_verify::verify(&compiled.image.bytes) {
        Ok(image) => image,
        Err(rejection) => {
            crate::report_simple_error(rejection.code(), "the compiled image failed verification");
            return ExitCode::FAILURE;
        }
    };

    let interface = match interface_of(&image) {
        Ok(interface) => interface,
        Err(error) => {
            crate::report_simple_error(
                marrow_codes::Code::CliInterfaceUnbuildable,
                &render_interface_error(&error, &compiled.exports),
            );
            return ExitCode::FAILURE;
        }
    };

    let names: Vec<ExportName> = compiled
        .exports
        .iter()
        .map(|entry| ExportName {
            id: entry.id,
            module: entry.module.clone(),
            item: entry.item.clone(),
        })
        .collect();
    let client_source = tsgen::generate_client(&interface, &names, image.image_id());

    if let Err(error) = std::fs::create_dir_all(&args.out) {
        crate::report_simple_error(
            marrow_codes::Code::IoWrite,
            &format!("failed to create {}: {error}", args.out.display()),
        );
        return ExitCode::FAILURE;
    }
    for (name, contents) in [
        ("client.mts", client_source.as_str()),
        ("marrow-supervisor.mjs", tsgen::SUPERVISOR_MJS),
        ("marrow-supervisor.d.mts", tsgen::SUPERVISOR_DTS),
    ] {
        let path = args.out.join(name);
        if let Err(error) = std::fs::write(&path, contents) {
            crate::report_simple_error(
                marrow_codes::Code::IoWrite,
                &format!("failed to write {}: {error}", path.display()),
            );
            return ExitCode::FAILURE;
        }
        println!("{}", path.display());
    }
    ExitCode::SUCCESS
}

/// Render a typed interface error with the offending export named through the
/// compiler's export directory.
fn render_interface_error(
    error: &InterfaceError,
    directory: &[marrow_compile::ExportEntry],
) -> String {
    let export = match error {
        InterfaceError::SignatureTooComplex { export }
        | InterfaceError::TypeIndexOutOfRange { export } => export,
    };
    let name = directory
        .iter()
        .find(|entry| entry.id == *export)
        .map(|entry| format!("{}.{}", entry.module, entry.item))
        .unwrap_or_else(|| "an export".to_string());
    format!("`{name}`: {error}")
}

fn parse_options(options: &[String]) -> Result<ClientArgs, ExitCode> {
    let mut out: Option<PathBuf> = None;
    let mut iter = options.iter();
    while let Some(option) = iter.next() {
        match option.as_str() {
            "--out" => once(
                &mut out,
                PathBuf::from(flag_value(&mut iter, Command::Client, "--out")?),
                Command::Client,
                "`--out` directory",
            )?,
            other => return Err(unknown_option(Command::Client, other)),
        }
    }
    Ok(ClientArgs {
        out: out.unwrap_or_else(|| PathBuf::from("client")),
    })
}
