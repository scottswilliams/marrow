//! `marrow run <export> [--format jsonl] [-- <args>...]`.
//!
//! The production run path: capture the project at the working directory, compile
//! it to canonical image bytes, verify them into a sealed image, resolve the named
//! export, and run a storeless export on the VM. Each of the four failure families
//! surfaces as its own typed [`Record`]; the value or the first failure sets the
//! exit code.
//!
//! A durable export runs against a provisioned store with `marrow run … --store
//! <dir>`: the terminal never opens the store — it verifies the companion runner
//! against the release manifest, spawns it as an attached session, submits one call,
//! and renders the result ([`run_persistent`]). Without `--store` there is no store
//! to bind, so a durable export reports the typed `cli.durable_unsupported` outcome.
//! A fresh durable declaration with no ledger identity is minted only on the
//! storeless path — the run-mint window is closed for a persistent store; see
//! [`mint_missing_identities`].

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
    /// The persistent store to run against (`--store <dir>`). When present, the run is
    /// served by a companion attached to the store and no identity is auto-minted; when
    /// absent, the run is storeless and a missing durable identity is minted.
    store: Option<PathBuf>,
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
    // durable declarations lack ledger identities, storeless `run` — and only storeless
    // `run` — mints them into `marrow.ids` and compiles again; any other failure reports
    // as-is. The run-mint window is closed for a persistent store (`--store`): once a store
    // is bindable, a fresh anchor is a precise `check.durable_identity` failure the developer
    // resolves deliberately, never an additive auto-mint that could readopt an orphaned id or
    // diverge from the store's committed ledger. Tombstone-aware minting is the accepted
    // apply action's job (F03), not pulled forward here.
    let compiled = match compile(&project) {
        Ok(compiled) => compiled,
        Err(CompileFailure::Diagnostics(diagnostics)) if args.store.is_some() => {
            return emit(
                args.format,
                &diagnostic_records(diagnostics.as_slice()),
                &[],
                &[],
                ExitCode::FAILURE,
            );
        }
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
                        Err(CompileFailure::ResourceLimit(limit)) => {
                            return emit(
                                args.format,
                                &[compiler_resource_limit_record(limit)],
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
        Err(CompileFailure::ResourceLimit(limit)) => {
            return emit(
                args.format,
                &[compiler_resource_limit_record(limit)],
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

    // Persistent path: `marrow run … --store <dir>` runs the export against a provisioned
    // store. The CLI never opens the store — it verifies the companion runner against the
    // release manifest and spawns it as an attached session (durable or storeless), submits
    // one call, and renders the result. The spawn is invisible in ordinary output.
    if let Some(store_dir) = &args.store {
        let function = image.function(func_index);
        return run_persistent(
            args.format,
            &image,
            &compiled.image.bytes,
            *export_id.bytes(),
            store_dir,
            function.params(),
            &args.call_args,
        );
    }

    // Durable execution needs a store: without `--store`, T01's in-process store open died
    // at D00, so a durable export (nonempty demand) is reported with the typed trough
    // outcome rather than run. Durable source tests already run through `marrow test`.
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

/// The operational record for a compiler resource-limit outcome. It carries the typed
/// kind detail — which fixed aggregate bound was exhausted — so an operator can bisect
/// the limit; the numeric bound and any source location stay internal, and no image is
/// produced.
fn compiler_resource_limit_record(limit: marrow_compile::CompileResourceLimit) -> Record {
    Record::CompilerResourceLimit {
        kind_detail: limit.kind().detail(),
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

/// Run an export against a persistent store through the companion attached session, then
/// render the result exactly as a storeless run would — no runner, wire, or lifecycle
/// vocabulary reaches the output. Locates and verifies the companion against the release
/// manifest first; installation damage yields an actionable repair message.
fn run_persistent(
    format: Format,
    image: &VerifiedImage,
    image_bytes: &[u8],
    export_id: [u8; 32],
    store: &Path,
    params: &[ImageType],
    call_args: &[String],
) -> ExitCode {
    let runner = match crate::companion::discover_companion() {
        Ok(runner) => runner,
        Err(damage) => {
            eprintln!(
                "{}: {}",
                marrow_codes::Code::CliInstallationDamaged.as_str(),
                damage.message(),
            );
            return ExitCode::FAILURE;
        }
    };

    let args = match build_json_args(params, call_args) {
        Ok(args) => args,
        Err(code) => return code,
    };

    match marrow_runner::attach_and_call(&runner, image, image_bytes, store, export_id, args) {
        Ok(outcome) => {
            let record = call_outcome_to_record(outcome);
            let exit = match &record {
                Record::Value(_) => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            };
            emit(format, &[record], image.record_types(), image.enums(), exit)
        }
        Err(error) => {
            eprintln!("{}", error.code());
            ExitCode::FAILURE
        }
    }
}

/// Map a companion call outcome onto the terminal's outcome record. A durable-shape reject is
/// reported as the store-run trough outcome; other typed rejects keep their code.
fn call_outcome_to_record(outcome: marrow_runner::CallOutcome) -> Record {
    match outcome {
        marrow_runner::CallOutcome::Value(value) => Record::Value(value),
        marrow_runner::CallOutcome::Fault { code, line, column } => Record::Fault {
            code: fault_code(&code),
            line,
            column,
            detail: None,
        },
        marrow_runner::CallOutcome::Reject { code } => Record::OperationalError {
            code: if code == marrow_codes::Code::RunnerDurableUnsupported.as_str() {
                marrow_codes::Code::CliDurableUnsupported.as_str()
            } else {
                fault_code(&code)
            },
            detail: None,
        },
    }
}

/// Intern a wire-carried dotted code back to its static form so the record renders the stable
/// code without allocating; an unrecognized code (never expected on the beta line) falls back
/// to an internal invariant code rather than leaking an unstyled string.
fn fault_code(code: &str) -> &'static str {
    marrow_codes::Code::from_code(code)
        .map(marrow_codes::Code::as_str)
        .unwrap_or_else(|| marrow_codes::Code::CliCompilerInvariant.as_str())
}

/// Build the wire JSON arguments for a persistent call from the command-line strings and the
/// export's parameter types, validating the same way a storeless run does. A record or
/// optional parameter has no command-line spelling.
fn build_json_args(
    params: &[ImageType],
    args: &[String],
) -> Result<Vec<marrow_runner::Json>, ExitCode> {
    if params.len() != args.len() {
        return Err(usage(&format!(
            "this export takes {} argument(s), found {}",
            params.len(),
            args.len()
        )));
    }
    let mut out = Vec::with_capacity(params.len());
    for (param, text) in params.iter().zip(args) {
        match param {
            ImageType::Scalar {
                scalar,
                optional: false,
            } => out.push(scalar_to_json(*scalar, text).map_err(|message| usage(&message))?),
            _ => {
                return Err(usage(
                    "a struct argument cannot be passed on the command line",
                ));
            }
        }
    }
    Ok(out)
}

/// Validate a command-line scalar against its type and produce its wire JSON. Numbers and
/// booleans carry their JSON kind; the text-shaped scalars (text/bytes/temporal) travel as a
/// canonical string, validated here so a malformed value is a terminal usage error rather
/// than a wire rejection.
fn scalar_to_json(scalar: Scalar, text: &str) -> Result<marrow_runner::Json, String> {
    use marrow_runner::Json;
    match scalar {
        Scalar::Int => text
            .parse::<i64>()
            .map(Json::Int)
            .map_err(|_| format!("`{text}` is not an integer")),
        Scalar::Bool => match text {
            "true" => Ok(Json::Bool(true)),
            "false" => Ok(Json::Bool(false)),
            _ => Err(format!("`{text}` is not a boolean (true/false)")),
        },
        Scalar::Text => Ok(Json::Str(text.to_string())),
        Scalar::Bytes => decode_hex_bytes(text)
            .map(|_| Json::Str(text.to_string()))
            .ok_or_else(|| format!("`{text}` is not `0x`-prefixed lowercase hex")),
        Scalar::Date => marrow_temporal::parse_date(text.as_bytes())
            .map(|_| Json::Str(text.to_string()))
            .ok_or_else(|| format!("`{text}` is not a canonical date `YYYY-MM-DD`")),
        Scalar::Instant => marrow_temporal::parse_instant(text.as_bytes())
            .map(|_| Json::Str(text.to_string()))
            .ok_or_else(|| format!("`{text}` is not a canonical UTC instant")),
        Scalar::Duration => marrow_temporal::parse_duration(text.as_bytes())
            .map(|_| Json::Str(text.to_string()))
            .ok_or_else(|| format!("`{text}` is not a canonical duration `PT<seconds>S`")),
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
    let mut store: Option<PathBuf> = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--" => {
                call_args.extend(iter.by_ref().cloned());
                break;
            }
            "--store" => match iter.next() {
                Some(dir) => store = Some(PathBuf::from(dir)),
                None => return Err(usage("`--store` needs a store directory")),
            },
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
        store,
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
