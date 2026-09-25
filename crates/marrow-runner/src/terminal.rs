//! The terminal-side plumbing beneath the companion client.
//!
//! The one-shot native client ([`crate::attach_and_call`]) spawns a verified stock runner,
//! reads its one launch descriptor, connects the private socket, proves the launch nonce, and
//! checks the runner proves its session token and served image identity back before the call.
//! This module owns the companion process, its settlement, and the framed wire I/O; the
//! client owns what follows the handshake.
//!
//! The deadline discipline matches the runner's channel: a non-blocking poll against a monotonic
//! clock, never `set_read_timeout` (`SO_RCVTIMEO` is `EINVAL` on `AF_UNIX` on macOS), so both
//! ends of one wire share one discipline for descriptor and framed I/O. Child settlement is
//! separate from those I/O deadlines.

use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use marrow_codes::{Code, DurableCommitState};
use marrow_local_wire::{
    ClientMessage, Id32, Json, ServerMessage, WireError, frame_body_len, parse_strict,
};
use marrow_verify::VerifiedImage;
use marrow_vm::Value;

use crate::channel::{PollStop, mint_id, poll_until};
use crate::staging::{StagedImage, stage_image};
use crate::transfer;

/// The outcome of one companion call: the durable-run outcomes projected onto the terminal — a
/// returned value (or unit), a source-mapped runtime fault, or a typed reject the runner issued
/// (an unknown export, an argument mismatch, a parked durable shape). None of these leaks runner,
/// wire, or lifecycle vocabulary; the terminal renders them as an ordinary run outcome.
pub enum CallOutcome {
    /// The export returned: `None` is unit, `Some` a decoded value.
    Value(Option<Value>),
    /// A source-mapped runtime fault.
    Fault { code: Code, line: u32, column: u32 },
    /// The export did not return. The source fault and classified durable state
    /// are orthogonal; no recovery witness is exposed to the terminal.
    Incomplete {
        code: Code,
        durable: DurableCommitState,
        line: u32,
        column: u32,
    },
    /// The runner declined the request with a typed code.
    Reject { code: Code },
    /// The request was dispatched to the runner, but no exact valid correlated reply could be
    /// accepted, so the call's durable outcome is unknowable from this side. It is
    /// **not** replayed — a mutating call whose outcome is unknown must never run twice — and
    /// a read-only refresh observes the store's current state. The orthogonal cause records
    /// transport, wire, correlation, unsolicited-message, or value-decode failure. A valid
    /// runtime-fault reply and a pre-dispatch [`ClientError`] remain distinct.
    OutcomeUnknown { cause: OutcomeUnknownCause },
}

/// Orthogonal evidence explaining why no exact valid reply could be accepted
/// after a request was completely written. The call outcome remains unknown for
/// every variant; the cause never downgrades it to an ordinary client error.
#[derive(Debug)]
pub enum OutcomeUnknownCause {
    /// Socket I/O failed or timed out while exchanging the reply.
    Io(Direction, std::io::Error),
    /// The reply frame or message violated the local-wire grammar.
    Wire(WireError),
    /// The reply carried a call turn other than the dispatched turn.
    TurnMismatch { expected: u32, received: u32 },
    /// A complete non-call message arrived while a call reply was required.
    UnsolicitedMessage,
    /// A value reply did not decode against the export's sealed return type.
    ReplyDecode,
}

impl OutcomeUnknownCause {
    /// The stable cause discriminator, independent of its diagnostic code.
    pub fn kind(&self) -> CauseKind {
        match self {
            Self::Io(..) => CauseKind::Io,
            Self::Wire(_) => CauseKind::Wire,
            Self::TurnMismatch { .. } => CauseKind::TurnMismatch,
            Self::UnsolicitedMessage => CauseKind::UnsolicitedMessage,
            Self::ReplyDecode => CauseKind::ReplyDecode,
        }
    }

    /// The stable code for the distinct post-dispatch cause.
    pub fn code(&self) -> Code {
        match self {
            Self::Io(direction, _) => direction.code(),
            Self::Wire(error) => error.code(),
            Self::TurnMismatch { .. } | Self::UnsolicitedMessage => Code::WireMalformed,
            Self::ReplyDecode => Code::RunnerReplyEncode,
        }
    }
}

/// The stable discriminator a reporter names for an outcome-unknown cause, kept
/// apart from the cause's diagnostic code. One renderer spells it for output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CauseKind {
    Io,
    Wire,
    TurnMismatch,
    UnsolicitedMessage,
    ReplyDecode,
}

impl CauseKind {
    /// The reported spelling of this discriminator.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::Wire => "wire",
            Self::TurnMismatch => "turn_mismatch",
            Self::UnsolicitedMessage => "unsolicited_message",
            Self::ReplyDecode => "reply_decode",
        }
    }
}

/// Require the one call-reply turn corresponding to a completely written
/// request. A non-call message is unsolicited; another call turn is a
/// correlation mismatch.
pub(crate) fn require_reply_turn(
    expected: u32,
    received: Option<u32>,
) -> Result<(), OutcomeUnknownCause> {
    match received {
        Some(received) if received == expected => Ok(()),
        Some(received) => Err(OutcomeUnknownCause::TurnMismatch { expected, received }),
        None => Err(OutcomeUnknownCause::UnsolicitedMessage),
    }
}

/// Preserve the typed reason a post-dispatch operation failed while the public
/// call disposition remains outcome-unknown.
pub(crate) fn post_dispatch_cause(error: ClientError) -> OutcomeUnknownCause {
    match error {
        ClientError::Io(direction, error) => OutcomeUnknownCause::Io(direction, error),
        ClientError::Wire(error) => OutcomeUnknownCause::Wire(error),
        ClientError::ReplyDecode => OutcomeUnknownCause::ReplyDecode,
        ClientError::Handshake
        | ClientError::ActivationUncertain { .. }
        | ClientError::ActivationOutcomeUnknown { .. }
        | ClientError::ImageStage(_)
        | ClientError::Spawn(_)
        | ClientError::Descriptor => OutcomeUnknownCause::UnsolicitedMessage,
    }
}

/// Which half of a wire exchange failed. The registry names `io.read` for reading a runner
/// protocol frame and `io.write` for writing one, so a transport failure must say which it
/// was. Connecting, preparing a socket, and drawing the launch nonce write nothing to the
/// companion, so they report the reading half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Read,
    Write,
}

impl Direction {
    /// The registry code for a transport failure in this direction.
    pub const fn code(self) -> Code {
        match self {
            Self::Read => Code::IoRead,
            Self::Write => Code::IoWrite,
        }
    }
}

/// Why a companion call could not complete. These are the terminal's own operational errors —
/// distinct from the call outcome above — each carrying a stable dotted code the terminal reports
/// without wire vocabulary.
#[derive(Debug)]
pub enum ClientError {
    /// An authenticated, image-bound attach reported an unconfirmed activation.
    ActivationUncertain { instance: String },
    /// Native attach was spawned, but its startup outcome could not be established.
    /// No invocation was sent; the binding may nevertheless have changed.
    ActivationOutcomeUnknown { cause: Box<ClientError> },
    /// The temporary image could not be written for the runner to read.
    ImageStage(std::io::Error),
    /// The companion could not be spawned.
    Spawn(std::io::Error),
    /// The launch descriptor line was missing or malformed.
    Descriptor,
    /// The Unix socket could not be connected, or an I/O error occurred on it. The
    /// direction names which half of the exchange failed, so the reported code is the one
    /// the registry documents for that half.
    Io(Direction, std::io::Error),
    /// A frame was rejected by the wire owner.
    Wire(WireError),
    /// The runner did not prove the expected session and interface, or spoke an out-of-protocol
    /// message.
    Handshake,
    /// The runner's reply value did not decode against the export's return type.
    ReplyDecode,
}

impl ClientError {
    /// The stable dotted code the terminal reports.
    pub fn code(&self) -> Code {
        match self {
            ClientError::ActivationUncertain { .. } => Code::StoreActivationUncertain,
            ClientError::ActivationOutcomeUnknown { cause } => cause.code(),
            ClientError::ImageStage(_) => Code::IoWrite,
            ClientError::Spawn(_) => Code::RunnerSpawn,
            ClientError::Descriptor | ClientError::Handshake => Code::RunnerHandshake,
            ClientError::Io(direction, _) => direction.code(),
            ClientError::Wire(wire) => wire.code(),
            ClientError::ReplyDecode => Code::RunnerReplyEncode,
        }
    }

    pub(crate) fn activation_unknown(self) -> Self {
        match self {
            Self::ActivationUncertain { .. } | Self::ActivationOutcomeUnknown { .. } => self,
            cause => Self::ActivationOutcomeUnknown {
                cause: Box::new(cause),
            },
        }
    }
}

/// The per-call deadline: a request/reply exchange blocks at most this long before the wire
/// I/O times out.
pub(crate) const CALL_DEADLINE: Duration = Duration::from_secs(10);

/// Draw a fresh launch nonce from OS entropy, mapping an entropy failure to a terminal I/O
/// error. The terminal sets this nonce in the runner's environment and proves it in the
/// handshake.
pub(crate) fn mint_nonce() -> Result<Id32, ClientError> {
    mint_id().map_err(|error| ClientError::Io(Direction::Read, error))
}

/// The wire identity of a verified image: the exact image identity a runner proves back and a
/// terminal recomputes independently. One owner of the `image_id → wire Id32` projection.
pub(crate) fn image_identity(image: &VerifiedImage) -> Id32 {
    Id32::from_bytes(image.image_id().0)
}

/// Require that the runner's published interface is exactly the image the terminal spawned it
/// with. A mismatch means the runner opened a different program, and the terminal refuses before
/// sending any call.
pub(crate) fn require_interface(
    descriptor: &Descriptor,
    image: &VerifiedImage,
) -> Result<(), ClientError> {
    if descriptor.interface == image_identity(image) {
        Ok(())
    } else {
        Err(ClientError::Handshake)
    }
}

/// The launch descriptor the runner publishes: the interface it serves, its session token, and
/// the socket to connect to. The nonce is not echoed (the terminal set it).
pub(crate) struct Descriptor {
    pub(crate) interface: Id32,
    pub(crate) session: Id32,
    pub(crate) socket: PathBuf,
}

/// A cleanup failure independent of the companion's reported call or startup outcome.
#[derive(Debug)]
pub enum CompanionCleanupError {
    /// Direct-child reap could not be confirmed within the settlement bound. The caller
    /// receives the still-owned child and the retained stage; dropping this error reaps
    /// neither.
    Unreaped {
        child: Child,
        staging: PathBuf,
        cause: io::Error,
    },
    /// No child was spawned, or its reap was confirmed, but stage removal failed.
    Staging { path: PathBuf, cause: io::Error },
}

/// Startup failed and no session was returned. Cleanup does not replace the startup error.
#[derive(Debug)]
pub struct CompanionStartupError {
    pub error: ClientError,
    pub cleanup: Result<(), CompanionCleanupError>,
}

impl From<ClientError> for CompanionStartupError {
    fn from(error: ClientError) -> Self {
        Self {
            error,
            cleanup: Ok(()),
        }
    }
}

fn remove_stage(staging: &mut StagedImage) -> Result<(), CompanionCleanupError> {
    let path = staging.dir().to_path_buf();
    staging
        .remove()
        .map_err(|cause| CompanionCleanupError::Staging { path, cause })
}

/// A spawned direct child and its staging directory. Explicit settlement reports failure and
/// hands back an unreaped child; Drop applies the same policy but can only discard it.
pub(crate) struct Companion {
    child: Option<Child>,
    staging: StagedImage,
}

/// Time a companion is given to exit on its own before the terminal stops waiting quietly.
const GRACE: Duration = Duration::from_millis(100);
/// Further time allowed after the grace period lapses: the rest of the companion's budget to
/// close its store.
///
/// It is the per-call deadline because a close is the same filesystem work a call does, under
/// the same machine load. A shorter budget reports a healthy but slow close as a cleanup
/// failure, and the terminal never kills the companion to shorten it: a signal mid-close
/// leaves the engine unclean and the next attach refuses it.
const REAP: Duration = CALL_DEADLINE;

impl Companion {
    pub(crate) fn settle(mut self) -> Result<(), CompanionCleanupError> {
        self.settle_inner(GRACE, REAP)
    }

    fn settle_inner(
        &mut self,
        grace: Duration,
        reap: Duration,
    ) -> Result<(), CompanionCleanupError> {
        // Custody moves out of `self` so Drop cannot settle a second time.
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        // The companion waits out both bounds unsignalled: it may be closing the store.
        match observe_exit(&mut child, grace + reap) {
            Ok(Some(_)) => remove_stage(&mut self.staging),
            outcome => Err(CompanionCleanupError::Unreaped {
                child,
                staging: self.staging.retain(),
                cause: outcome
                    .err()
                    .unwrap_or_else(|| io::ErrorKind::TimedOut.into()),
            }),
        }
    }
}

fn observe_exit(
    child: &mut Child,
    timeout: Duration,
) -> io::Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        sleep(Duration::from_millis(1).min(remaining));
    }
}

impl Drop for Companion {
    fn drop(&mut self) {
        let _ = self.settle_inner(GRACE, REAP);
    }
}

/// Spawn the verified companion at `runner_exe` attached to the persistent store at `store`,
/// staging `image_bytes` to a private file passed as `--image`, and read its one launch
/// descriptor. `runner_exe` must already be the release-verified stock runner. A descriptor
/// that cannot be read leaves the attach outcome unknown: the runner may already have
/// rebound the store.
pub(crate) fn spawn_companion(
    runner_exe: &Path,
    image_bytes: &[u8],
    store: &Path,
    nonce: Id32,
) -> Result<(Companion, Result<Descriptor, ClientError>), CompanionStartupError> {
    let (mut stdout, child_stdout) =
        UnixStream::pair().map_err(|error| ClientError::Io(Direction::Read, error))?;
    let mut staging = stage_image(image_bytes).map_err(ClientError::ImageStage)?;

    let mut command = Command::new(runner_exe);
    command
        .arg("attach")
        .arg("--image")
        .arg(staging.path())
        .arg("--store")
        .arg(store);
    let child = command
        .env("MARROW_RUNNER_NONCE", nonce.to_hex())
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(child_stdout)))
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| CompanionStartupError {
            error: ClientError::Spawn(error),
            cleanup: remove_stage(&mut staging),
        })?;
    // Command retains its stdout endpoint for another spawn; release that parent copy.
    drop(command);
    let companion = Companion {
        child: Some(child),
        staging,
    };
    let descriptor =
        read_descriptor(&mut stdout, CALL_DEADLINE).map_err(ClientError::activation_unknown);
    Ok((companion, descriptor))
}

/// The most bytes the one launch-descriptor line may occupy — bounded before allocation
/// even though the companion is release-verified. Includes the final LF; the object carries
/// two 64-hex identities and a socket path.
const MAX_DESCRIPTOR_BYTES: usize = 64 * 1024;

/// Read and parse the one launch-descriptor line the runner prints to stdout.
fn read_descriptor(stdout: &mut UnixStream, timeout: Duration) -> Result<Descriptor, ClientError> {
    stdout
        .set_nonblocking(true)
        .map_err(|error| ClientError::Io(Direction::Read, error))?;
    let deadline = Instant::now() + timeout;
    let mut line = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let remaining = (MAX_DESCRIPTOR_BYTES - line.len()).min(chunk.len());
        match stdout.read(&mut chunk[..remaining]) {
            Ok(0) => return Err(ClientError::Descriptor),
            Ok(count) => {
                if let Some(end) = chunk[..count].iter().position(|byte| *byte == b'\n') {
                    line.extend_from_slice(&chunk[..end]);
                    return std::str::from_utf8(&line)
                        .ok()
                        .and_then(parse_descriptor)
                        .ok_or(ClientError::Descriptor);
                }
                line.extend_from_slice(&chunk[..count]);
                if line.len() == MAX_DESCRIPTOR_BYTES {
                    return Err(ClientError::Descriptor);
                }
            }
            Err(error) => poll_or_fail(error, deadline, Direction::Read)?,
        }
    }
}

fn parse_descriptor(line: &str) -> Option<Descriptor> {
    let Json::Object(pairs) = parse_strict(line.as_bytes()).ok()? else {
        return None;
    };
    if pairs.len() != 3 {
        return None;
    }
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

/// Connect the private socket, prove `nonce`, and verify the runner proves its session token and
/// served interface identity back. Returns the non-blocking stream positioned just after the
/// handshake, ready for the request/reply exchange. An attach whose activation the runner
/// reports as uncertain yields no stream: the terminal must not send an invocation.
pub(crate) fn connect_and_handshake(
    descriptor: &Descriptor,
    nonce: Id32,
    deadline: Duration,
) -> Result<UnixStream, ClientError> {
    let mut stream = UnixStream::connect(&descriptor.socket)
        .map_err(|error| ClientError::Io(Direction::Read, error))?;
    stream
        .set_nonblocking(true)
        .map_err(|error| ClientError::Io(Direction::Read, error))?;

    write_message(&mut stream, &ClientMessage::Hello { nonce }, deadline)?;
    match read_message_with_turn(&mut stream, deadline)? {
        (ServerMessage::Ready { session, interface }, None)
            if session == descriptor.session && interface == descriptor.interface => {}
        (
            ServerMessage::ActivationUncertain {
                session,
                interface,
                instance,
            },
            None,
        ) if session == descriptor.session && interface == descriptor.interface => {
            return Err(ClientError::ActivationUncertain { instance });
        }
        _ => return Err(ClientError::Handshake),
    }
    Ok(stream)
}

/// Map a server reply to one call's outcome, decoding a returned value against the export's
/// declared return type. `Ready` and `ActivationUncertain` are out of protocol once a session
/// is running.
pub(crate) fn reply_to_outcome(
    image: &VerifiedImage,
    export_id: [u8; 32],
    message: ServerMessage,
) -> Result<CallOutcome, ClientError> {
    match message {
        ServerMessage::Value { data } => decode_reply(image, export_id, &data),
        ServerMessage::Fault { code, span } => Ok(CallOutcome::Fault {
            code,
            line: span.line,
            column: span.column,
        }),
        ServerMessage::Incomplete {
            code,
            durable,
            span,
        } => Ok(CallOutcome::Incomplete {
            code,
            durable,
            line: span.line,
            column: span.column,
        }),
        ServerMessage::Reject { code } => Ok(CallOutcome::Reject { code }),
        ServerMessage::Ready { .. } | ServerMessage::ActivationUncertain { .. } => {
            Err(ClientError::Handshake)
        }
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
    let ret = image
        .function(export.function())
        .expect("verified export function")
        .body()
        .ret();
    match ret {
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

pub(crate) fn write_message(
    stream: &mut UnixStream,
    message: &ClientMessage,
    timeout: Duration,
) -> Result<(), ClientError> {
    write_message_with_turn(stream, message, 0, timeout)
}

pub(crate) fn write_message_with_turn(
    stream: &mut UnixStream,
    message: &ClientMessage,
    turn: u32,
    timeout: Duration,
) -> Result<(), ClientError> {
    let frame = message.encode_with_turn(turn).map_err(ClientError::Wire)?;
    let deadline = Instant::now() + timeout;
    let mut buf = frame.as_slice();
    while !buf.is_empty() {
        match stream.write(buf) {
            Ok(0) => {
                return Err(ClientError::Io(
                    Direction::Write,
                    io::ErrorKind::WriteZero.into(),
                ));
            }
            Ok(n) => buf = &buf[n..],
            Err(error) => poll_or_fail(error, deadline, Direction::Write)?,
        }
    }
    Ok(())
}

pub(crate) fn read_message_with_turn(
    stream: &mut UnixStream,
    timeout: Duration,
) -> Result<(ServerMessage, Option<u32>), ClientError> {
    let deadline = Instant::now() + timeout;
    let mut header = [0u8; 4];
    read_exact_deadline(stream, &mut header, deadline)?;
    let len = frame_body_len(header).map_err(ClientError::Wire)?;
    let mut body = vec![0u8; len];
    read_exact_deadline(stream, &mut body, deadline)?;
    ServerMessage::decode_with_turn(&body).map_err(ClientError::Wire)
}

fn read_exact_deadline(
    stream: &mut UnixStream,
    buf: &mut [u8],
    deadline: Instant,
) -> Result<(), ClientError> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => {
                return Err(ClientError::Io(
                    Direction::Read,
                    io::ErrorKind::UnexpectedEof.into(),
                ));
            }
            Ok(n) => filled += n,
            Err(error) => poll_or_fail(error, deadline, Direction::Read)?,
        }
    }
    Ok(())
}

/// Absorb a polled error through the channel's one poll discipline, mapping a silent
/// deadline to a timed-out terminal I/O error in the caller's direction.
fn poll_or_fail(
    error: io::Error,
    deadline: Instant,
    direction: Direction,
) -> Result<(), ClientError> {
    poll_until(error, deadline, POLL).map_err(|stop| match stop {
        PollStop::Expired => ClientError::Io(direction, io::ErrorKind::TimedOut.into()),
        PollStop::Failed(error) => ClientError::Io(direction, error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "spawns clean-exit companion controls"]
    fn confirmed_reap_distinguishes_removed_stage_from_removal_failure() {
        for replacement in [false, true] {
            let staging = stage_image(b"fixture image").expect("stage");
            let dir = staging.dir().to_path_buf();
            let mut child = Command::new("/bin/sh")
                .args(["-c", "exit 0"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("clean child");
            assert!(
                observe_exit(&mut child, Duration::from_secs(1))
                    .expect("observe reap")
                    .is_some()
            );
            assert!(
                child
                    .try_wait()
                    .expect("cached reap")
                    .expect("exit")
                    .success()
            );
            if replacement {
                std::fs::remove_dir_all(&dir).expect("remove fixture stage");
                std::fs::write(&dir, b"retained replacement").expect("replacement file");
            }
            let companion = Companion {
                child: Some(child),
                staging,
            };
            let result = companion.settle();
            if replacement {
                assert!(
                    matches!(result, Err(CompanionCleanupError::Staging { path, .. }) if path == dir)
                );
                assert_eq!(
                    std::fs::read(&dir).expect("retained after Drop"),
                    b"retained replacement"
                );
                std::fs::remove_file(&dir).expect("remove owned replacement fixture");
            } else {
                result.expect("settled");
                assert!(!dir.exists());
            }
        }
    }

    /// Settlement observes the child within its own bounds and never waits on the parent's
    /// handle to it: a child the parent still gates is handed back unreaped with its stage
    /// retained, before the parent releases it.
    #[test]
    #[ignore = "spawns a parent-controlled companion"]
    fn companion_settlement_does_not_wait_for_parent_release() {
        let staging = stage_image(b"fixture image").expect("private stage");
        let observed_dir = staging.dir().to_path_buf();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "read gate"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("controlled child");
        let gate = child.stdin.take().expect("parent release gate");
        let (send, receive) = std::sync::mpsc::channel();
        let owner = std::thread::spawn(move || {
            let mut companion = Companion {
                child: Some(child),
                staging,
            };
            let result =
                companion.settle_inner(Duration::from_millis(10), Duration::from_millis(100));
            let _ = send.send(result);
        });
        let observed = receive.recv_timeout(Duration::from_millis(500));
        drop(gate);
        owner.join().expect("owner finished");
        let Ok(Err(CompanionCleanupError::Unreaped {
            mut child, staging, ..
        })) = observed
        else {
            panic!("settlement must return the gated child before parent release");
        };
        assert_eq!(staging, observed_dir);
        assert!(
            observed_dir.exists(),
            "an unreaped child's stage is retained"
        );
        child.wait().expect("released child exits");
        std::fs::remove_dir_all(&observed_dir).expect("remove retained stage");
    }

    #[test]
    #[ignore = "spawns native-kind startup and Drop controls"]
    fn native_startup_refusal_and_drop_allow_natural_exit() {
        use std::os::unix::fs::PermissionsExt;
        for explicit in [true, false] {
            let mut fixture = stage_image(b"fixture root").expect("fixture root");
            let root = fixture.dir().to_path_buf();
            let script = root.join("runner");
            let marker = root.join("natural-exit");
            let quoted = marker
                .to_str()
                .expect("fixture UTF-8")
                .replace('\'', "'\\''");
            std::fs::write(&script, format!(
                "#!/bin/sh\nprintf 'invalid descriptor\\n'\n/bin/sleep 0.3\nprintf closed > '{quoted}'\n"
            )).expect("startup fixture");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
                .expect("script mode");
            let (companion, descriptor) = spawn_companion(
                &script,
                b"fixture image",
                &root.join("store"),
                Id32::from_bytes([1; 32]),
            )
            .expect("spawn fixture");
            let stage = companion.staging.dir().to_path_buf();
            let descriptor_refused = descriptor.is_err();
            let cleanup = if explicit {
                Some(companion.settle())
            } else {
                drop(companion);
                None
            };
            let marker_bytes = std::fs::read(&marker);
            eprintln!("native startup fixture: {}", root.display());
            assert!(descriptor_refused);
            assert!(matches!(cleanup, None | Some(Ok(()))));
            assert_eq!(marker_bytes.expect("natural exit reached"), b"closed");
            assert!(!stage.exists());
            fixture.remove().expect("remove successful control");
        }
    }

    fn descriptor_json(socket: &str) -> String {
        format!(
            "{{\"interface\":\"{}\",\"session\":\"{}\",\"socket\":\"{socket}\"}}",
            "12".repeat(32),
            "34".repeat(32)
        )
    }

    #[test]
    fn descriptor_requires_exact_fields_lf_and_byte_bound() {
        let valid = descriptor_json("/tmp/socket");
        let overhead = descriptor_json("").len() + 1;
        for (bytes, accepted) in [
            (format!("{valid}\n"), true),
            (valid.clone(), false),
            (format!(" {valid}\n"), false),
            (format!("{valid}\r\n"), false),
            (
                format!("{}\n", valid.replace("}", ",\"extra\":\"value\"}")),
                false,
            ),
            (
                format!(
                    "{}\n",
                    descriptor_json(&"p".repeat(MAX_DESCRIPTOR_BYTES - overhead))
                ),
                true,
            ),
            (
                format!(
                    "{}\n",
                    descriptor_json(&"p".repeat(MAX_DESCRIPTOR_BYTES - overhead + 1))
                ),
                false,
            ),
        ] {
            let (mut read, mut write) = UnixStream::pair().expect("stdout");
            let writer = std::thread::spawn(move || {
                let _ = write.write_all(bytes.as_bytes());
            });
            let result = read_descriptor(&mut read, Duration::from_secs(1));
            drop(read);
            writer.join().expect("writer closes");
            assert_eq!(result.is_ok(), accepted);
        }
    }

    #[test]
    fn a_preloaded_partial_descriptor_times_out_with_its_writer_still_open() {
        let (mut read, mut write) = UnixStream::pair().expect("stdout");
        write.write_all(b"{\"interface\":").expect("partial prefix");
        assert!(
            matches!(read_descriptor(&mut read, Duration::from_millis(10)),
                Err(ClientError::Io(_, error)) if error.kind() == io::ErrorKind::TimedOut
            )
        );
        write
            .write_all(b"still open")
            .expect("writer remained open through observation");
    }

    #[test]
    #[ignore = "spawns a parent-controlled descriptor writer"]
    fn descriptor_read_does_not_wait_for_a_live_child_to_close_stdout() {
        for prefix in ["", "partial"] {
            let (mut stdout, child_stdout) = UnixStream::pair().expect("private stdout");
            let mut child = Command::new("/bin/sh")
                .args([
                    "-c",
                    "printf %s \"$1\"; read gate",
                    "descriptor-writer",
                    prefix,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::from(OwnedFd::from(child_stdout)))
                .stderr(Stdio::null())
                .spawn()
                .expect("controlled child");
            let gate = child.stdin.take().expect("parent release gate");
            let (send, receive) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let result = read_descriptor(&mut stdout, Duration::from_millis(10));
                let _ = send.send(result);
                child.wait().expect("reap controlled child");
            });
            let observed = receive.recv_timeout(Duration::from_millis(200));
            drop(gate);
            reader.join().expect("descriptor reader cleanup");
            assert!(
                matches!(observed,
                    Ok(Err(ClientError::Io(_, error))) if error.kind() == io::ErrorKind::TimedOut
                ),
                "descriptor read must finish before the parent releases stdout"
            );
        }
    }

    #[test]
    #[ignore = "binds a Unix socket; run with the sandbox disabled"]
    fn activation_report_requires_both_session_and_image_agreement() {
        for mismatch in [None, Some("session"), Some("image")] {
            let channel = crate::Channel::bind().expect("bind");
            let nonce = Id32::from_bytes([1; 32]);
            let session = Id32::from_bytes([2; 32]);
            let interface = Id32::from_bytes([3; 32]);
            let instance = marrow_lifecycle::StoreInstanceId::from_bytes([4; 16]);
            let descriptor = Descriptor {
                session: if mismatch == Some("session") {
                    Id32::from_bytes([5; 32])
                } else {
                    session
                },
                interface: if mismatch == Some("image") {
                    Id32::from_bytes([6; 32])
                } else {
                    interface
                },
                socket: channel.socket_path().to_path_buf(),
            };
            let server = std::thread::spawn(move || {
                let result = channel.report_activation_uncertain(
                    &crate::LaunchSecrets {
                        expected_nonce: nonce,
                        session,
                    },
                    interface,
                    instance,
                    &crate::Deadlines::default(),
                    1,
                );
                channel.teardown();
                result.expect("delivered response");
            });
            let error = connect_and_handshake(&descriptor, nonce, Duration::from_secs(1))
                .expect_err("no invocation stream on uncertainty")
                .activation_unknown();
            if mismatch.is_none() {
                assert!(
                    matches!(error, ClientError::ActivationUncertain { instance: actual } if actual == instance.to_hex())
                );
            } else {
                assert!(
                    matches!(error, ClientError::ActivationOutcomeUnknown { cause } if matches!(*cause, ClientError::Handshake))
                );
            }
            server.join().expect("server");
        }
    }
}
