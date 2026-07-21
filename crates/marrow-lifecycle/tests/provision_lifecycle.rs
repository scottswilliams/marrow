//! Kill-point crash fixtures for the persistent provision/open flow.
//!
//! Provision publishes complete-or-not-at-all and one winner claims a destination; open
//! takes the single-owner lock (naming the live owner on contention) and audits an unclean
//! prior shutdown. These fixtures drive those invariants over real temporary directories,
//! including a spawned child that holds a store while the parent is refused by pid.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use marrow_image::LedgerIdBytes;
use marrow_kernel::codec::value::ScalarKind;
use marrow_kernel::durable::{FieldSchema, SiteSpec, SiteTarget, StoreSchema};
use marrow_lifecycle::{
    ActiveBinding, EngineKind, HeadMap, LogicalHead, OpenError, Preflight, ProvisionError,
    ProvisionRequest, StoreEnvelope, StoreInstanceId, open, preflight, provision,
};

/// A unique temporary directory removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "marrow-lifecycle-{tag}-{}-{nonce}-{counter}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&path).expect("create temp base");
        Self { path }
    }

    fn store(&self) -> PathBuf {
        self.path.join("store")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn schemas() -> Vec<StoreSchema> {
    vec![StoreSchema {
        root_name: "app".into(),
        key: vec![ScalarKind::Int],
        fields: vec![FieldSchema::scalar("value", ScalarKind::Int, true)],
        groups: Vec::new(),
        branches: Vec::new(),
        indexes: Vec::new(),
    }]
}

fn sites() -> Vec<SiteSpec> {
    vec![SiteSpec {
        root: 0,
        target: SiteTarget::WholePayload,
    }]
}

fn request(instance: StoreInstanceId) -> ProvisionRequest {
    let envelope = StoreEnvelope {
        instance,
        writer_toolchain: "0.1.0".into(),
        engine_kind: EngineKind::Redb,
        engine_format_version: 1,
    };
    let head_map = HeadMap::assign(&[LedgerIdBytes::from_bytes([1; 16])]).expect("head map");
    let binding = ActiveBinding {
        image_format_version: 0,
        image_id: [0x11; 32],
        durable_contract: [0x22; 32],
        interface: [0x33; 32],
        ceiling: [0x44; 32],
    };
    ProvisionRequest {
        envelope,
        head: LogicalHead::provision(binding, head_map),
        schemas: schemas(),
        sites: sites(),
    }
}

/// A fresh instance for a test.
fn instance() -> StoreInstanceId {
    StoreInstanceId::draw().expect("entropy")
}

/// Kill-point: AFTER the rename the store is complete and reopens. Provision publishes a
/// complete store; preflight sees Complete; open succeeds and carries the published instance.
#[test]
fn provision_publishes_complete_and_open_reopens() {
    let dir = TempDir::new("publish");
    let store = dir.store();
    let id = instance();

    assert_eq!(preflight(&store), Preflight::Absent);
    let provisioned = provision(&store, request(id)).expect("provision");
    assert_eq!(provisioned.instance, id);
    assert_eq!(preflight(&store), Preflight::Complete);

    let opened = open(&store, schemas(), sites()).expect("open");
    assert_eq!(
        opened.envelope.instance, id,
        "the reopened store carries its instance"
    );
    assert_eq!(opened.head.binding.durable_contract, [0x22; 32]);
    drop(opened); // releases the lock (clean shutdown truncates the lock body)

    // A clean reopen is not an unclean shutdown: it still opens.
    let reopened = open(&store, schemas(), sites()).expect("reopen after clean close");
    assert_eq!(reopened.envelope.instance, id);
}

/// Kill-point: BEFORE the rename only a temporary directory exists and the destination is
/// absent — a crash mid-build never publishes a partial store. A leftover provisioning temp
/// beside an absent destination leaves preflight Absent, and a fresh provision still wins.
#[test]
fn a_pre_rename_crash_state_keeps_the_destination_absent() {
    let dir = TempDir::new("pre-rename");
    let store = dir.store();

    // Model the pre-rename crash state: a leftover temp-shaped sibling, destination absent.
    let leftover = dir
        .path
        .join(format!(".store.provisioning.{}.999", std::process::id()));
    std::fs::create_dir_all(leftover.join("junk")).expect("leftover temp");

    assert_eq!(
        preflight(&store),
        Preflight::Absent,
        "a pre-rename crash never publishes the destination",
    );

    // A fresh provision still succeeds and publishes a complete store.
    let id = instance();
    assert_eq!(
        provision(&store, request(id)).expect("provision").instance,
        id
    );
    assert_eq!(preflight(&store), Preflight::Complete);
    assert!(
        leftover.exists(),
        "the leftover temp is ignored, not required"
    );
}

/// A failed/absent preflight creates no file: probing an absent or incomplete store never
/// mutates the filesystem.
#[test]
fn a_failed_preflight_creates_no_file() {
    let dir = TempDir::new("no-file");
    let store = dir.store();

    let before = list(&dir.path);
    assert_eq!(preflight(&store), Preflight::Absent);
    assert_eq!(
        list(&dir.path),
        before,
        "preflight on absent created nothing"
    );

    // Incomplete: a directory with no artifacts.
    std::fs::create_dir_all(&store).expect("mkdir");
    let before = list(&store);
    assert_eq!(preflight(&store), Preflight::Incomplete);
    assert_eq!(
        list(&store),
        before,
        "preflight on incomplete created nothing"
    );

    // Open refuses an incomplete store without touching it.
    assert!(matches!(
        open(&store, schemas(), sites()),
        Err(OpenError::Incomplete)
    ));
    assert_eq!(list(&store), before, "a refused open created nothing");
}

/// A second open of a held store is refused with StoreInUse naming the live owner (this
/// process, in-process advisory-lock contention).
#[test]
fn a_second_open_is_store_in_use_naming_the_owner() {
    let dir = TempDir::new("in-use");
    let store = dir.store();
    let id = instance();
    provision(&store, request(id)).expect("provision");

    let held = open(&store, schemas(), sites()).expect("first open holds the lock");
    match open(&store, schemas(), sites()) {
        Err(OpenError::Lock(error)) => {
            assert_eq!(error.code(), "store.locked");
            match error {
                marrow_lifecycle::LockError::StoreInUse { owner: Some(owner) } => {
                    assert_eq!(owner.pid, std::process::id(), "names the live owner pid");
                    assert_eq!(owner.instance, id, "names the held store instance");
                }
                other => panic!("expected a named StoreInUse owner, got {other:?}"),
            }
        }
        Ok(_) => panic!("a second open must be StoreInUse, but it opened"),
        Err(other) => panic!("a second open must be StoreInUse, got {other}"),
    }
    drop(held);
    // Once released, the store opens again.
    open(&store, schemas(), sites()).expect("reopen after release");
}

/// Creator race: many threads provision the same destination concurrently; exactly one wins
/// and every loser is AlreadyProvisioned. One ownership lineage survives — the destination
/// carries exactly the winner's instance.
#[test]
fn concurrent_provision_has_one_winner_and_one_lineage() {
    let dir = TempDir::new("race");
    let store = dir.store();

    let winners: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                scope.spawn(move || {
                    let id = instance();
                    match provision(&store, request(id)) {
                        Ok(p) => {
                            assert_eq!(p.instance, id);
                            Some(id)
                        }
                        Err(ProvisionError::AlreadyProvisioned) => None,
                        Err(other) => panic!("unexpected provision error in race: {other}"),
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().expect("thread"))
            .collect()
    });

    assert_eq!(winners.len(), 1, "exactly one provisioner wins the race");
    assert_eq!(preflight(&store), Preflight::Complete);
    let opened = open(&store, schemas(), sites()).expect("open the survivor");
    assert_eq!(
        opened.envelope.instance, winners[0],
        "the destination carries exactly the winner's instance (one lineage)",
    );
}

/// Unclean prior shutdown: a stale lock body (an owner descriptor left by a crash) makes the
/// next open run the integrity audit; a healthy store still opens, and the clean close then
/// truncates the lock so the following open is not unclean.
#[test]
fn an_unclean_prior_shutdown_runs_the_audit_and_a_healthy_store_opens() {
    let dir = TempDir::new("unclean");
    let store = dir.store();
    let id = instance();
    provision(&store, request(id)).expect("provision");

    // Simulate a crashed owner: write a stale owner descriptor into the lock body WITHOUT
    // holding the advisory lock, then leave it (a clean shutdown would have truncated it).
    {
        let held = open(&store, schemas(), sites()).expect("open to populate the lock body");
        // Copy the current lock body, then drop `held` (which truncates it), then restore the
        // stale body — modelling a crash that left the descriptor behind.
        let lock_path = store.join(marrow_lifecycle::LOCK_FILE);
        let stale = std::fs::read(&lock_path).expect("read lock body");
        drop(held);
        assert!(
            std::fs::read(&lock_path)
                .expect("post-close lock body")
                .is_empty(),
            "a clean close truncates the lock body",
        );
        std::fs::write(&lock_path, &stale).expect("restore a stale (crash) lock body");
    }

    // The next open sees the unclean prior shutdown, runs the audit, and the healthy store
    // opens. (The audit covers crash-path corruption only — see the module note.)
    let opened = open(&store, schemas(), sites()).expect("open runs the audit and succeeds");
    drop(opened);
    // The clean close truncated the lock, so the following open is not unclean and still opens.
    open(&store, schemas(), sites()).expect("clean reopen");
}

/// Cross-process contention: a spawned child holds the store while the parent is refused with
/// StoreInUse naming the CHILD's pid — a real second-process lock, not just same-process
/// advisory contention.
#[test]
fn a_child_process_holding_the_store_blocks_the_parent_by_pid() {
    let dir = TempDir::new("child-lock");
    let store = dir.store();
    let id = instance();
    provision(&store, request(id)).expect("provision");

    let ready = dir.path.join("child-ready");
    let release = dir.path.join("child-release");

    // Re-invoke this test binary in the ignored child-holder helper, passing the store dir and
    // the coordination files by env. The child opens the store (taking the lock), touches
    // `ready`, and holds until `release` appears.
    let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
        .args(["--exact", "child_holder_helper", "--ignored", "--nocapture"])
        .env("MARROW_LC_STORE", &store)
        .env("MARROW_LC_READY", &ready)
        .env("MARROW_LC_RELEASE", &release)
        .spawn()
        .expect("spawn child holder");

    // Wait for the child to hold the lock.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ready.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child never acquired the lock"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // The parent is refused, named by the child's pid.
    let outcome = open(&store, schemas(), sites());
    match outcome {
        Err(OpenError::Lock(marrow_lifecycle::LockError::StoreInUse { owner: Some(owner) })) => {
            assert_eq!(
                owner.pid,
                child.id(),
                "the parent is blocked by the child's pid"
            );
            assert_eq!(owner.instance, id);
        }
        Ok(_) => {
            let _ = std::fs::write(&release, b"");
            let _ = child.wait();
            panic!("expected StoreInUse naming the child, but the parent opened");
        }
        Err(other) => {
            let _ = std::fs::write(&release, b"");
            let _ = child.wait();
            panic!("expected StoreInUse naming the child, got {other}");
        }
    }

    // Release the child and confirm the store reopens once it exits.
    std::fs::write(&release, b"").expect("signal release");
    child.wait().expect("child exits");
    open(&store, schemas(), sites()).expect("reopen after the child releases");
}

/// The child-holder helper: NOT a real test (ignored). When invoked with the coordination
/// env vars by [`a_child_process_holding_the_store_blocks_the_parent_by_pid`], it opens the
/// store (holding the lock), signals readiness, and holds until released. A bare `cargo test`
/// run skips it (ignored) and, without the env vars, it is a no-op.
#[test]
#[ignore = "child-process helper, driven only by the cross-process lock fixture"]
fn child_holder_helper() {
    let Ok(store) = std::env::var("MARROW_LC_STORE") else {
        return; // invoked by a bare `--ignored` run without coordination: no-op.
    };
    let ready = std::env::var("MARROW_LC_READY").expect("ready path");
    let release = std::env::var("MARROW_LC_RELEASE").expect("release path");
    let store = PathBuf::from(store);

    let _held = open(&store, schemas(), sites()).expect("child opens and holds the store");
    std::fs::write(&ready, b"").expect("signal ready");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !Path::new(&release).exists() {
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Drop `_held` on return, releasing the lock.
}

fn list(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
