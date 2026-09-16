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

use crate::project::compile_project;
use crate::tsgen::{self, ExportName};

struct ClientArgs {
    out: PathBuf,
}

pub(crate) fn client(rest: &[String]) -> ExitCode {
    let Some((target, options)) = rest.split_first() else {
        return crate::command_output::usage("marrow client takes a target: typescript");
    };
    if target != "typescript" {
        return crate::command_output::usage(&format!(
            "unknown client target `{target}`; the supported target is typescript"
        ));
    }
    let args = match parse_options(options) {
        Ok(args) => args,
        Err(code) => return code,
    };

    // Family 1: source diagnostics. Unlike `run`, the generator never mints
    // identities — a project with unminted durable declarations fails precisely.
    let compiled = match compile_project(Path::new("."), marrow_compile::compile, None) {
        Ok((compiled, _)) => compiled,
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
            id: *entry.id.bytes(),
            module: entry.module.clone(),
            item: entry.item.clone(),
        })
        .collect();
    let client_source = tsgen::generate_client(&interface, &names, image.image_id().0);

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
            "--out" => match iter.next() {
                Some(dir) => {
                    if out.replace(PathBuf::from(dir)).is_some() {
                        return Err(crate::command_output::usage(
                            "marrow client typescript takes one --out directory",
                        ));
                    }
                }
                None => return Err(crate::command_output::usage("`--out` needs a directory")),
            },
            other => return Err(crate::unknown_option("client", other)),
        }
    }
    Ok(ClientArgs {
        out: out.unwrap_or_else(|| PathBuf::from("client")),
    })
}
