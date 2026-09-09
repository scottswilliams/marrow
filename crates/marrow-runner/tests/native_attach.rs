//! The persistent terminal path over a real native store and a real companion process.
//!
//! This is the F02b exit-gate journey: the E06 Workshop image is provisioned to a native
//! store, then driven through add / read / move / cross-root rollback / re-read entirely
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
    let image = marrow_verify::verify(&bytes).expect("verify");
    (image, bytes)
}

fn provision(store: &Path, image: &VerifiedImage) {
    let prepared = marrow_lifecycle::prepare(image.clone());
    let report = marrow_lifecycle::ProvisionReport::new(store, &prepared).expect("flat-executable");
    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    marrow_lifecycle::provision_image(store, &prepared, &approval).expect("provision");
}

fn export_id(image: &VerifiedImage, name: &str) -> [u8; 32] {
    let export = image
        .exports()
        .iter()
        .find(|export| {
            image
                .function(export.function())
                .expect("verified function")
                .body()
                .name()
                == name
        })
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
            CallOutcome::Incomplete { code, durable, .. } => {
                panic!("`{name}` was incomplete: {code} ({durable:?})")
            }
            CallOutcome::Reject { code } => panic!("`{name}` rejected: {code}"),
            CallOutcome::OutcomeUnknown { .. } => panic!("`{name}` outcome unknown"),
        }
    }

    fn fault(&self, name: &str, args: Vec<Json>) -> String {
        match self.call(name, args) {
            CallOutcome::Fault { code, .. } => code,
            CallOutcome::Value(_) => panic!("`{name}` completed"),
            CallOutcome::Incomplete { code, durable, .. } => {
                panic!("`{name}` was incomplete: {code} ({durable:?})")
            }
            CallOutcome::Reject { code } => panic!("`{name}` rejected: {code}"),
            CallOutcome::OutcomeUnknown { .. } => panic!("`{name}` outcome unknown"),
        }
    }
}

fn present_name(name: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(name.into())))))
}

/// The full Workshop journey over the companion path, each step a separate process attaching
/// to the persistent store: add commits across both roots and is read back by a later
/// process; a committed move advances the tally; an add whose tag collides in the unique
/// index faults and rolls its whole cross-root region back; the final reads show every root
/// at its prior committed value — all surviving the close/reopen between every call.
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

    // The catalogued tally is staged before the duplicate asset tag faults at its write.
    // Fresh processes must observe rollback of that earlier mutation of the other root.
    assert_eq!(
        terminal.fault(
            "add",
            vec![
                Json::Int(2),
                Json::Str("T-100".into()),
                Json::Str("Impostor".into()),
                Json::Str("power".into()),
                Json::Str(epoch),
            ],
        ),
        "run.unique_index",
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod image_input {
    use super::{compile_verify, runner_exe, scratch};
    use std::path::PathBuf;
    use std::process::{Child, Command, Output, Stdio};
    use std::time::{Duration, Instant};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let store = scratch();
            let directory = store.parent().expect("scratch parent").to_path_buf();
            std::fs::create_dir_all(&directory).expect("create scratch directory");
            Self(directory)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    struct ChildGuard(Option<Child>);

    impl ChildGuard {
        fn spawn(command: &mut Command) -> Self {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            Self(Some(command.spawn().expect("spawn guarded child")))
        }

        fn running(&mut self) -> bool {
            self.0
                .as_mut()
                .expect("child remains owned")
                .try_wait()
                .expect("poll guarded child")
                .is_none()
        }

        fn finish(mut self) -> Output {
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.running() {
                assert!(
                    Instant::now() < deadline,
                    "image reader did not exit before its deadline"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            self.0
                .take()
                .expect("child remains owned")
                .wait_with_output()
                .expect("collect guarded child")
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(child) = self.0.as_mut() {
                if !matches!(child.try_wait(), Ok(Some(_))) {
                    child.kill().ok();
                }
                child.wait().ok();
            }
        }
    }

    #[test]
    fn image_ingress_refuses_a_bounded_stream_without_waiting_for_eof() {
        // The directory outlives both guarded children, including panic cleanup.
        let scratch = Scratch::new();
        let fifo = scratch.0.join("image.fifo");
        let made = Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .output()
            .expect("create image FIFO");
        assert!(made.status.success(), "mkfifo failed: {made:?}");

        // Only this shell owns the FIFO writer. Even after an early refusal/EPIPE,
        // it retains that descriptor while parent-owned stdin remains open.
        let mut writer_command = Command::new("/bin/sh");
        writer_command
            .args([
                "-c",
                r#"trap '' PIPE
exec 3>"$1" || exit 1
chunk=0000000000000000
for double in 1 2 3 4; do chunk=$chunk$chunk; done
remaining=$2
while [ "$remaining" -gt 0 ]; do
    if [ "$remaining" -ge 256 ]; then
        printf '%s' "$chunk" >&3 || break
        remaining=$((remaining - 256))
    else
        printf '0' >&3 || break
        remaining=$((remaining - 1))
    fi
done
IFS= read -r hold
"#,
                "image-writer",
            ])
            .arg(&fifo)
            .arg((marrow_image::bounds::MAX_IMAGE_BYTES + 1).to_string())
            .stdin(Stdio::piped());
        let mut writer = ChildGuard::spawn(&mut writer_command);
        let mut reader_command = Command::new(runner_exe());
        reader_command.arg("--image").arg(&fifo);
        let output = ChildGuard::spawn(&mut reader_command).finish();
        assert!(
            writer.running(),
            "the writer released EOF before the reader result"
        );
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 runner diagnostic");
        assert_eq!(stderr.trim(), marrow_codes::Code::ImageEnvelope.as_str());
    }

    #[test]
    fn a_small_valid_image_loads_for_provision_preview() {
        let scratch = Scratch::new();
        let (image, bytes) = compile_verify();
        assert!(bytes.len() < marrow_image::bounds::MAX_IMAGE_BYTES);
        let path = scratch.0.join("image.mwi");
        let store = scratch.0.join("store");
        std::fs::write(&path, bytes).expect("write valid image");
        let prepared = marrow_lifecycle::prepare(image);
        let report = marrow_lifecycle::ProvisionReport::new(&store, &prepared)
            .expect("valid provision report")
            .render();
        let mut command = Command::new(runner_exe());
        command
            .arg("provision")
            .arg("--image")
            .arg(&path)
            .arg("--store")
            .arg(&store);
        let output = ChildGuard::spawn(&mut command).finish();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 provision report");
        assert!(stderr.starts_with(&report), "{stderr}");
        assert!(
            stderr.ends_with("Re-run with --yes to accept this report and provision the store.\n"),
            "{stderr}"
        );
        assert!(!store.exists());
    }
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

fn describe(outcome: &CallOutcome) -> String {
    match outcome {
        CallOutcome::Value(value) => format!("value {value:?}"),
        CallOutcome::Fault { code, .. } => format!("fault {code}"),
        CallOutcome::Incomplete { code, durable, .. } => {
            format!("incomplete {code} ({durable:?})")
        }
        CallOutcome::Reject { code } => format!("reject {code}"),
        CallOutcome::OutcomeUnknown { .. } => "outcome unknown".to_string(),
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
