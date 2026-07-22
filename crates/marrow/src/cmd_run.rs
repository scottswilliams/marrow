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
    // `run` — mints them into `.marrow/ids` and compiles again; any other failure reports
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
    /// drawn and `.marrow/ids` was published atomically.
    Minted,
    /// The failure is not (only) missing mintable identity; report it as-is.
    NotApplicable,
    /// Minting itself failed; `.marrow/ids` is unchanged.
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
        Ok(()) => {
            emit_commit_steer(Path::new("."));
            MintOutcome::Minted
        }
        Err(_) => MintOutcome::Failed(marrow_codes::Code::IoWrite.as_str()),
    }
}

/// After a mint publishes the ledger, steer the developer to commit it: when the
/// project sits inside a Git repository whose index lacks `.marrow/ids` (the file
/// is untracked or ignored), print a one-line stderr notice. Informational only —
/// it never affects records, exit codes, or the published artifact.
fn emit_commit_steer(root: &Path) {
    if ledger_absent_from_git_index(root) == Some(true) {
        eprintln!(
            "note: {} is not tracked by Git; commit it — durable identity travels with the source",
            marrow_project::IDS_FILE
        );
    }
}

/// Whether a surrounding Git repository's index lacks the ledger path.
/// `None` means no repository was found or the probe was not cheap (no notice
/// either way). The probe is dependency-free: walk up to the nearest `.git`,
/// resolve a worktree's `gitdir:` file, and scan the binary index once for the
/// ledger's path bytes. Index entries store paths literally in versions 2 and 3
/// (Git's defaults); a prefix-compressed v4 index may miss the path and repeat
/// the notice, which is acceptable for a one-line steer.
fn ledger_absent_from_git_index(root: &Path) -> Option<bool> {
    /// Directory levels searched above the project root before giving up.
    const MAX_ASCENT: usize = 64;
    /// Largest index read for the probe; a bigger index skips the notice.
    const MAX_INDEX_BYTES: u64 = 64 << 20;
    let mut dir = root.canonicalize().ok()?;
    for _ in 0..MAX_ASCENT {
        let dot_git = dir.join(".git");
        let git_dir = if dot_git.is_dir() {
            Some(dot_git.clone())
        } else if dot_git.is_file() {
            // A linked worktree: `.git` is a file `gitdir: <path>`.
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let target = text.strip_prefix("gitdir:")?.trim();
            let target = Path::new(target);
            Some(if target.is_absolute() {
                target.to_path_buf()
            } else {
                dir.join(target)
            })
        } else {
            None
        };
        if let Some(git_dir) = git_dir {
            let index = git_dir.join("index");
            let Ok(metadata) = std::fs::metadata(&index) else {
                // A repository with no index tracks nothing yet.
                return Some(true);
            };
            if metadata.len() > MAX_INDEX_BYTES {
                return None;
            }
            let bytes = std::fs::read(&index).ok()?;
            let needle = marrow_project::IDS_FILE.as_bytes();
            return Some(!bytes.windows(needle.len()).any(|window| window == needle));
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
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

/// Publish `.marrow/ids` atomically and durably: create the project-metadata
/// directory, sweep temp debris a crashed earlier publish left behind, write a
/// sibling temp file and sync its data to disk, rename it over the artifact,
/// then sync the directory so the rename itself survives power loss. A reader
/// observes either the old complete artifact or the new one — never a torn
/// write — and the data sync before the rename keeps that true across power
/// loss (an unsynced rename can be journaled ahead of the data blocks). The
/// temp file is removed on failure.
fn publish_ids(root: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let meta = root.join(marrow_project::META_DIR);
    std::fs::create_dir_all(&meta)?;
    sweep_stale_publish_temps(&meta);
    let target = root.join(marrow_project::IDS_FILE);
    let temp = root.join(format!(
        "{}.tmp.{}",
        marrow_project::IDS_FILE,
        std::process::id()
    ));
    if let Err(error) = write_synced(&temp, bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    sync_directory(&meta)
}

/// Write `bytes` to `path` and sync the file's data to disk before returning,
/// so a rename that follows never becomes visible ahead of the content.
fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Sync a directory so a completed rename inside it survives power loss.
/// Directory handles are only opennable for sync on Unix; the mint path is
/// already Unix-gated by the entropy source.
fn sync_directory(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Remove `ids.tmp.*` siblings a crashed earlier publish left behind. A crash
/// between temp write and rename leaves the mint gap open, so the next durable
/// run republishes and lands here: the committed metadata directory never
/// accumulates debris. Removal is best-effort — publication must not fail over
/// housekeeping.
fn sweep_stale_publish_temps(meta: &Path) {
    let Ok(entries) = std::fs::read_dir(meta) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("ids.tmp."))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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

    // Validate the command-line arguments once, exactly as a storeless run does (text →
    // `Value`), then project each onto its wire JSON. The terminal never passes a struct.
    let values = match decode_args(params, call_args) {
        Ok(values) => values,
        Err(message) => return usage(&message),
    };
    let Some(args) = values.iter().map(value_to_wire).collect::<Option<Vec<_>>>() else {
        return usage("this export cannot be called from the terminal");
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
        marrow_runner::CallOutcome::Incomplete {
            code,
            durable,
            line,
            column,
        } => Record::Incomplete {
            code: fault_code(&code),
            durable: match durable {
                marrow_runner::DurableState::KnownOld => marrow_vm::DurableCommitState::KnownOld,
                marrow_runner::DurableState::KnownNew => marrow_vm::DurableCommitState::KnownNew,
                marrow_runner::DurableState::Unknown => marrow_vm::DurableCommitState::Unknown,
            },
            line,
            column,
        },
        marrow_runner::CallOutcome::Reject { code } => Record::OperationalError {
            code: if code == marrow_codes::Code::RunnerDurableUnsupported.as_str() {
                marrow_codes::Code::CliDurableUnsupported.as_str()
            } else {
                fault_code(&code)
            },
            detail: None,
        },
        // A dispatched call whose reply was lost to the runner's death: a distinct typed
        // outcome, never a generic timeout. The store may or may not have changed; the call
        // was not retried; a read-only refresh observes the current state.
        marrow_runner::CallOutcome::OutcomeUnknown => Record::OutcomeUnknown,
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
        Value::Bytes(bytes) => Json::Str(render_hex_bytes(bytes)),
        Value::Date(days) => Json::Str(marrow_temporal::format_date(*days)?),
        Value::Instant(nanos) => Json::Str(marrow_temporal::format_instant(*nanos)?),
        Value::Duration(nanos) => Json::Str(marrow_temporal::format_duration(*nanos)),
        _ => return None,
    })
}

/// Render bytes as the `0x`-prefixed lowercase-hex string the wire and the CLI both spell a
/// `bytes` value with.
fn render_hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
        out.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
    }
    out
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
