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
use marrow_verify::{VerifiedImage, verify};

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
    let ids = std::fs::read(fixture_dir().join("marrow.ids")).expect("read fixture ledger");
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
    std::env::temp_dir().join(format!("marrow-native-kill-{tag}-{}-{nonce}-{counter}", std::process::id()))
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

/// A native call dispatched to a real `attach` process whose runner is then killed before it
/// replies ends at end-of-stream, not a reply — the boundary the terminal client classifies as
/// `OutcomeUnknown` for a `Dispatched` handoff stage. The mutating call is never replayed.
#[test]
#[ignore = "spawns a runner that binds a Unix socket; run with the sandbox disabled"]
fn a_native_call_lost_to_runner_death_after_dispatch_is_outcome_unknown() {
    let (image, bytes) = compile_verify();
    let store = scratch("store").join("store");
    std::fs::create_dir_all(store.parent().expect("parent")).expect("scratch dir");
    provision(&store, &image);

    // Stage the image to a private file the runner reads via --image.
    let image_path = scratch("img").with_extension("mwi");
    std::fs::write(&image_path, &bytes).expect("stage image");

    // The supervisor-set launch nonce: handed through the environment (never echoed on the
    // descriptor line), proven in the handshake.
    let nonce = marrow_runner::mint_id().expect("nonce");
    let mut child = Command::new(runner_exe())
        .arg("attach")
        .arg("--image")
        .arg(&image_path)
        .arg("--store")
        .arg(&store)
        .env("MARROW_RUNNER_NONCE", nonce.to_hex())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn runner attach");

    // Read the one launch-descriptor line.
    let mut line = String::new();
    {
        let stdout = child.stdout.take().expect("runner stdout");
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read descriptor line");
    }
    let mut guard = KillOnDrop(child);
    let socket = descriptor_field(&line, "socket");

    // Handshake, then dispatch a mutating call (`add`).
    let mut stream = UnixStream::connect(&socket).expect("connect");
    stream
        .write_all(&ClientMessage::Hello { nonce }.encode().expect("encode hello"))
        .expect("send hello");
    let ready = recv(&mut stream);
    assert!(
        matches!(ready, Some(ServerMessage::Ready { .. })),
        "the runner proved the handshake: {ready:?}"
    );
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");
    stream
        .write_all(
            &ClientMessage::Request {
                export: export_id(&image, "add"),
                args: vec![
                    Json::Int(1),
                    Json::Str("T-100".into()),
                    Json::Str("Cordless Drill".into()),
                    Json::Str("power".into()),
                    Json::Str(epoch),
                ],
            }
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
    assert!(lost.is_none(), "a killed runner sends no reply, not a value: {lost:?}");
    assert_eq!(
        classify(HandoffStage::Dispatched),
        LossClass::OutcomeUnknown,
        "a dispatched call lost to death is outcome-unknown, never replayed",
    );

    let _ = std::fs::remove_dir_all(store.parent().expect("parent"));
    let _ = std::fs::remove_file(&image_path);
}
