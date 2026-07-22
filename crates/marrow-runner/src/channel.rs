//! The supervised local channel: the g02p Unix-domain discipline in safe `std`.
//!
//! The runner is the server. It creates a mode-0700 temporary directory, binds one
//! Unix listener inside it before any client connects, and accepts connections until
//! one authenticates — proving the launch nonce the supervisor issued — or a bounded
//! attempt count is exhausted (the adjudicated first-connection-wins answer: a
//! same-uid racer that connects first drives one failed attempt while the real
//! client is accepted on a later one). On a successful handshake the runner proves
//! its session token back and pins the served interface identity.
//!
//! Two platform disciplines are enforced here (g02p carry-forwards):
//!
//! - **Poll-based deadlines.** `setsockopt(SO_RCVTIMEO)` is `EINVAL` on `AF_UNIX` on
//!   macOS, so a read deadline is enforced by putting the stream in non-blocking mode
//!   and polling against a monotonic [`Instant`] with a short sleep — never
//!   `set_read_timeout`. Accept uses the same non-blocking poll.
//! - **Explicit fail-closed teardown.** Teardown (kill the listener, unlink the
//!   socket, remove the temp dir) runs on every non-panic path through an explicit
//!   [`Channel::teardown`], not by relying on `Drop` — which does not run under
//!   `panic = "abort"`. `Drop` is a best-effort backstop for the unwinding case only.
//!
//! The channel moves only bytes; framing and message grammar are the wire crate's.

use std::io::{self, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use marrow_local_wire::{ClientMessage, Id32, ServerMessage, WireError, frame_body_len};

/// The observer-local deadlines and poll interval for the channel.
#[derive(Debug, Clone, Copy)]
pub struct Deadlines {
    /// Bound on completing the handshake with one connecting peer.
    pub handshake: Duration,
    /// Bound on completing one request or response frame once its first byte
    /// arrives (an idle attached session waits for the next request without a
    /// deadline; a stalled half-frame is bounded by this).
    pub frame: Duration,
    /// Bound on accepting an authenticating connection across all attempts.
    pub accept: Duration,
    /// The non-blocking poll sleep.
    pub poll: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Deadlines {
            handshake: Duration::from_secs(2),
            frame: Duration::from_secs(2),
            accept: Duration::from_secs(5),
            poll: Duration::from_millis(1),
        }
    }
}

/// The two 256-bit secrets that gate one launched session: the nonce a client must
/// present, and the session token the runner proves back.
#[derive(Debug, Clone, Copy)]
pub struct LaunchSecrets {
    pub expected_nonce: Id32,
    pub session: Id32,
}

/// Why authentication did not yield a connection.
#[derive(Debug)]
pub enum AcceptError {
    /// No connection authenticated within the attempt budget.
    Unauthenticated,
    /// The accept deadline elapsed with no authenticating peer.
    Timeout,
    /// An OS error on the listener or a stream.
    Io(io::Error),
}

/// How a bounded read ended other than success.
#[derive(Debug)]
enum ReadError {
    /// The peer closed the connection.
    PeerDied,
    /// The observer deadline elapsed.
    Timeout,
    /// The framing was rejected by the single wire owner (e.g. an oversized frame).
    Wire(WireError),
    /// An OS error.
    Io(io::Error),
}

/// A bound Unix channel: the 0700 temp dir and the listener inside it. Held by the
/// runner for one session's lifetime, then explicitly torn down.
pub struct Channel {
    dir: PathBuf,
    socket_path: PathBuf,
    listener: UnixListener,
    closed: bool,
}

impl Channel {
    /// Create a mode-0700 temp dir and bind one listener inside it. The nonce/session
    /// are supplied to [`Self::accept_authenticated`]; binding happens before any
    /// client can connect.
    pub fn bind() -> io::Result<Channel> {
        let dir = make_private_dir()?;
        let socket_path = dir.join("s");
        let listener = UnixListener::bind(&socket_path).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&dir);
        })?;
        listener.set_nonblocking(true)?;
        Ok(Channel {
            dir,
            socket_path,
            listener,
            closed: false,
        })
    }

    /// The bound socket path a supervisor hands to the client.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Accept connections until one proves the launch nonce, then prove the session
    /// token back and pin `interface`. A connection that fails the handshake for any
    /// reason is closed and the next is tried, up to `max_attempts`.
    pub fn accept_authenticated(
        &self,
        secrets: &LaunchSecrets,
        interface: Id32,
        deadlines: &Deadlines,
        max_attempts: u32,
    ) -> Result<Connection, AcceptError> {
        let accept_deadline = Instant::now() + deadlines.accept;
        for _ in 0..max_attempts {
            let mut stream = self.accept_polled(accept_deadline, deadlines.poll)?;
            stream.set_nonblocking(true).map_err(AcceptError::Io)?;
            match handshake(&mut stream, secrets, interface, deadlines) {
                Ok(()) => return Ok(Connection { stream }),
                // Fail closed: drop this connection and try the next attempt.
                Err(_) => {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            }
        }
        Err(AcceptError::Unauthenticated)
    }

    /// Accept an authenticated client, then construct the request handler and run its session.
    ///
    /// The handler is built by `make_handler` **after** the handshake proves the launch nonce,
    /// so a resource the handler opens on construction — the ephemeral-memory attachment — never
    /// opens for a peer that has not authenticated. An eager caller (the storeless service, the
    /// native session whose store must open before the handshake) passes a closure that returns
    /// an already-built handler; the ordering guarantee is then vacuous but the one discipline is
    /// shared. Returns once the client hangs up or the session closes fail-closed.
    pub fn accept_and_serve<H: Handler>(
        &self,
        secrets: &LaunchSecrets,
        interface: Id32,
        deadlines: &Deadlines,
        max_attempts: u32,
        make_handler: impl FnOnce() -> H,
    ) -> Result<(), AcceptError> {
        let mut connection =
            self.accept_authenticated(secrets, interface, deadlines, max_attempts)?;
        let mut handler = make_handler();
        connection
            .run_session(&mut handler, deadlines)
            .map_err(AcceptError::Io)
    }

    fn accept_polled(&self, deadline: Instant, poll: Duration) -> Result<UnixStream, AcceptError> {
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => return Ok(stream),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(AcceptError::Timeout);
                    }
                    sleep(poll);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(AcceptError::Io(err)),
            }
        }
    }

    /// Explicit fail-closed teardown: close the listener, unlink the socket, and
    /// remove the temp dir. Runs on every non-panic path.
    pub fn teardown(mut self) {
        self.close();
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        // Dropping the listener closes it; then remove the whole private dir, which
        // also unlinks the socket node.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        // Best-effort backstop for the unwinding case; the production path uses the
        // explicit teardown, which also covers a `panic = "abort"` build.
        self.close();
    }
}

/// A request handler over one launched program: the storeless [`Service`](crate::Service)
/// and the native attached session share this seam so the connection's serial request loop
/// is written once. `handle` takes `&mut self` because the attached session opens a durable
/// session per request; the storeless handler ignores the mutability.
pub trait Handler {
    /// Produce the response to one client message.
    fn handle(&mut self, message: ClientMessage) -> ServerMessage;

    /// Whether the just-produced response is the final response for this session. An
    /// incomplete durable invocation whose state remains unknown closes only after its
    /// typed response is written, so no later request can enter the retired owner.
    fn close_after_response(&self) -> bool {
        false
    }
}

/// An authenticated connection: the byte stream after a proven handshake.
pub struct Connection {
    stream: UnixStream,
}

impl Connection {
    /// Attend to requests over this connection until the client hangs up or a fault
    /// closes it. One request is handled at a time (a single serial worker). This is
    /// the long-lived attached-session loop; it is deliberately not named `serve`.
    pub fn run_session<H: Handler>(
        &mut self,
        handler: &mut H,
        deadlines: &Deadlines,
    ) -> io::Result<()> {
        loop {
            let body = match self.read_request(deadlines) {
                Ok(Some(body)) => body,
                Ok(None) => return Ok(()), // clean client hangup
                Err(ReadError::Wire(wire)) => {
                    // A framing rejection (e.g. oversized): report it and close, since
                    // the byte stream is no longer reliably aligned.
                    let reject = ServerMessage::Reject {
                        code: wire.code_str().to_string(),
                    };
                    let _ = self.write_message(&reject, deadlines);
                    return Ok(());
                }
                // A stalled half-frame or peer death ends the session fail-closed.
                Err(ReadError::Timeout | ReadError::PeerDied) => return Ok(()),
                Err(ReadError::Io(err)) => return Err(err),
            };
            let response = match ClientMessage::decode(&body) {
                Ok(message) => handler.handle(message),
                Err(wire) => ServerMessage::Reject {
                    code: wire.code_str().to_string(),
                },
            };
            let close_after_response = handler.close_after_response();
            if self.write_message(&response, deadlines).is_err() {
                // The client went away while we replied; end the session.
                return Ok(());
            }
            if close_after_response {
                return Ok(());
            }
        }
    }

    /// Wait (without a deadline, an attached session may idle) for the next request's
    /// first byte, then read the rest of the frame under the frame deadline. `Ok(None)`
    /// is a clean hangup before a request begins.
    fn read_request(&mut self, deadlines: &Deadlines) -> Result<Option<Vec<u8>>, ReadError> {
        let mut first = [0u8; 1];
        loop {
            match self.stream.read(&mut first) {
                Ok(0) => return Ok(None),
                Ok(_) => break,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => sleep(deadlines.poll),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(ReadError::Io(err)),
            }
        }
        let deadline = Instant::now() + deadlines.frame;
        let mut header = [first[0], 0, 0, 0];
        read_exact_deadline(&mut self.stream, &mut header[1..], deadline, deadlines.poll)?;
        let len = frame_body_len(header).map_err(ReadError::Wire)?;
        let mut body = vec![0u8; len];
        read_exact_deadline(&mut self.stream, &mut body, deadline, deadlines.poll)?;
        Ok(Some(body))
    }

    fn write_message(&mut self, message: &ServerMessage, deadlines: &Deadlines) -> io::Result<()> {
        let frame = encode_response(message);
        let deadline = Instant::now() + deadlines.frame;
        write_all_deadline(&mut self.stream, &frame, deadline, deadlines.poll)
    }
}

/// Encode a response frame, downgrading a value too large to frame into a typed
/// reject so the runner never emits an over-limit frame.
fn encode_response(message: &ServerMessage) -> Vec<u8> {
    match message.encode() {
        Ok(frame) => frame,
        Err(_) => ServerMessage::Reject {
            code: marrow_codes::Code::WireFrameTooLarge.as_str().to_string(),
        }
        .encode()
        .expect("a reject frame is within the frame bound"),
    }
}

/// The runner side of the handshake: read the client's `Hello`, verify the nonce,
/// then prove the session token and interface identity with `Ready`.
fn handshake(
    stream: &mut UnixStream,
    secrets: &LaunchSecrets,
    interface: Id32,
    deadlines: &Deadlines,
) -> Result<(), HandshakeError> {
    let deadline = Instant::now() + deadlines.handshake;
    let body = read_frame(stream, deadline, deadlines.poll).map_err(|_| HandshakeError)?;
    match ClientMessage::decode(&body) {
        Ok(ClientMessage::Hello { nonce }) if nonce == secrets.expected_nonce => {}
        _ => return Err(HandshakeError),
    }
    let ready = ServerMessage::Ready {
        session: secrets.session,
        interface,
    };
    let frame = ready.encode().map_err(|_| HandshakeError)?;
    let write_deadline = Instant::now() + deadlines.handshake;
    write_all_deadline(stream, &frame, write_deadline, deadlines.poll).map_err(|_| HandshakeError)
}

/// A handshake failed and the connection must be closed fail-closed.
struct HandshakeError;

fn read_frame(
    stream: &mut UnixStream,
    deadline: Instant,
    poll: Duration,
) -> Result<Vec<u8>, ReadError> {
    let mut header = [0u8; 4];
    read_exact_deadline(stream, &mut header, deadline, poll)?;
    let len = frame_body_len(header).map_err(ReadError::Wire)?;
    let mut body = vec![0u8; len];
    read_exact_deadline(stream, &mut body, deadline, poll)?;
    Ok(body)
}

fn read_exact_deadline(
    stream: &mut UnixStream,
    buf: &mut [u8],
    deadline: Instant,
    poll: Duration,
) -> Result<(), ReadError> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return Err(ReadError::PeerDied),
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(ReadError::Timeout);
                }
                sleep(poll);
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(ReadError::Io(err)),
        }
    }
    Ok(())
}

fn write_all_deadline(
    stream: &mut UnixStream,
    mut buf: &[u8],
    deadline: Instant,
    poll: Duration,
) -> io::Result<()> {
    while !buf.is_empty() {
        match stream.write(buf) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "wrote zero bytes to the peer",
                ));
            }
            Ok(n) => buf = &buf[n..],
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "write deadline"));
                }
                sleep(poll);
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Create a fresh mode-0700 directory under the system temp dir, named from OS
/// entropy so two runners on one host never collide.
fn make_private_dir() -> io::Result<PathBuf> {
    let mut suffix = [0u8; 16];
    fill_entropy(&mut suffix)?;
    let mut name = String::from("marrow-runner-");
    for byte in suffix {
        name.push_str(&format!("{byte:02x}"));
    }
    let dir = std::env::temp_dir().join(name);
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

/// Draw 32 bytes from the OS entropy source as a fresh wire identity (a launch
/// nonce or a session token).
pub fn mint_id() -> io::Result<Id32> {
    let mut bytes = [0u8; 32];
    fill_entropy(&mut bytes)?;
    Ok(Id32::from_bytes(bytes))
}

#[cfg(unix)]
fn fill_entropy(buf: &mut [u8]) -> io::Result<()> {
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}
