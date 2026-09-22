//! The persistent terminal path over a real native store and a real companion process.
//!
//! The Workshop image is provisioned to a native
//! store, then driven through add / read / move / cross-root rollback / re-read entirely
//! over the companion path — each call spawning a fresh `marrow-runner attach` process that
//! opens the store, runs one call against a durable session, commits, and closes. Because
//! every call is its own process, a committed write observed by a later call proves the
//! durable round-trip **across a restart**: the store is closed and reopened between every
//! step. The terminal-side wire client under test is `attach_and_call`; companion discovery
//! and release verification are covered by the terminal's own unit tests.

use marrow_test_programs::program;
use marrow_test_support::Scratch;

use std::path::{Path, PathBuf};

use marrow_image::ImageType;
use marrow_runner::{CallOutcome, Json, attach_and_call};
use marrow_test_support::broken_output;
use marrow_verify::VerifiedImage;
use marrow_vm::Value;

fn runner_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_marrow-runner"))
}

/// The exact reply and the cleanup verdict are independent: a direct child that outlives the
/// terminal's unsignalled settlement budget (the grace period plus the per-call deadline) is
/// handed back unreaped with its stage retained, and the caller reaps it explicitly. The
/// wrapper therefore lingers past that budget, which bounds this test at about twelve seconds.
#[test]
#[ignore = "spawns a native runner inside a controlled lingering direct child; about twelve seconds"]
fn exact_reply_survives_unconfirmed_native_cleanup_and_explicit_reap() {
    use std::os::unix::fs::PermissionsExt;
    let program::Program { image, bytes } = program::workshop();
    let scratch = Scratch::new("native-attach");
    let store = scratch.store();
    let root = scratch.path();
    provision(store, &image);
    let wrapper = root.join("lingering-runner");
    let marker = root.join("entered-linger");
    let quote = |path: &Path| {
        format!(
            "'{}'",
            path.to_str()
                .expect("fixture UTF-8 path")
                .replace('\'', "'\\''")
        )
    };
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n{} \"$@\"\nprintf waiting > {}\nexec /bin/sleep 12\n",
            quote(&runner_exe()),
            quote(&marker),
        ),
    )
    .expect("controlled wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let started = std::time::Instant::now();
    let completion = attach_and_call(
        &wrapper,
        &image,
        &bytes,
        store,
        program::export_id(&image, "catalogued"),
        vec![],
    );
    let elapsed = started.elapsed();
    eprintln!(
        "native lingering fixture: {}; settled in {elapsed:?}",
        root.display()
    );
    assert!(matches!(
        completion.outcome,
        Ok(CallOutcome::Value(Some(Value::Int(0))))
    ));
    assert_eq!(
        std::fs::read(&marker).expect("stock runner exited before linger"),
        b"waiting"
    );
    let Err(marrow_runner::CompanionCleanupError::Unreaped {
        mut child, staging, ..
    }) = completion.cleanup
    else {
        panic!("native linger must report unconfirmed cleanup");
    };
    // The unreaped child stays owned and unsignalled, so the caller can still reap it.
    assert!(child.wait().expect("explicit reap").success());
    assert!(staging.exists());
    std::fs::remove_dir_all(staging).expect("remove stage after explicit reap");
    std::fs::remove_dir_all(root).expect("remove owned successful fixture");
}

#[test]
#[ignore = "spawns executable child controls"]
fn startup_loss_distinguishes_native_spawn_from_spawn_failure() {
    let program::Program { image, bytes } = program::workshop();
    let scratch = Scratch::new("native-attach");
    let store = scratch.store();
    let export = program::export_id(&image, "assetName");
    let failed_spawn = attach_and_call(
        &store.join("absent-runner"),
        &image,
        &bytes,
        store,
        export,
        Vec::new(),
    );
    failed_spawn.cleanup.expect("no spawned child");
    assert!(matches!(
        failed_spawn.outcome,
        Err(marrow_runner::ClientError::Spawn(_))
    ));
    let exited = attach_and_call(
        &std::env::current_exe().expect("test executable rejects runner arguments"),
        &image,
        &bytes,
        store,
        export,
        Vec::new(),
    );
    exited.cleanup.expect("exited child reaped");
    assert!(matches!(exited.outcome,
        Err(marrow_runner::ClientError::ActivationOutcomeUnknown { cause })
            if matches!(*cause, marrow_runner::ClientError::Descriptor)
    ));
    assert!(!store.exists(), "the fixture child never opened a store");
}

#[test]
#[ignore = "spawns native attach and binds a Unix socket"]
fn closed_launch_descriptor_does_not_undo_a_completed_rebind() {
    use std::process::{Command, Stdio};
    let old = program::workshop().image;
    let program::Program { image: new, bytes } =
        program::workshop_with("\nfn version(): int { return 2 }\n");
    assert_ne!(old.image_id(), new.image_id());
    for capture_diagnostic in [true, false] {
        let scratch = Scratch::new("native-attach");
        let store = scratch.store();
        let base = scratch.path();
        provision(store, &old);
        let before = marrow_lifecycle::audit(store, marrow_lifecycle::prepare(old.clone()))
            .expect("old active store");
        let image = base.join("program.image");
        std::fs::write(&image, &bytes).expect("image");
        let writer = broken_output();
        let diagnostic = if capture_diagnostic {
            Stdio::piped()
        } else {
            broken_output()
        };
        let output = Command::new(runner_exe())
            .args(["attach", "--image"])
            .arg(&image)
            .arg("--store")
            .arg(store)
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(diagnostic)
            .output()
            .expect("attach child");
        eprintln!("fixture: {}; child: {output:?}", base.display());
        std::fs::write(base.join("stderr"), &output.stderr).expect("retain diagnostic");
        assert_eq!(output.status.code(), Some(1), "{}", base.display());
        if capture_diagnostic {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains(marrow_codes::Code::IoWrite.as_str())
            );
        }
        let after = marrow_lifecycle::audit(store, marrow_lifecycle::prepare(new.clone()))
            .expect("new image is already active without a second attach");
        assert_eq!(after.instance, before.instance);
        assert_eq!(after.image_id, new.image_id());
        assert!(after.is_clean());
        std::fs::remove_dir_all(base).expect("remove successful fixture");
    }
}

fn provision(store: &Path, image: &VerifiedImage) {
    let prepared = marrow_lifecycle::prepare(image.clone());
    let report = marrow_lifecycle::ProvisionReport::new(store, &prepared).expect("flat-executable");
    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    marrow_lifecycle::provision_image(store, &prepared, &approval).expect("provision");
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
        let completion = attach_and_call(
            &self.runner,
            &self.image,
            &self.bytes,
            &self.store,
            program::export_id(&self.image, name),
            args,
        );
        completion.cleanup.expect("companion settled");
        completion.outcome.unwrap_or_else(|error| {
            panic!("companion call `{name}` failed: {}", error.code().as_str())
        })
    }

    fn value(&self, name: &str, args: Vec<Json>) -> Option<Value> {
        match self.call(name, args) {
            CallOutcome::Value(value) => value,
            CallOutcome::Fault { code, .. } => panic!("`{name}` faulted: {}", code.as_str()),
            CallOutcome::Incomplete { code, durable, .. } => {
                panic!("`{name}` was incomplete: {} ({durable:?})", code.as_str())
            }
            CallOutcome::Reject { code } => panic!("`{name}` rejected: {}", code.as_str()),
            CallOutcome::OutcomeUnknown { .. } => panic!("`{name}` outcome unknown"),
        }
    }

    fn fault(&self, name: &str, args: Vec<Json>) -> &'static str {
        match self.call(name, args) {
            CallOutcome::Fault { code, .. } => code.as_str(),
            CallOutcome::Value(_) => panic!("`{name}` completed"),
            CallOutcome::Incomplete { code, durable, .. } => {
                panic!("`{name}` was incomplete: {} ({durable:?})", code.as_str())
            }
            CallOutcome::Reject { code } => panic!("`{name}` rejected: {}", code.as_str()),
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
    let program::Program { image, bytes } = program::workshop_with(
        r#"
pub fn setMovesExplicit(v: int): Result<int, string> {
    transaction {
        ^tallies["moves"] = Tally(count: v)
        if v > 100 {
            return err("value is large")
        }
    }
    return ok(v)
}

pub fn setMovesRequired(v: int): Result<int, string> {
    transaction {
        ^tallies["moves"] = Tally(count: v)
        require v <= 100 else "value is large"
    }
    return ok(v)
}
"#,
    );
    let scratch = Scratch::new("native-attach");
    let store = scratch.store().to_path_buf();
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

    // Each returned Result is followed by a fresh companion reading a changed tally.
    for name in ["setMovesExplicit", "setMovesRequired"] {
        let id = program::export_id(&terminal.image, name);
        let export = terminal
            .image
            .exports()
            .iter()
            .find(|export| export.id().bytes() == &id)
            .expect("verified export");
        let ImageType::Enum {
            idx,
            optional: false,
        } = terminal
            .image
            .function(export.function())
            .expect("verified function")
            .body()
            .ret()
        else {
            panic!("{name} returns a non-optional Result");
        };
        let idx = idx.wire_index();
        let variants = terminal.image.enums()[usize::from(idx)].variants();
        for (value, member, payload) in [
            (200, "err", Value::Text("value is large".into())),
            (50, "ok", Value::Int(50)),
        ] {
            let variant = u16::try_from(
                variants
                    .iter()
                    .position(|candidate| candidate.name().as_ref() == member)
                    .expect("verified Result member"),
            )
            .expect("verified member index fits u16");
            assert_eq!(
                terminal.value(name, vec![Json::Int(value)]),
                Some(Value::Enum(idx, variant, vec![payload].into_boxed_slice())),
            );
            assert_eq!(terminal.value("moveCount", vec![]), Some(Value::Int(value)),);
        }
    }

    let _ = std::fs::remove_dir_all(terminal.store.parent().expect("parent"));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod image_input {
    use super::{Scratch, program, runner_exe};
    use std::process::{Child, Command, Output, Stdio};
    use std::time::{Duration, Instant};

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
        let scratch = Scratch::new("native-attach-image-input");
        let fifo = scratch.path().join("image.fifo");
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
        let scratch = Scratch::new("native-attach-image-input");
        let program::Program { image, bytes } = program::workshop();
        assert!(bytes.len() < marrow_image::bounds::MAX_IMAGE_BYTES);
        let path = scratch.path().join("image.mwi");
        let store = scratch.path().join("store");
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
    let program::Program {
        image: image_a,
        bytes: bytes_a,
    } = program::workshop();
    let program::Program {
        image: image_b,
        bytes: bytes_b,
    } = program::workshop_with("\nfn _editProbe(): int {\n    return 0\n}\n");
    // Same durable contract / interface / ceiling, different code.
    assert_ne!(
        image_a.image_id().0,
        image_b.image_id().0,
        "the body edit must change the image identity",
    );

    let scratch = Scratch::new("native-attach");
    let store = scratch.store().to_path_buf();
    provision(&store, &image_a);
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");
    let runner = runner_exe();

    // Commit an asset under image A.
    let added = attach_and_call(
        &runner,
        &image_a,
        &bytes_a,
        &store,
        program::export_id(&image_a, "add"),
        vec![
            Json::Int(3),
            Json::Str("T-300".into()),
            Json::Str("Impact Driver".into()),
            Json::Str("power".into()),
            Json::Str(epoch),
        ],
    );
    added.cleanup.expect("image A companion settled");
    match added.outcome.expect("add under image A") {
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
        program::export_id(&image_b, "assetName"),
        vec![Json::Int(3)],
    );
    read.cleanup.expect("image B companion settled");
    match read.outcome.expect("assetName under image B") {
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
        CallOutcome::Fault { code, .. } => format!("fault {}", code.as_str()),
        CallOutcome::Incomplete { code, durable, .. } => {
            format!("incomplete {} ({durable:?})", code.as_str())
        }
        CallOutcome::Reject { code } => format!("reject {}", code.as_str()),
        CallOutcome::OutcomeUnknown { .. } => "outcome unknown".to_string(),
    }
}

/// A committed add is durable across a restart with its `log` descendant: a later process
/// reads back both the asset name and its first note entry.
#[test]
fn a_committed_add_is_durable_with_its_log_descendant() {
    let program::Program { image, bytes } = program::workshop();
    let scratch = Scratch::new("native-attach");
    let store = scratch.store().to_path_buf();
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
