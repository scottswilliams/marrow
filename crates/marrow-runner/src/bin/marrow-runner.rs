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
//!
//! Teardown of the listener, socket, and temp dir is explicit and runs on every
//! non-panic exit path.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_local_wire::{Json, encode};
use marrow_runner::{Channel, Deadlines, Id32, LaunchSecrets, Service, mint_id};

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
}

fn main() -> ExitCode {
    match parse_args() {
        Some(Command::Serve { image }) => serve(&image),
        Some(Command::Provision {
            image,
            store,
            accept,
        }) => provision_command(&image, &store, accept),
        Some(Command::Attach { image, store }) => attach(&image, &store),
        None => {
            eprintln!(
                "usage: marrow-runner --image <path>\n       marrow-runner provision --image \
                 <path> --store <dir> [--yes]\n       marrow-runner attach --image <path> \
                 --store <dir>"
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
    let Some((schemas, sites)) = marrow_vm::derive_store_schemas(&image) else {
        eprintln!(
            "{}: the program's durable shape is not yet executable by the store, so it cannot \
             be provisioned",
            marrow_codes::Code::CliDurableUnsupported.as_str()
        );
        return ExitCode::FAILURE;
    };

    let report = marrow_lifecycle::ProvisionReport::new(store, &image, &schemas);
    // The report is the guided first-use flow: destination, roots, effects, and initial
    // ceiling in source vocabulary. Printed for review before any write.
    eprint!("{}", report.render());

    if !accept {
        eprintln!("Re-run with --yes to accept this report and provision the store.");
        return ExitCode::from(2);
    }

    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    match marrow_lifecycle::provision_image(store, &image, schemas, sites, &approval) {
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
                marrow_codes::Code::CliTransferExcluded.as_str()
            );
            return ExitCode::FAILURE;
        }
    };
    let interface = service.interface_id();
    serve_over_channel(service, interface)
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
    let Some((schemas, sites)) = marrow_vm::derive_store_schemas(&image) else {
        eprintln!(
            "{}: the program's durable shape is not yet executable by the store",
            marrow_codes::Code::CliDurableUnsupported.as_str()
        );
        return ExitCode::FAILURE;
    };

    let opened = match marrow_lifecycle::attach(store, &image, schemas, sites) {
        Ok(marrow_lifecycle::AttachOutcome::AlreadyActive(opened)) => opened,
        // A binding-only rebind atomically updated the active code with the durable contract
        // unchanged; the store is open on the new image. The receipt is confirmed-commit
        // evidence, consumed here rather than echoed to the client (the spawn is invisible).
        Ok(marrow_lifecycle::AttachOutcome::Rebound { store, .. }) => store,
        Err(error) => {
            eprintln!("{}: {error}", error.code());
            return ExitCode::FAILURE;
        }
    };

    let service = match Service::build(image) {
        Ok(service) => service,
        Err(error) => {
            eprintln!(
                "{}: {error}",
                marrow_codes::Code::CliTransferExcluded.as_str()
            );
            return ExitCode::FAILURE;
        }
    };
    let interface = service.interface_id();
    serve_over_channel(
        marrow_runner::AttachedService::new(service, opened),
        interface,
    )
}

/// The shared channel discipline for both serving modes: mint the session secrets, bind a
/// private local channel, publish one launch descriptor for the supervisor/terminal, admit
/// one authenticated client, and serve it until it hangs up, tearing the channel down on
/// every non-panic path.
fn serve_over_channel<H: marrow_runner::Handler>(mut handler: H, interface: Id32) -> ExitCode {
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
    let outcome = run_session_once(&channel, &mut handler, interface, &secrets, &deadlines);
    channel.teardown();

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn run_session_once<H: marrow_runner::Handler>(
    channel: &Channel,
    handler: &mut H,
    interface: Id32,
    secrets: &LaunchSecrets,
    deadlines: &Deadlines,
) -> Result<(), ()> {
    let mut connection = channel
        .accept_authenticated(secrets, interface, deadlines, MAX_ACCEPT_ATTEMPTS)
        .map_err(|error| {
            eprintln!(
                "{}: {error:?}",
                marrow_codes::Code::RunnerHandshake.as_str()
            )
        })?;
    connection
        .run_session(handler, deadlines)
        .map_err(|err| eprintln!("{}: {err}", marrow_codes::Code::IoRead.as_str()))
}

fn parse_args() -> Option<Command> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        // `provision` and `attach` branch before the `--image` serve path.
        Some("provision") => parse_provision(args),
        Some("attach") => parse_attach(args),
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
