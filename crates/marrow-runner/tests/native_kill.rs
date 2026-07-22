//! The native attached path's death boundary (term 13): a call dispatched to a real
//! `marrow-runner attach` process whose runner then dies before replying is classified
//! `OutcomeUnknown` and never replayed.
//!
//! Like the other socket suites this spawns a runner that binds a Unix listener, which the
//! command sandbox denies with `EPERM`, so it is `#[ignore]`d and run with the sandbox
//! disabled:
//!
//! ```text
//! cargo test -p marrow-runner --test native_kill -- --ignored
//! ```
//!
//! The terminal-side classification (a lost reply after dispatch → `CallOutcome::OutcomeUnknown`)
//! is covered deterministically by the `client` unit tests; this proves the other half — that a
//! real native runner, killed after it has been handed the request, closes the connection
//! (end-of-stream) rather than replying, which is exactly the boundary the client maps to
//! `OutcomeUnknown` for a `Dispatched` handoff stage.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use marrow_local_wire::{
    ClientMessage, HandoffStage, Id32, Json, LossClass, ServerMessage, classify, frame_body_len,
};
use marrow_runner::{CallOutcome, attach_and_call};
use marrow_verify::{VerifiedImage, verify};
use marrow_vm::Value;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/v01/conformance/workshop")
}

fn runner_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_marrow-runner"))
}

fn compile_verify() -> (VerifiedImage, Vec<u8>) {
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
    (verify(&bytes).expect("verify"), bytes)
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

fn scratch(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "marrow-native-kill-{tag}-{}-{nonce}-{counter}",
        std::process::id()
    ))
}

fn provision(store: &Path, image: &VerifiedImage) {
    let (schemas, sites) = marrow_vm::derive_store_schemas(image).expect("flat-executable");
    let report = marrow_lifecycle::ProvisionReport::new(store, image, &schemas);
    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    marrow_lifecycle::provision_image(store, image, schemas, sites, &approval).expect("provision");
}

/// One field of the launch descriptor JSON line, by key.
fn descriptor_field(line: &str, key: &str) -> String {
    let needle = format!("\"{key}\":\"");
    let start = line.find(&needle).expect("descriptor field present") + needle.len();
    let rest = &line[start..];
    let end = rest.find('"').expect("descriptor field terminator");
    rest[..end].to_string()
}

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

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launch_attached(image_bytes: &[u8], store: &Path) -> (KillOnDrop, UnixStream, PathBuf) {
    let image_path = scratch("img").with_extension("mwi");
    std::fs::write(&image_path, image_bytes).expect("stage image");

    let nonce = marrow_runner::mint_id().expect("nonce");
    let mut child = Command::new(runner_exe())
        .arg("attach")
        .arg("--image")
        .arg(&image_path)
        .arg("--store")
        .arg(store)
        .env("MARROW_RUNNER_NONCE", nonce.to_hex())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn runner attach");

    let mut line = String::new();
    {
        let stdout = child.stdout.take().expect("runner stdout");
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read descriptor line");
    }
    let socket = descriptor_field(&line, "socket");
    let mut stream = UnixStream::connect(&socket).expect("connect");
    stream
        .write_all(
            &ClientMessage::Hello { nonce }
                .encode()
                .expect("encode hello"),
        )
        .expect("send hello");
    let ready = recv(&mut stream);
    assert!(
        matches!(ready, Some(ServerMessage::Ready { .. })),
        "the runner proved the handshake: {ready:?}",
    );
    (KillOnDrop(child), stream, image_path)
}

fn call_value(
    image: &VerifiedImage,
    bytes: &[u8],
    store: &Path,
    name: &str,
    args: Vec<Json>,
) -> Option<Value> {
    match attach_and_call(
        &runner_exe(),
        image,
        bytes,
        store,
        *export_id(image, name).bytes(),
        args,
    )
    .unwrap_or_else(|error| panic!("post-crash `{name}` call failed: {}", error.code()))
    {
        CallOutcome::Value(value) => value,
        CallOutcome::Fault { code, .. } => panic!("post-crash `{name}` faulted: {code}"),
        CallOutcome::Incomplete { code, durable, .. } => {
            panic!("post-crash `{name}` was incomplete: {code} ({durable:?})")
        }
        CallOutcome::Reject { code } => panic!("post-crash `{name}` was rejected: {code}"),
        CallOutcome::OutcomeUnknown => panic!("post-crash `{name}` lost its reply"),
    }
}

fn snapshot(
    image: &VerifiedImage,
    bytes: &[u8],
    store: &Path,
    id: i64,
) -> (Option<Value>, Option<Value>, Option<Value>, Option<Value>) {
    (
        call_value(image, bytes, store, "present", vec![Json::Int(id)]),
        call_value(image, bytes, store, "assetName", vec![Json::Int(id)]),
        call_value(image, bytes, store, "hasLog", vec![Json::Int(id)]),
        call_value(image, bytes, store, "catalogued", Vec::new()),
    )
}

fn old_snapshot() -> (Option<Value>, Option<Value>, Option<Value>, Option<Value>) {
    (
        Some(Value::Bool(false)),
        Some(Value::Optional(None)),
        Some(Value::Bool(false)),
        Some(Value::Int(0)),
    )
}

fn new_snapshot(name: &str) -> (Option<Value>, Option<Value>, Option<Value>, Option<Value>) {
    (
        Some(Value::Bool(true)),
        Some(Value::Optional(Some(Box::new(Value::Text(name.into()))))),
        Some(Value::Bool(true)),
        Some(Value::Int(1)),
    )
}

fn add_request(image: &VerifiedImage, id: i64, name: &str) -> ClientMessage {
    ClientMessage::Request {
        export: export_id(image, "add"),
        args: vec![
            Json::Int(id),
            Json::Str(format!("T-{id}")),
            Json::Str(name.into()),
            Json::Str("power".into()),
            Json::Str(marrow_temporal::format_instant(0).expect("epoch instant")),
        ],
    }
}

/// Killing an authenticated native runner before any request bytes are sent leaves the exact
/// provisioned state unchanged. The terminal-side handoff stage is `BeforeSend`, so this loss is
/// `NotStarted`; reopening performs the unclean-owner audit but cannot invent or replay a call.
#[test]
#[ignore = "spawns a runner that binds a Unix socket; run with the sandbox disabled"]
fn native_death_before_request_is_not_started_and_leaves_the_old_state() {
    let (image, bytes) = compile_verify();
    let store = scratch("before-send-store").join("store");
    std::fs::create_dir_all(store.parent().expect("parent")).expect("scratch dir");
    provision(&store, &image);

    let (mut guard, mut stream, image_path) = launch_attached(&bytes, &store);
    guard.0.kill().expect("kill runner before request");
    let _ = guard.0.wait();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(recv(&mut stream), None, "no call reply can exist");
    assert_eq!(classify(HandoffStage::BeforeSend), LossClass::NotStarted,);
    assert_eq!(
        snapshot(&image, &bytes, &store, 1),
        old_snapshot(),
        "a death before request dispatch leaves the exact provisioned state",
    );

    let _ = std::fs::remove_dir_all(store.parent().expect("parent"));
    let _ = std::fs::remove_file(&image_path);
}

/// Once the typed value reply has arrived, killing the attached runner cannot roll the
/// confirmed transaction back. A fresh process observes the exact cross-root committed state;
/// no second application invocation is used to establish it.
#[test]
#[ignore = "spawns a runner that binds a Unix socket; run with the sandbox disabled"]
fn native_death_after_reply_preserves_the_exact_committed_state() {
    let (image, bytes) = compile_verify();
    let store = scratch("after-reply-store").join("store");
    std::fs::create_dir_all(store.parent().expect("parent")).expect("scratch dir");
    provision(&store, &image);

    let (mut guard, mut stream, image_path) = launch_attached(&bytes, &store);
    stream
        .write_all(
            &add_request(&image, 2, "Impact Driver")
                .encode()
                .expect("encode request"),
        )
        .expect("dispatch request");
    assert_eq!(
        recv(&mut stream),
        Some(ServerMessage::Value {
            data: Json::Bool(true),
        }),
        "the caller receives the completed invocation before process death",
    );
    guard.0.kill().expect("kill runner after reply");
    let _ = guard.0.wait();
    assert_eq!(
        snapshot(&image, &bytes, &store, 2),
        new_snapshot("Impact Driver"),
        "a received reply pins the exact confirmed cross-root state",
    );

    let _ = std::fs::remove_dir_all(store.parent().expect("parent"));
    let _ = std::fs::remove_file(&image_path);
}

/// A native call dispatched to a real `attach` process whose runner is then killed before it
/// replies ends at end-of-stream, not a reply — the boundary the terminal client classifies as
/// `OutcomeUnknown` for a `Dispatched` handoff stage. Reopening after the crash observes either
/// the exact old state or the exact fully committed cross-root state, never a torn mixture, and
/// the one dispatched mutation is never replayed.
#[test]
#[ignore = "spawns a runner that binds a Unix socket; run with the sandbox disabled"]
fn a_native_call_lost_to_runner_death_after_dispatch_is_outcome_unknown() {
    let (image, bytes) = compile_verify();
    let store = scratch("store").join("store");
    std::fs::create_dir_all(store.parent().expect("parent")).expect("scratch dir");
    provision(&store, &image);

    let (mut guard, mut stream, image_path) = launch_attached(&bytes, &store);

    // Dispatch a mutating call (`add`).
    stream
        .write_all(
            &add_request(&image, 1, "Cordless Drill")
                .encode()
                .expect("encode request"),
        )
        .expect("dispatch request");

    // Kill the runner after the request is dispatched but before its reply is read.
    guard.0.kill().expect("kill runner");
    let _ = guard.0.wait();

    // The reply never arrives: the connection ends at end-of-stream. The terminal client maps
    // this, for a dispatched request, to CallOutcome::OutcomeUnknown (see the `client` unit
    // tests); the wire loss model classifies the same stage as OutcomeUnknown.
    std::thread::sleep(Duration::from_millis(50));
    let lost = recv(&mut stream);
    assert!(
        lost.is_none(),
        "a killed runner sends no reply, not a value: {lost:?}"
    );
    assert_eq!(
        classify(HandoffStage::Dispatched),
        LossClass::OutcomeUnknown,
        "a dispatched call lost to death is outcome-unknown, never replayed",
    );

    let observed = snapshot(&image, &bytes, &store, 1);
    assert!(
        observed == old_snapshot() || observed == new_snapshot("Cordless Drill"),
        "crash recovery must expose one atomic state across the asset, nested log, and tally; \
         observed {observed:?}",
    );

    let _ = std::fs::remove_dir_all(store.parent().expect("parent"));
    let _ = std::fs::remove_file(&image_path);
}
