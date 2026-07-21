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
    let (schemas, _) = marrow_vm::derive_store_schemas(&image).expect("flat-executable");
    ProvisionReport::new(store, &image, &schemas).token()
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

    let response = service.handle(ClientMessage::Provision {
        store: store.display().to_string(),
        approval: approval_token(&store),
    });

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

    let response = service.handle(ClientMessage::Provision {
        store: store.display().to_string(),
        approval: "0000000000000000".to_string(),
    });

    assert!(
        matches!(response, ServerMessage::Reject { .. }),
        "a mismatched approval is rejected, got {response:?}",
    );
    assert!(!store.exists(), "a rejected provision publishes no store");
    let _ = std::fs::remove_dir_all(&base);
}
