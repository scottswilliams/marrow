//! The supervised-channel discipline and the handoff-boundary loss classification,
//! exercised over a real Unix-domain socket.
//!
//! The six channel journeys bind a Unix listener and connect to it, which the
//! command sandbox denies with `EPERM`, so those tests are `#[ignore]`d and run
//! explicitly with the sandbox disabled:
//!
//! ```text
//! cargo test -p marrow-runner --test channel -- --ignored
//! ```
//!
//! A default-battery test compiles and verifies their shared source without
//! opening a socket, so source-surface drift is not hidden by the ignore boundary.
//!
//! The runner side (which owns a non-`Send` `VerifiedImage`) stays on the test
//! thread; the client side runs on a spawned thread and speaks the wire protocol
//! directly.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel as mpsc_channel};
use std::thread;
use std::time::Duration;

use marrow_local_wire::{
    ClientMessage, DurableState, HandoffStage, Id32, Json, LossClass, ServerMessage, Span,
    classify, frame_body_len,
};
use marrow_runner::{Channel, Deadlines, Handler, LaunchSecrets, Service, mint_id};

const ADD: &str = "pub fn add(a: int, b: int): int {\n    return a + b\n}\n";

/// Brisk deadlines so the timeout-shaped tests finish quickly.
fn quick() -> Deadlines {
    Deadlines {
        handshake: Duration::from_millis(300),
        frame: Duration::from_millis(300),
        accept: Duration::from_millis(600),
        poll: Duration::from_millis(1),
    }
}

fn service_for(source: &str) -> (Service, Id32) {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        None,
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    let export = Id32::from_bytes(
        *compiled
            .exports
            .iter()
            .find(|entry| entry.item == "add")
            .expect("add export")
            .id
            .bytes(),
    );
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
    (Service::build(image).expect("service"), export)
}

#[test]
fn embedded_test_sources_compile_and_verify() {
    let (_service, _export) = service_for(ADD);
}

fn connect(path: &Path) -> UnixStream {
    UnixStream::connect(path).expect("client connects")
}

fn send(stream: &mut UnixStream, message: &ClientMessage) -> std::io::Result<()> {
    stream.write_all(&message.encode().expect("encode client message"))
}

/// Read one server frame, or `None` if the peer closed before a full frame arrived.
fn recv(stream: &mut UnixStream) -> Option<ServerMessage> {
    let mut header = [0u8; 4];
    read_full(stream, &mut header)?;
    let len = frame_body_len(header).ok()?;
    let mut body = vec![0u8; len];
    read_full(stream, &mut body)?;
    ServerMessage::decode(&body).ok()
}

fn read_full(stream: &mut UnixStream, buf: &mut [u8]) -> Option<()> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return None,
            Ok(n) => filled += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(())
}

fn secrets(nonce: Id32, session: Id32) -> LaunchSecrets {
    LaunchSecrets {
        expected_nonce: nonce,
        session,
    }
}

/// The happy path: a generated-style client handshakes and gets a real value back.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn end_to_end_storeless_call() {
    let (mut service, export) = service_for(ADD);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();
    let interface = service.interface_id();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        send(&mut stream, &ClientMessage::Hello { nonce }).unwrap();
        let ready = recv(&mut stream);
        send(
            &mut stream,
            &ClientMessage::Request {
                export,
                args: vec![Json::Int(2), Json::Int(3)],
            },
        )
        .unwrap();
        let value = recv(&mut stream);
        (ready, value)
    });

    let mut conn = channel
        .accept_authenticated(&secrets(nonce, session), interface, &quick(), 16)
        .expect("accept");
    conn.run_session(&mut service, &quick()).expect("serve");
    let (ready, value) = client.join().unwrap();
    channel.teardown();

    assert_eq!(ready, Some(ServerMessage::Ready { session, interface }));
    assert_eq!(value, Some(ServerMessage::Value { data: Json::Int(5) }));
}

struct UnknownIncompleteHandler;

impl Handler for UnknownIncompleteHandler {
    fn handle(&mut self, _message: ClientMessage) -> ServerMessage {
        ServerMessage::Incomplete {
            code: "run.commit".to_string(),
            durable: DurableState::Unknown,
            span: Span { line: 4, column: 2 },
        }
    }

    fn close_after_response(&self) -> bool {
        true
    }
}

#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn unknown_incomplete_is_written_once_then_the_session_closes() {
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();
    let interface = Id32::from_bytes([0x44; 32]);

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        send(&mut stream, &ClientMessage::Hello { nonce }).unwrap();
        let ready = recv(&mut stream);
        send(
            &mut stream,
            &ClientMessage::Request {
                export: Id32::from_bytes([0x55; 32]),
                args: Vec::new(),
            },
        )
        .unwrap();
        let incomplete = recv(&mut stream);
        let _ = send(
            &mut stream,
            &ClientMessage::Request {
                export: Id32::from_bytes([0x66; 32]),
                args: Vec::new(),
            },
        );
        let later = recv(&mut stream);
        (ready, incomplete, later)
    });

    let mut conn = channel
        .accept_authenticated(&secrets(nonce, session), interface, &quick(), 16)
        .expect("accept");
    conn.run_session(&mut UnknownIncompleteHandler, &quick())
        .expect("serve");
    drop(conn);
    let (ready, incomplete, later) = client.join().unwrap();
    channel.teardown();

    assert_eq!(ready, Some(ServerMessage::Ready { session, interface }));
    assert!(matches!(
        incomplete,
        Some(ServerMessage::Incomplete {
            durable: DurableState::Unknown,
            ..
        })
    ));
    assert_eq!(later, None, "no later request receives a reply");
}

struct ClassifiedBeforeReplyHandler {
    classified: Sender<()>,
    release: Receiver<()>,
}

impl Handler for ClassifiedBeforeReplyHandler {
    fn handle(&mut self, _message: ClientMessage) -> ServerMessage {
        self.classified.send(()).expect("signal classification");
        self.release.recv().expect("release response");
        ServerMessage::Incomplete {
            code: "run.commit".to_string(),
            durable: DurableState::KnownNew,
            span: Span { line: 7, column: 3 },
        }
    }
}

/// Losing the transport after the invocation has been classified but before the reply can be
/// written gives the caller no internal durable-state fact. The request is already dispatched,
/// so the only caller-side classification is `OutcomeUnknown`; the typed `KnownNew` reply is not
/// partially delivered or retried.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn loss_after_classification_before_reply_is_outcome_unknown() {
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();
    let interface = Id32::from_bytes([0x73; 32]);
    let (classified_tx, classified_rx) = mpsc_channel();
    let (release_tx, release_rx) = mpsc_channel();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        send(&mut stream, &ClientMessage::Hello { nonce }).unwrap();
        assert!(matches!(
            recv(&mut stream),
            Some(ServerMessage::Ready { .. })
        ));
        send(
            &mut stream,
            &ClientMessage::Request {
                export: Id32::from_bytes([0x74; 32]),
                args: Vec::new(),
            },
        )
        .unwrap();
        classified_rx.recv().expect("classified");
        stream
            .shutdown(std::net::Shutdown::Both)
            .expect("drop reply transport");
        release_tx.send(()).expect("release handler");
        assert_eq!(recv(&mut stream), None, "no typed reply reached the caller");
        classify(HandoffStage::Dispatched)
    });

    let mut handler = ClassifiedBeforeReplyHandler {
        classified: classified_tx,
        release: release_rx,
    };
    let mut conn = channel
        .accept_authenticated(&secrets(nonce, session), interface, &quick(), 16)
        .expect("accept");
    conn.run_session(&mut handler, &quick()).expect("serve");
    assert_eq!(client.join().expect("client"), LossClass::OutcomeUnknown);
    channel.teardown();
}

/// First-connection-wins is bounded: a same-uid racer that connects first with a
/// bad nonce costs one attempt, and the real client is admitted on the next.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn a_bad_nonce_racer_does_not_starve_the_real_client() {
    let (mut service, export) = service_for(ADD);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();
    let interface = service.interface_id();

    let client = thread::spawn(move || {
        // A racer connects first with the wrong nonce and is refused.
        let mut bad = connect(&path);
        let _ = send(
            &mut bad,
            &ClientMessage::Hello {
                nonce: Id32::from_bytes([0xff; 32]),
            },
        );
        assert!(recv(&mut bad).is_none(), "the racer gets no Ready");
        drop(bad);

        // The real client is then admitted.
        let mut good = connect(&path);
        send(&mut good, &ClientMessage::Hello { nonce }).unwrap();
        let ready = recv(&mut good);
        send(
            &mut good,
            &ClientMessage::Request {
                export,
                args: vec![Json::Int(20), Json::Int(1)],
            },
        )
        .unwrap();
        let value = recv(&mut good);
        (ready, value)
    });

    let mut conn = channel
        .accept_authenticated(&secrets(nonce, session), interface, &quick(), 16)
        .expect("the real client is admitted after the racer");
    conn.run_session(&mut service, &quick()).expect("serve");
    let (ready, value) = client.join().unwrap();
    channel.teardown();

    assert!(matches!(ready, Some(ServerMessage::Ready { .. })));
    assert_eq!(
        value,
        Some(ServerMessage::Value {
            data: Json::Int(21)
        })
    );
}

/// A client that connects but never sends its hello does not hang the runner: the
/// handshake deadline fires and the accept budget is eventually exhausted.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn a_silent_client_times_out_without_hanging() {
    let (service, _export) = service_for(ADD);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();
    let interface = service.interface_id();

    let client = thread::spawn(move || {
        let _held = connect(&path);
        thread::sleep(Duration::from_millis(800)); // connect, then stay silent
    });

    let outcome = channel.accept_authenticated(&secrets(nonce, session), interface, &quick(), 16);
    client.join().unwrap();
    channel.teardown();
    assert!(outcome.is_err(), "a silent peer must not hang the runner");
}

/// A partial request frame (a length announced, the body withheld) ends the session
/// on the frame deadline rather than over-reading or hanging.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn a_partial_frame_ends_the_session() {
    let (mut service, _export) = service_for(ADD);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();
    let interface = service.interface_id();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        send(&mut stream, &ClientMessage::Hello { nonce }).unwrap();
        let _ready = recv(&mut stream);
        // Announce a 200-byte body, then send only a few bytes and stall.
        stream.write_all(&[0, 0, 0, 200, 1, b'{', b'"']).unwrap();
        // Hold until the server times out and drops us.
        assert!(recv(&mut stream).is_none());
    });

    let mut conn = channel
        .accept_authenticated(&secrets(nonce, session), interface, &quick(), 16)
        .expect("accept");
    // serve must return (on the frame deadline), not block forever.
    conn.run_session(&mut service, &quick())
        .expect("serve returns");
    drop(conn);
    client.join().unwrap();
    channel.teardown();
}

/// Death before the request was sent — a refused handshake — is `NotStarted`: the
/// client never received `Ready`, so its call provably never ran.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn death_before_send_classifies_not_started() {
    let (service, _export) = service_for(ADD);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();
    let interface = service.interface_id();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        // Present the wrong nonce; the runner refuses and closes.
        let _ = send(
            &mut stream,
            &ClientMessage::Hello {
                nonce: Id32::from_bytes([1; 32]),
            },
        );
        let ready = recv(&mut stream);
        // No Ready arrived and no request was sent.
        assert!(ready.is_none());
        classify(HandoffStage::BeforeSend)
    });

    let outcome = channel.accept_authenticated(&secrets(nonce, session), interface, &quick(), 16);
    let verdict = client.join().unwrap();
    channel.teardown();
    assert!(outcome.is_err());
    assert_eq!(verdict, LossClass::NotStarted);
}

/// Death after the request was dispatched — the runner dies before replying — is
/// `OutcomeUnknown`: the call may have run, and its result is unknowable.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn death_after_dispatch_classifies_outcome_unknown() {
    let (service, export) = service_for(ADD);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();
    let interface = service.interface_id();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        send(&mut stream, &ClientMessage::Hello { nonce }).unwrap();
        let _ready = recv(&mut stream);
        // Dispatch the request; the runner has already died.
        let _ = send(
            &mut stream,
            &ClientMessage::Request {
                export,
                args: vec![Json::Int(1), Json::Int(2)],
            },
        );
        let reply = recv(&mut stream);
        assert!(reply.is_none(), "no reply arrives after the runner dies");
        classify(HandoffStage::Dispatched)
    });

    // Accept and handshake, then die immediately (drop the connection) instead of
    // serving.
    let conn = channel
        .accept_authenticated(&secrets(nonce, session), interface, &quick(), 16)
        .expect("accept");
    drop(conn);
    let verdict = client.join().unwrap();
    channel.teardown();
    assert_eq!(verdict, LossClass::OutcomeUnknown);
}
