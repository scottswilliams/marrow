//! The ephemeral attached session over a real socket: hello-before-mint ordering, the durable
//! call path, the death-boundary loss classes, and the handshake-confusion probes.
//!
//! Like the `channel` suite, these bind a Unix listener and connect to it, which the command
//! sandbox denies with `EPERM`, so they are `#[ignore]`d and run explicitly with the sandbox
//! disabled:
//!
//! ```text
//! cargo test -p marrow-runner --test ephemeral_kill -- --ignored
//! ```
//!
//! A default-battery test compiles the shared fixture without opening a socket, so surface drift
//! is not hidden by the ignore boundary. The runner side (which owns a non-`Send` `VerifiedImage`)
//! stays on the test thread; the client side runs on a spawned thread and speaks the wire
//! directly.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use marrow_local_wire::{ClientMessage, Id32, Json, ServerMessage, frame_body_len};
use marrow_runner::{AttachedEphemeralService, Channel, Deadlines, LaunchSecrets, mint_id};
use marrow_verify::VerifiedImage;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/v01/conformance/workshop")
}

fn compile_verify() -> VerifiedImage {
    let source = std::fs::read(fixture_dir().join("src/main.mw")).expect("read fixture source");
    let ids = std::fs::read(fixture_dir().join(".marrow/ids")).expect("read fixture ledger");
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source,
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(&ids),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let bytes = marrow_compile::compile(&project)
        .expect("compile")
        .image
        .bytes;
    marrow_verify::verify(&bytes).expect("verify")
}

fn identity_of(image: &VerifiedImage) -> Id32 {
    Id32::from_bytes(image.image_id().0)
}

fn export_id(image: &VerifiedImage, name: &str) -> Id32 {
    Id32::from_bytes(
        *image
            .exports()
            .iter()
            .find(|export| image.function(export.function()).name() == name)
            .unwrap_or_else(|| panic!("export `{name}` present"))
            .id()
            .bytes(),
    )
}

/// Brisk deadlines so the timeout-shaped tests finish quickly.
fn quick() -> Deadlines {
    Deadlines {
        handshake: Duration::from_millis(300),
        frame: Duration::from_millis(300),
        accept: Duration::from_millis(600),
        poll: Duration::from_millis(1),
    }
}

fn secrets(nonce: Id32, session: Id32) -> LaunchSecrets {
    LaunchSecrets {
        expected_nonce: nonce,
        session,
    }
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

/// The fixture compiles and verifies without opening a socket, so drift in the durable surface
/// these tests drive is caught even in the sandboxed default battery.
#[test]
fn embedded_workshop_source_compiles_and_verifies() {
    let image = compile_verify();
    assert!(
        !image.exports().is_empty(),
        "the workshop image exports calls"
    );
}

/// The memory attachment opens strictly after the handshake: a client that fails to authenticate
/// never causes the handler — and thus the in-memory store — to be constructed.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn a_refused_handshake_never_opens_the_memory_attachment() {
    let image = compile_verify();
    let identity = identity_of(&image);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        // Present the wrong nonce; the runner refuses and closes.
        let _ = send(
            &mut stream,
            &ClientMessage::Hello {
                nonce: Id32::from_bytes([0xab; 32]),
            },
        );
        assert!(
            recv(&mut stream).is_none(),
            "a refused client gets no Ready"
        );
    });

    let minted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&minted);
    let outcome = channel.accept_and_serve(&secrets(nonce, session), identity, &quick(), 4, || {
        flag.store(true, Ordering::SeqCst);
        AttachedEphemeralService::mint(image)
    });
    client.join().unwrap();
    channel.teardown();

    assert!(outcome.is_err(), "an unauthenticated peer is not served");
    assert!(
        !minted.load(Ordering::SeqCst),
        "the memory attachment must not open for an unauthenticated peer",
    );
}

/// An authenticated client drives durable calls over one attachment: `add` commits and a later
/// `catalogued` read on the same session observes the committed counter — the in-memory store
/// persists across requests within the session, all over the real wire and the real handler.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn an_authenticated_client_commits_and_reads_back() {
    let image = compile_verify();
    let identity = identity_of(&image);
    let add = export_id(&image, "add");
    let catalogued = export_id(&image, "catalogued");
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        send(&mut stream, &ClientMessage::Hello { nonce }).unwrap();
        let ready = recv(&mut stream);
        send(
            &mut stream,
            &ClientMessage::Request {
                export: add,
                args: vec![
                    Json::Int(1),
                    Json::Str("T-100".into()),
                    Json::Str("Cordless Drill".into()),
                    Json::Str("power".into()),
                    Json::Str(epoch),
                ],
            },
        )
        .unwrap();
        let added = recv(&mut stream);
        send(
            &mut stream,
            &ClientMessage::Request {
                export: catalogued,
                args: vec![],
            },
        )
        .unwrap();
        let count = recv(&mut stream);
        (ready, added, count)
    });

    let mint_flag = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&mint_flag);
    channel
        .accept_and_serve(&secrets(nonce, session), identity, &quick(), 4, || {
            flag.store(true, Ordering::SeqCst);
            AttachedEphemeralService::mint(image)
        })
        .expect("serve the authenticated client");
    let (ready, added, count) = client.join().unwrap();
    channel.teardown();

    assert_eq!(
        ready,
        Some(ServerMessage::Ready {
            session,
            interface: identity
        })
    );
    assert_eq!(
        added,
        Some(ServerMessage::Value {
            data: Json::Bool(true)
        })
    );
    assert_eq!(
        count,
        Some(ServerMessage::Value { data: Json::Int(1) }),
        "the committed counter reads back on a later request in the same session",
    );
    assert!(
        mint_flag.load(Ordering::SeqCst),
        "the attachment opened after the handshake"
    );
}

/// Death before the request was sent — a refused handshake — is `NotStarted`: the client never
/// received `Ready`, so its call provably never ran.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn death_before_send_classifies_not_started() {
    use marrow_local_wire::{HandoffStage, LossClass, classify};

    let image = compile_verify();
    let identity = identity_of(&image);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        let _ = send(
            &mut stream,
            &ClientMessage::Hello {
                nonce: Id32::from_bytes([1; 32]),
            },
        );
        assert!(recv(&mut stream).is_none(), "no Ready arrives");
        classify(HandoffStage::BeforeSend)
    });

    let outcome = channel.accept_authenticated(&secrets(nonce, session), identity, &quick(), 4);
    let verdict = client.join().unwrap();
    channel.teardown();
    assert!(outcome.is_err());
    assert_eq!(verdict, LossClass::NotStarted);
}

/// Death after the request was dispatched — the runner dies before replying — is
/// `OutcomeUnknown`: the call may have run against the in-memory store, and its result is
/// unknowable and never replayed.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn death_after_dispatch_classifies_outcome_unknown() {
    use marrow_local_wire::{HandoffStage, LossClass, classify};

    let image = compile_verify();
    let identity = identity_of(&image);
    let add = export_id(&image, "add");
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        send(&mut stream, &ClientMessage::Hello { nonce }).unwrap();
        let _ready = recv(&mut stream);
        // Dispatch a mutating request; the runner has already died.
        let _ = send(
            &mut stream,
            &ClientMessage::Request {
                export: add,
                args: vec![
                    Json::Int(1),
                    Json::Str("T-100".into()),
                    Json::Str("Cordless Drill".into()),
                    Json::Str("power".into()),
                    Json::Str(epoch),
                ],
            },
        );
        assert!(
            recv(&mut stream).is_none(),
            "no reply after the runner dies"
        );
        classify(HandoffStage::Dispatched)
    });

    // Accept and handshake, then die immediately (drop the connection) instead of serving.
    let conn = channel
        .accept_authenticated(&secrets(nonce, session), identity, &quick(), 4)
        .expect("accept");
    drop(conn);
    let verdict = client.join().unwrap();
    channel.teardown();
    assert_eq!(verdict, LossClass::OutcomeUnknown);
}

/// A `Request` sent as the first frame — before any `Hello` — fails the handshake typed: the
/// runner never proves `Ready`, so the two handshakes cannot be confused into a served call.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn a_request_before_hello_fails_the_handshake() {
    let image = compile_verify();
    let identity = identity_of(&image);
    let add = export_id(&image, "add");
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        // The first frame is a Request, not a Hello.
        let _ = send(
            &mut stream,
            &ClientMessage::Request {
                export: add,
                args: vec![],
            },
        );
        recv(&mut stream)
    });

    let outcome = channel.accept_authenticated(&secrets(nonce, session), identity, &quick(), 4);
    let reply = client.join().unwrap();
    channel.teardown();
    assert!(outcome.is_err(), "a request-first client is not admitted");
    assert!(
        reply.is_none(),
        "no Ready and no value for a request-first client"
    );
}

/// A `Provision` sent as the first frame — before any `Hello` — likewise fails the handshake:
/// provisioning is a separate one-shot command, never a mid-handshake operation.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn a_provision_before_hello_fails_the_handshake() {
    let image = compile_verify();
    let identity = identity_of(&image);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        let _ = send(
            &mut stream,
            &ClientMessage::Provision {
                store: "/tmp/whatever".into(),
                approval: "deadbeef".into(),
            },
        );
        recv(&mut stream)
    });

    let outcome = channel.accept_authenticated(&secrets(nonce, session), identity, &quick(), 4);
    let reply = client.join().unwrap();
    channel.teardown();
    assert!(outcome.is_err());
    assert!(reply.is_none());
}

/// After a proven handshake, a second `Hello` and a mid-session `Provision` are typed rejects,
/// never a served call: the ephemeral session admits only `Request`s once it is running.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn post_handshake_hello_and_provision_are_rejected() {
    let image = compile_verify();
    let identity = identity_of(&image);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        send(&mut stream, &ClientMessage::Hello { nonce }).unwrap();
        let _ready = recv(&mut stream);
        send(
            &mut stream,
            &ClientMessage::Hello {
                nonce: Id32::from_bytes([2; 32]),
            },
        )
        .unwrap();
        let after_hello = recv(&mut stream);
        send(
            &mut stream,
            &ClientMessage::Provision {
                store: "/tmp/x".into(),
                approval: "ab".into(),
            },
        )
        .unwrap();
        let after_provision = recv(&mut stream);
        (after_hello, after_provision)
    });

    channel
        .accept_and_serve(&secrets(nonce, session), identity, &quick(), 4, || {
            AttachedEphemeralService::mint(image)
        })
        .expect("serve");
    let (after_hello, after_provision) = client.join().unwrap();
    channel.teardown();

    let handshake = marrow_codes::Code::RunnerHandshake.as_str().to_string();
    assert_eq!(
        after_hello,
        Some(ServerMessage::Reject {
            code: handshake.clone(),
        }),
        "a second Hello is a typed handshake reject",
    );
    assert_eq!(
        after_provision,
        Some(ServerMessage::Reject { code: handshake }),
        "a mid-session Provision is a typed handshake reject",
    );
}

/// A client that pins the wrong served identity refuses the runner's `Ready`: the handshake
/// proves the exact image identity, and a mismatch is caught before any call.
#[test]
#[ignore = "binds a Unix socket; run with the sandbox disabled"]
fn a_client_refuses_a_mismatched_identity() {
    let image = compile_verify();
    let identity = identity_of(&image);
    let channel = Channel::bind().expect("bind");
    let path = channel.socket_path().to_path_buf();
    let nonce = mint_id().unwrap();
    let session = mint_id().unwrap();

    // The client independently expects a different identity than the runner serves.
    let expected_by_client = Id32::from_bytes([0x5a; 32]);
    assert_ne!(expected_by_client, identity);

    let client = thread::spawn(move || {
        let mut stream = connect(&path);
        send(&mut stream, &ClientMessage::Hello { nonce }).unwrap();
        match recv(&mut stream) {
            Some(ServerMessage::Ready { interface, .. }) => interface == expected_by_client,
            _ => false,
        }
    });

    channel
        .accept_and_serve(&secrets(nonce, session), identity, &quick(), 4, || {
            AttachedEphemeralService::mint(image)
        })
        .expect("serve");
    let client_accepted = client.join().unwrap();
    channel.teardown();
    assert!(
        !client_accepted,
        "a client must refuse a Ready whose served identity is not the one it expects",
    );
}
