//! The terminal side of the persistent companion path.
//!
//! [`attach_and_call`] is the client half of the local wire: it spawns a verified stock
//! runner as a native attached session over a persistent store, admits nothing else, submits
//! exactly one call, and renders the result back as a runtime [`Value`]. The terminal
//! (`marrow run … --store`) drives it. The runner is the sole opener of the store; this side
//! never touches the store directory, the engine, or a lifecycle state — it only speaks the
//! wire to the process that does.
//!
//! The one launched session is gated by two secrets exactly as the supervised g02p channel
//! is: the terminal mints a launch nonce, hands it to the spawned runner through the
//! `MARROW_RUNNER_NONCE` environment variable (so it is never echoed on the descriptor line),
//! proves it in the handshake, and checks the runner proves its session token and served
//! interface identity back before it sends the call.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use marrow_local_wire::{
    ClientMessage, Id32, Json, ServerMessage, WireError, frame_body_len, parse_strict,
};
use marrow_verify::VerifiedImage;
use marrow_vm::Value;

use crate::channel::mint_id;
use crate::descriptor::ret_to_image;
use crate::transfer;

/// The outcome of one companion call: the four durable-run outcomes projected onto the
/// terminal — a returned value (or unit), a source-mapped runtime fault, or a typed reject
/// the runner issued (an unknown export, an argument mismatch, a parked durable shape). None
/// of these leaks runner, wire, or lifecycle vocabulary; the terminal renders them as an
/// ordinary run outcome.
pub enum CallOutcome {
    /// The export returned: `None` is unit, `Some` a decoded value.
    Value(Option<Value>),
    /// A source-mapped runtime fault.
    Fault {
        code: String,
        line: u32,
        column: u32,
    },
    /// The runner declined the request with a typed code.
    Reject { code: String },
}

/// Why a companion call could not complete. These are the terminal's own operational errors
/// — distinct from the call outcome above — each carrying a stable dotted code the terminal
/// reports without wire vocabulary.
#[derive(Debug)]
pub enum ClientError {
    /// The temporary image could not be written for the runner to read.
    ImageStage(std::io::Error),
    /// The companion could not be spawned.
    Spawn(std::io::Error),
    /// The launch descriptor line was missing or malformed.
    Descriptor,
    /// The Unix socket could not be connected, or an I/O error occurred on it.
    Io(std::io::Error),
    /// A frame was rejected by the wire owner.
    Wire(WireError),
    /// The runner did not prove the expected session and interface, or spoke an
    /// out-of-protocol message.
    Handshake,
    /// The runner's reply value did not decode against the export's return type.
    ReplyDecode,
}

impl ClientError {
    /// The stable dotted code the terminal reports.
    pub fn code(&self) -> &'static str {
        use marrow_codes::Code;
        match self {
            ClientError::ImageStage(_) => Code::IoWrite.as_str(),
            ClientError::Spawn(_) => Code::RunnerSpawn.as_str(),
            ClientError::Descriptor | ClientError::Handshake => Code::RunnerHandshake.as_str(),
            ClientError::Io(_) => Code::IoRead.as_str(),
            ClientError::Wire(wire) => wire.code_str(),
            ClientError::ReplyDecode => Code::RunnerReplyEncode.as_str(),
        }
    }
}

/// The launch descriptor the runner publishes: the interface it serves, its session token,
/// and the socket to connect to. The nonce is not echoed (the terminal set it).
struct Descriptor {
    interface: Id32,
    session: Id32,
    socket: PathBuf,
}

/// A spawned companion plus its private staging directory, torn down on drop so a panic or an
/// early return never leaks the child process or the temporary image.
struct Companion {
    child: Child,
    dir: PathBuf,
}

impl Drop for Companion {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Spawn the verified companion at `runner_exe`, attach it to the persistent store at
/// `store`, and submit exactly one call to `export_id` with `args`. The companion is the sole
/// opener of the store. `runner_exe` must already be the release-verified stock runner (the
/// terminal verifies it against the release manifest before calling this).
pub fn attach_and_call(
    runner_exe: &Path,
    image: &VerifiedImage,
    image_bytes: &[u8],
    store: &Path,
    export_id: [u8; 32],
    args: Vec<Json>,
) -> Result<CallOutcome, ClientError> {
    let deadline = Duration::from_secs(10);
    let nonce = mint_id().map_err(ClientError::Io)?;

    let (mut companion, descriptor) = spawn_companion(runner_exe, image_bytes, store, nonce)?;

    // The companion must serve exactly the image we spawned it with: its published identity
    // is the image identity, which we recompute independently. A mismatch means it opened a
    // different program and we refuse before sending the call.
    let expected = Id32::from_bytes(image.image_id().0);
    if descriptor.interface != expected {
        return Err(ClientError::Handshake);
    }

    let outcome = call_over_socket(image, &descriptor, nonce, export_id, args, deadline);
    // The call is done and its socket dropped, so the companion has already seen the client
    // hang up and is exiting; wait for it here so the ordinary path is a clean exit rather
    // than the drop guard's kill. The guard still removes the staging directory.
    let _ = companion.child.wait();
    outcome
}

fn spawn_companion(
    runner_exe: &Path,
    image_bytes: &[u8],
    store: &Path,
    nonce: Id32,
) -> Result<(Companion, Descriptor), ClientError> {
    let dir = stage_dir();
    create_private_dir(&dir).map_err(ClientError::ImageStage)?;
    let image_path = dir.join("image.mwi");
    write_private(&image_path, image_bytes).map_err(ClientError::ImageStage)?;

    let mut child = Command::new(runner_exe)
        .arg("attach")
        .arg("--image")
        .arg(&image_path)
        .arg("--store")
        .arg(store)
        .env("MARROW_RUNNER_NONCE", nonce.to_hex())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            let _ = std::fs::remove_dir_all(&dir);
            ClientError::Spawn(error)
        })?;

    let descriptor = match read_descriptor(&mut child) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&dir);
            return Err(error);
        }
    };
    Ok((Companion { child, dir }, descriptor))
}

/// The most bytes the one launch-descriptor line may occupy — bounded before allocation
/// (law 9) even though the companion is release-verified. The line is a small fixed JSON
/// object (three 64-hex ids and a socket path).
const MAX_DESCRIPTOR_BYTES: u64 = 64 * 1024;

/// Read and parse the one launch-descriptor line the runner prints to stdout.
fn read_descriptor(child: &mut Child) -> Result<Descriptor, ClientError> {
    let stdout = child.stdout.take().ok_or(ClientError::Descriptor)?;
    let mut line = String::new();
    BufReader::new(stdout.take(MAX_DESCRIPTOR_BYTES))
        .read_line(&mut line)
        .map_err(ClientError::Io)?;
    parse_descriptor(&line).ok_or(ClientError::Descriptor)
}

fn parse_descriptor(line: &str) -> Option<Descriptor> {
    let Json::Object(pairs) = parse_strict(line.trim().as_bytes()).ok()? else {
        return None;
    };
    let field = |name: &str| {
        pairs.iter().find_map(|(key, value)| match value {
            Json::Str(text) if key == name => Some(text.clone()),
            _ => None,
        })
    };
    Some(Descriptor {
        interface: Id32::from_hex(&field("interface")?)?,
        session: Id32::from_hex(&field("session")?)?,
        socket: PathBuf::from(field("socket")?),
    })
}

/// Connect, prove the nonce, verify the runner proves the session and interface back, submit
/// one request, and decode the reply.
fn call_over_socket(
    image: &VerifiedImage,
    descriptor: &Descriptor,
    nonce: Id32,
    export_id: [u8; 32],
    args: Vec<Json>,
    deadline: Duration,
) -> Result<CallOutcome, ClientError> {
    // Enforce deadlines by non-blocking poll against a monotonic clock, the same discipline
    // the server's channel uses (a `SO_RCVTIMEO` read timeout is not the portable choice on
    // `AF_UNIX`), so both ends of one wire share one deadline discipline and a hung companion
    // never hangs the terminal.
    let mut stream = UnixStream::connect(&descriptor.socket).map_err(ClientError::Io)?;
    stream.set_nonblocking(true).map_err(ClientError::Io)?;

    write_message(&mut stream, &ClientMessage::Hello { nonce }, deadline)?;
    match read_message(&mut stream, deadline)? {
        ServerMessage::Ready { session, interface }
            if session == descriptor.session && interface == descriptor.interface => {}
        _ => return Err(ClientError::Handshake),
    }

    write_message(
        &mut stream,
        &ClientMessage::Request {
            export: Id32::from_bytes(export_id),
            args,
        },
        deadline,
    )?;
    match read_message(&mut stream, deadline)? {
        ServerMessage::Value { data } => decode_reply(image, export_id, &data),
        ServerMessage::Fault { code, span } => Ok(CallOutcome::Fault {
            code,
            line: span.line,
            column: span.column,
        }),
        ServerMessage::Reject { code } => Ok(CallOutcome::Reject { code }),
        // A second `Ready` or a `Provisioned` is out of protocol after the handshake.
        _ => Err(ClientError::Handshake),
    }
}

/// Decode a returned wire value against the export's declared return type. A unit-returning
/// export sends `null`, which is the [`CallOutcome::Value(None)`] the terminal renders as no
/// value; any other return type decodes through the shared transfer codec.
fn decode_reply(
    image: &VerifiedImage,
    export_id: [u8; 32],
    data: &Json,
) -> Result<CallOutcome, ClientError> {
    let export = image
        .export_by_id(marrow_image::ExportId::from_bytes(export_id))
        .ok_or(ClientError::Handshake)?;
    let ret = image.function(export.function()).ret();
    match ret_to_image(ret) {
        marrow_image::ImageType::Unit => match data {
            Json::Null => Ok(CallOutcome::Value(None)),
            _ => Err(ClientError::ReplyDecode),
        },
        ty => transfer::decode_arg(image, &ty, data)
            .map(|value| CallOutcome::Value(Some(value)))
            .ok_or(ClientError::ReplyDecode),
    }
}

/// The non-blocking poll interval, matching the server channel's.
const POLL: Duration = Duration::from_millis(1);

fn write_message(
    stream: &mut UnixStream,
    message: &ClientMessage,
    timeout: Duration,
) -> Result<(), ClientError> {
    let frame = message.encode().map_err(ClientError::Wire)?;
    let deadline = Instant::now() + timeout;
    let mut buf = frame.as_slice();
    while !buf.is_empty() {
        match stream.write(buf) {
            Ok(0) => return Err(ClientError::Io(io::ErrorKind::WriteZero.into())),
            Ok(n) => buf = &buf[n..],
            Err(error) => poll_or_fail(&error, deadline)?,
        }
    }
    Ok(())
}

fn read_message(stream: &mut UnixStream, timeout: Duration) -> Result<ServerMessage, ClientError> {
    let deadline = Instant::now() + timeout;
    let mut header = [0u8; 4];
    read_exact_deadline(stream, &mut header, deadline)?;
    let len = frame_body_len(header).map_err(ClientError::Wire)?;
    let mut body = vec![0u8; len];
    read_exact_deadline(stream, &mut body, deadline)?;
    ServerMessage::decode(&body).map_err(ClientError::Wire)
}

fn read_exact_deadline(
    stream: &mut UnixStream,
    buf: &mut [u8],
    deadline: Instant,
) -> Result<(), ClientError> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return Err(ClientError::Io(io::ErrorKind::UnexpectedEof.into())),
            Ok(n) => filled += n,
            Err(error) => poll_or_fail(&error, deadline)?,
        }
    }
    Ok(())
}

/// Sleep one poll interval on `WouldBlock` until the deadline, ignore `Interrupted`, and
/// surface anything else. A deadline reached while the peer is silent is a timed-out I/O
/// error.
fn poll_or_fail(error: &io::Error, deadline: Instant) -> Result<(), ClientError> {
    match error.kind() {
        io::ErrorKind::WouldBlock => {
            if Instant::now() >= deadline {
                Err(ClientError::Io(io::ErrorKind::TimedOut.into()))
            } else {
                sleep(POLL);
                Ok(())
            }
        }
        io::ErrorKind::Interrupted => Ok(()),
        _ => Err(ClientError::Io(io::Error::new(
            error.kind(),
            error.to_string(),
        ))),
    }
}

/// A private staging directory for the temporary image, named from OS entropy so two
/// terminals never collide.
fn stage_dir() -> PathBuf {
    let suffix = mint_id().map(|id| id.to_hex()).unwrap_or_else(|_| {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )
    });
    std::env::temp_dir().join(format!("marrow-run-{suffix}"))
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}
