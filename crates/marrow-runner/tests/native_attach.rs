//! The persistent terminal path over a real native store and a real companion process.
//!
//! This is the F02b exit-gate journey: the E06 Workshop image is provisioned to a native
//! store, then driven through add / read / correct / cross-root rollback / re-read entirely
//! over the companion path — each call spawning a fresh `marrow-runner attach` process that
//! opens the store, runs one call against a durable session, commits, and closes. Because
//! every call is its own process, a committed write observed by a later call proves the
//! durable round-trip **across a restart**: the store is closed and reopened between every
//! step. The terminal-side wire client under test is `attach_and_call`; companion discovery
//! and release verification are covered by the terminal's own unit tests.

use std::path::{Path, PathBuf};

use marrow_runner::{CallOutcome, Json, attach_and_call};
use marrow_verify::VerifiedImage;
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
    let image = marrow_verify::verify(&bytes).expect("verify");
    (image, bytes)
}

fn provision(store: &Path, image: &VerifiedImage) {
    let (schemas, sites) = marrow_vm::derive_store_schemas(image).expect("flat-executable");
    let report = marrow_lifecycle::ProvisionReport::new(store, image, &schemas);
    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    marrow_lifecycle::provision_image(store, image, schemas, sites, &approval).expect("provision");
}

fn export_id(image: &VerifiedImage, name: &str) -> [u8; 32] {
    let export = image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .unwrap_or_else(|| panic!("export `{name}` present"));
    *export.id().bytes()
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
        "marrow-native-attach-{}-{nonce}-{counter}/store",
        std::process::id()
    ))
}

/// Drive one call over a freshly spawned companion process attached to the store.
struct Terminal {
    image: VerifiedImage,
    bytes: Vec<u8>,
    store: PathBuf,
    runner: PathBuf,
}

impl Terminal {
    fn call(&self, name: &str, args: Vec<Json>) -> CallOutcome {
        attach_and_call(
            &self.runner,
            &self.image,
            &self.bytes,
            &self.store,
            export_id(&self.image, name),
            args,
        )
        .unwrap_or_else(|error| panic!("companion call `{name}` failed: {}", error.code()))
    }

    fn value(&self, name: &str, args: Vec<Json>) -> Option<Value> {
        match self.call(name, args) {
            CallOutcome::Value(value) => value,
            CallOutcome::Fault { code, .. } => panic!("`{name}` faulted: {code}"),
            CallOutcome::Reject { code } => panic!("`{name}` rejected: {code}"),
        }
    }

    fn fault(&self, name: &str, args: Vec<Json>) -> String {
        match self.call(name, args) {
            CallOutcome::Fault { code, .. } => code,
            CallOutcome::Value(_) => panic!("`{name}` did not fault"),
            CallOutcome::Reject { code } => panic!("`{name}` rejected: {code}"),
        }
    }
}

fn present_name(name: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(name.into())))))
}

/// The full Workshop journey over the companion path, each step a separate process attaching
/// to the persistent store: add commits across both roots and is read back by a later
/// process; a committed move advances the tally; an unguarded move on an absent asset faults
/// and rolls its whole cross-root region back; the final reads show every root at its prior
/// committed value — all surviving the close/reopen between every call.
#[test]
fn workshop_journey_over_the_companion_path() {
    let (image, bytes) = compile_verify();
    let store = scratch();
    std::fs::create_dir_all(store.parent().expect("parent")).expect("scratch dir");
    provision(&store, &image);
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");
    let terminal = Terminal {
        image,
        bytes,
        store,
        runner: runner_exe(),
    };

    // add commits an asset across ^assets and ^tallies; a *separate* companion process reads
    // it back — the store was closed and reopened in between.
    assert_eq!(
        terminal.value(
            "add",
            vec![
                Json::Int(1),
                Json::Str("T-100".into()),
                Json::Str("Cordless Drill".into()),
                Json::Str("power".into()),
                Json::Str(epoch.clone()),
            ],
        ),
        Some(Value::Bool(true)),
    );
    assert_eq!(
        terminal.value("assetName", vec![Json::Int(1)]),
        present_name("Cordless Drill"),
    );
    assert_eq!(terminal.value("catalogued", vec![]), Some(Value::Int(1)));

    // A committed cross-root move, then read back from another process.
    terminal.value("recordMove", vec![Json::Int(1), Json::Str("Bay 3".into())]);
    assert_eq!(
        terminal.value("location", vec![Json::Int(1)]),
        present_name("Bay 3"),
    );
    assert_eq!(terminal.value("moveCount", vec![]), Some(Value::Int(1)));

    // Cross-root rollback: a move on an absent asset faults required-missing and rolls the
    // whole staged region back across both roots.
    assert_eq!(
        terminal.fault("recordMove", vec![Json::Int(2), Json::Str("Bay 9".into())],),
        "run.required_missing",
    );

    // Every root stands at its prior committed value after the rolled-back fault — proven by
    // fresh processes reopening the store.
    assert_eq!(
        terminal.value("assetName", vec![Json::Int(1)]),
        present_name("Cordless Drill"),
    );
    assert_eq!(
        terminal.value("location", vec![Json::Int(1)]),
        present_name("Bay 3"),
    );
    assert_eq!(
        terminal.value("present", vec![Json::Int(2)]),
        Some(Value::Bool(false)),
    );
    assert_eq!(terminal.value("catalogued", vec![]), Some(Value::Int(1)));
    assert_eq!(terminal.value("moveCount", vec![]), Some(Value::Int(1)));

    let _ = std::fs::remove_dir_all(terminal.store.parent().expect("parent"));
}

/// A committed add is durable across a restart with its `log` descendant: a later process
/// reads back both the asset name and its first note entry.
#[test]
fn a_committed_add_is_durable_with_its_log_descendant() {
    let (image, bytes) = compile_verify();
    let store = scratch();
    std::fs::create_dir_all(store.parent().expect("parent")).expect("scratch dir");
    provision(&store, &image);
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");
    let terminal = Terminal {
        image,
        bytes,
        store,
        runner: runner_exe(),
    };

    terminal.value(
        "add",
        vec![
            Json::Int(7),
            Json::Str("T-700".into()),
            Json::Str("Sander".into()),
            Json::Str("power".into()),
            Json::Str(epoch),
        ],
    );
    assert_eq!(
        terminal.value("assetName", vec![Json::Int(7)]),
        present_name("Sander"),
    );
    assert_eq!(
        terminal.value("noteText", vec![Json::Int(7), Json::Int(1)]),
        present_name("catalogued"),
    );

    let _ = std::fs::remove_dir_all(terminal.store.parent().expect("parent"));
}
