//! `marrow image --out <dir> --accept-ceiling <id>`.
//!
//! The deployment image-emit command: capture the project at the working
//! directory, compile it to canonical image bytes, independently verify them, and
//! write the verified `program.image` into the output directory — the durable
//! artifact a `MarrowDeployment` pins beside its release-verified runner. This is
//! the stock way to compose the verified image a packaged application ships; the
//! generated client (`marrow client typescript`) and this image are a matched pair
//! built from the same source.
//!
//! Composition is deliberately not silent about durable authority. A deployment's
//! ceiling is the union of what its exports demand (see
//! `marrow_image::CeilingDescriptor`), and the store an application provisions
//! records that ceiling as the maximum authority it will ever admit. So the command
//! renders each export's demand and requires the owner to name the accepted ceiling
//! id (`--accept-ceiling`) that matches the image's own demand union before it
//! writes anything. A missing or mismatched id writes no image and prints the
//! actual ceiling id to accept: the owner cannot widen (or narrow) a deployment's
//! durable authority by accident, and there is no target-runtime widening.
//!
//! The image bytes are produced through the same compile → verify path
//! `marrow client typescript` uses; this command does not link the runner (the
//! CLI→runner Rust edge is a lane absence target) and opens no store.

use std::path::PathBuf;
use std::process::ExitCode;

use marrow_verify::{CeilingDescriptor, VerifiedImage};

use crate::demand::demand_lines;

struct ImageArgs {
    out: PathBuf,
    accept_ceiling: Option<String>,
}

pub(crate) fn image(rest: &[String]) -> ExitCode {
    let args = match parse_options(rest) {
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

    // Family 1: source diagnostics. Like the client generator, the image command
    // never mints identities — a project with unminted durable declarations fails
    // precisely rather than composing an image the store could not admit.
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
        Err(marrow_compile::CompileFailure::ResourceLimit(limit)) => {
            crate::report_simple_error(
                marrow_codes::Code::CliCompilerResourceLimit.as_str(),
                &format!(
                    "the compiler reached a fixed resource limit ({})",
                    limit.kind().detail()
                ),
            );
            return ExitCode::FAILURE;
        }
        Err(marrow_compile::CompileFailure::Invariant(_)) => {
            crate::report_simple_error(
                marrow_codes::Code::CliCompilerInvariant.as_str(),
                "the compiler failed an internal consistency check",
            );
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

    // The image's own deployment ceiling: the union of every export's demand. The
    // store an application provisions under this image records this exact ceiling.
    let ceiling_id = CeilingDescriptor::from_demand_union(image.demand_union())
        .ceiling_id()
        .to_hex();

    match &args.accept_ceiling {
        None => {
            crate::report_simple_error(
                marrow_codes::Code::CliCeilingUnaccepted.as_str(),
                &format!(
                    "this image's deployment ceiling id is {ceiling_id}; re-run with \
                     --accept-ceiling {ceiling_id} to compose the deployment image after \
                     reviewing the demand printed below"
                ),
            );
            // Render the demand so the owner reviews exactly what authority they accept.
            render_demand(&compiled, &image);
            return ExitCode::FAILURE;
        }
        Some(accepted) if accepted != &ceiling_id => {
            crate::report_simple_error(
                marrow_codes::Code::CliCeilingUnaccepted.as_str(),
                &format!(
                    "the accepted ceiling id does not match this image; its deployment ceiling \
                     id is {ceiling_id}. No image was written."
                ),
            );
            return ExitCode::FAILURE;
        }
        Some(_) => {}
    }

    let image_id = image.image_id().to_hex();

    if let Err(error) = std::fs::create_dir_all(&args.out) {
        crate::report_simple_error(
            marrow_codes::Code::IoWrite.as_str(),
            &format!("failed to create {}: {error}", args.out.display()),
        );
        return ExitCode::FAILURE;
    }
    let image_path = args.out.join("program.image");
    if let Err(error) = std::fs::write(&image_path, &compiled.image.bytes) {
        crate::report_simple_error(
            marrow_codes::Code::IoWrite.as_str(),
            &format!("failed to write {}: {error}", image_path.display()),
        );
        return ExitCode::FAILURE;
    }

    // Machine-readable identity facts on stdout for a composition script: the image
    // identity the runner re-verifies at launch, and the accepted deployment ceiling
    // id the store is provisioned under. The composer pins both in the deployment
    // manifest; neither can be reconstructed downstream without the demand model.
    println!("image {image_id}");
    println!("ceiling {ceiling_id}");
    println!("{}", image_path.display());
    ExitCode::SUCCESS
}

/// Render each export's durable demand in source spelling on standard error, in
/// `module.item` order — the same reconstruction `marrow check` prints, so the owner
/// reviews the exact authority the accepted ceiling admits. A coherence failure
/// (the same compilation that verified) prints a terse internal-error line.
fn render_demand(compiled: &marrow_compile::Compiled, image: &VerifiedImage) {
    match demand_lines(&compiled.exports, &compiled.naming, image) {
        Ok(lines) => {
            for line in lines {
                eprintln!("{line}");
            }
        }
        Err(error) => eprintln!("{}", error.internal_message()),
    }
}

fn parse_options(options: &[String]) -> Result<ImageArgs, ExitCode> {
    let mut out: Option<PathBuf> = None;
    let mut accept_ceiling: Option<String> = None;
    let mut iter = options.iter();
    while let Some(option) = iter.next() {
        match option.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return Err(ExitCode::SUCCESS);
            }
            "--out" => match iter.next() {
                Some(dir) => {
                    if out.replace(PathBuf::from(dir)).is_some() {
                        return Err(usage("marrow image takes one --out directory"));
                    }
                }
                None => return Err(usage("`--out` needs a directory")),
            },
            "--accept-ceiling" => match iter.next() {
                Some(id) => {
                    if accept_ceiling.replace(id.clone()).is_some() {
                        return Err(usage("marrow image takes one --accept-ceiling id"));
                    }
                }
                None => return Err(usage("`--accept-ceiling` needs a ceiling id")),
            },
            other => return Err(crate::unknown_option("image", other)),
        }
    }
    let Some(out) = out else {
        return Err(usage("marrow image needs an --out directory"));
    };
    Ok(ImageArgs {
        out,
        accept_ceiling,
    })
}

const HELP: &str = "\
Usage:
  marrow image --out <dir> --accept-ceiling <id>

Compile and independently verify the project at the working directory and write
the verified program.image into <dir> — the durable artifact a deployment pins
beside its release-verified runner. The image's demand union defines its
deployment ceiling; the command requires --accept-ceiling to equal that ceiling's
id before writing, and prints the id to accept when it is absent or wrong, so a
deployment's durable authority is named, never widened by accident. On success it
prints the image id, the accepted ceiling id, and the written path.
";

fn usage(message: &str) -> ExitCode {
    eprintln!("{message}; run marrow image --help for usage");
    ExitCode::from(2)
}
