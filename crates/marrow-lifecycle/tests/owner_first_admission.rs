//! Owner-first admission of the persisted store artifacts.
//!
//! An open holds the physical store owner before it reads or decodes anything, so the
//! envelope and the head are one lock-protected admission snapshot. Two families of
//! invariant are driven here through the production `open` path over real directories:
//!
//! - **Nothing preempts exclusion.** A contender meeting a held store is told the store is
//!   locked, whatever state the holder's artifacts are in — malformed, truncated, or
//!   deleted outright. A decode verdict about the holder's bytes is not a verdict a
//!   contender is entitled to.
//! - **Every read is bounded and pinned.** Each artifact is opened from the retained store
//!   directory without following a link, must be a regular one-link file, and is refused
//!   beyond its exact byte ceiling before its bytes are allocated. The largest artifact the
//!   encoder can produce is admitted; one byte more is refused as a limit, not as
//!   corruption, which is what proves the ceiling ran ahead of the decoder.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use marrow_image::LedgerIdBytes;
use marrow_kernel::codec::value::ScalarKind;
use marrow_kernel::durable::{FieldSchema, SiteSpec, SiteTarget, StoreSchema};
use marrow_lifecycle::{
    ActiveBinding, EngineKind, HeadMap, LogicalHead, OpenError, ProvisionRequest, StoreEnvelope,
    StoreInstanceId, open, provision,
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
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "marrow-owner-first-{tag}-{}-{nonce}-{counter}",
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

fn binding() -> ActiveBinding {
    ActiveBinding {
        image_format_version: 0,
        image_id: [0x11; 32],
        durable_contract: [0x22; 32],
        interface: [0x33; 32],
    }
}

fn envelope(instance: StoreInstanceId, toolchain: &str) -> StoreEnvelope {
    StoreEnvelope {
        instance,
        writer_toolchain: toolchain.into(),
        engine_kind: EngineKind::Redb,
        engine_format_version: 1,
    }
}

fn head(entries: usize, ceiling_payload: Vec<u8>) -> LogicalHead {
    let ids: Vec<LedgerIdBytes> = (0..entries)
        .map(|index| {
            let mut bytes = [0u8; 16];
            bytes[0..8].copy_from_slice(&(index as u64).to_be_bytes());
            LedgerIdBytes::from_bytes(bytes)
        })
        .collect();
    LogicalHead::provision(
        binding(),
        ceiling_payload,
        HeadMap::assign(&ids).expect("head map"),
    )
}

fn request(instance: StoreInstanceId) -> ProvisionRequest {
    ProvisionRequest {
        envelope: envelope(instance, "0.1.0"),
        head: head(1, vec![0x44, 0x45]),
        schemas: schemas(),
        sites: sites(),
    }
}

fn instance() -> StoreInstanceId {
    StoreInstanceId::draw().expect("entropy")
}

/// A provisioned store at a fresh temporary destination.
fn provisioned(tag: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new(tag);
    let store = dir.store();
    provision(&store, request(instance())).expect("provision");
    (dir, store)
}

fn envelope_path(store: &Path) -> PathBuf {
    store.join(marrow_lifecycle::ENVELOPE_FILE)
}

fn head_path(store: &Path) -> PathBuf {
    store.join(marrow_lifecycle::HEAD_FILE)
}

/// How an artifact file is made unreadable while its store is held.
#[derive(Clone, Copy)]
enum Damage {
    /// The body is present but its digest no longer reseals it.
    Malformed,
    /// The body is cut short of its last field.
    Truncated,
    /// The file is gone.
    Absent,
}

impl Damage {
    fn apply(self, path: &Path) {
        match self {
            Damage::Malformed => {
                let mut bytes = std::fs::read(path).expect("read artifact");
                bytes[5] ^= 0xFF;
                std::fs::write(path, &bytes).expect("write malformed artifact");
            }
            Damage::Truncated => {
                let bytes = std::fs::read(path).expect("read artifact");
                std::fs::write(path, &bytes[..bytes.len() - 1]).expect("truncate artifact");
            }
            Damage::Absent => std::fs::remove_file(path).expect("remove artifact"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Damage::Malformed => "malformed",
            Damage::Truncated => "truncated",
            Damage::Absent => "absent",
        }
    }
}

/// The headline invariant: a contender meeting a held store is refused with `store.locked`
/// naming the live owner, whatever state the holder's envelope is in. Reading and decoding
/// the envelope before the owner lock is taken would hand a contender a verdict about the
/// holder's bytes — a decode error, or an "incomplete store" reading of a deleted artifact —
/// in place of the exclusion that actually applies to it.
#[test]
fn a_contender_is_locked_out_whatever_state_the_holder_envelope_is_in() {
    for damage in [Damage::Malformed, Damage::Truncated, Damage::Absent] {
        let (_dir, store) = provisioned(&format!("contender-envelope-{}", damage.label()));
        let held = open(&store, schemas(), sites()).expect("the holder opens the store");
        damage.apply(&envelope_path(&store));

        match open(&store, schemas(), sites()) {
            Err(OpenError::Lock(error)) => assert_eq!(
                error.code(),
                "store.locked",
                "a contender meeting a {} holder envelope must be told the store is locked",
                damage.label(),
            ),
            Ok(_) => panic!(
                "a contender opened a held store whose envelope is {}",
                damage.label()
            ),
            Err(other) => panic!(
                "a {} holder envelope preempted exclusion with {other}",
                damage.label()
            ),
        }
        drop(held);
    }
}

/// The same invariant for the head: exclusion is decided before the head is read, so a
/// contender is never handed a decode verdict — or a completeness verdict — about the
/// holder's head.
#[test]
fn a_contender_is_locked_out_whatever_state_the_holder_head_is_in() {
    for damage in [Damage::Malformed, Damage::Truncated, Damage::Absent] {
        let (_dir, store) = provisioned(&format!("contender-head-{}", damage.label()));
        let held = open(&store, schemas(), sites()).expect("the holder opens the store");
        damage.apply(&head_path(&store));

        match open(&store, schemas(), sites()) {
            Err(OpenError::Lock(error)) => assert_eq!(
                error.code(),
                "store.locked",
                "a contender meeting a {} holder head must be told the store is locked",
                damage.label(),
            ),
            Ok(_) => panic!(
                "a contender opened a held store whose head is {}",
                damage.label()
            ),
            Err(other) => panic!(
                "a {} holder head preempted exclusion with {other}",
                damage.label()
            ),
        }
        drop(held);
    }
}

/// The exact envelope ceiling, driven at N and N+1. The largest envelope the encoder can
/// produce — a writer toolchain at its own bound — is exactly 126 bytes and opens; one byte
/// more is refused as a representational limit rather than as corruption, which is what
/// proves the ceiling was applied before the decoder saw the bytes.
#[test]
fn the_envelope_ceiling_admits_its_maximum_and_refuses_one_byte_more() {
    let dir = TempDir::new("envelope-ceiling");
    let store = dir.store();
    let id = instance();
    let maximal = envelope(id, &"t".repeat(64));
    let bytes = maximal.encode();
    assert_eq!(
        bytes.len(),
        126,
        "the largest envelope the encoder produces is the ceiling",
    );
    provision(
        &store,
        ProvisionRequest {
            envelope: maximal,
            head: head(1, vec![0x44]),
            schemas: schemas(),
            sites: sites(),
        },
    )
    .expect("provision a maximal envelope");

    let opened = open(&store, schemas(), sites()).expect("a maximal envelope is admitted");
    assert_eq!(opened.envelope.instance, id);
    drop(opened);

    let mut oversize = bytes.clone();
    oversize.push(0x00);
    std::fs::write(envelope_path(&store), &oversize).expect("write an oversize envelope");
    match open(&store, schemas(), sites()) {
        Err(error) => assert_eq!(
            error.code(),
            "store.limit",
            "one byte past the envelope ceiling is a limit refusal, not a decode verdict",
        ),
        Ok(_) => panic!("an envelope past its ceiling was admitted"),
    }
}

/// The exact head ceiling, driven at N and N+1. A head carrying the largest identity map and
/// the largest accepted-ceiling payload its own decoder admits is exactly 5,505,218 bytes and
/// opens; one byte more is a limit refusal, taken before the bytes are decoded.
#[test]
fn the_head_ceiling_admits_its_maximum_and_refuses_one_byte_more() {
    let dir = TempDir::new("head-ceiling");
    let store = dir.store();
    let maximal = head(65_536, vec![0x5A; 4 * 1024 * 1024]);
    let bytes = maximal.encode();
    assert_eq!(
        bytes.len(),
        5_505_218,
        "the largest head the encoder produces is the ceiling",
    );
    provision(
        &store,
        ProvisionRequest {
            envelope: envelope(instance(), "0.1.0"),
            head: maximal,
            schemas: schemas(),
            sites: sites(),
        },
    )
    .expect("provision a maximal head");

    let opened = open(&store, schemas(), sites()).expect("a maximal head is admitted");
    assert_eq!(opened.head.head_map.len(), 65_536);
    drop(opened);

    let mut oversize = bytes.clone();
    oversize.push(0x00);
    std::fs::write(head_path(&store), &oversize).expect("write an oversize head");
    match open(&store, schemas(), sites()) {
        Err(error) => assert_eq!(
            error.code(),
            "store.limit",
            "one byte past the head ceiling is a limit refusal, not a decode verdict",
        ),
        Ok(_) => panic!("a head past its ceiling was admitted"),
    }
}

/// An artifact reached through a symbolic link is refused, not followed. The store directory
/// names its artifacts; a link standing in for one names bytes outside the directory the
/// owner holds, so admission refuses it rather than reading through it.
#[cfg(unix)]
#[test]
fn a_symbolic_link_standing_in_for_an_artifact_is_refused() {
    for artifact in ["envelope", "head"] {
        let (dir, store) = provisioned(&format!("symlink-{artifact}"));
        let target = dir.path.join(format!("{artifact}-elsewhere"));
        let path = store.join(artifact);
        std::fs::rename(&path, &target).expect("move the artifact outside the store directory");
        std::os::unix::fs::symlink(&target, &path).expect("link the artifact name to it");

        match open(&store, schemas(), sites()) {
            Ok(_) => panic!("admission followed a symbolic link standing in for the {artifact}"),
            Err(error) => assert_ne!(
                error.code(),
                "store.locked",
                "a linked {artifact} is a substitution refusal, not contention",
            ),
        }
    }
}

/// An artifact carrying a second hard link is refused. A one-link regular file is the whole
/// of the artifact's reachability under the owner's directory; a second link is a name the
/// owner does not hold, through which the bytes admission just checked can be rewritten.
#[cfg(unix)]
#[test]
fn a_second_hard_link_to_an_artifact_is_refused() {
    for artifact in ["envelope", "head"] {
        let (dir, store) = provisioned(&format!("hardlink-{artifact}"));
        let path = store.join(artifact);
        std::fs::hard_link(&path, dir.path.join(format!("{artifact}-alias")))
            .expect("add a second link to the artifact");

        match open(&store, schemas(), sites()) {
            Ok(_) => panic!("admission accepted a multiply-linked {artifact}"),
            Err(error) => assert_ne!(
                error.code(),
                "store.locked",
                "a multiply-linked {artifact} is a substitution refusal, not contention",
            ),
        }
    }
}

/// The sibling matrix over both artifacts: a valid pair opens, and each malformation is
/// reported as itself — an unknown writer version as a format-version refusal, a tampered or
/// torn body as corruption — rather than flattened into one verdict.
#[test]
fn each_artifact_malformation_is_reported_as_itself() {
    for artifact in ["envelope", "head"] {
        for (tag, damage, expected) in [
            (
                "tampered",
                Box::new(|bytes: &mut Vec<u8>| bytes[5] ^= 0xFF) as Box<dyn Fn(&mut Vec<u8>)>,
                "store.corruption",
            ),
            (
                "unknown-version",
                Box::new(|bytes: &mut Vec<u8>| bytes[4] = 0x7F),
                "store.format_version",
            ),
            (
                "bad-magic",
                Box::new(|bytes: &mut Vec<u8>| bytes[0] = b'X'),
                "store.corruption",
            ),
            (
                "trailing",
                Box::new(|bytes: &mut Vec<u8>| bytes.push(0x00)),
                "store.corruption",
            ),
            (
                "truncated",
                Box::new(|bytes: &mut Vec<u8>| {
                    bytes.pop();
                }),
                "store.corruption",
            ),
        ] {
            let (_dir, store) = provisioned(&format!("typed-{artifact}-{tag}"));
            open(&store, schemas(), sites()).expect("the undamaged pair opens");

            let path = store.join(artifact);
            let mut bytes = std::fs::read(&path).expect("read artifact");
            damage(&mut bytes);
            std::fs::write(&path, &bytes).expect("write damaged artifact");

            match open(&store, schemas(), sites()) {
                Ok(_) => panic!("a {tag} {artifact} was admitted"),
                Err(error) => assert_eq!(
                    error.code(),
                    expected,
                    "a {tag} {artifact} must report itself, not a flattened verdict",
                ),
            }
        }
    }
}

/// A refused open publishes no store artifact. Taking the owner lock is what makes exclusion
/// decidable before anything is read, so the lock entry itself may appear; the engine,
/// envelope, and head never do.
#[test]
fn a_refused_open_publishes_no_store_artifact() {
    let dir = TempDir::new("refused-publishes-nothing");
    let store = dir.store();
    std::fs::create_dir_all(&store).expect("an existing but empty store directory");

    assert!(matches!(
        open(&store, schemas(), sites()),
        Err(OpenError::Incomplete),
    ));
    let mut names = std::fs::read_dir(&store)
        .expect("read the refused directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert!(
        names.iter().all(|name| name == marrow_lifecycle::LOCK_FILE),
        "a refused open published a store artifact: {names:?}",
    );
}
