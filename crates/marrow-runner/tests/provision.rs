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

#[test]
fn recovery_output_failure_keeps_completed_activation_and_preserved_bytes() {
    use std::process::{Command, Stdio};

    for format in ["text", "jsonl"] {
        let base = scratch();
        std::fs::create_dir_all(&base).expect("scratch base");
        let bytes = image_bytes();
        let image_path = base.join("program.image");
        std::fs::write(&image_path, &bytes).expect("image");
        let store = base.join("store");
        let image = marrow_verify::verify(&bytes).expect("verify");
        let prepared = marrow_lifecycle::prepare(image.clone());
        let report = ProvisionReport::new(&store, &prepared).expect("report");
        let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
        let provisioned =
            marrow_lifecycle::provision_image(&store, &prepared, &approval).expect("provision");
        let head = std::fs::read(store.join(marrow_lifecycle::HEAD_FILE)).expect("head");
        std::fs::write(store.join("envelope.replacing"), b"interrupted metadata").expect("debris");
        let (reader, writer) = std::io::pipe().expect("output pipe");
        drop(reader);
        let output = Command::new(env!("CARGO_BIN_EXE_marrow-runner"))
            .arg("recover")
            .arg("--image")
            .arg(&image_path)
            .arg("--store")
            .arg(&store)
            .args(["--format", format])
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(Stdio::piped())
            .output()
            .expect("recovery child completes");
        std::fs::write(base.join("stderr"), &output.stderr).expect("retain child diagnostic");
        assert_eq!(output.status.code(), Some(1), "fixture: {}", base.display());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .starts_with(marrow_codes::Code::IoWrite.as_str())
        );
        assert!(!store.join("envelope.replacing").exists());
        let preserved: Vec<_> = std::fs::read_dir(&store)
            .expect("fixture entries")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| {
                path.file_name()
                    .expect("name")
                    .to_string_lossy()
                    .starts_with("envelope.replacing.preserved.")
            })
            .collect();
        assert_eq!(preserved.len(), 1);
        assert_eq!(
            std::fs::read(&preserved[0]).expect("preserved bytes"),
            b"interrupted metadata"
        );
        assert_eq!(
            std::fs::read(store.join(marrow_lifecycle::HEAD_FILE)).expect("head unchanged"),
            head
        );
        let marrow_lifecycle::AttachOutcome::AlreadyActive(attachment) =
            marrow_lifecycle::attach(&store, marrow_lifecycle::prepare(image))
                .expect("activation survived delivery failure")
        else {
            panic!("no rebind");
        };
        assert_eq!(attachment.envelope().instance, provisioned.instance);
        drop(attachment);
        std::fs::remove_dir_all(base).expect("remove successful fixture");
    }
}

enum ClosedStream {
    Stdout,
    Stderr,
}

#[test]
fn logical_transfer_keeps_completed_effects_when_receipt_delivery_fails() {
    use std::process::{Command, Stdio};
    for restore in [false, true] {
        for close_diagnostic in [false, true] {
            let base = scratch();
            std::fs::create_dir_all(&base).expect("scratch");
            eprintln!("preserved transfer output failure: {}", base.display());
            let bytes = image_bytes();
            let image = marrow_verify::verify(&bytes).expect("image");
            let image_path = base.join("program.image");
            std::fs::write(&image_path, &bytes).expect("image file");
            let source = base.join("source");
            let prepared = marrow_lifecycle::prepare(image);
            let report = ProvisionReport::new(&source, &prepared).expect("report");
            let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
            let provisioned =
                marrow_lifecycle::provision_image(&source, &prepared, &approval).expect("source");
            let head = std::fs::read(source.join(marrow_lifecycle::HEAD_FILE)).expect("head");
            let backup = base.join("backup");
            marrow_lifecycle::backup(&source, &bytes, &backup).expect("complete input");
            let destination = base.join("destination");
            let mut command = Command::new(env!("CARGO_BIN_EXE_marrow-runner"));
            if restore {
                command
                    .arg("restore")
                    .arg("--from")
                    .arg(&backup)
                    .arg("--store")
                    .arg(&destination);
            } else {
                command
                    .arg("backup")
                    .arg("--image")
                    .arg(&image_path)
                    .arg("--store")
                    .arg(&source)
                    .arg("--out")
                    .arg(&destination);
            }
            let (reader, writer) = std::io::pipe().expect("stdout pipe");
            drop(reader);
            command
                .args(["--format", "jsonl"])
                .stdin(Stdio::null())
                .stdout(writer);
            if close_diagnostic {
                let (reader, writer) = std::io::pipe().expect("stderr pipe");
                drop(reader);
                command.stderr(writer);
            } else {
                command.stderr(Stdio::piped());
            }
            let output = command.output().expect("transfer exits");
            assert_eq!(output.status.code(), Some(1));
            if restore {
                assert_eq!(
                    std::fs::read(destination.join(marrow_lifecycle::HEAD_FILE))
                        .expect("published head"),
                    head
                );
                assert!(matches!(
                    marrow_lifecycle::preflight(&destination).expect("artifact presence"),
                    marrow_lifecycle::Preflight::Complete
                ));
            } else {
                assert_eq!(
                    std::fs::read(&destination).expect("published backup"),
                    std::fs::read(&backup).expect("complete backup")
                );
            }
            if !close_diagnostic {
                let diagnostic = String::from_utf8(output.stderr).expect("diagnostic");
                let line = diagnostic
                    .lines()
                    .find(|line| line.starts_with('{'))
                    .expect("retained lifecycle result");
                let receipt = marrow_local_wire::parse_strict(line.as_bytes()).expect("receipt");
                let marrow_local_wire::Json::Object(fields) = receipt else {
                    panic!("receipt object")
                };
                assert!(fields.contains(&(
                    "outcome".into(),
                    marrow_local_wire::Json::Str("complete".into())
                )));
                let (_, marrow_local_wire::Json::Str(instance)) = fields
                    .iter()
                    .find(|(key, _)| key == "instance")
                    .expect("known instance")
                else {
                    panic!("instance string")
                };
                assert_eq!(instance.len(), 32);
                if restore {
                    assert_ne!(instance, &provisioned.instance.to_hex());
                } else {
                    assert_eq!(instance, &provisioned.instance.to_hex());
                }
            }
            // Inspect bytes only after the failed child. Never reopen its native body.
        }
    }
}

#[test]
fn restore_command_refuses_incomplete_input_without_a_usable_destination() {
    use std::process::Command;
    let base = scratch();
    std::fs::create_dir_all(&base).expect("scratch");
    eprintln!("preserved invalid transfer inputs: {}", base.display());
    let bytes = image_bytes();
    let source = base.join("source");
    let prepared = marrow_lifecycle::prepare(marrow_verify::verify(&bytes).expect("image"));
    let report = ProvisionReport::new(&source, &prepared).expect("report");
    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    marrow_lifecycle::provision_image(&source, &prepared, &approval).expect("source");
    let backup = base.join("backup");
    marrow_lifecycle::backup(&source, &bytes, &backup).expect("backup");
    let mut truncated = std::fs::read(&backup).expect("complete backup");
    truncated.pop();
    let mut oversized = b"MWBK\0".to_vec();
    oversized
        .extend_from_slice(&((marrow_image::bounds::MAX_IMAGE_BYTES + 1) as u32).to_be_bytes());
    for (name, input, expected_code, has_stage) in [
        (
            "header",
            b"bad".to_vec(),
            marrow_codes::Code::StoreCorruption,
            false,
        ),
        ("bound", oversized, marrow_codes::Code::StoreLimit, false),
        (
            "truncated",
            truncated,
            marrow_codes::Code::StoreCorruption,
            true,
        ),
    ] {
        let path = base.join(name);
        std::fs::write(&path, input).expect("invalid input");
        let destination = base.join(format!("{name}-store"));
        let output = Command::new(env!("CARGO_BIN_EXE_marrow-runner"))
            .arg("restore")
            .arg("--from")
            .arg(&path)
            .arg("--store")
            .arg(&destination)
            .args(["--format", "jsonl"])
            .output()
            .expect("restore returns");
        assert_eq!(output.status.code(), Some(1));
        assert!(!destination.exists());
        let receipt =
            marrow_local_wire::parse_strict(output.stdout.trim_ascii()).expect("refusal receipt");
        let marrow_local_wire::Json::Object(fields) = receipt else {
            panic!("receipt object")
        };
        assert!(fields.contains(&(
            "code".into(),
            marrow_local_wire::Json::Str(expected_code.as_str().into())
        )));
        let stage = fields.iter().find(|(key, _)| key == "unpublished");
        assert_eq!(stage.is_some(), has_stage);
        if let Some((_, marrow_local_wire::Json::Str(stage))) = stage {
            assert!(
                !std::path::Path::new(stage)
                    .join(marrow_lifecycle::HEAD_FILE)
                    .exists()
            );
        }
    }
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
