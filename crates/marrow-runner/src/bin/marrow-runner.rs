//! The stock Marrow runner binary.
//!
//! Two commands:
//!
//! - `marrow-runner --image <path>` reads a compiled program image, verifies it, binds a
//!   private local channel, publishes one launch-descriptor line (interface identity, launch
//!   nonce, session token, socket path) to stdout for its supervisor, admits one
//!   authenticated client, and serves that client's storeless calls until it hangs up. The
//!   launch nonce a client must present is read from the `MARROW_RUNNER_NONCE` environment
//!   variable (64 lowercase hex) when a supervisor sets it, and minted from OS entropy
//!   otherwise.
//! - `marrow-runner provision --image <path> --store <dir> [--yes]` provisions a fresh
//!   persistent store for the image at the destination. It renders the provision report in
//!   source vocabulary (destination, durable roots by name, effects and initial ceiling in
//!   demand terms — never an identity hash); with `--yes` it accepts that exact report and
//!   publishes the store, printing a one-line JSON receipt naming the store instance;
//!   without `--yes` it prints the report and exits without writing, so a first provision is
//!   an explicit, reviewable action.
//! - `marrow-runner audit --image <path> --store <dir> [--format text|jsonl]` audits the
//!   store read-only against the image, which must be its active binding, and prints the
//!   findings and the logical digest; it opens no channel.
//!
//! Teardown of the listener, socket, and temp dir is explicit and runs on every
//! non-panic exit path.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_local_wire::{Json, encode};
use marrow_runner::{
    AttachedEphemeralService, Channel, Deadlines, Handler, Id32, LaunchSecrets, Service, mint_id,
};

/// The bounded number of connection attempts admitted before giving up (the
/// first-connection-wins bound: a same-uid racer costs one attempt).
const MAX_ACCEPT_ATTEMPTS: u32 = 16;

/// The command the runner was invoked to perform.
enum Command {
    /// Serve the image's storeless exports over a private channel.
    Serve { image: PathBuf },
    /// Provision a fresh persistent store for the image at `store`. `accept` is `--yes`.
    Provision {
        image: PathBuf,
        store: PathBuf,
        accept: bool,
    },
    /// Attach the image to the persistent store at `store` and serve its exports.
    Attach { image: PathBuf, store: PathBuf },
    /// Audit the persistent store at `store` read-only against the image.
    Audit {
        image: PathBuf,
        store: PathBuf,
        format: ReportFormat,
    },
    /// Attach the image to a fresh process-local in-memory store and serve its exports. The
    /// store never persists — it is discarded when this process exits.
    AttachEphemeral { image: PathBuf },
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

/// How `audit` renders its report.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReportFormat {
    Text,
    Jsonl,
}

fn main() -> ExitCode {
    match parse_args() {
        Some(Command::Serve { image }) => serve(&image),
        Some(Command::Audit {
            image,
            store,
            format,
        }) => audit_command(&image, &store, format),
        Some(Command::Provision {
            image,
            store,
            accept,
        }) => provision_command(&image, &store, accept),
        Some(Command::Attach { image, store }) => attach(&image, &store),
        Some(Command::AttachEphemeral { image }) => attach_ephemeral(&image),
        Some(Command::Import {
            image,
            store,
            jsonl,
            root,
            keys,
        }) => import_command(&image, &store, &jsonl, &root, &keys),
        None => {
            eprintln!(
                "usage: marrow-runner --image <path>\n       marrow-runner provision --image \
                 <path> --store <dir> [--yes]\n       marrow-runner attach --image <path> \
                 --store <dir>\n       marrow-runner attach-ephemeral --image \
                 <path>\n       marrow-runner import --image <path> --store <dir> \
                 --jsonl <path> --root <name> --keys <col,...>\n       marrow-runner audit \
                 --image <path> --store <dir> [--format text|jsonl]"
            );
            ExitCode::from(2)
        }
    }
}

/// Read and verify the program image at `path`, printing a typed diagnostic and returning the
/// exit code on failure. Shared by both commands.
fn load_image(path: &Path) -> Result<marrow_verify::VerifiedImage, ExitCode> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("{}: {err}", marrow_codes::Code::IoRead.as_str());
            return Err(ExitCode::FAILURE);
        }
    };
    marrow_verify::verify(&bytes).map_err(|rejection| {
        eprintln!("{}", rejection.code());
        ExitCode::FAILURE
    })
}

/// Provision a persistent store for the image at `store`. Renders the provision report in
/// source vocabulary; with `accept` (`--yes`) it accepts that exact report and publishes the
/// store, printing a one-line JSON receipt; otherwise it prints the report and exits without
/// writing.
fn provision_command(image_path: &Path, store: &Path, accept: bool) -> ExitCode {
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return code,
    };
    let prepared = marrow_lifecycle::prepare(image);
    let report = match marrow_lifecycle::ProvisionReport::new(store, &prepared) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{}: {error}", error.code());
            return ExitCode::FAILURE;
        }
    };
    // The report is the guided first-use flow: destination, roots, effects, and initial
    // ceiling in source vocabulary. Printed for review before any write.
    eprint!("{}", report.render());

    if !accept {
        eprintln!("Re-run with --yes to accept this report and provision the store.");
        return ExitCode::from(2);
    }

    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    match marrow_lifecycle::provision_image(store, &prepared, &approval) {
        Ok(provisioned) => {
            // The receipt names the store instance and destination in a canonical JSON line;
            // it prints no internal identity hash as its primary output.
            println!(
                "{}",
                encode(&Json::Object(vec![
                    (
                        "instance".to_string(),
                        Json::Str(provisioned.instance.to_hex()),
                    ),
                    ("store".to_string(), Json::Str(store.display().to_string()),),
                ]))
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}: {error}", error.code());
            ExitCode::FAILURE
        }
    }
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
            eprintln!(
                "{}: {error}",
                marrow_codes::Code::CliInterfaceUnbuildable.as_str()
            );
            return ExitCode::FAILURE;
        }
    };
    let interface = service.interface_id();
    serve_over_channel(interface, move || service)
}

/// Attach the image to the persistent store at `store` through the privileged lifecycle
/// actor and serve its durable and storeless exports over a private local channel (the
/// `attach` command). The lifecycle actor takes the store's single-owner lock, rereads the
/// head, and classifies the image: an identical or binding-only-updated image opens; a
/// contract change is a typed refusal pointing at `marrow apply`. The CLI never opens the
/// store — it spawns this command and speaks the wire protocol to it.
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
        // A demand-exceeds-ceiling authority refusal happens before the store is opened (zero
        // engine calls). Rather than exit — which the terminal would see only as a spawn
        // failure — serve a typed refusal over the channel so the terminal renders a
        // `CallOutcome::Reject`; the full source-vocabulary sentence goes to stderr, the
        // byte-log pipe the trusted main owns. Binding the channel and proving the handshake
        // open no store, so the refusal path stays zero-engine-call.
        Err(marrow_lifecycle::LifecycleError::DemandExceedsCeiling(refusal)) => {
            eprintln!("{}: {refusal}", refusal.code());
            let code = refusal.code();
            return serve_over_channel(identity, move || marrow_runner::RefusalService::new(code));
        }
        Err(error) => {
            eprintln!("{}: {error}", error.code());
            return ExitCode::FAILURE;
        }
    };

    let attached = marrow_runner::AttachedService::new(attachment);
    serve_over_channel(identity, move || attached)
}

/// Attach the image to a fresh process-local in-memory store and serve its durable and storeless
/// exports over a private local channel (the `attach-ephemeral` command). Unlike the native
/// `attach`, no persistent store is opened, no single-owner lock is taken, and no lifecycle
/// classification runs: the store is minted in RAM and discarded when this process exits. The
/// handshake identity is the exact image identity, computed here before the channel binds; the
/// in-memory store itself is opened only *after* a client proves the handshake, since the handler
/// is constructed after the accept. The CLI never opens a store — it spawns this command and
/// speaks the wire protocol to it.
fn attach_ephemeral(image_path: &Path) -> ExitCode {
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return code,
    };
    // The identity is the image identity, known without opening the in-memory store; the store
    // is minted inside the handler builder, which runs only after the handshake.
    let identity = Id32::from_bytes(image.image_id().0);
    let prepared = marrow_lifecycle::prepare(image);
    serve_over_channel(identity, move || AttachedEphemeralService::mint(prepared))
}

/// The shared channel discipline for every serving mode: mint the session secrets, bind a
/// private local channel, publish one launch descriptor for the supervisor/terminal, admit one
/// authenticated client, build the request handler, and serve it until it hangs up, tearing the
/// channel down on every non-panic path.
///
/// `make_handler` is invoked only after the handshake proves the launch nonce, so a resource the
/// handler opens on construction — the ephemeral-memory store — never opens for an unauthenticated
/// peer. The eager modes (storeless, native) pass a closure returning an already-built handler.
fn serve_over_channel<H: Handler>(interface: Id32, make_handler: impl FnOnce() -> H) -> ExitCode {
    let expected_nonce = match nonce_from_env() {
        Ok(nonce) => nonce,
        Err(()) => return ExitCode::FAILURE,
    };
    let (expected_nonce, published_nonce) = match expected_nonce {
        Some(nonce) => (nonce, None),
        None => match mint_id() {
            Ok(nonce) => (nonce, Some(nonce)),
            Err(err) => {
                eprintln!("{}: {err}", marrow_codes::Code::IoRead.as_str());
                return ExitCode::FAILURE;
            }
        },
    };
    let session = match mint_id() {
        Ok(session) => session,
        Err(err) => {
            eprintln!("{}: {err}", marrow_codes::Code::IoRead.as_str());
            return ExitCode::FAILURE;
        }
    };

    let channel = match Channel::bind() {
        Ok(channel) => channel,
        Err(err) => {
            eprintln!("{}: {err}", marrow_codes::Code::IoWrite.as_str());
            return ExitCode::FAILURE;
        }
    };

    println!(
        "{}",
        launch_descriptor(
            interface,
            // A minted nonce is published for a standalone launch; a supervisor-set
            // one is already known to the supervisor and is not echoed.
            published_nonce,
            session,
            channel.socket_path().to_string_lossy().as_ref(),
        )
    );

    let deadlines = Deadlines::default();
    let secrets = LaunchSecrets {
        expected_nonce,
        session,
    };
    // The handler is built only once a client authenticates, so the ephemeral store never opens
    // for an unauthenticated peer.
    let outcome = channel
        .accept_and_serve(
            &secrets,
            interface,
            &deadlines,
            MAX_ACCEPT_ATTEMPTS,
            make_handler,
        )
        .map_err(|error| {
            // A session I/O error keeps the read code; a failure to admit a client is a handshake
            // failure.
            let code = match error {
                marrow_runner::AcceptError::Io(_) => marrow_codes::Code::IoRead,
                _ => marrow_codes::Code::RunnerHandshake,
            };
            eprintln!("{}: {error:?}", code.as_str());
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
        // `provision`, `attach`, and `attach-ephemeral` branch before the `--image` serve path.
        // Each launch is a distinct keyword mapping to a distinct `Command` variant, so the four
        // handshakes can never be confused into one another by flag order or a stray flag.
        Some("provision") => parse_provision(args),
        Some("attach") => parse_attach(args),
        Some("attach-ephemeral") => parse_attach_ephemeral(args),
        Some("import") => parse_import(args),
        Some("audit") => parse_audit(args),
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

/// Parse `audit --image <path> --store <dir> [--format text|jsonl]` in any flag order.
/// `--image` and `--store` are required; the format defaults to text.
fn parse_audit(mut args: impl Iterator<Item = String>) -> Option<Command> {
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
    Some(Command::Audit {
        image: image?,
        store: store?,
        format,
    })
}

/// Parse `attach-ephemeral --image <path>` in any flag order. Only `--image` is accepted: an
/// ephemeral attachment has no store, so a `--store` (or any other flag) is refused rather than
/// silently treated as a native attach.
fn parse_attach_ephemeral(mut args: impl Iterator<Item = String>) -> Option<Command> {
    let mut image: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" => image = Some(PathBuf::from(args.next()?)),
            _ => return None,
        }
    }
    Some(Command::AttachEphemeral { image: image? })
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
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return code,
    };
    let prepared = marrow_lifecycle::prepare(image);
    // The import target indexes the store's root table, which is the prepared projection's.
    let Some(projection) = prepared.projection() else {
        let error = marrow_lifecycle::ImportError::UnsupportedShape(
            marrow_lifecycle::ShapeFault::NotExecutable,
        );
        eprintln!("{}: {error}", error.code());
        return ExitCode::FAILURE;
    };
    let Some(root_index) = projection
        .roots()
        .iter()
        .position(|schema| schema.root_name() == root_name)
    else {
        eprintln!(
            "{}: no store root named `{root_name}` in this program",
            marrow_codes::Code::ConfigInvalid.as_str()
        );
        return ExitCode::FAILURE;
    };

    // Provision on first import; an existing complete store is imported into as-is. A
    // destination that cannot be examined is neither: reporting it as partially formed would
    // tell an operator to remove a store this process merely could not see.
    let classified = match marrow_lifecycle::preflight(store) {
        Ok(classified) => classified,
        Err(error) => {
            eprintln!("{}: {error}", error.code());
            return ExitCode::FAILURE;
        }
    };
    match classified {
        marrow_lifecycle::Preflight::Complete => {}
        marrow_lifecycle::Preflight::Incomplete => {
            eprintln!(
                "{}: a partially formed store exists at the destination; remove it and retry",
                marrow_codes::Code::StoreIo.as_str()
            );
            return ExitCode::FAILURE;
        }
        marrow_lifecycle::Preflight::Absent => {
            let provisioned =
                marrow_lifecycle::ProvisionReport::new(store, &prepared).and_then(|report| {
                    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
                    marrow_lifecycle::provision_image(store, &prepared, &approval)
                });
            match provisioned {
                Ok(_) => eprintln!("provisioned a fresh store at {}", store.display()),
                Err(error) => {
                    eprintln!("{}: {error}", error.code());
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    let file = match std::fs::File::open(jsonl) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("{}: {err}", marrow_codes::Code::IoRead.as_str());
            return ExitCode::FAILURE;
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
            println!(
                "{}",
                encode(&Json::Object(vec![
                    (
                        "rows_imported".to_string(),
                        Json::Int(report.rows_imported as i64),
                    ),
                    (
                        "batches_committed".to_string(),
                        Json::Int(report.batches_committed as i64),
                    ),
                ]))
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}: {error}", error.code());
            ExitCode::FAILURE
        }
    }
}

/// Audit the store read-only against the image (the `audit` command). The lifecycle takes
/// the store's single-owner lock, admits the image as the exact active binding, runs the
/// kernel's logical walk through a read-only engine, and releases the lock; this command
/// only renders the result. A logically clean report exits `0`; findings, engine errors,
/// and refusals exit `1`. Physical integrity is not checked.
fn audit_command(image_path: &Path, store: &Path, format: ReportFormat) -> ExitCode {
    let image = match load_image(image_path) {
        Ok(image) => image,
        Err(code) => return code,
    };
    let store_text = store.display().to_string();
    let audit = match marrow_lifecycle::audit(store, marrow_lifecycle::prepare(image)) {
        Ok(audit) => audit,
        Err(error) => {
            match format {
                ReportFormat::Text => eprintln!("{}: {error}", error.code()),
                ReportFormat::Jsonl => println!(
                    "{}",
                    encode(&Json::Object(vec![
                        ("code".to_string(), Json::Str(error.code().to_string())),
                        ("kind".to_string(), Json::Str("doctor".to_string())),
                        ("outcome".to_string(), Json::Str("error".to_string())),
                        ("store".to_string(), Json::Str(store_text)),
                    ]))
                ),
            }
            return ExitCode::FAILURE;
        }
    };
    match format {
        ReportFormat::Text => print!("{}", render_audit(&audit, store)),
        ReportFormat::Jsonl => {
            for line in audit_records(&audit, store_text) {
                println!("{}", encode(&line));
            }
        }
    }
    if audit.is_clean() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
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
        "entries {}, descendant-only {}, index cells {}, cells {}",
        summary.entries, summary.descendant_only, summary.index_cells, summary.cells,
    );
    let _ = writeln!(out, "digest {}", digest.to_hex());
    if summary.findings == 0 {
        out.push_str("no findings\n");
    } else {
        let _ = writeln!(out, "findings {}", summary.findings);
        for finding in findings {
            let _ = writeln!(out, "  {} at {}", finding.code.as_str(), finding.place);
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
fn audit_records(audit: &marrow_lifecycle::StoreAudit, store: String) -> Vec<Json> {
    let text = |value: &str| Json::Str(value.to_string());
    let mut head = vec![
        ("kind".to_string(), text("doctor")),
        ("scope".to_string(), text("logical")),
        ("physical_integrity".to_string(), text("not_checked")),
        ("store".to_string(), Json::Str(store)),
        ("instance".to_string(), Json::Str(audit.instance.to_hex())),
        ("image".to_string(), Json::Str(audit.image_id.to_hex())),
    ];
    let mut records = Vec::new();
    let marrow_lifecycle::StoreAudit {
        summary,
        findings,
        digest,
        ..
    } = audit;
    let count = |value: u64| Json::Int(i64::try_from(value).unwrap_or(i64::MAX));
    head.push((
        "outcome".to_string(),
        text(if summary.findings == 0 {
            "clean"
        } else {
            "findings"
        }),
    ));
    head.push(("digest".to_string(), Json::Str(digest.to_hex())));
    head.push(("entries".to_string(), count(summary.entries)));
    head.push((
        "descendant_only".to_string(),
        count(summary.descendant_only),
    ));
    head.push(("index_cells".to_string(), count(summary.index_cells)));
    head.push(("cells".to_string(), count(summary.cells)));
    head.push(("findings".to_string(), count(summary.findings)));
    head.push(("listed".to_string(), count(findings.len() as u64)));
    records.push(Json::Object(head));
    for finding in findings {
        records.push(Json::Object(vec![
            ("kind".to_string(), text("finding")),
            ("code".to_string(), text(finding.code.as_str())),
            ("place".to_string(), Json::Str(finding.place.clone())),
        ]));
    }
    records
}

fn nonce_from_env() -> Result<Option<Id32>, ()> {
    match std::env::var("MARROW_RUNNER_NONCE") {
        Ok(text) => Id32::from_hex(&text).map(Some).ok_or_else(|| {
            eprintln!(
                "{}: MARROW_RUNNER_NONCE is not 64 lowercase hex",
                marrow_codes::Code::ConfigInvalid.as_str()
            );
        }),
        Err(_) => Ok(None),
    }
}

/// One canonical JSON launch-descriptor line.
fn launch_descriptor(interface: Id32, nonce: Option<Id32>, session: Id32, socket: &str) -> String {
    let mut pairs = vec![
        ("interface".to_string(), Json::Str(interface.to_hex())),
        ("session".to_string(), Json::Str(session.to_hex())),
        ("socket".to_string(), Json::Str(socket.to_string())),
    ];
    if let Some(nonce) = nonce {
        pairs.push(("nonce".to_string(), Json::Str(nonce.to_hex())));
    }
    encode(&Json::Object(pairs))
}
