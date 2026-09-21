//! `marrow run <export> [--stdin] [--format jsonl] [-- <args>...]`.
//!
//! The production run path: capture the project at the working directory, compile
//! it to canonical image bytes, verify them into a sealed image, resolve the named
//! export, and run a storeless export on the VM. Invocation outcomes retain their
//! typed [`Record`]. Input refusal precedes invocation; rendering or delivery
//! failure makes the command fail even if the invocation has completed.
//!
//! A durable export runs against a provisioned store with `marrow run … --store
//! <dir>`: the terminal never opens the store — it verifies the companion runner
//! against the release manifest, spawns it as an attached session, submits one call,
//! and renders the result ([`run_persistent`]). Without `--store` there is no store
//! to bind, so a durable export reports the typed `cli.durable_unsupported` outcome.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use marrow_codes::Code;
use marrow_compile::{CompileFailure, ExportEntry, ExportId, SourceDiagnostic, compile};
use marrow_project::{
    DurableIdentityId, IdentityAnchor, IdentityMintFailure, ProjectInput, SourceOrigin,
};
use marrow_project_fs::IdsPublication;
use marrow_verify::{
    ImageType, Scalar, SealedEnumType, SealedRecordType, VerifiedFunction, VerifiedImage,
};
use marrow_vm::Value;

use crate::Command;
use crate::command_output::{Flags, OutputFormat, once, unknown_option, usage};
use crate::outcome::{MAX_TEXT_BYTES, Record};
use crate::project::capture_project;
use crate::term_style::{Palette, Stream};

pub(crate) const HELP: &str = "\
Usage:
  marrow run <export> [--stdin] [--store <dir>] [--format text|jsonl] [-- <args>...]

Compile and verify the project at the working directory, then run one exported
function, named bare or as `module.item`. Arguments after `--` are decoded in order
against the export's scalar parameters; --stdin instead supplies one string of at
most 65536 UTF-8 bytes to an export taking exactly one string parameter. A durable
export runs against the store named by --store through the companion runner; without
--store a durable export is `cli.durable_unsupported`. The first storeless run of a
project with durable declarations writes `.marrow/ids`.

Text output is the returned value: a top-level `ok(v)` prints `v`, and a top-level
`err(e)` prints `error: e` on standard error and exits 1. JSONL output is one
record. A diagnostic prints as `file:line:column: code: message`.
";

struct RunArgs {
    export: String,
    format: OutputFormat,
    call_args: CallArgs,
    /// The persistent store to run against (`--store <dir>`). When present, the run is
    /// served by a companion attached to the store and no identity is auto-minted; when
    /// absent, the run is storeless and a missing durable identity is minted.
    store: Option<PathBuf>,
}

#[derive(Debug)]
enum CallArgs {
    Positional(Vec<String>),
    Stdin,
}

#[derive(Debug)]
enum ArgumentError {
    Usage(String),
    /// An argument's text crosses the language text bound, carrying its byte length.
    Limit(usize),
    Input(io::Error),
}

/// What one `run` produced: the records to emit and the exit they imply. A refusal
/// already written to standard error carries no record, so the emit writes nothing.
struct Outcome {
    records: Vec<Record>,
    exit: ExitCode,
}

impl Outcome {
    fn failed(records: Vec<Record>) -> Outcome {
        Outcome {
            records,
            exit: ExitCode::FAILURE,
        }
    }

    fn operational(code: Code, detail: Option<String>) -> Outcome {
        Outcome::failed(vec![Record::OperationalError { code, detail }])
    }

    /// A refusal already reported on standard error; there is nothing to emit.
    fn reported(exit: ExitCode) -> Outcome {
        Outcome {
            records: Vec::new(),
            exit,
        }
    }

    /// The exit the records imply once the invocation has settled: success only when
    /// every record is a completed value — a returned top-level `err` is a failure —
    /// and the exit already recorded was success.
    fn settled(self, enums: &[SealedEnumType]) -> Outcome {
        let completed = self.records.iter().all(|record| record.completed(enums));
        Outcome {
            exit: if completed {
                self.exit
            } else {
                ExitCode::FAILURE
            },
            records: self.records,
        }
    }
}

pub(crate) fn run(rest: &[String]) -> ExitCode {
    let args = match parse_args(rest) {
        Ok(args) => args,
        Err(code) => return code,
    };
    // The verified image outlives the records, so a returned value renders against
    // that image's own record and enum tables at the single emit below.
    let mut image = None;
    let outcome = run_inner(&args, &mut image).unwrap_or_else(|outcome| outcome);
    let (types, enums) = match &image {
        Some(image) => (image.record_types(), image.enums()),
        None => ([].as_slice(), [].as_slice()),
    };
    emit(args.format, &outcome.records, types, enums, outcome.exit)
}

/// The two sinks text output is delivered to, each with the palette its stream admits.
struct Sinks<'a, O: Write, E: Write> {
    out: &'a mut O,
    out_palette: Palette,
    err: &'a mut E,
    err_palette: Palette,
}

fn run_inner(args: &RunArgs, image_slot: &mut Option<VerifiedImage>) -> Result<Outcome, Outcome> {
    // A live `.marrow/ids` publication marker makes the committed ledger
    // indeterminate, so the one command that writes the ledger settles any
    // interrupted publication before it reads the project. Every other front
    // door reports the marker instead.
    crate::project::recover_identity_publication(Path::new("."))
        .map_err(|failure| Outcome::failed(vec![Record::capture(failure)]))?;

    let project = capture_project(Path::new("."))
        .map_err(|failure| Outcome::failed(vec![Record::capture(failure)]))?;

    let compiled = compile_or_mint(&project, args.store.is_some())?;

    // Resolve the caller-supplied name to a stable id through the compiler's export
    // directory, before verification, so no source string reaches the image. The VM
    // dispatches only on this verified id.
    let export_id = resolve_export(&compiled.exports, &args.export)
        .map_err(|message| Outcome::reported(usage(Command::Run, &message)))?;

    // Family 2: artifact decode/verify rejection. The compiler cannot mint a
    // verified image — only `marrow_verify::verify` can.
    let verified = marrow_verify::verify(&compiled.image.bytes).map_err(|rejection| {
        Outcome::failed(vec![Record::ArtifactRejected {
            code: rejection.code(),
        }])
    })?;
    let image: &VerifiedImage = image_slot.insert(verified);

    let Some(export) = image.export_by_id(export_id) else {
        // The directory named an id the verified image does not carry: a compiler
        // bug, since the same draft produced both.
        return Err(Outcome::operational(
            marrow_codes::Code::CliCompilerInvariant,
            Some("the export directory and the verified image disagree".to_string()),
        ));
    };
    let function = image
        .function(export.function())
        .expect("verified export function");

    // The companion spawn is invisible in ordinary output.
    if let Some(store_dir) = &args.store {
        return run_persistent(
            image,
            &compiled.image.bytes,
            *export_id.bytes(),
            store_dir,
            function.body().params(),
            &args.call_args,
        )
        .map(|outcome| outcome.settled(image.enums()));
    }

    // Durable execution needs a store, and the terminal opens none. Durable source
    // tests run through `marrow test`.
    if !function.demand().is_empty() {
        return Err(Outcome::operational(
            marrow_codes::Code::CliDurableUnsupported,
            None,
        ));
    }

    let call_args = decode_call_args(function.body().params(), &args.call_args)?;

    // Family 3: source-mapped runtime fault, the value, or the program's own `err`.
    let record = run_storeless(function, call_args);
    Ok(Outcome {
        records: vec![record],
        exit: ExitCode::SUCCESS,
    }
    .settled(image.enums()))
}

/// Compile the captured project. Family 1 is source diagnostics; when compilation
/// fails *only* because fresh durable declarations lack ledger identities, storeless
/// `run` mints them into `.marrow/ids` and compiles again. `--store` closes that
/// window; see [`mint_missing_identities`].
fn compile_or_mint(
    project: &ProjectInput,
    has_store: bool,
) -> Result<marrow_compile::Compiled, Outcome> {
    match compile(project) {
        Ok(compiled) => Ok(compiled),
        Err(CompileFailure::Diagnostics(diagnostics)) if !has_store => {
            match mint_missing_identities(project, diagnostics.as_slice()) {
                MintOutcome::Minted => {
                    let recaptured = capture_project(Path::new("."))
                        .map_err(|failure| Outcome::failed(vec![Record::capture(failure)]))?;
                    compile(&recaptured)
                        .map_err(|failure| Outcome::failed(Record::compile_failure(&failure)))
                }
                MintOutcome::NotApplicable => {
                    Err(Outcome::failed(Record::diagnostics(diagnostics.as_slice())))
                }
                MintOutcome::Failed(code, detail) => Err(Outcome::operational(code, detail)),
            }
        }
        Err(failure) => Err(Outcome::failed(Record::compile_failure(&failure))),
    }
}

/// What the `run` mint pre-pass did with a compile failure.
enum MintOutcome {
    /// Every diagnostic was a mintable identity gap; fresh identities were
    /// drawn and `.marrow/ids` was published atomically.
    Minted,
    /// The failure is not (only) missing mintable identity; report it as-is.
    NotApplicable,
    /// Minting itself failed; `.marrow/ids` is unchanged.
    Failed(Code, Option<String>),
}

/// The `marrow run` mint: when a compile failed *only* because fresh durable
/// declarations have no ledger row, draw one id per missing anchor from OS entropy
/// and hand the admitted successor to the adapter's publication owner, which compares
/// it against the filesystem and installs it or refuses. The artifact is untouched on
/// any refusal.
///
/// Storeless `run` is the only path that mints: `marrow check`, `marrow test` and
/// every other command report `check.durable_identity` precisely, so a build never
/// mutates the tree. Once a store is bindable an additive auto-mint could readopt an
/// orphaned id or diverge from the store's committed ledger, so `--store` closes the
/// window too. The compiler stays a read-only ledger consumer: its typed
/// `IdentityGap` payloads are the sole input here, and the CLI never classifies
/// durable declarations itself.
///
/// Minting is the root project's alone. A dependency commits its own ledger in its own
/// tree, so a gap a dependency declares is left to its own `check.durable_identity`
/// report, which steers the developer to run `marrow run` in the library and commit the
/// ledger it publishes there. Only the root path ever reaches the publication guard.
fn mint_missing_identities(
    project: &ProjectInput,
    diagnostics: &[SourceDiagnostic],
) -> MintOutcome {
    // Non-gap diagnostics do not block the mint: an unminted root cascades, and a
    // gap is emitted only for a durable declaration whose shape already validated,
    // so the recompile reports whatever genuinely remains. A retired anchor is never
    // re-mintable, so its failure stays precise and unminted.
    let mut anchors: Vec<IdentityAnchor> = Vec::new();
    for diagnostic in diagnostics {
        match diagnostic.identity_gap() {
            Some(gap) if *gap.origin() != SourceOrigin::Root => {}
            Some(gap) if gap.retired() => return MintOutcome::NotApplicable,
            Some(gap) => anchors.push(gap.anchor()),
            None => {}
        }
    }
    let mut anchors = anchors.into_iter();
    let Some(first) = anchors.next() else {
        return MintOutcome::NotApplicable;
    };
    let rest = anchors.collect();
    // Before entropy as well as before capture: entropy drawn against a ledger
    // whose committed generation is still undecided would be admitted against
    // the wrong state.
    if let Err(failure) = crate::project::recover_identity_publication(Path::new(".")) {
        return MintOutcome::Failed(failure.code, None);
    }
    let publication = match project.admit_identity_mints_with(first, rest, |exact_count| {
        let mut candidates = Vec::with_capacity(exact_count);
        for _ in 0..exact_count {
            candidates.push(draw_entropy_id()?);
        }
        Ok::<_, std::io::Error>(candidates)
    }) {
        Ok(publication) => publication,
        Err(failure) => {
            let (code, detail) = mint_failure_record(&failure);
            return MintOutcome::Failed(code, detail);
        }
    };
    match crate::project::publish_identity_ledger(Path::new("."), publication) {
        Ok(IdsPublication::Published) => MintOutcome::Minted,
        // The ledger was replaced between admission and publication, so the
        // successor was never installed and the artifact is the other writer's.
        Ok(IdsPublication::ConcurrentChange) => {
            MintOutcome::Failed(marrow_codes::Code::ProjectIdsMint, None)
        }
        Err(failure) => MintOutcome::Failed(failure.code, None),
    }
}

/// The code and detail a refused mint reports: a candidate-supply failure is the
/// entropy read's own operational failure carrying its text, and an admission
/// refusal is the ledger's code.
fn mint_failure_record(failure: &IdentityMintFailure<io::Error>) -> (Code, Option<String>) {
    match failure {
        IdentityMintFailure::Supply(error) => {
            (Code::IoRead, Some(format!("entropy source: {error}")))
        }
        IdentityMintFailure::Mutation(refusal) => (refusal.code(), None),
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
fn run_storeless(function: VerifiedFunction<'_>, call_args: Vec<Value>) -> Record {
    match marrow_vm::run(function, call_args) {
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
    image: &VerifiedImage,
    image_bytes: &[u8],
    export_id: [u8; 32],
    store: &Path,
    params: &[ImageType],
    call_args: &CallArgs,
) -> Result<Outcome, Outcome> {
    let runner = crate::companion::discover_companion().map_err(|damage| {
        let _ = writeln!(
            io::stderr().lock(),
            "{}: {}",
            marrow_codes::Code::CliInstallationDamaged.as_str(),
            damage.message(),
        );
        Outcome::reported(ExitCode::FAILURE)
    })?;

    // Installation discovery precedes argument consumption on the persistent path.
    let values = decode_call_args(params, call_args)?;
    let Some(args) = values.iter().map(value_to_wire).collect::<Option<Vec<_>>>() else {
        return Err(Outcome::reported(usage(
            Command::Run,
            "this export cannot be called from the terminal",
        )));
    };

    Ok(attached_records(marrow_runner::attach_and_call(
        &runner,
        image,
        image_bytes,
        store,
        export_id,
        args,
    )))
}

/// The records a companion call produced: the call's own outcome, then a cleanup
/// failure when the companion was not reaped. The exit is [`Outcome::settled`]'s to
/// decide from those records.
fn attached_records(completion: marrow_runner::AttachCompletion) -> Outcome {
    let record = match completion.outcome {
        Ok(outcome) => call_outcome_to_record(outcome),
        Err(marrow_runner::ClientError::ActivationUncertain { instance }) => {
            Record::ActivationUncertain { instance }
        }
        Err(marrow_runner::ClientError::ActivationOutcomeUnknown { cause }) => {
            Record::ActivationOutcomeUnknown {
                cause_code: cause.code(),
            }
        }
        Err(error) => Record::OperationalError {
            code: error.code(),
            detail: None,
        },
    };
    let mut records = vec![record];
    if let Err(error) = completion.cleanup {
        records.push(cleanup_record(error));
    }
    Outcome {
        records,
        exit: ExitCode::SUCCESS,
    }
}

fn cleanup_record(error: marrow_runner::CompanionCleanupError) -> Record {
    match error {
        marrow_runner::CompanionCleanupError::Unreaped {
            child,
            staging,
            cause,
        } => Record::CompanionUnreaped {
            pid: child.id(),
            staging: staging.display().to_string(),
            cause: cause.to_string(),
        },
        marrow_runner::CompanionCleanupError::Staging { path, cause } => Record::CompanionStaging {
            path: path.display().to_string(),
            cause: cause.to_string(),
        },
    }
}

/// Map a companion call outcome onto the terminal's outcome record. A durable-shape reject is
/// reported as the store-run trough outcome; other typed rejects keep their code.
fn call_outcome_to_record(outcome: marrow_runner::CallOutcome) -> Record {
    match outcome {
        marrow_runner::CallOutcome::Value(value) => Record::Value(value),
        marrow_runner::CallOutcome::Fault { code, line, column } => Record::Fault {
            code,
            line,
            column,
            detail: None,
        },
        marrow_runner::CallOutcome::Incomplete {
            code,
            durable,
            line,
            column,
        } => Record::Incomplete {
            code,
            durable,
            line,
            column,
        },
        marrow_runner::CallOutcome::Reject { code } => Record::OperationalError {
            code: if code == marrow_codes::Code::RunnerDurableUnsupported {
                marrow_codes::Code::CliDurableUnsupported
            } else {
                code
            },
            detail: None,
        },
        // Any failure to accept one exact valid reply after dispatch preserves
        // outcome-unknown and its orthogonal typed cause.
        marrow_runner::CallOutcome::OutcomeUnknown { cause } => Record::OutcomeUnknown {
            cause: cause.kind(),
            cause_code: cause.code(),
        },
    }
}

/// Project a validated command-line argument value onto its wire JSON. Total over the scalar
/// values [`decode_args`] produces (numbers and booleans carry their JSON kind; the
/// text-shaped scalars travel as their canonical string); a non-scalar — which the terminal
/// rejects at decode — yields `None`.
fn value_to_wire(value: &Value) -> Option<marrow_runner::Json> {
    use marrow_runner::Json;
    Some(match value {
        Value::Int(n) => Json::Int(*n),
        Value::Bool(b) => Json::Bool(*b),
        Value::Text(text) => Json::Str(text.to_string()),
        Value::Bytes(bytes) => Json::Str(marrow_vm::render::hex_bytes(bytes, MAX_TEXT_BYTES).ok()?),
        Value::Date(days) => Json::Str(marrow_temporal::format_date(*days)?),
        Value::Instant(nanos) => Json::Str(marrow_temporal::format_instant(*nanos)?),
        Value::Duration(nanos) => Json::Str(marrow_temporal::format_duration(*nanos)),
        _ => return None,
    })
}

fn decode_call_args(params: &[ImageType], args: &CallArgs) -> Result<Vec<Value>, Outcome> {
    materialize_args(params, args, &mut io::stdin().lock()).map_err(|error| match error {
        ArgumentError::Usage(message) => Outcome::reported(usage(Command::Run, &message)),
        ArgumentError::Limit(len) => Outcome::operational(
            marrow_codes::Code::CliArgumentLimit,
            Some(format!(
                "an argument carries {len} bytes of text; the limit is {MAX_TEXT_BYTES}"
            )),
        ),
        ArgumentError::Input(error) => {
            Outcome::operational(marrow_codes::Code::IoRead, Some(error.to_string()))
        }
    })
}

fn materialize_args(
    params: &[ImageType],
    args: &CallArgs,
    reader: &mut impl Read,
) -> Result<Vec<Value>, ArgumentError> {
    match args {
        CallArgs::Positional(args) => decode_args(params, args),
        CallArgs::Stdin => {
            if !matches!(
                params,
                [ImageType::Scalar {
                    scalar: Scalar::Text,
                    optional: false
                }]
            ) {
                return Err(ArgumentError::Usage(
                    "`--stdin` requires an export with exactly one nonoptional string parameter"
                        .into(),
                ));
            }
            let text = read_stdin(reader).map_err(ArgumentError::Input)?;
            decode_args(params, &[text])
        }
    }
}

fn read_stdin(reader: &mut impl Read) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stdin exceeds 65536 UTF-8 bytes",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "stdin is not valid UTF-8"))
}

/// Decode positional CLI arguments against the export's parameter types. A scalar
/// parameter decodes from its text; a record (`struct`) parameter has no
/// command-line spelling, so an export taking one cannot be run from the terminal.
///
/// The terminal is the boundary that admits an argument, so the language text bound
/// applies here rather than downstream: a storeless invocation runs in this process
/// with no wire, and a `--store` invocation crosses one, and both must refuse the
/// same argument. An argument's canonical text is what the bound measures, which is
/// also exactly what [`value_to_wire`] puts on the wire for every scalar it carries.
fn decode_args(params: &[ImageType], args: &[String]) -> Result<Vec<Value>, ArgumentError> {
    if params.len() != args.len() {
        return Err(ArgumentError::Usage(format!(
            "this export takes {} argument(s), found {}",
            params.len(),
            args.len()
        )));
    }
    params
        .iter()
        .zip(args)
        .map(|(param, text)| {
            if text.len() > MAX_TEXT_BYTES {
                return Err(ArgumentError::Limit(text.len()));
            }
            match param {
                ImageType::Scalar {
                    scalar,
                    optional: false,
                } => decode_arg(*scalar, text).map_err(ArgumentError::Usage),
                _ => Err(ArgumentError::Usage(
                    "a struct argument cannot be passed on the command line".to_string(),
                )),
            }
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
        Scalar::Bytes => marrow_vm::render::decode_hex_bytes(text)
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

fn parse_args(rest: &[String]) -> Result<RunArgs, ExitCode> {
    const COMMAND: Command = Command::Run;
    let mut export: Option<String> = None;
    let mut format: Option<OutputFormat> = None;
    let mut call_args = CallArgs::Positional(Vec::new());
    let mut store: Option<PathBuf> = None;
    let mut flags = Flags::new(rest, COMMAND);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--" => {
                match &mut call_args {
                    CallArgs::Positional(args) => args.extend(flags.by_ref().cloned()),
                    CallArgs::Stdin if flags.next().is_some() => {
                        return Err(usage(
                            COMMAND,
                            "`--stdin` cannot be combined with positional arguments",
                        ));
                    }
                    CallArgs::Stdin => {}
                }
                break;
            }
            "--stdin" => call_args = CallArgs::Stdin,
            "--store" => flags.read(&mut store, "--store", PathBuf::from)?,
            "--format" => flags.read_format(&mut format)?,
            other if other.starts_with('-') => return Err(unknown_option(COMMAND, other)),
            other => once(&mut export, other.to_string(), COMMAND, "export name")?,
        }
    }
    Ok(RunArgs {
        export: export.ok_or_else(|| usage(COMMAND, "marrow run takes an export name"))?,
        format: format.unwrap_or(OutputFormat::Text),
        call_args,
        store,
    })
}

/// Deliver the records: JSONL to standard output; text to standard output, except the
/// program's own `err` report, which goes to standard error. A sink failure is reported
/// on stderr if that channel remains writable. The call may already have completed;
/// output failure never dispatches it again.
fn emit(
    format: OutputFormat,
    records: &[Record],
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    exit: ExitCode,
) -> ExitCode {
    let sinks = Sinks {
        out: &mut io::stdout().lock(),
        out_palette: Palette::for_stream(Stream::Stdout),
        err: &mut io::stderr().lock(),
        err_palette: Palette::for_stream(Stream::Stderr),
    };
    crate::command_output::finish(emit_to(sinks, format, records, types, enums, exit))
}

fn emit_to(
    sinks: Sinks<'_, impl Write, impl Write>,
    format: OutputFormat,
    records: &[Record],
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    mut exit: ExitCode,
) -> io::Result<ExitCode> {
    for record in records {
        let (writer, palette): (&mut dyn Write, Palette) = match format {
            OutputFormat::Text if record.is_program_error(enums) => {
                (&mut *sinks.err, sinks.err_palette)
            }
            OutputFormat::Text | OutputFormat::Jsonl => (&mut *sinks.out, sinks.out_palette),
        };
        let render = |record: &Record| match format {
            OutputFormat::Jsonl => record.to_jsonl(types, enums),
            OutputFormat::Text => record.to_text(palette, types, enums),
        };
        // A value the output bound refuses is replaced by the typed refusal record,
        // which is payload-free and always renders, and the command fails.
        let text = render(record).unwrap_or_else(|()| {
            exit = ExitCode::FAILURE;
            let refusal = Record::OperationalError {
                code: marrow_codes::Code::IoWrite,
                detail: None,
            };
            render(&refusal).expect("a payload-free operational error always renders")
        });
        if format == OutputFormat::Jsonl || !text.is_empty() {
            writer.write_all(text.as_bytes())?;
            writer.write_all(b"\n")?;
        }
    }
    sinks.out.flush()?;
    sinks.err.flush()?;
    Ok(exit)
}

#[cfg(test)]
mod terminal_tests {
    use super::*;

    const TEXT_LIMIT: usize = 65_536;

    /// Both sinks as plain byte buffers.
    fn sinks<'a, O: Write, E: Write>(out: &'a mut O, err: &'a mut E) -> Sinks<'a, O, E> {
        Sinks {
            out,
            out_palette: Palette::for_test(false),
            err,
            err_palette: Palette::for_test(false),
        }
    }

    /// Emit into fresh buffers, returning `(exit, stdout, stderr)`.
    fn emitted(
        format: OutputFormat,
        records: &[Record],
        exit: ExitCode,
    ) -> (ExitCode, Vec<u8>, Vec<u8>) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = emit_to(sinks(&mut out, &mut err), format, records, &[], &[], exit)
            .expect("buffers accept every write");
        (exit, out, err)
    }

    #[test]
    fn attach_emits_known_outcome_and_cleanup_failure_as_separate_records() {
        let instance = "12".repeat(16);
        for (outcome, first) in [
            (
                Ok(marrow_runner::CallOutcome::Value(Some(Value::Int(7)))),
                Record::Value(Some(Value::Int(7))),
            ),
            (
                Err(marrow_runner::ClientError::ActivationUncertain {
                    instance: instance.clone(),
                }),
                Record::ActivationUncertain { instance },
            ),
            (
                Err(marrow_runner::ClientError::ActivationOutcomeUnknown {
                    cause: Box::new(marrow_runner::ClientError::Handshake),
                }),
                Record::ActivationOutcomeUnknown {
                    cause_code: Code::RunnerHandshake,
                },
            ),
        ] {
            let Outcome { records, exit } = attached_records(marrow_runner::AttachCompletion {
                outcome,
                cleanup: Err(marrow_runner::CompanionCleanupError::Staging {
                    path: PathBuf::from("/tmp/retained"),
                    cause: io::ErrorKind::PermissionDenied.into(),
                }),
            })
            .settled(&[]);
            assert_eq!(exit, ExitCode::FAILURE);
            assert_eq!(records.len(), 2);
            assert_eq!(records[0], first);
            assert!(
                matches!(&records[1], Record::CompanionStaging { path, .. } if path == "/tmp/retained")
            );
            let (exit, bytes, _) = emitted(OutputFormat::Jsonl, &records, exit);
            assert_eq!(exit, ExitCode::FAILURE);
            let text = String::from_utf8(bytes).expect("UTF-8 output");
            let lines: Vec<_> = text.lines().collect();
            assert_eq!(lines.len(), 2);
            assert_eq!(lines[0], first.to_jsonl(&[], &[]).unwrap());
            assert!(lines[1].contains("\"kind\":\"cleanup\""));
        }
    }

    fn text(value: &str) -> Record {
        Record::Value(Some(Value::Text(Rc::from(value))))
    }

    #[test]
    fn stdin_preserves_empty_control_and_exact_limit_utf8_bytes() {
        let params = [ImageType::scalar(Scalar::Text)];
        for input in [
            String::new(),
            "\0\r\ncafé\n".into(),
            "é".repeat(TEXT_LIMIT / 2),
        ] {
            let mut reader = input.as_bytes();
            let values = materialize_args(&params, &CallArgs::Stdin, &mut reader)
                .expect("bounded UTF-8 input");
            assert_eq!(values, vec![Value::Text(Rc::from(input.as_str()))]);
            assert!(reader.is_empty());
        }
    }

    struct RefusingReader {
        reads: usize,
    }

    impl Read for RefusingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            Err(io::ErrorKind::PermissionDenied.into())
        }
    }

    #[test]
    fn signature_and_positional_decoding_do_not_read_stdin() {
        let mut reader = RefusingReader { reads: 0 };
        for params in [
            vec![],
            vec![ImageType::scalar(Scalar::Int)],
            vec![ImageType::Scalar {
                scalar: Scalar::Text,
                optional: true,
            }],
            vec![ImageType::scalar(Scalar::Text); 2],
        ] {
            assert!(matches!(
                materialize_args(&params, &CallArgs::Stdin, &mut reader),
                Err(ArgumentError::Usage(_))
            ));
        }
        let values = materialize_args(
            &[ImageType::scalar(Scalar::Int)],
            &CallArgs::Positional(vec!["37".into()]),
            &mut reader,
        )
        .expect("ordinary scalar decoder");
        assert_eq!(values, vec![Value::Int(37)]);
        assert_eq!(reader.reads, 0);
    }

    #[test]
    fn invalid_utf8_and_reader_failure_remain_input_errors() {
        let params = [ImageType::scalar(Scalar::Text)];
        let mut invalid = b"valid prefix\xff".as_slice();
        let error =
            materialize_args(&params, &CallArgs::Stdin, &mut invalid).expect_err("invalid UTF-8");
        assert!(
            matches!(error, ArgumentError::Input(error) if error.kind() == io::ErrorKind::InvalidData)
        );
        let mut reader = RefusingReader { reads: 0 };
        let error =
            materialize_args(&params, &CallArgs::Stdin, &mut reader).expect_err("reader failure");
        assert!(
            matches!(error, ArgumentError::Input(error) if error.kind() == io::ErrorKind::PermissionDenied)
        );
        assert_eq!(reader.reads, 1);
    }

    #[test]
    fn excess_input_is_refused_without_reading_to_eof() {
        struct OpenInput(usize);
        impl Read for OpenInput {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if buffer.is_empty() {
                    return Ok(0);
                }
                assert!(
                    self.0 < TEXT_LIMIT + 1,
                    "must not wait for the next input byte"
                );
                let count = buffer.len().min(TEXT_LIMIT + 1 - self.0);
                buffer[..count].fill(b'a');
                self.0 += count;
                Ok(count)
            }
        }
        let mut reader = OpenInput(0);
        let error = read_stdin(&mut reader).expect_err("one excess byte");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(reader.0, TEXT_LIMIT + 1);
    }

    #[test]
    fn stdin_accepts_an_empty_tail_and_a_persistent_destination() {
        let args = ["report", "--stdin", "--store", "store", "--"].map(String::from);
        let parsed = parse_args(&args).expect("stdin and store flags");
        assert!(matches!(parsed.call_args, CallArgs::Stdin));
        assert_eq!(parsed.store, Some(PathBuf::from("store")));
        let conflict = ["report", "--stdin", "--", "argument"].map(String::from);
        assert!(matches!(parse_args(&conflict), Err(exit) if exit == ExitCode::from(2)));
    }

    #[test]
    fn render_refusal_sets_failed_status_before_any_value_bytes() {
        for (format, expected) in [
            (OutputFormat::Text, "io.write\n"),
            (
                OutputFormat::Jsonl,
                "{\"code\":\"io.write\",\"kind\":\"run\",\"outcome\":\"error\"}\n",
            ),
        ] {
            let (exit, output, err) = emitted(
                format,
                &[text(&"a".repeat(TEXT_LIMIT + 1))],
                ExitCode::SUCCESS,
            );
            assert_eq!(exit, ExitCode::FAILURE);
            assert_eq!(output, expected.as_bytes());
            assert!(err.is_empty());
        }
        let bytes = Record::Value(Some(Value::Bytes(vec![0; TEXT_LIMIT / 2].into())));
        let (exit, output, _) = emitted(OutputFormat::Jsonl, &[bytes], ExitCode::SUCCESS);
        assert_eq!(exit, ExitCode::FAILURE);
        assert_eq!(
            output,
            b"{\"code\":\"io.write\",\"kind\":\"run\",\"outcome\":\"error\"}\n"
        );
    }

    #[test]
    fn output_keeps_existing_empty_and_newline_framing() {
        for (value, expected) in [("", ""), ("a", "a\n"), ("a\n", "a\n\n")] {
            let (exit, output, err) =
                emitted(OutputFormat::Text, &[text(value)], ExitCode::SUCCESS);
            assert_eq!(exit, ExitCode::SUCCESS);
            assert_eq!(output, expected.as_bytes());
            assert!(err.is_empty());
        }
        let (_, output, _) = emitted(
            OutputFormat::Jsonl,
            &[text(&"\0".repeat(TEXT_LIMIT))],
            ExitCode::SUCCESS,
        );
        assert_eq!(output.len(), 393_259);
        assert!(output.ends_with(b"\n"));
    }

    #[test]
    fn write_and_flush_errors_preserve_only_the_delivered_prefix() {
        struct FailingWriter {
            remaining: usize,
            bytes: Vec<u8>,
        }
        impl Write for FailingWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.remaining == 0 {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                let count = self.remaining.min(bytes.len());
                self.bytes.extend_from_slice(&bytes[..count]);
                self.remaining -= count;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        for accepted in [0, 3, usize::MAX] {
            let mut writer = FailingWriter {
                remaining: accepted,
                bytes: Vec::new(),
            };
            let mut err = Vec::new();
            let error = emit_to(
                sinks(&mut writer, &mut err),
                OutputFormat::Text,
                &[text("report")],
                &[],
                &[],
                ExitCode::SUCCESS,
            )
            .expect_err("sink refuses a write or its final flush");
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
            assert_eq!(writer.bytes, b"report\n"[..accepted.min(7)]);
        }
    }
}

#[cfg(test)]
mod mint_tests {
    use super::*;

    #[test]
    fn a_supply_failure_reports_the_read_code_with_its_text_and_a_refusal_the_mint_code() {
        let supply = IdentityMintFailure::Supply(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "urandom closed",
        ));
        assert_eq!(
            mint_failure_record(&supply),
            (
                Code::IoRead,
                Some("entropy source: urandom closed".to_string())
            ),
        );

        let refusal =
            IdentityMintFailure::Mutation(marrow_project::IdentityMutationError::CandidateCount {
                expected: 1,
                actual: 0,
            });
        assert_eq!(mint_failure_record(&refusal), (Code::ProjectIdsMint, None));
    }
}
