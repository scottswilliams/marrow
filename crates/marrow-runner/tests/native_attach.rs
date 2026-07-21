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
    compile_verify_with("")
}

/// Compile the Workshop image, optionally appending `extra` source (used to produce a
/// body-only-edited image with the same durable contract, interface, and ceiling).
fn compile_verify_with(extra: &str) -> (VerifiedImage, Vec<u8>) {
    let mut source = std::fs::read(fixture_dir().join("src/main.mw")).expect("read fixture source");
    source.extend_from_slice(extra.as_bytes());
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

/// The persistent edit-to-run loop over the companion path: a committed write under one
/// image is read back after a body-only edit. The edited image (a fresh private helper — same
/// durable contract, interface, and ceiling, different bytes) is picked up on the next run
/// with no extra action: the companion binds-only-rebinds the active image and the prior
/// committed durable data stands intact.
#[test]
fn a_body_edit_rebinds_and_preserves_committed_data() {
    let (image_a, bytes_a) = compile_verify();
    let (image_b, bytes_b) = compile_verify_with("\nfn _f02bEditProbe(): int {\n    return 0\n}\n");
    // Same durable contract / interface / ceiling, different code.
    assert_ne!(
        image_a.image_id().0,
        image_b.image_id().0,
        "the body edit must change the image identity",
    );

    let store = scratch();
    std::fs::create_dir_all(store.parent().expect("parent")).expect("scratch dir");
    provision(&store, &image_a);
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");
    let runner = runner_exe();

    // Commit an asset under image A.
    match attach_and_call(
        &runner,
        &image_a,
        &bytes_a,
        &store,
        export_id(&image_a, "add"),
        vec![
            Json::Int(3),
            Json::Str("T-300".into()),
            Json::Str("Impact Driver".into()),
            Json::Str("power".into()),
            Json::Str(epoch),
        ],
    )
    .expect("add under image A")
    {
        CallOutcome::Value(Some(Value::Bool(true))) => {}
        other => panic!("add did not return true: {}", describe(&other)),
    }

    // Read it back under the body-edited image B: the companion rebinds to B and the data
    // committed under A is intact.
    let read = attach_and_call(
        &runner,
        &image_b,
        &bytes_b,
        &store,
        export_id(&image_b, "assetName"),
        vec![Json::Int(3)],
    )
    .expect("assetName under image B");
    match read {
        CallOutcome::Value(value) => assert_eq!(value, present_name("Impact Driver")),
        other => panic!(
            "assetName under B did not return the committed name: {}",
            describe(&other)
        ),
    }

    let _ = std::fs::remove_dir_all(store.parent().expect("parent"));
}

/// The minimal backup/restore slice round-trips one populated store through the native path:
/// a store populated over the companion path is backed up to a disposable slice, restored
/// into a fresh store directory, and the restored store returns the same committed data over
/// the companion path. The disposable slice makes no digest claim — the restored head's
/// reserved data-digest slots stay zero (an unsequenced store, FR01 §2).
#[test]
fn a_populated_store_round_trips_through_the_backup_slice() {
    let (image, bytes) = compile_verify();
    let (schemas, sites) = marrow_vm::derive_store_schemas(&image).expect("flat-executable");
    let source = scratch();
    std::fs::create_dir_all(source.parent().expect("parent")).expect("scratch dir");
    provision(&source, &image);
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");
    let runner = runner_exe();

    // Populate the source store over the companion path.
    attach_and_call(
        &runner,
        &image,
        &bytes,
        &source,
        export_id(&image, "add"),
        vec![
            Json::Int(9),
            Json::Str("T-900".into()),
            Json::Str("Angle Grinder".into()),
            Json::Str("power".into()),
            Json::Str(epoch),
        ],
    )
    .expect("populate source");

    // Back the source up to a disposable slice, then release its lock.
    let mut slice: Vec<u8> = Vec::new();
    {
        let opened = marrow_lifecycle::open(&source, schemas.clone(), sites.clone()).expect("open");
        marrow_lifecycle::backup_slice(&opened, &mut slice).expect("backup slice");
    }

    // Restore into a fresh store directory.
    let dest = scratch();
    std::fs::create_dir_all(dest.parent().expect("parent")).expect("dest scratch");
    marrow_lifecycle::restore_slice(&mut slice.as_slice(), &dest, schemas.clone(), sites.clone())
        .expect("restore slice");

    // The restored store returns the committed data over the companion path.
    let restored = attach_and_call(
        &runner,
        &image,
        &bytes,
        &dest,
        export_id(&image, "assetName"),
        vec![Json::Int(9)],
    )
    .expect("read restored");
    match restored {
        CallOutcome::Value(value) => assert_eq!(value, present_name("Angle Grinder")),
        other => panic!(
            "restored read did not return the committed name: {}",
            describe(&other)
        ),
    }

    // The disposable slice carries no digest claim: the restored head's reserved data-digest
    // slots are zero (an unsequenced store).
    let head = marrow_lifecycle::open(&dest, schemas, sites)
        .expect("open restored")
        .head;
    assert_eq!(
        head.data_digest, [0u8; 32],
        "the slice makes no digest claim"
    );
    assert_eq!(head.data_digest_position, 0, "the store stays unsequenced");

    let _ = std::fs::remove_dir_all(source.parent().expect("parent"));
    let _ = std::fs::remove_dir_all(dest.parent().expect("parent"));
}

fn describe(outcome: &CallOutcome) -> String {
    match outcome {
        CallOutcome::Value(value) => format!("value {value:?}"),
        CallOutcome::Fault { code, .. } => format!("fault {code}"),
        CallOutcome::Reject { code } => format!("reject {code}"),
    }
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
