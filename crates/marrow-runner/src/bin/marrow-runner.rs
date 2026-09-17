//! The stock Marrow runner binary.
//!
//! Each serving mode verifies a compiled image, binds a private local channel, publishes one
//! launch-descriptor line (identity, launch nonce, session token, socket path) to stdout, and
//! admits exactly one authenticated client. The launch nonce is read from
//! `MARROW_RUNNER_NONCE` (64 lowercase hex) when a supervisor sets it, and minted from OS
//! entropy otherwise. Teardown of the listener, socket, and temp dir is explicit and runs on
//! every non-panic exit path.
//!
//! The lifecycle commands (`provision`, `audit`, `recover`, `import`) open no channel. They
//! render in source vocabulary — destination, durable roots by name, effects and ceiling in
//! demand terms — never an identity hash, and a first provision writes nothing without the
//! explicit `--yes` acceptance of the exact report it printed.
//!
//! Every report leaves through a fallible writer, so a closed pipe is an `io.write` failure
//! rather than a panic; the print macros are refused at compile time.

#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_local_wire::{Json, encode};
use marrow_runner::{Channel, Deadlines, Handler, Id32, LaunchSecrets, Service, mint_id};

mod store_apply;
mod store_transfer;

/// The bounded number of connection attempts admitted before giving up (the
/// first-connection-wins bound: a same-uid racer costs one attempt).
const MAX_ACCEPT_ATTEMPTS: u32 = 16;

/// The command the runner was invoked to perform.
enum Command {
    Apply(store_apply::Command),
    Transfer(store_transfer::Command),
    /// Serve the image's storeless exports over a private channel.
    Serve {
        image: PathBuf,
    },
    /// Provision a fresh persistent store for the image at `store`. `accept` is `--yes`.
    Provision {
        image: PathBuf,
        store: PathBuf,
        accept: bool,
    },
    /// Attach the image to the persistent store at `store` and serve its exports.
    Attach {
        image: PathBuf,
        store: PathBuf,
    },
    /// Inspect or explicitly recover the store against a verified image.
    Store {
        operation: StoreOperation,
        image: PathBuf,
        store: PathBuf,
        format: ReportFormat,
    },
    /// Populate the store at `store` from a flat-scalar JSONL corpus through the trusted
    /// importer. Provisions the store first when it does not yet exist.
    Import {
        image: PathBuf,
        store: PathBuf,
        jsonl: PathBuf,
        root: String,
        keys: Vec<String>,
    },
}

#[derive(Clone, Copy)]
enum StoreOperation {
    Audit,
    Recover,
}

/// How store inspection and recovery render their results.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReportFormat {
    Text,
    Jsonl,
}

fn main() -> ExitCode {
    match parse_args() {
        Some(Command::Apply(command)) => finish_output(store_apply::run(command)),
        Some(Command::Transfer(command)) => finish_output(store_transfer::run(command)),
        Some(Command::Serve { image }) => serve(&image),
        Some(Command::Store {
            operation,
            image,
            store,
            format,
        }) => match operation {
            StoreOperation::Audit => finish_output(audit_output(&image, &store, format)),
            StoreOperation::Recover => finish_output(recovery_output(&image, &store, format)),
        },
        Some(Command::Provision {
            image,
            store,
            accept,
        }) => provision_command(&image, &store, accept),
        Some(Command::Attach { image, store }) => attach(&image, &store),
        Some(Command::Import {
            image,
            store,
            jsonl,
            root,
            keys,
        }) => import_command(&image, &store, &jsonl, &root, &keys),
        None => {
            let _ = writeln!(
                std::io::stderr(),
                "usage: marrow-runner --image <path>\n       marrow-runner provision --image \
                 <path> --store <dir> [--yes]\n       marrow-runner attach --image <path> \
                 --store <dir>\n       marrow-runner import --image <path> --store <dir> \
                 --jsonl <path> --root <name> --keys <col,...>\n       marrow-runner audit \
                 --image <path> --store <dir> [--format text|jsonl]\n       marrow-runner apply \
                 --store <dir> --old-image <image> --new-image <image> [--accept-ceiling <id>] [--format text|jsonl]\n       marrow-runner recover \
                 --image <path> --store <dir> [--format text|jsonl]\n       marrow-runner backup \
                 --image <path> --store <dir> --out <backup> [--format text|jsonl]\n       marrow-runner restore \
                 --from <backup> --store <dir> [--format text|jsonl]"
            );
            ExitCode::from(2)
        }
    }
}

/// Read and verify the program image at `path`, printing a typed diagnostic and returning the
/// exit code on failure.
fn load_image(path: &Path) -> Result<marrow_verify::VerifiedImage, ExitCode> {
    let bytes = read_image_bytes(path)?;
    marrow_verify::verify(&bytes).map_err(|rejection| {
        let _ = writeln!(std::io::stderr(), "{}", rejection.code().as_str());
        ExitCode::FAILURE
    })
}

fn read_image_bytes(path: &Path) -> Result<Vec<u8>, ExitCode> {
    let mut bytes = Vec::new();
    // One excess byte lets verification refuse oversize without waiting for EOF.
    let read = std::fs::File::open(path).and_then(|file| {
        file.take((marrow_image::bounds::MAX_IMAGE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
    });
    if let Err(err) = read {
        let _ = writeln!(
            std::io::stderr(),
            "{}: {err}",
            marrow_codes::Code::IoRead.as_str()
        );
        return Err(ExitCode::FAILURE);
    }
    Ok(bytes)
}

fn provision_command(image_path: &Path, store: &Path, accept: bool) -> ExitCode {
    finish_output(provision_output(image_path, store, accept))
}

fn finish_output(result: std::io::Result<ExitCode>) -> ExitCode {
    result.unwrap_or_else(|error| {
        let _ = writeln!(
            std::io::stderr(),
            "{}: {error}",
            marrow_codes::Code::IoWrite.as_str()
        );
        ExitCode::FAILURE
    })
}

fn write_receipt(output: &mut dyn Write, receipt: &str) -> std::io::Result<()> {
    writeln!(output, "{receipt}")?;
    output.flush()
}

/// One command report under construction: named fields in the order the text rendering
/// lists them. The JSON rendering is canonical, so field order carries no meaning there.
#[derive(Default)]
struct Receipt(Vec<(String, Json)>);

impl Receipt {
    /// A report tagged `kind`, the discriminator every multi-shape stream carries first.
    fn kind(kind: &str) -> Self {
        Self::default().text("kind", kind)
    }

    fn text(self, key: &str, value: impl Into<String>) -> Self {
        self.field(key, Json::Str(value.into()))
    }

    /// A counter, refused rather than clamped when it lies outside the JSON integer range.
    fn count(self, key: &str, value: u64) -> std::io::Result<Self> {
        let value = i64::try_from(value).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{key} exceeds the receipt's integer range"),
            )
        })?;
        Ok(self.field(key, Json::Int(value)))
    }

    fn field(mut self, key: &str, value: Json) -> Self {
        self.0.push((key.to_string(), value));
        self
    }

    fn into_json(self) -> Json {
        Json::Object(self.0)
    }
}

/// Deliver one store-command receipt in `format`. A delivery failure does not undo the
/// lifecycle effects the receipt describes, so the known result is repeated on the
/// diagnostic channel whenever that channel is still writable.
fn deliver(
    output: &mut dyn Write,
    diagnostic: &mut dyn Write,
    receipt: &Json,
    format: ReportFormat,
) -> std::io::Result<()> {
    let result = match format {
        ReportFormat::Jsonl => write_receipt(output, &encode(receipt)),
        ReportFormat::Text => write_text(output, receipt),
    };
    if result.is_err() {
        let _ = writeln!(
            diagnostic,
            "{}: receipt delivery failed; known lifecycle result follows",
            marrow_codes::Code::IoWrite.as_str()
        );
        let _ = write_receipt(diagnostic, &encode(receipt));
    }
    result
}

/// Render a receipt object as one `key: value` line per field. A string value is spelled
/// as itself; every other value keeps its JSON spelling.
fn write_text(output: &mut dyn Write, receipt: &Json) -> std::io::Result<()> {
    if let Json::Object(fields) = receipt {
        for (key, value) in fields {
            match value {
                Json::Str(value) => writeln!(output, "{key}: {value}")?,
                _ => writeln!(output, "{key}: {}", encode(value))?,
            }
        }
    }
    output.flush()
}

/// The `--store` and `--format` flags every store command accepts, parsed in one owner.
/// Returns `None` when the flag is one of these two but its value is missing, unrecognised,
/// or already given, so a caller's `?` refuses the whole command line.
fn shared_store_flag(
    flag: &str,
    args: &mut impl Iterator<Item = String>,
    store: &mut Option<PathBuf>,
    format: &mut Option<ReportFormat>,
) -> Option<SharedFlag> {
    match flag {
        "--store" if store.is_none() => *store = Some(PathBuf::from(args.next()?)),
        "--format" if format.is_none() => {
            *format = Some(match args.next()?.as_str() {
                "text" => ReportFormat::Text,
                "jsonl" => ReportFormat::Jsonl,
                _ => return None,
            });
        }
        "--store" | "--format" => return None,
        _ => return Some(SharedFlag::Other),
    }
    Some(SharedFlag::Taken)
}

/// Whether [`shared_store_flag`] consumed the word or left it to the calling command.
enum SharedFlag {
    Taken,
    Other,
}

fn provision_output(image_path: &Path, store: &Path, accept: bool) -> std::io::Result<ExitCode> {
    let store_text = validate_store_output(store)?;
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return Ok(code),
    };
    let prepared = marrow_lifecycle::prepare(image);
    let report = match marrow_lifecycle::ProvisionReport::new(store, &prepared) {
        Ok(report) => report,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "{}: {error}", error.code().as_str());
            return Ok(ExitCode::FAILURE);
        }
    };
    // Printed for review before any write.
    let mut stderr = std::io::stderr().lock();
    stderr.write_all(report.render().as_bytes())?;
    stderr.flush()?;

    if !accept {
        let _ = writeln!(
            stderr,
            "Re-run with --yes to accept this report and provision the store."
        );
        return Ok(ExitCode::from(2));
    }

    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    match marrow_lifecycle::provision_image(store, &prepared, &approval) {
        Ok(provisioned) => {
            // The receipt names the store instance and destination in a canonical JSON line;
            // it prints no internal identity hash as its primary output.
            let receipt = Receipt::default()
                .text("instance", provisioned.instance.to_hex())
                .text("store", store_text);
            deliver(
                &mut std::io::stdout().lock(),
                &mut stderr,
                &receipt.into_json(),
                ReportFormat::Jsonl,
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            let _ = writeln!(stderr, "{}: {error}", error.code().as_str());
            write_provision_failure(&mut std::io::stdout().lock(), store, &error)?;
            Ok(ExitCode::FAILURE)
        }
    }
}

fn write_provision_failure(
    output: &mut dyn Write,
    store: &Path,
    error: &marrow_lifecycle::ProvisionImageError,
) -> std::io::Result<()> {
    let receipt = if let Some((reason, instance)) = error.uncertainty() {
        Receipt::kind("provision_uncertain")
            .text("code", reason.code().as_str())
            .text("instance", instance.to_hex())
    } else if let Some(cleanup) = error.cleanup() {
        let stage = cleanup
            .stage
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid provision stage component",
                )
            })?;
        let cleanup = Receipt::default()
            .text("code", marrow_codes::Code::StoreIo.as_str())
            .field(
                "os_error",
                cleanup
                    .source
                    .raw_os_error()
                    .map_or(Json::Null, |value| Json::Int(i64::from(value))),
            )
            .text("stage", stage);
        Receipt::kind("provision_failed")
            .field("cleanup", cleanup.into_json())
            .text("code", error.code().as_str())
    } else {
        return Ok(());
    };
    let receipt = receipt.text("store", validate_store_output(store)?);
    write_receipt(output, &encode(&receipt.into_json()))
}

fn validate_store_output(store: &Path) -> std::io::Result<&str> {
    let text = store.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "store spelling must be UTF-8",
        )
    })?;
    if text.len() > marrow_local_wire::MAX_STRING_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "store spelling exceeds the output string bound",
        ));
    }
    Ok(text)
}

/// Serve the image's storeless exports over a private local channel (the `--image` command).
fn serve(image_path: &Path) -> ExitCode {
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return code,
    };
    let service = match Service::build(image) {
        Ok(service) => service,
        Err(error) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}: {error}",
                marrow_codes::Code::CliInterfaceUnbuildable.as_str()
            );
            return ExitCode::FAILURE;
        }
    };
    let interface = service.interface_id();
    serve_over_channel(interface, service)
}

/// Attach the image to the persistent store at `store` through the privileged lifecycle
/// actor and serve its exports over a private local channel. The actor takes the store's
/// single-owner lock, rereads the head, and classifies the image: an identical or
/// binding-only-updated image opens; a contract change is a typed refusal pointing at
/// `marrow apply`. The CLI never opens the store — it spawns this command and speaks the
/// wire protocol to it.
fn attach(image_path: &Path, store: &Path) -> ExitCode {
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return code,
    };
    // The attached session pins the exact image identity (not the transfer-graph interface),
    // so a program with a non-transferable export still serves its transferable exports over
    // the terminal path; a call to a non-transferable export fails closed at the per-call
    // transfer codec rather than being refused wholesale here.
    let identity = Id32::from_bytes(image.image_id().0);

    let attachment = match marrow_lifecycle::attach(store, marrow_lifecycle::prepare(image)) {
        Ok(marrow_lifecycle::AttachOutcome::AlreadyActive(attachment)) => attachment,
        // A binding-only rebind atomically updated the active code with the durable contract
        // unchanged; the store is open on the new image. The receipt is confirmed-commit
        // evidence, consumed here rather than echoed to the client (the spawn is invisible).
        Ok(marrow_lifecycle::AttachOutcome::Rebound { attachment, .. }) => attachment,
        // This authority refusal precedes every engine call. The authenticated refusal
        // service exposes that known result; exiting before the handshake would leave
        // activation outcome unknown to the client. Channel setup opens no store.
        Err(marrow_lifecycle::LifecycleError::Refused(
            marrow_lifecycle::AdmissionRefusal::Exceeds(refusal),
        )) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}: {refusal}",
                refusal.code().as_str()
            );
            return serve_over_channel(
                identity,
                marrow_runner::RefusalService::new(refusal.code()),
            );
        }
        Err(error @ marrow_lifecycle::LifecycleError::ActivationUncertain { instance, .. }) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}: {error}",
                error.code().as_str()
            );
            let _ = with_channel(identity, move |channel, secrets, deadlines| {
                channel.report_activation_uncertain(
                    secrets,
                    identity,
                    instance,
                    deadlines,
                    MAX_ACCEPT_ATTEMPTS,
                )
            });
            return ExitCode::FAILURE;
        }
        Err(error) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}: {error}",
                error.code().as_str()
            );
            return ExitCode::FAILURE;
        }
    };

    serve_over_channel(identity, marrow_runner::AttachedService::new(attachment))
}

/// The shared channel discipline for every serving mode: mint the session secrets, bind a
/// private local channel, publish one launch descriptor for the supervisor/terminal, admit one
/// authenticated client, and serve `handler` until it hangs up, tearing the channel down on
/// every non-panic path.
fn serve_over_channel(interface: Id32, mut handler: impl Handler) -> ExitCode {
    with_channel(interface, move |channel, secrets, deadlines| {
        channel.accept_and_serve(
            secrets,
            interface,
            deadlines,
            MAX_ACCEPT_ATTEMPTS,
            &mut handler,
        )
    })
}

/// Publish one fallible launch descriptor and own channel cleanup for both startup outcomes.
fn with_channel(
    interface: Id32,
    run: impl FnOnce(&Channel, &LaunchSecrets, &Deadlines) -> Result<(), marrow_runner::AcceptError>,
) -> ExitCode {
    let expected_nonce = match nonce_from_env() {
        Ok(nonce) => nonce,
        Err(()) => return ExitCode::FAILURE,
    };
    let (expected_nonce, published_nonce) = match expected_nonce {
        Some(nonce) => (nonce, None),
        None => match mint_id() {
            Ok(nonce) => (nonce, Some(nonce)),
            Err(err) => {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "{}: {err}",
                    marrow_codes::Code::IoRead.as_str()
                );
                return ExitCode::FAILURE;
            }
        },
    };
    let session = match mint_id() {
        Ok(session) => session,
        Err(err) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}: {err}",
                marrow_codes::Code::IoRead.as_str()
            );
            return ExitCode::FAILURE;
        }
    };

    let channel = match Channel::bind() {
        Ok(channel) => channel,
        Err(err) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}: {err}",
                marrow_codes::Code::IoWrite.as_str()
            );
            return ExitCode::FAILURE;
        }
    };

    let descriptor = launch_descriptor(
        interface,
        // A minted nonce is published for a standalone launch; a supervisor-set
        // one is already known to the supervisor and is not echoed.
        published_nonce,
        session,
        channel.socket_path().to_string_lossy().as_ref(),
    );
    if let Err(error) = write_receipt(&mut std::io::stdout().lock(), &descriptor) {
        let _ = writeln!(
            std::io::stderr().lock(),
            "{}: {error}",
            marrow_codes::Code::IoWrite.as_str()
        );
        channel.teardown();
        return ExitCode::FAILURE;
    }

    let deadlines = Deadlines::default();
    let secrets = LaunchSecrets {
        expected_nonce,
        session,
    };
    let outcome = run(&channel, &secrets, &deadlines).map_err(|error| {
        // A session I/O error keeps the read code; a failure to admit a client is a handshake
        // failure.
        let code = match error {
            marrow_runner::AcceptError::Io(_) => marrow_codes::Code::IoRead,
            _ => marrow_codes::Code::RunnerHandshake,
        };
        let _ = writeln!(std::io::stderr().lock(), "{}: {error:?}", code.as_str());
    });
    channel.teardown();

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn parse_args() -> Option<Command> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        // Each launch is a distinct leading keyword, so no flag order can turn one launch
        // mode into another.
        Some("provision") => parse_provision(args),
        Some("attach") => parse_attach(args),
        Some("import") => parse_import(args),
        Some("audit") => parse_store(args, StoreOperation::Audit),
        Some("recover") => parse_store(args, StoreOperation::Recover),
        Some("apply") => store_apply::parse(args).map(Command::Apply),
        Some("backup") => store_transfer::parse("backup", args).map(Command::Transfer),
        Some("restore") => store_transfer::parse("restore", args).map(Command::Transfer),
        Some("--image") => args.next().map(|image| Command::Serve {
            image: PathBuf::from(image),
        }),
        _ => None,
    }
}

/// Parse `attach --image <path> --store <dir>` in any flag order. Both are required.
fn parse_attach(mut args: impl Iterator<Item = String>) -> Option<Command> {
    let mut image: Option<PathBuf> = None;
    let mut store: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" => image = Some(PathBuf::from(args.next()?)),
            "--store" => store = Some(PathBuf::from(args.next()?)),
            _ => return None,
        }
    }
    Some(Command::Attach {
        image: image?,
        store: store?,
    })
}

/// Parse the image, store and format shared by audit and recovery.
/// `--image` and `--store` are required; the format defaults to text.
fn parse_store(
    mut args: impl Iterator<Item = String>,
    operation: StoreOperation,
) -> Option<Command> {
    let mut image: Option<PathBuf> = None;
    let mut store: Option<PathBuf> = None;
    let mut format = ReportFormat::Text;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" => image = Some(PathBuf::from(args.next()?)),
            "--store" => store = Some(PathBuf::from(args.next()?)),
            "--format" => {
                format = match args.next()?.as_str() {
                    "text" => ReportFormat::Text,
                    "jsonl" => ReportFormat::Jsonl,
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
    Some(Command::Store {
        operation,
        image: image?,
        store: store?,
        format,
    })
}

/// Parse `provision --image <path> --store <dir> [--yes]` in any flag order. Both `--image`
/// and `--store` are required; `--yes` is the acceptance of the rendered report.
fn parse_provision(args: impl Iterator<Item = String>) -> Option<Command> {
    let mut image: Option<PathBuf> = None;
    let mut store: Option<PathBuf> = None;
    let mut accept = false;
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" => image = Some(PathBuf::from(args.next()?)),
            "--store" => store = Some(PathBuf::from(args.next()?)),
            "--yes" => accept = true,
            _ => return None,
        }
    }
    Some(Command::Provision {
        image: image?,
        store: store?,
        accept,
    })
}

/// Parse `import --image <path> --store <dir> --jsonl <path> --root <name> --keys <col,...>`
/// in any flag order. All flags except `--keys` are required; `--keys` defaults to a single
/// `id` column (the common single-key primary root).
fn parse_import(mut args: impl Iterator<Item = String>) -> Option<Command> {
    let mut image: Option<PathBuf> = None;
    let mut store: Option<PathBuf> = None;
    let mut jsonl: Option<PathBuf> = None;
    let mut root: Option<String> = None;
    let mut keys: Vec<String> = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" => image = Some(PathBuf::from(args.next()?)),
            "--store" => store = Some(PathBuf::from(args.next()?)),
            "--jsonl" => jsonl = Some(PathBuf::from(args.next()?)),
            "--root" => root = Some(args.next()?),
            "--keys" => keys = args.next()?.split(',').map(str::to_owned).collect(),
            _ => return None,
        }
    }
    if keys.is_empty() {
        keys.push("id".to_string());
    }
    Some(Command::Import {
        image: image?,
        store: store?,
        jsonl: jsonl?,
        root: root?,
        keys,
    })
}

/// Populate the store from a flat-scalar JSONL corpus through the trusted importer.
///
/// The store shape is derived from the verified image (its single owner), so the corpus is
/// validated against exactly the program's declared types. When no store exists yet, it is
/// provisioned first from the same image (a trusted bulk import is an explicit first-population
/// action); an existing store is imported into only when this image is its exact active
/// binding. Every row is created through the path kernel by `import_jsonl`; no raw key, engine
/// handle, or transaction is exposed here.
fn import_command(
    image_path: &Path,
    store: &Path,
    jsonl: &Path,
    root_name: &str,
    keys: &[String],
) -> ExitCode {
    finish_output(import_output(image_path, store, jsonl, root_name, keys))
}

fn import_output(
    image_path: &Path,
    store: &Path,
    jsonl: &Path,
    root_name: &str,
    keys: &[String],
) -> std::io::Result<ExitCode> {
    validate_store_output(store)?;
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return Ok(code),
    };
    let prepared = marrow_lifecycle::prepare(image);
    // The import target indexes the store's root table, which is the prepared projection's.
    let Some(projection) = prepared.projection() else {
        let error = marrow_lifecycle::ImportError::UnsupportedShape(
            marrow_lifecycle::ShapeFault::NotExecutable,
        );
        let _ = writeln!(std::io::stderr(), "{}: {error}", error.code().as_str());
        return Ok(ExitCode::FAILURE);
    };
    let Some(root_index) = projection
        .roots()
        .iter()
        .position(|schema| schema.root_name() == root_name)
    else {
        let _ = writeln!(
            std::io::stderr(),
            "{}: no store root named `{root_name}` in this program",
            marrow_codes::Code::ConfigInvalid.as_str()
        );
        return Ok(ExitCode::FAILURE);
    };

    // Provision on first import; an existing complete store is imported into as-is. A
    // destination that cannot be examined is neither: reporting it as partially formed would
    // tell an operator to remove a store this process merely could not see.
    let classified = match marrow_lifecycle::preflight(store) {
        Ok(classified) => classified,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "{}: {error}", error.code().as_str());
            return Ok(ExitCode::FAILURE);
        }
    };
    match classified {
        marrow_lifecycle::Preflight::Complete => {}
        marrow_lifecycle::Preflight::Incomplete => {
            let _ = writeln!(
                std::io::stderr(),
                "{}: a partially formed store exists at the destination; remove it and retry",
                marrow_codes::Code::StoreIo.as_str()
            );
            return Ok(ExitCode::FAILURE);
        }
        marrow_lifecycle::Preflight::Absent => {
            let provisioned =
                marrow_lifecycle::ProvisionReport::new(store, &prepared).and_then(|report| {
                    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
                    marrow_lifecycle::provision_image(store, &prepared, &approval)
                });
            match provisioned {
                Ok(_) => {
                    let _ = writeln!(
                        std::io::stderr(),
                        "provisioned a fresh store at {}",
                        store.display()
                    );
                }
                Err(error) => {
                    let _ = writeln!(std::io::stderr(), "{}: {error}", error.code().as_str());
                    write_provision_failure(&mut std::io::stdout().lock(), store, &error)?;
                    return Ok(ExitCode::FAILURE);
                }
            }
        }
    }

    let file = match std::fs::File::open(jsonl) {
        Ok(file) => file,
        Err(err) => {
            let _ = writeln!(
                std::io::stderr(),
                "{}: {err}",
                marrow_codes::Code::IoRead.as_str()
            );
            return Ok(ExitCode::FAILURE);
        }
    };
    let target = marrow_lifecycle::ImportTarget {
        root: root_index as u16,
        key_columns: keys.to_vec(),
    };
    match marrow_lifecycle::import_jsonl(
        store,
        prepared,
        target,
        std::io::BufReader::new(file),
        marrow_lifecycle::InvocationGrant::full_store(),
        marrow_lifecycle::ImportLimits::DEFAULT,
    ) {
        Ok(report) => {
            let receipt = Receipt::default()
                .count("rows_imported", report.rows_imported)?
                .count("batches_committed", report.batches_committed)?;
            deliver(
                &mut std::io::stdout().lock(),
                &mut std::io::stderr().lock(),
                &receipt.into_json(),
                ReportFormat::Jsonl,
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "{}: {error}", error.code().as_str());
            Ok(ExitCode::FAILURE)
        }
    }
}

fn recovery_output(
    image_path: &Path,
    store: &Path,
    format: ReportFormat,
) -> std::io::Result<ExitCode> {
    let store_text = validate_store_output(store)?;
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return Ok(code),
    };
    let result = marrow_lifecycle::recover(store, marrow_lifecycle::prepare(image));
    write_recovery_result(&mut std::io::stdout().lock(), store_text, &result, format)?;
    Ok(if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The recovery report in either format, through one fallible writer. A delivery failure is
/// reported as `io.write` and nothing is echoed elsewhere: the report cannot be reconstructed
/// after the fact, and the store's state is what `marrow doctor` shows.
fn write_recovery_result(
    output: &mut dyn Write,
    store: &str,
    result: &Result<marrow_lifecycle::RecoveredStore, marrow_lifecycle::RecoveryError>,
    format: ReportFormat,
) -> std::io::Result<()> {
    match format {
        ReportFormat::Jsonl => write_receipt(
            output,
            &encode(&recovery_receipt(store, result).into_json()),
        ),
        ReportFormat::Text => write_recovery_text(output, store, result),
    }
}

/// The recovery report as an operator reads it: the activated store or the typed failure
/// first, then one `preserved` line per artifact the recovery moved aside.
fn write_recovery_text(
    output: &mut dyn Write,
    store: &str,
    result: &Result<marrow_lifecycle::RecoveredStore, marrow_lifecycle::RecoveryError>,
) -> std::io::Result<()> {
    let preserved = match result {
        Ok(recovered) => {
            writeln!(
                output,
                "Activated store {store}\ninstance {}\nimage {}",
                recovered.instance.to_hex(),
                recovered.image_id.to_hex()
            )?;
            &recovered.preserved
        }
        Err(error) => {
            writeln!(output, "{}: {error}", error.code().as_str())?;
            &error.preserved
        }
    };
    for name in preserved {
        writeln!(output, "preserved {name}")?;
    }
    output.flush()
}

/// The recovery report: the store, every preserved artifact name, and the activated
/// identities or the typed failure with the instance it concerned.
fn recovery_receipt(
    store: &str,
    result: &Result<marrow_lifecycle::RecoveredStore, marrow_lifecycle::RecoveryError>,
) -> Receipt {
    let preserved = match result {
        Ok(recovered) => &recovered.preserved,
        Err(error) => &error.preserved,
    };
    let receipt = Receipt::kind("recovery").text("store", store).field(
        "preserved",
        Json::Array(preserved.iter().cloned().map(Json::Str).collect()),
    );
    match result {
        Ok(recovered) => receipt
            .text("outcome", "activated")
            .text("instance", recovered.instance.to_hex())
            .text("image", recovered.image_id.to_hex()),
        Err(error) => {
            let receipt = receipt
                .text("outcome", "error")
                .text("code", error.code().as_str());
            match &error.fault {
                marrow_lifecycle::RecoveryFault::Completion { instance, .. } => {
                    receipt.text("instance", instance.to_hex())
                }
                marrow_lifecycle::RecoveryFault::Logical(report) => {
                    receipt.text("instance", report.instance.to_hex())
                }
                _ => receipt,
            }
        }
    }
}

/// Audit the store read-only against the image (the `audit` command). The lifecycle takes
/// the store's single-owner lock, admits the image as the exact active binding, runs the
/// kernel's logical walk through a read-only engine, and releases the lock; this command
/// only renders the result. A logically clean report exits `0`; findings, engine errors,
/// and refusals exit `1`. Physical integrity is not checked.
fn audit_output(
    image_path: &Path,
    store: &Path,
    format: ReportFormat,
) -> std::io::Result<ExitCode> {
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return Ok(code),
    };
    let store_text = store.display().to_string();
    let mut stdout = std::io::stdout().lock();
    let audit = match marrow_lifecycle::audit(store, marrow_lifecycle::prepare(image)) {
        Ok(audit) => audit,
        Err(error) => {
            match format {
                ReportFormat::Text => {
                    let _ = writeln!(std::io::stderr(), "{}: {error}", error.code().as_str());
                }
                ReportFormat::Jsonl => {
                    let receipt = Receipt::kind("doctor")
                        .text("code", error.code().as_str())
                        .text("outcome", "error")
                        .text("store", store_text);
                    deliver(
                        &mut stdout,
                        &mut std::io::stderr().lock(),
                        &receipt.into_json(),
                        format,
                    )?;
                }
            }
            return Ok(ExitCode::FAILURE);
        }
    };
    match format {
        ReportFormat::Text => {
            stdout.write_all(render_audit(&audit, store).as_bytes())?;
            stdout.flush()?;
        }
        ReportFormat::Jsonl => {
            for record in audit_records(&audit, store_text)? {
                write_receipt(&mut stdout, &encode(&record.into_json()))?;
            }
        }
    }
    Ok(if audit.is_clean() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The logical report and its physical-integrity limitation.
fn render_audit(audit: &marrow_lifecycle::StoreAudit, store: &Path) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "Logical store audit: {}", store.display());
    let _ = writeln!(out, "Physical integrity was not checked.");
    let _ = writeln!(out, "instance {}", audit.instance.to_hex());
    let _ = writeln!(out, "image {}", audit.image_id.to_hex());
    let marrow_lifecycle::StoreAudit {
        summary,
        findings,
        digest,
        ..
    } = audit;
    let _ = writeln!(
        out,
        "entries {}, index cells {}, cells {}",
        summary.entries, summary.index_cells, summary.cells,
    );
    let _ = writeln!(out, "digest {}", digest.to_hex());
    if summary.findings == 0 {
        out.push_str("no findings\n");
    } else {
        let _ = writeln!(out, "findings {}", summary.findings);
        for finding in findings {
            let _ = writeln!(
                out,
                "  {} at {}",
                finding.code.as_str(),
                audit.place(&finding.site)
            );
        }
        let unlisted = summary.findings - findings.len() as u64;
        if unlisted > 0 {
            let _ = writeln!(out, "  ... {unlisted} more not listed");
        }
    }
    out
}

/// The JSONL projection of an audit: one `doctor` record, then one `finding` record per
/// retained finding. `findings` counts every finding; `listed` counts the records that
/// follow, so a capped report is explicit.
fn audit_records(
    audit: &marrow_lifecycle::StoreAudit,
    store: String,
) -> std::io::Result<Vec<Receipt>> {
    let marrow_lifecycle::StoreAudit {
        summary,
        findings,
        digest,
        ..
    } = audit;
    let head = Receipt::kind("doctor")
        .text("scope", "logical")
        .text("physical_integrity", "not_checked")
        .text("store", store)
        .text("instance", audit.instance.to_hex())
        .text("image", audit.image_id.to_hex())
        .text(
            "outcome",
            if summary.findings == 0 {
                "clean"
            } else {
                "findings"
            },
        )
        .text("digest", digest.to_hex())
        .count("entries", summary.entries)?
        .count("index_cells", summary.index_cells)?
        .count("cells", summary.cells)?
        .count("findings", summary.findings)?
        .count("listed", findings.len() as u64)?;
    Ok(std::iter::once(head)
        .chain(findings.iter().map(|finding| {
            Receipt::kind("finding")
                .text("code", finding.code.as_str())
                .text("place", audit.place(&finding.site))
        }))
        .collect())
}

fn nonce_from_env() -> Result<Option<Id32>, ()> {
    match std::env::var("MARROW_RUNNER_NONCE") {
        Ok(text) => Id32::from_hex(&text).map(Some).ok_or_else(|| {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}: MARROW_RUNNER_NONCE is not 64 lowercase hex",
                marrow_codes::Code::ConfigInvalid.as_str()
            );
        }),
        Err(_) => Ok(None),
    }
}

/// One canonical JSON launch-descriptor line.
fn launch_descriptor(interface: Id32, nonce: Option<Id32>, session: Id32, socket: &str) -> String {
    let descriptor = Receipt::default()
        .text("interface", interface.to_hex())
        .text("session", session.to_hex())
        .text("socket", socket);
    let descriptor = match nonce {
        Some(nonce) => descriptor.text("nonce", nonce.to_hex()),
        None => descriptor,
    };
    encode(&descriptor.into_json())
}

/// How a test [`Sink`] fails: after accepting `WriteAt(n)` bytes in total, or on flush.
#[cfg(test)]
#[derive(Clone, Copy)]
enum Failure {
    WriteAt(usize),
    Flush,
}

/// An output channel that retains what it accepted and then fails, so a test can assert both
/// the bytes a caller committed and that the caller never claimed delivery.
#[cfg(test)]
struct Sink {
    bytes: Vec<u8>,
    failure: Failure,
}

#[cfg(test)]
impl Sink {
    fn new(failure: Failure) -> Self {
        Self {
            bytes: Vec::new(),
            failure,
        }
    }
}

#[cfg(test)]
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = match self.failure {
            Failure::WriteAt(limit) => bytes.len().min(limit.saturating_sub(self.bytes.len())),
            Failure::Flush => bytes.len(),
        };
        if count == 0 {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.bytes.extend_from_slice(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.failure {
            Failure::Flush => Err(std::io::ErrorKind::BrokenPipe.into()),
            Failure::WriteAt(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod output_tests {
    #[test]
    fn durable_mutators_validate_output_spelling_before_loading_or_effects() {
        let mut invalid = vec![PathBuf::from(
            "s".repeat(marrow_local_wire::MAX_STRING_BYTES + 1),
        )];
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            invalid.push(PathBuf::from(std::ffi::OsString::from_vec(vec![0xff])));
        }
        for store in invalid {
            let image = Path::new("absent-validation-image");
            for result in [
                provision_output(image, &store, true),
                import_output(image, &store, Path::new("absent-import"), "root", &[]),
                recovery_output(image, &store, ReportFormat::Jsonl),
            ] {
                assert_eq!(
                    result
                        .expect_err("reject spelling before image load")
                        .kind(),
                    std::io::ErrorKind::InvalidInput
                );
            }
        }
    }

    #[test]
    fn provision_output_preserves_primary_and_cleanup_failure() {
        let error =
            marrow_lifecycle::ProvisionImageError::Provision(marrow_lifecycle::ProvisionError {
                fault: marrow_lifecycle::ProvisionFault::AlreadyProvisioned,
                cleanup: Some(marrow_lifecycle::ProvisionCleanupFailure {
                    stage: PathBuf::from("parent/.marrow-provisioning.123.0"),
                    source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                }),
            });
        let mut output = Vec::new();
        write_provision_failure(&mut output, Path::new("parent/store"), &error).unwrap();
        assert_eq!(
            std::str::from_utf8(&output).unwrap(),
            "{\"cleanup\":{\"code\":\"store.io\",\"os_error\":null,\"stage\":\".marrow-provisioning.123.0\"},\"code\":\"store.locked\",\"kind\":\"provision_failed\",\"store\":\"parent/store\"}\n"
        );
        for limit in [0, 7, output.len() - 1] {
            let mut sink = Sink::new(Failure::WriteAt(limit));
            assert_eq!(
                write_provision_failure(&mut sink, Path::new("parent/store"), &error)
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::BrokenPipe
            );
            assert_eq!(sink.bytes, output[..limit]);
        }
        let mut sink = Sink::new(Failure::Flush);
        assert_eq!(
            write_provision_failure(&mut sink, Path::new("parent/store"), &error)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert_eq!(sink.bytes, output);
    }

    #[test]
    fn uncertainty_output_preserves_published_identity_and_ordinary_refusal_is_silent() {
        let instance = marrow_lifecycle::StoreInstanceId::from_bytes([0x12; 16]);
        let error = marrow_lifecycle::ProvisionImageError::Provision(
            marrow_lifecycle::ProvisionFault::PublicationUncertain {
                instance,
                source: std::io::Error::from(std::io::ErrorKind::Other),
            }
            .into(),
        );
        let mut output = Vec::new();
        super::write_provision_failure(&mut output, std::path::Path::new("store"), &error)
            .expect("write uncertainty");
        assert_eq!(
            String::from_utf8(output).expect("UTF-8"),
            format!(
                "{{\"code\":\"store.publication_uncertain\",\"instance\":\"{}\",\"kind\":\"provision_uncertain\",\"store\":\"store\"}}\n",
                instance.to_hex()
            )
        );
        let mut output = Vec::new();
        super::write_provision_failure(
            &mut output,
            std::path::Path::new("store"),
            &marrow_lifecycle::ProvisionImageError::Unapproved,
        )
        .expect("ordinary refusal");
        assert!(output.is_empty());
    }
    use super::*;

    #[test]
    fn provision_activation_uncertainty_keeps_the_actual_instance_in_output() {
        let instance = marrow_lifecycle::StoreInstanceId::from_bytes([0x34; 16]);
        let error = marrow_lifecycle::ProvisionImageError::Provision(
            marrow_lifecycle::ProvisionFault::ActivationUncertain {
                instance,
                source: marrow_lifecycle::AdmissionError {
                    entry: marrow_lifecycle::StoreEntry::Envelope,
                    fault: marrow_lifecycle::AdmissionFault::MultiplyLinked { links: 2 },
                },
            }
            .into(),
        );
        let mut output = Vec::new();
        write_provision_failure(&mut output, Path::new("store"), &error)
            .expect("write uncertainty");
        assert_eq!(
            output,
            format!(
                "{{\"code\":\"store.activation_uncertain\",\"instance\":\"{}\",\"kind\":\"provision_uncertain\",\"store\":\"store\"}}\n",
                instance.to_hex()
            )
            .as_bytes()
        );
    }

    #[test]
    fn receipt_delivery_stops_at_partial_write_and_requires_flush() {
        let receipt = "{\"instance\":\"0123456789abcdef0123456789abcdef\",\"store\":\"fixture\"}";
        let expected = format!("{receipt}\n");
        for limit in [0, 5, receipt.len()] {
            let mut sink = Sink::new(Failure::WriteAt(limit));
            assert_eq!(
                write_receipt(&mut sink, receipt).unwrap_err().kind(),
                std::io::ErrorKind::BrokenPipe
            );
            assert_eq!(sink.bytes, expected.as_bytes()[..limit]);
        }
        let mut sink = Sink::new(Failure::Flush);
        assert_eq!(
            write_receipt(&mut sink, receipt).unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert_eq!(sink.bytes, expected.as_bytes());
        let mut healthy = Vec::new();
        write_receipt(&mut healthy, receipt).expect("healthy receipt");
        assert_eq!(healthy, expected.as_bytes());
    }

    #[test]
    fn recovery_receipts_keep_identity_and_preservation_in_both_formats() {
        let instance = marrow_lifecycle::StoreInstanceId::from_bytes([0x51; 16]);
        let image_id = marrow_image::ImageId([0x62; 32]);
        let preserved =
            vec!["envelope.replacing.preserved.00000000000000000000000000000001".into()];
        let results = [
            Ok(marrow_lifecycle::RecoveredStore {
                instance,
                image_id,
                preserved: preserved.clone(),
            }),
            Err(marrow_lifecycle::RecoveryError {
                fault: marrow_lifecycle::RecoveryFault::Completion {
                    instance,
                    source: marrow_lifecycle::AuditError::Open(marrow_lifecycle::OpenError::Io(
                        std::io::ErrorKind::Other.into(),
                    )),
                },
                preserved: preserved.clone(),
            }),
            Err(marrow_lifecycle::RecoveryError {
                fault: marrow_lifecycle::RecoveryFault::HeadMismatch,
                preserved: Vec::new(),
            }),
        ];
        for (index, result) in results.iter().enumerate() {
            let mut fields = vec![
                ("kind".into(), Json::Str("recovery".into())),
                ("store".into(), Json::Str("store".into())),
                (
                    "preserved".into(),
                    Json::Array(if index < 2 {
                        preserved.iter().cloned().map(Json::Str).collect()
                    } else {
                        Vec::new()
                    }),
                ),
                (
                    "outcome".into(),
                    Json::Str(if index == 0 { "activated" } else { "error" }.into()),
                ),
            ];
            match index {
                0 => fields.extend([
                    ("instance".into(), Json::Str(instance.to_hex())),
                    ("image".into(), Json::Str(image_id.to_hex())),
                ]),
                1 => fields.extend([
                    (
                        "code".into(),
                        Json::Str("store.activation_uncertain".into()),
                    ),
                    ("instance".into(), Json::Str(instance.to_hex())),
                ]),
                _ => fields.push(("code".into(), Json::Str("store.corruption".into()))),
            }
            let expected = Json::Object(fields);
            assert_eq!(recovery_receipt("store", result).into_json(), expected);

            let mut jsonl = Vec::new();
            write_recovery_result(&mut jsonl, "store", result, ReportFormat::Jsonl).expect("jsonl");
            assert_eq!(jsonl, format!("{}\n", encode(&expected)).as_bytes());

            let mut text = Vec::new();
            write_recovery_result(&mut text, "store", result, ReportFormat::Text).expect("text");
            let text = String::from_utf8(text).expect("UTF-8");
            if index < 2 {
                assert!(text.contains(&instance.to_hex()));
                assert!(text.contains(&format!("preserved {}\n", preserved[0])));
            } else {
                assert!(!text.contains("preserved "));
            }
            if let Err(error) = result {
                assert!(text.starts_with(error.code().as_str()));
            }

            // A broken pipe in either format is the writer's error; nothing is repeated
            // elsewhere and the bytes already accepted are the whole of what left.
            for format in [ReportFormat::Jsonl, ReportFormat::Text] {
                for failure in [Failure::WriteAt(0), Failure::WriteAt(5), Failure::Flush] {
                    let mut sink = Sink::new(failure);
                    assert_eq!(
                        write_recovery_result(&mut sink, "store", result, format)
                            .unwrap_err()
                            .kind(),
                        std::io::ErrorKind::BrokenPipe
                    );
                    if let Failure::WriteAt(limit) = failure {
                        assert_eq!(sink.bytes.len(), limit);
                    }
                }
            }
        }
    }

    #[test]
    fn receipt_counters_refuse_values_past_the_integer_range() {
        let receipt = Receipt::default()
            .count("n", i64::MAX as u64)
            .expect("the largest representable counter");
        assert_eq!(
            receipt.into_json(),
            Json::Object(vec![("n".into(), Json::Int(i64::MAX))])
        );
        let Err(error) = Receipt::default().count("n", i64::MAX as u64 + 1) else {
            panic!("one past the range is refused");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
