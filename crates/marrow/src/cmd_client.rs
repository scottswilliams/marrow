//! `marrow client typescript [--out <dir>]`.
//!
//! The refound client generator: capture the project at the working directory,
//! compile it to canonical image bytes, verify them, reconstruct the wire
//! interface from the verified image, and emit the deterministic strict
//! TypeScript client (`client.mts`) beside the pinned Node supervision module
//! (`marrow-supervisor.mjs` + its `.d.mts` declarations) into the output
//! directory (default `client`). Stable inputs yield byte-identical output.
//!
//! The interface reconstruction here is the same thin projection the runner
//! performs at launch: both feed the image's export/record/enum facts to the one
//! semantic owner, `marrow_image::Interface::build`. The projection is repeated
//! rather than shared because the CLI must not link the runner (the CLI→runner
//! Rust edge is a lane absence target); the transfer law and identity live only
//! in `marrow-image`.

use std::path::PathBuf;
use std::process::ExitCode;

use marrow_image::{
    EnumShape, ExportSignature, FieldShape, ImageType, Interface, InterfaceError, RecordShape,
    VariantShape,
};
use marrow_verify::{RetShape, VerifiedImage};

use crate::tsgen::{self, ExportName};

struct ClientArgs {
    out: PathBuf,
}

pub(crate) fn client(rest: &[String]) -> ExitCode {
    let Some((target, options)) = rest.split_first() else {
        return usage("marrow client takes a target: typescript");
    };
    if target != "typescript" {
        return usage(&format!(
            "unknown client target `{target}`; the supported target is typescript"
        ));
    }
    let args = match parse_options(options) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let project = match crate::project::capture_project(&PathBuf::from(".")) {
        Ok(project) => project,
        Err(failure) => {
            crate::report_simple_error(failure.code, &failure.message);
            return ExitCode::FAILURE;
        }
    };

    // Family 1: source diagnostics. Unlike `run`, the generator never mints
    // identities — a project with unminted durable declarations fails precisely.
    let compiled = match marrow_compile::compile(&project) {
        Ok(compiled) => compiled,
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => {
            for diagnostic in &diagnostics {
                eprintln!(
                    "{}:{}:{}: {}: {}",
                    diagnostic.file().as_str(),
                    diagnostic.line(),
                    diagnostic.column(),
                    diagnostic.code,
                    diagnostic.message
                );
            }
            return ExitCode::FAILURE;
        }
        Err(marrow_compile::CompileFailure::ResourceLimit(_)) => {
            let (code, message) = compiler_resource_limit_report();
            crate::report_simple_error(code, message);
            return ExitCode::FAILURE;
        }
        Err(marrow_compile::CompileFailure::Invariant(_)) => {
            let (code, message) = compiler_invariant_report();
            crate::report_simple_error(code, message);
            return ExitCode::FAILURE;
        }
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
                marrow_codes::Code::CliTransferExcluded.as_str(),
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
    let client_source = tsgen::generate_client(&interface, &names);

    if let Err(error) = std::fs::create_dir_all(&args.out) {
        crate::report_simple_error(
            marrow_codes::Code::IoWrite.as_str(),
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
                marrow_codes::Code::IoWrite.as_str(),
                &format!("failed to write {}: {error}", path.display()),
            );
            return ExitCode::FAILURE;
        }
        println!("{}", path.display());
    }
    ExitCode::SUCCESS
}

/// Reconstruct the wire interface from a verified image through its public
/// accessors, feeding the single transfer/identity owner in `marrow-image`.
fn interface_of(image: &VerifiedImage) -> Result<Interface, InterfaceError> {
    let records: Vec<RecordShape> = image
        .record_types()
        .iter()
        .map(|record| RecordShape {
            fields: record
                .fields()
                .iter()
                .map(|field| FieldShape {
                    name: field.name.to_string(),
                    ty: field.ty,
                    required: field.required,
                })
                .collect(),
        })
        .collect();
    let enums: Vec<EnumShape> = image
        .enums()
        .iter()
        .map(|enum_type| EnumShape {
            variants: enum_type
                .variants()
                .iter()
                .map(|variant| VariantShape {
                    name: variant.name.to_string(),
                    category: variant.category,
                    payload: variant.payload.clone(),
                })
                .collect(),
        })
        .collect();
    let exports: Vec<ExportSignature> = image
        .exports()
        .iter()
        .map(|export| {
            let function = image.function(export.function());
            ExportSignature {
                id: export.id(),
                params: function.params().to_vec(),
                ret: ret_to_image(function.ret()),
                demand_id: export.demand_id(),
            }
        })
        .collect();
    Interface::build(exports, &records, &enums)
}

fn ret_to_image(ret: RetShape) -> ImageType {
    match ret {
        RetShape::Unit => ImageType::Unit,
        RetShape::Scalar { scalar, optional } => ImageType::Scalar { scalar, optional },
        RetShape::Record { idx, optional } => ImageType::Record { idx, optional },
        RetShape::Enum { idx, optional } => ImageType::Enum { idx, optional },
        RetShape::Collection { idx, optional } => ImageType::Collection { idx, optional },
        RetShape::Identity { root, optional } => ImageType::Identity { root, optional },
    }
}

/// Render a typed interface error with the offending export named through the
/// compiler's export directory.
fn render_interface_error(
    error: &InterfaceError,
    directory: &[marrow_compile::ExportEntry],
) -> String {
    let export = match error {
        InterfaceError::TransferTypeExcluded { export, .. }
        | InterfaceError::SignatureTooComplex { export }
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
                        return Err(usage("marrow client typescript takes one --out directory"));
                    }
                }
                None => return Err(usage("`--out` needs a directory")),
            },
            other => return Err(crate::unknown_option("client", other)),
        }
    }
    Ok(ClientArgs {
        out: out.unwrap_or_else(|| PathBuf::from("client")),
    })
}

fn usage(message: &str) -> ExitCode {
    eprintln!("{message}; run marrow --help for usage");
    ExitCode::from(2)
}

fn compiler_invariant_report() -> (&'static str, &'static str) {
    (
        marrow_codes::Code::CliCompilerInvariant.as_str(),
        "the compiler failed an internal consistency check",
    )
}

/// The fixed code and bounded message a compiler resource-limit outcome emits on
/// stderr. The generator writes no client and no stdout: it fails the whole program
/// closed with one fixed line carrying no source location or limit payload.
fn compiler_resource_limit_report() -> (&'static str, &'static str) {
    (
        marrow_codes::Code::CliCompilerResourceLimit.as_str(),
        "the compiler reached a fixed resource limit",
    )
}

#[cfg(test)]
mod compiler_invariant_tests {
    #[test]
    fn invariant_mapper_is_fixed_and_payload_free() {
        assert_eq!(
            super::compiler_invariant_report(),
            (
                marrow_codes::Code::CliCompilerInvariant.as_str(),
                "the compiler failed an internal consistency check",
            )
        );
    }
}
