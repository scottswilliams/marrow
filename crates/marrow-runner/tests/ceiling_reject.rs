//! The G03 term-3 (D08) refusal surfaced end-to-end through the production native path.
//!
//! A store is provisioned under a read-only image, recording its demand union as the accepted
//! deployment ceiling. A later, broadened image (the same read-only export edited to also
//! mutate) is then run against that store over the real `marrow-runner attach` process. The
//! lifecycle actor refuses it before opening the store, and the runner serves that refusal as a
//! typed wire reject: the terminal receives `CallOutcome::Reject { code:
//! "store.demand_exceeds_ceiling" }`, the store head is byte-unchanged (zero engine calls), and
//! the prior program still runs. This is the client-visible half of the effect-ceiling MUST-WIN.
//!
//! Spawns a runner that binds a Unix socket, which the sandbox denies; run with the sandbox
//! disabled (the workspace battery already runs that way).

use std::path::{Path, PathBuf};

use marrow_runner::{CallOutcome, Json, attach_and_call};
use marrow_verify::{VerifiedImage, verify};

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

const SHAPE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter
"#;

fn source_read_only() -> String {
    format!("{SHAPE}\npub fn readValue(n: int): int {{\n    return ^counters[n].value ?? 0\n}}\n")
}

fn source_broadened() -> String {
    format!(
        "{SHAPE}\npub fn readValue(n: int): int {{\n    var result = 0\n    \
         transaction {{\n        place slot = ^counters[n]\n        \
         if exists(slot) {{\n            slot.label = \"seen\"\n        }}\n        \
         result = ^counters[n].value ?? 0\n    }}\n    return result\n}}\n"
    )
}

fn compile(source: &str) -> (VerifiedImage, Vec<u8>) {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    (
        verify(&compiled.image.bytes).expect("verify"),
        compiled.image.bytes,
    )
}

fn provision(store: &Path, image: &VerifiedImage) {
    let (schemas, sites) = marrow_vm::derive_store_schemas(image).expect("flat-executable");
    let report = marrow_lifecycle::ProvisionReport::new(store, image, &schemas);
    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    marrow_lifecycle::provision_image(store, image, schemas, sites, &approval).expect("provision");
}

fn export_id(image: &VerifiedImage, name: &str) -> [u8; 32] {
    *image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .unwrap_or_else(|| panic!("export `{name}` present"))
        .id()
        .bytes()
}

fn runner_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_marrow-runner"))
}

fn scratch() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "marrow-g03-reject-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&base).expect("scratch base");
    base.join("store")
}

#[test]
fn a_broadened_image_is_rejected_end_to_end_through_the_native_path() {
    let (read_only, _) = compile(&source_read_only());
    let (broadened, broadened_bytes) = compile(&source_broadened());
    let store = scratch();
    provision(&store, &read_only);

    let head_before = std::fs::read(store.join("head")).expect("head");

    // Run the broadened image against the store provisioned under the read-only ceiling. The
    // runner refuses it before opening the store and serves a typed reject.
    let outcome = attach_and_call(
        &runner_exe(),
        &broadened,
        &broadened_bytes,
        &store,
        export_id(&broadened, "readValue"),
        vec![Json::Int(1)],
    )
    .expect("the runner serves a typed reject, not a spawn failure");

    match outcome {
        CallOutcome::Reject { code } => assert_eq!(
            code, "store.demand_exceeds_ceiling",
            "the broadened image is rejected as demand-exceeds-ceiling over the wire",
        ),
        CallOutcome::Value(_) => panic!("the broadened image must be rejected, not run"),
        CallOutcome::Fault { code, .. } => panic!("expected a reject, got fault {code}"),
        CallOutcome::Incomplete { code, durable, .. } => {
            panic!("expected a reject, got incomplete {code} ({durable:?})")
        }
        CallOutcome::OutcomeUnknown { .. } => panic!("expected a reject, got outcome-unknown"),
    }

    // Zero engine calls: the head is byte-unchanged, and the prior read-only program still runs.
    assert_eq!(
        head_before,
        std::fs::read(store.join("head")).expect("head"),
        "the reject wrote nothing to the store",
    );
    let prior = attach_and_call(
        &runner_exe(),
        &read_only,
        &compile(&source_read_only()).1,
        &store,
        export_id(&read_only, "readValue"),
        vec![Json::Int(1)],
    )
    .expect("the prior program still attaches");
    match prior {
        CallOutcome::Value(Some(marrow_vm::Value::Int(0))) => {}
        CallOutcome::Value(other) => {
            panic!("the prior program returned an unexpected value: {other:?}")
        }
        CallOutcome::Reject { code } => panic!("the prior program was rejected: {code}"),
        CallOutcome::Fault { code, .. } => panic!("the prior program faulted: {code}"),
        CallOutcome::Incomplete { code, durable, .. } => {
            panic!("the prior program was incomplete: {code} ({durable:?})")
        }
        CallOutcome::OutcomeUnknown { .. } => panic!("the prior program outcome was unknown"),
    }

    let _ = std::fs::remove_dir_all(store.parent().expect("parent"));
}
