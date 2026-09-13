//! The runner's provision dispatch: a `ClientMessage::Provision` over the session provisions
//! the launched image's store, gated by the accepted-report token. The server (runner) is one
//! caller of the wire `Provision` DTO; the encoder here is the other. No socket is bound.

use std::path::PathBuf;

use marrow_lifecycle::ProvisionReport;
use marrow_local_wire::{ClientMessage, ServerMessage};
use marrow_runner::Service;

const SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn readValue(n: int): int {
    return ^counters[n].value ?? 0
}
"#;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// Compile the durable fixture to image bytes (deterministic — the same bytes verify to the
/// same image, so the test can build both a Service and a separate image for the report).
fn image_bytes() -> Vec<u8> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        SOURCE.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    marrow_compile::compile(&project)
        .expect("compile")
        .image
        .bytes
}

fn scratch() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "marrow-runner-provision-{}-{nonce}-{counter}",
        std::process::id()
    ))
}

/// The report token the owner accepts: derived from the same image the service serves.
fn approval_token(store: &std::path::Path) -> String {
    let image = marrow_verify::verify(&image_bytes()).expect("verify");
    ProvisionReport::new(store, &marrow_lifecycle::prepare(image))
        .expect("flat-executable")
        .token()
}

/// A `Provision` with a matching approval token provisions the store and receipts the
/// instance; opening the destination confirms the store is complete.
#[test]
fn a_provision_request_with_a_matching_approval_provisions() {
    let base = scratch();
    std::fs::create_dir_all(&base).expect("scratch base");
    let store = base.join("store");
    let service = Service::build(marrow_verify::verify(&image_bytes()).expect("verify"))
        .expect("service builds");

    let response = service
        .handle(
            ClientMessage::Provision {
                store: store.display().to_string(),
                approval: approval_token(&store),
            },
            None,
        )
        .expect("provision response fits");
    let (response, turn) =
        ServerMessage::decode_with_turn(&response.as_bytes()[4..]).expect("response decodes");
    assert_eq!(turn, None);

    match response {
        ServerMessage::Provisioned { instance } => {
            assert_eq!(instance.len(), 32, "the instance is 32 hex characters");
            assert!(store.is_dir(), "the store directory was published");
        }
        other => panic!("expected Provisioned, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// A `Provision` whose approval token does not match the report the runner rebuilds is
/// rejected, and no store is published.
#[test]
fn a_provision_request_with_a_wrong_approval_is_rejected() {
    let base = scratch();
    std::fs::create_dir_all(&base).expect("scratch base");
    let store = base.join("store");
    let service = Service::build(marrow_verify::verify(&image_bytes()).expect("verify"))
        .expect("service builds");

    let response = service
        .handle(
            ClientMessage::Provision {
                store: store.display().to_string(),
                approval: "0000000000000000".to_string(),
            },
            None,
        )
        .expect("reject response fits");
    let (response, turn) =
        ServerMessage::decode_with_turn(&response.as_bytes()[4..]).expect("response decodes");
    assert_eq!(turn, Some(0));

    assert!(
        matches!(response, ServerMessage::Reject { .. }),
        "a mismatched approval is rejected, got {response:?}",
    );
    assert!(!store.exists(), "a rejected provision publishes no store");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn provision_receipt_failure_preserves_the_published_store() {
    use std::process::{Command, Stdio};

    let base = scratch();
    std::fs::create_dir_all(&base).expect("scratch base");
    let bytes = image_bytes();
    let image_path = base.join("program.image");
    std::fs::write(&image_path, &bytes).expect("write image");
    let store = base.join("store");
    let (reader, writer) = std::io::pipe().expect("output pipe");
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_marrow-runner"))
        .arg("provision")
        .arg("--image")
        .arg(&image_path)
        .arg("--store")
        .arg(&store)
        .arg("--yes")
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .expect("provision child completes");
    std::fs::write(base.join("stderr"), &output.stderr).expect("retain child stderr");

    let image = marrow_verify::verify(&bytes).expect("verify image");
    let attachment = match marrow_lifecycle::attach(&store, marrow_lifecycle::prepare(image))
        .expect("published store admits the exact image")
    {
        marrow_lifecycle::AttachOutcome::AlreadyActive(attachment) => attachment,
        marrow_lifecycle::AttachOutcome::Rebound { .. } => panic!("image is already active"),
    };
    eprintln!("published store verified; scratch: {}", base.display());
    drop(attachment);
    assert_eq!(std::fs::read(&image_path).expect("retained image"), bytes);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .any(|line| line.starts_with(marrow_codes::Code::IoWrite.as_str()))
    );
    std::fs::remove_dir_all(base).expect("remove successful fixture");
}

enum ImportStore {
    Fresh,
    Existing,
}

enum ClosedStream {
    Stdout,
    Stderr,
}

fn import_with_closed_stream(destination: ImportStore, closed: ClosedStream) {
    use marrow_local_wire::{Id32, Json};
    use marrow_runner::{AttachedService, Handler};
    use std::process::{Command, Stdio};

    let base = scratch();
    std::fs::create_dir_all(&base).expect("scratch base");
    let bytes = image_bytes();
    let image = marrow_verify::verify(&bytes).expect("verify image");
    assert_eq!(image.exports().len(), 1);
    let read = Id32::from_bytes(*image.exports()[0].id().bytes());
    let image_path = base.join("program.image");
    let corpus_path = base.join("seed.jsonl");
    std::fs::write(&image_path, &bytes).expect("write image");
    let corpus = b"{\"id\":1,\"value\":7}\n";
    std::fs::write(&corpus_path, corpus).expect("write corpus");
    let store = base.join("store");
    if matches!(destination, ImportStore::Existing) {
        let prepared = marrow_lifecycle::prepare(image.clone());
        let report = ProvisionReport::new(&store, &prepared).expect("report");
        let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
        marrow_lifecycle::provision_image(&store, &prepared, &approval).expect("provision");
    }
    let (reader, writer) = std::io::pipe().expect("output pipe");
    drop(reader);
    let mut command = Command::new(env!("CARGO_BIN_EXE_marrow-runner"));
    command
        .args(["import", "--image"])
        .arg(&image_path)
        .arg("--store")
        .arg(&store)
        .arg("--jsonl")
        .arg(&corpus_path)
        .args(["--root", "counters", "--keys", "id"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match closed {
        ClosedStream::Stdout => {
            command.stdout(writer);
        }
        ClosedStream::Stderr => {
            command.stderr(writer);
        }
    }
    let output = command.output().expect("import child completes");
    std::fs::write(base.join("stdout"), &output.stdout).expect("retain stdout");
    std::fs::write(base.join("stderr"), &output.stderr).expect("retain stderr");
    eprintln!("import fixture: {}", base.display());
    let attachment = match marrow_lifecycle::attach(&store, marrow_lifecycle::prepare(image))
        .expect("published store admits exact image")
    {
        marrow_lifecycle::AttachOutcome::AlreadyActive(attachment) => attachment,
        marrow_lifecycle::AttachOutcome::Rebound { .. } => panic!("image is already active"),
    };
    let mut service = AttachedService::new(attachment);
    let response = service
        .handle(
            ClientMessage::Request {
                export: read,
                args: vec![Json::Int(1)],
            },
            Some(0),
        )
        .expect("read imported value");
    let (response, _) = ServerMessage::decode_with_turn(&response.as_bytes()[4..]).expect("decode");
    assert_eq!(response, ServerMessage::Value { data: Json::Int(7) });
    drop(service);
    assert_eq!(std::fs::read(&corpus_path).expect("corpus"), corpus);
    assert_eq!(std::fs::read(&image_path).expect("image"), bytes);
    let expected = match closed {
        ClosedStream::Stdout => {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .any(|line| line.starts_with(marrow_codes::Code::IoWrite.as_str()))
            );
            1
        }
        ClosedStream::Stderr => {
            assert_eq!(
                output.stdout,
                b"{\"batches_committed\":1,\"rows_imported\":1}\n"
            );
            0
        }
    };
    assert_eq!(output.status.code(), Some(expected));
    std::fs::remove_dir_all(base).expect("remove successful fixture");
}

#[test]
fn import_receipt_failure_preserves_new_store_values() {
    import_with_closed_stream(ImportStore::Fresh, ClosedStream::Stdout);
}

#[test]
fn import_receipt_failure_preserves_existing_store_values() {
    import_with_closed_stream(ImportStore::Existing, ClosedStream::Stdout);
}

#[test]
fn import_notice_failure_does_not_interrupt_import() {
    import_with_closed_stream(ImportStore::Fresh, ClosedStream::Stderr);
}

#[test]
fn runner_usage_stderr_failure_keeps_usage_status() {
    use std::process::{Command, Stdio};
    for command in ["provision", "import"] {
        let (reader, writer) = std::io::pipe().expect("diagnostic pipe");
        drop(reader);
        let status = Command::new(env!("CARGO_BIN_EXE_marrow-runner"))
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(writer)
            .status()
            .expect("usage child completes");
        assert_eq!(status.code(), Some(2));
    }
}

#[test]
fn provision_report_failure_precedes_publication() {
    use std::process::{Command, Stdio};
    let base = scratch();
    std::fs::create_dir_all(&base).expect("scratch base");
    let image = base.join("program.image");
    std::fs::write(&image, image_bytes()).expect("image");
    let store = base.join("store");
    let (reader, writer) = std::io::pipe().expect("report pipe");
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_marrow-runner"))
        .args(["provision", "--image"])
        .arg(&image)
        .arg("--store")
        .arg(&store)
        .arg("--yes")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(writer)
        .output()
        .expect("provision child completes");
    assert!(!store.exists());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    std::fs::remove_dir_all(base).expect("remove successful fixture");
}

#[test]
fn invalid_image_with_closed_stderr_leaves_store_absent() {
    use std::process::{Command, Stdio};
    let base = scratch();
    std::fs::create_dir_all(&base).expect("scratch base");
    let invalid = base.join("invalid.image");
    std::fs::write(&invalid, b"not an image").expect("invalid image");
    let store = base.join("store");
    for image in [invalid, base.join("missing.image")] {
        for name in ["provision", "import"] {
            let (reader, writer) = std::io::pipe().expect("diagnostic pipe");
            drop(reader);
            let mut command = Command::new(env!("CARGO_BIN_EXE_marrow-runner"));
            command
                .arg(name)
                .arg("--image")
                .arg(&image)
                .arg("--store")
                .arg(&store);
            if name == "provision" {
                command.arg("--yes");
            } else {
                command
                    .arg("--jsonl")
                    .arg(base.join("unused.jsonl"))
                    .args(["--root", "counters", "--keys", "id"]);
            }
            let output = command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(writer)
                .output()
                .expect("refusal child completes");
            assert!(!store.exists());
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
        }
    }
    std::fs::remove_dir_all(base).expect("remove successful fixture");
}
