//! The stock Marrow runner binary.
//!
//! Usage: `marrow-runner --image <path>`. It reads a compiled program image,
//! verifies it, binds a private local channel, publishes one launch-descriptor line
//! (interface identity, launch nonce, session token, socket path) to stdout for its
//! supervisor, admits one authenticated client, and serves that client's storeless
//! calls until it hangs up. The launch nonce a client must present is read from the
//! `MARROW_RUNNER_NONCE` environment variable (64 lowercase hex) when a supervisor
//! sets it, and minted from OS entropy otherwise.
//!
//! Teardown of the listener, socket, and temp dir is explicit and runs on every
//! non-panic exit path.

use std::path::PathBuf;
use std::process::ExitCode;

use marrow_local_wire::{Json, encode};
use marrow_runner::{Channel, Deadlines, Id32, LaunchSecrets, Service, mint_id};

/// The bounded number of connection attempts admitted before giving up (the
/// first-connection-wins bound: a same-uid racer costs one attempt).
const MAX_ACCEPT_ATTEMPTS: u32 = 16;

fn main() -> ExitCode {
    let Some(image_path) = parse_args() else {
        eprintln!("usage: marrow-runner --image <path>");
        return ExitCode::from(2);
    };

    let bytes = match std::fs::read(&image_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("{}: {err}", marrow_codes::Code::IoRead.as_str());
            return ExitCode::FAILURE;
        }
    };
    let image = match marrow_verify::verify(&bytes) {
        Ok(image) => image,
        Err(rejection) => {
            eprintln!("{}", rejection.code());
            return ExitCode::FAILURE;
        }
    };
    let service = match Service::build(image) {
        Ok(service) => service,
        Err(error) => {
            eprintln!(
                "{}: {error}",
                marrow_codes::Code::CliCommandUnsupported.as_str()
            );
            return ExitCode::FAILURE;
        }
    };

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
            service.interface_id(),
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
    let outcome = run_session_once(&channel, &service, &secrets, &deadlines);
    channel.teardown();

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn run_session_once(
    channel: &Channel,
    service: &Service,
    secrets: &LaunchSecrets,
    deadlines: &Deadlines,
) -> Result<(), ()> {
    let mut connection = channel
        .accept_authenticated(
            secrets,
            service.interface_id(),
            deadlines,
            MAX_ACCEPT_ATTEMPTS,
        )
        .map_err(|error| {
            eprintln!(
                "{}: {error:?}",
                marrow_codes::Code::RunnerHandshake.as_str()
            )
        })?;
    connection
        .run_session(service, deadlines)
        .map_err(|err| eprintln!("{}: {err}", marrow_codes::Code::IoRead.as_str()))
}

fn parse_args() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--image") => args.next().map(PathBuf::from),
        _ => None,
    }
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
