//! `marrow run <export> [--format jsonl] [-- <args>...]`.
//!
//! The production run path: capture the project at the working directory, compile
//! it to canonical image bytes, verify them into a sealed image, resolve the named
//! export, and run a storeless export on the VM. Each of the four failure families
//! surfaces as its own typed [`Record`]; the value or the first failure sets the
//! exit code.
//!
//! Durable execution is in the trough for `run`. A durable export (nonempty
//! verified demand) compiles, verifies, and completes its identity here, but the CLI
//! opens no store — T01's in-process open died at D00. The export is reported with
//! the typed `cli.durable_unsupported` outcome; durable `run` returns with the
//! persistent companion path (F02b). (Durable source *tests* already run against a
//! fresh ephemeral-memory attachment through `marrow test`; `run` does not attach.)
//! A fresh durable declaration with no ledger identity is still minted here — `run`
//! is the one convenience mint action; see [`mint_missing_identities`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use marrow_compile::{CompileFailure, ExportEntry, ExportId, SourceDiagnostic, compile};
use marrow_project::{DurableIdentityId, IdentityAnchor, ProjectInput};
use marrow_verify::{
    FunctionIndex, ImageType, Scalar, SealedEnumType, SealedRecordType, VerifiedImage,
};
use marrow_vm::Value;

use crate::outcome::Record;
use crate::project::capture_project;

/// The output format for `marrow run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Jsonl,
}

struct RunArgs {
    export: String,
    format: Format,
    call_args: Vec<String>,
}

pub(crate) fn run(rest: &[String]) -> ExitCode {
    let args = match parse_args(rest) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let project = match capture_project(&PathBuf::from(".")) {
        Ok(project) => project,
        Err(failure) => {
            return emit(
                args.format,
                &[Record::OperationalError {
                    code: failure.code,
                    detail: Some(failure.message),
                }],
                &[],
                &[],
                ExitCode::FAILURE,
            );
        }
    };

    // Family 1: source diagnostics. When compilation fails *only* because fresh
    // durable declarations lack ledger identities, `run` — and only `run` — mints
    // them into `marrow.ids` and compiles again; any other failure reports as-is.
    let compiled = match compile(&project) {
        Ok(compiled) => compiled,
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            match mint_missing_identities(&project, diagnostics.as_slice()) {
                MintOutcome::Minted => {
                    let recaptured = match capture_project(&PathBuf::from(".")) {
                        Ok(project) => project,
                        Err(failure) => {
                            return emit(
                                args.format,
                                &[Record::OperationalError {
                                    code: failure.code,
                                    detail: Some(failure.message),
                                }],
                                &[],
                                &[],
                                ExitCode::FAILURE,
                            );
                        }
                    };
                    match compile(&recaptured) {
                        Ok(compiled) => compiled,
                        Err(CompileFailure::Diagnostics(diagnostics)) => {
                            return emit(
                                args.format,
                                &diagnostic_records(diagnostics.as_slice()),
                                &[],
                                &[],
                                ExitCode::FAILURE,
                            );
                        }
                        Err(CompileFailure::ResourceLimit(_)) => {
                            return emit(
                                args.format,
                                &[compiler_resource_limit_record()],
                                &[],
                                &[],
                                ExitCode::FAILURE,
                            );
                        }
                        Err(CompileFailure::Invariant(_)) => {
                            return emit(
                                args.format,
                                &[compiler_invariant_record()],
                                &[],
                                &[],
                                ExitCode::FAILURE,
                            );
                        }
                    }
                }
                MintOutcome::NotApplicable => {
                    return emit(
                        args.format,
                        &diagnostic_records(diagnostics.as_slice()),
                        &[],
                        &[],
                        ExitCode::FAILURE,
                    );
                }
                MintOutcome::Failed(code) => {
                    return emit(
                        args.format,
                        &[Record::OperationalError { code, detail: None }],
                        &[],
                        &[],
                        ExitCode::FAILURE,
                    );
                }
            }
        }
        Err(CompileFailure::ResourceLimit(_)) => {
            return emit(
                args.format,
                &[compiler_resource_limit_record()],
                &[],
                &[],
                ExitCode::FAILURE,
            );
        }
        Err(CompileFailure::Invariant(_)) => {
            return emit(
                args.format,
                &[compiler_invariant_record()],
                &[],
                &[],
                ExitCode::FAILURE,
            );
        }
    };

    // Resolve the caller-supplied name to a stable id through the compiler's export
    // directory, before verification, so no source string reaches the image. The VM
    // dispatches only on this verified id.
    let export_id = match resolve_export(&compiled.exports, &args.export) {
        Ok(id) => id,
        Err(message) => return usage(&message),
    };

    // Family 2: artifact decode/verify rejection. The compiler cannot mint a
    // verified image — only `marrow_verify::verify` can.
    let image = match marrow_verify::verify(&compiled.image.bytes) {
        Ok(image) => image,
        Err(rejection) => {
            return emit(
                args.format,
                &[Record::ArtifactRejected {
                    code: rejection.code(),
                }],
                &[],
                &[],
                ExitCode::FAILURE,
            );
        }
    };

    let Some(export) = image.export_by_id(export_id) else {
        // The directory named an id the verified image does not carry: a compiler
        // bug, since the same draft produced both.
        eprintln!("internal error: export directory and image disagree");
        return ExitCode::FAILURE;
    };
    let func_index = export.function();
    let demand = export.demand();

    // Durable execution is in the trough for `run`: T01's in-process store open died
    // at D00. The export has compiled, verified, and completed its identity, but the
    // CLI opens no store, so a durable export (nonempty demand) is reported with the
    // typed trough outcome rather than run. Durable `run` returns with the persistent
    // companion path (F02b); durable source tests already run through `marrow test`.
    if !demand.is_empty() {
        return emit(
            args.format,
            &[Record::OperationalError {
                code: marrow_codes::Code::CliDurableUnsupported.as_str(),
                detail: None,
            }],
            &[],
            &[],
            ExitCode::FAILURE,
        );
    }

    // Positional call arguments are decoded against the verified export signature.
    let function = image.function(func_index);
    let call_args = match decode_args(function.params(), &args.call_args) {
        Ok(values) => values,
        Err(message) => return usage(&message),
    };

    // Family 3: source-mapped runtime fault, or the value.
    let record = run_storeless(&image, func_index, call_args);

    let exit = match &record {
        Record::Value(_) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    };
    emit(
        args.format,
        &[record],
        image.record_types(),
        image.enums(),
        exit,
    )
}

/// The typed diagnostic records for a compile failure.
fn diagnostic_records(diagnostics: &[SourceDiagnostic]) -> Vec<Record> {
    diagnostics
        .iter()
        .map(|diagnostic| Record::Diagnostic {
            code: diagnostic.code,
            line: diagnostic.line(),
            column: diagnostic.column(),
        })
        .collect()
}

fn compiler_invariant_record() -> Record {
    Record::OperationalError {
        code: marrow_codes::Code::CliCompilerInvariant.as_str(),
        detail: None,
    }
}

/// The fixed payload-free operational record for a compiler resource-limit outcome.
/// The typed limit's kind and bound are internal; the CLI surfaces one fixed code
/// with no detail, and never a source location or an image.
fn compiler_resource_limit_record() -> Record {
    Record::OperationalError {
        code: marrow_codes::Code::CliCompilerResourceLimit.as_str(),
        detail: None,
    }
}

/// What the `run` mint pre-pass did with a compile failure.
enum MintOutcome {
    /// Every diagnostic was a mintable identity gap; fresh identities were
    /// drawn and `marrow.ids` was published atomically.
    Minted,
    /// The failure is not (only) missing mintable identity; report it as-is.
    NotApplicable,
    /// Minting itself failed; `marrow.ids` is unchanged.
    Failed(&'static str),
}

/// The `marrow run` convenience mint: when a compile failed *only* because
/// fresh durable declarations have no ledger row, draw one id per missing
/// anchor from OS entropy and publish the grown ledger atomically
/// (temp + rename), leaving the artifact untouched on any failure.
///
/// This is a live bridge. Caller: `cmd_run` (this file), and nothing else —
/// `marrow test` and every other path fail precisely, so CI never mutates the
/// tree. Isolation: CLI orchestration only; the compiler stays a read-only
/// ledger consumer (its typed `IdentityGap` payloads are the sole input here —
/// the CLI never classifies durable declarations itself). Absence test:
/// `durable_identity.rs` asserts the CI path writes nothing. Deletion
/// condition: F03's accepted apply action becomes the one mint owner (with
/// D04 sending durable `run` into the trough), and this pre-pass is deleted
/// with the in-process store seam.
fn mint_missing_identities(
    project: &ProjectInput,
    diagnostics: &[SourceDiagnostic],
) -> MintOutcome {
    // Act when the compile reported at least one mintable identity gap and no
    // retired anchor. Non-gap diagnostics do not block the mint: an unminted
    // root cascades (every operation over it reports unsupported), and the gaps
    // themselves are emitted only for a durable declaration whose shape already
    // validated — the recompile reports whatever genuinely remains. A retired
    // anchor is never re-mintable, so its failure stays precise and unminted.
    let mut anchors: Vec<IdentityAnchor> = Vec::new();
    for diagnostic in diagnostics {
        match &diagnostic.identity {
            Some(gap) if gap.retired => return MintOutcome::NotApplicable,
            Some(gap) => {
                let anchor = gap.anchor();
                if !anchors.contains(&anchor) {
                    anchors.push(anchor);
                }
            }
            None => {}
        }
    }
    if anchors.is_empty() {
        return MintOutcome::NotApplicable;
    }

    let mut mints: Vec<(IdentityAnchor, DurableIdentityId)> = Vec::with_capacity(anchors.len());
    for anchor in anchors {
        let id = match draw_entropy_id() {
            Ok(id) => id,
            Err(_) => return MintOutcome::Failed(marrow_codes::Code::ProjectIdsMint.as_str()),
        };
        mints.push((anchor, id));
    }
    let ledger = project.identity_ledger().cloned().unwrap_or_default();
    let minted = match ledger.with_minted(&mints) {
        Ok(minted) => minted,
        // A collision or state conflict: no retry, and the artifact bytes are
        // untouched (`with_minted` never mutated the source ledger).
        Err(_) => return MintOutcome::Failed(marrow_codes::Code::ProjectIdsMint.as_str()),
    };
    match publish_ids(Path::new("."), &minted.to_bytes()) {
        Ok(()) => MintOutcome::Minted,
        Err(_) => MintOutcome::Failed(marrow_codes::Code::IoWrite.as_str()),
    }
}

/// One 128-bit id drawn from the OS entropy source. No clock, hash, provider,
/// or retry: a failure surfaces as-is and the caller aborts the mint.
#[cfg(unix)]
fn draw_entropy_id() -> std::io::Result<DurableIdentityId> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(DurableIdentityId::from_bytes(bytes))
}

#[cfg(not(unix))]
fn draw_entropy_id() -> std::io::Result<DurableIdentityId> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "durable identity minting requires an approved OS entropy source on this platform",
    ))
}

/// Publish `marrow.ids` atomically: write a sibling temp file, then rename it
/// over the artifact, so a reader observes either the old complete artifact or
/// the new one — never a torn write. The temp file is removed on failure.
fn publish_ids(root: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = root.join(marrow_project::IDS_FILE);
    let temp = root.join(format!(
        "{}.tmp.{}",
        marrow_project::IDS_FILE,
        std::process::id()
    ));
    if let Err(error) = std::fs::write(&temp, bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

/// Resolve a caller-supplied export path to its [`ExportId`] through the compiler's
/// export directory. A `module.item` path (one containing a `.`) matches a module
/// and item exactly; a bare item matches by item name and is an error when more
/// than one module exports it.
fn resolve_export(directory: &[ExportEntry], query: &str) -> Result<ExportId, String> {
    if let Some((module, item)) = query.rsplit_once('.') {
        return directory
            .iter()
            .find(|entry| entry.module == module && entry.item == item)
            .map(|entry| entry.id)
            .ok_or_else(|| format!("no exported function `{query}` in this project"));
    }
    let mut matching = directory.iter().filter(|entry| entry.item == query);
    let first = matching
        .next()
        .ok_or_else(|| format!("no exported function `{query}` in this project"))?;
    if matching.next().is_some() {
        return Err(format!(
            "`{query}` is exported by more than one module; qualify it as `module.{query}`"
        ));
    }
    Ok(first.id)
}

/// Run a storeless export.
fn run_storeless(
    image: &VerifiedImage,
    func_index: FunctionIndex,
    call_args: Vec<Value>,
) -> Record {
    match marrow_vm::run(image, func_index, call_args) {
        Ok(value) => Record::Value(value),
        Err(fault) => Record::Fault {
            code: fault.code(),
            line: fault.line(),
            column: fault.column(),
            detail: fault.detail().map(str::to_owned),
        },
    }
}

/// Decode positional CLI arguments against the export's parameter types. A scalar
/// parameter decodes from its text; a record (`struct`) parameter has no
/// command-line spelling, so an export taking one cannot be run from the terminal.
fn decode_args(params: &[ImageType], args: &[String]) -> Result<Vec<Value>, String> {
    if params.len() != args.len() {
        return Err(format!(
            "this export takes {} argument(s), found {}",
            params.len(),
            args.len()
        ));
    }
    params
        .iter()
        .zip(args)
        .map(|(param, text)| match param {
            ImageType::Scalar {
                scalar,
                optional: false,
            } => decode_arg(*scalar, text),
            _ => Err("a struct argument cannot be passed on the command line".to_string()),
        })
        .collect()
}

fn decode_arg(scalar: Scalar, text: &str) -> Result<Value, String> {
    match scalar {
        Scalar::Int => text
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("`{text}` is not an integer")),
        Scalar::Bool => match text {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(format!("`{text}` is not a boolean (true/false)")),
        },
        Scalar::Text => Ok(Value::Text(Rc::from(text))),
        // A `bytes` argument is a `0x`-prefixed even-length lowercase-hex string,
        // matching how a `bytes` value renders back out.
        Scalar::Bytes => decode_hex_bytes(text)
            .map(|bytes| Value::Bytes(Rc::from(bytes.as_slice())))
            .ok_or_else(|| format!("`{text}` is not `0x`-prefixed lowercase hex")),
        // A temporal argument is its canonical text, matching how it renders back out.
        Scalar::Date => marrow_temporal::parse_date(text.as_bytes())
            .map(Value::Date)
            .ok_or_else(|| format!("`{text}` is not a canonical date `YYYY-MM-DD`")),
        Scalar::Instant => marrow_temporal::parse_instant(text.as_bytes())
            .map(Value::Instant)
            .ok_or_else(|| format!("`{text}` is not a canonical UTC instant")),
        Scalar::Duration => marrow_temporal::parse_duration(text.as_bytes())
            .map(Value::Duration)
            .ok_or_else(|| format!("`{text}` is not a canonical duration `PT<seconds>S`")),
    }
}

/// Decode a `0x`-prefixed even-length lowercase-hex string to bytes.
fn decode_hex_bytes(text: &str) -> Option<Vec<u8>> {
    let hex = text.strip_prefix("0x")?;
    if !hex.len().is_multiple_of(2)
        || hex
            .bytes()
            .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
    {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn parse_args(rest: &[String]) -> Result<RunArgs, ExitCode> {
    let mut export: Option<String> = None;
    let mut format = Format::Text;
    let mut call_args: Vec<String> = Vec::new();
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--" => {
                call_args.extend(iter.by_ref().cloned());
                break;
            }
            "--format" => match iter.next().map(String::as_str) {
                Some("jsonl") => format = Format::Jsonl,
                Some("text") => format = Format::Text,
                _ => return Err(usage("`--format` must be `text` or `jsonl`")),
            },
            other if other.starts_with('-') => {
                return Err(usage(&format!("unknown run option: {other}")));
            }
            other => {
                if export.replace(other.to_string()).is_some() {
                    return Err(usage("marrow run takes one export name"));
                }
            }
        }
    }
    let Some(export) = export else {
        return Err(usage("marrow run needs an export name"));
    };
    Ok(RunArgs {
        export,
        format,
        call_args,
    })
}

fn usage(message: &str) -> ExitCode {
    eprintln!("{message}; run marrow --help for usage");
    ExitCode::from(2)
}

/// Emit records in the selected format and return `exit`. JSONL is one canonical
/// object per line (LF-terminated); text prints each record's rendering.
fn emit(
    format: Format,
    records: &[Record],
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    exit: ExitCode,
) -> ExitCode {
    for record in records {
        match format {
            Format::Jsonl => println!("{}", record.to_jsonl(types, enums)),
            Format::Text => {
                let text = record.to_text(types, enums);
                if !text.is_empty() {
                    println!("{text}");
                }
            }
        }
    }
    exit
}

#[cfg(test)]
mod compiler_invariant_tests {
    #[test]
    fn invariant_mapper_is_one_payload_free_operational_record() {
        assert_eq!(
            super::compiler_invariant_record(),
            super::Record::OperationalError {
                code: marrow_codes::Code::CliCompilerInvariant.as_str(),
                detail: None,
            }
        );
    }
}
