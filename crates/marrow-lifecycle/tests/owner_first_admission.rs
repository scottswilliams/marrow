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
use marrow_kernel::durable::{SiteSpec, SiteTarget, StoreSchema, StoreSchemaBuilder};
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
    let mut builder = StoreSchemaBuilder::root("app", vec![ScalarKind::Int]);
    builder.scalar_field("value", ScalarKind::Int, true);
    vec![builder.finish().expect("a bounded schema builds")]
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

/// The third door into a held store: the lock entry itself. A contender reaches the holder
/// through the owner marker, so whatever the marker's bytes say — and however many names
/// reach it — the verdict a contender receives is the exclusion that applies to it, never a
/// verdict about the holder's marker. A second link does not divide exclusion (every opener
/// of either name locks the same node) and the marker's bytes are read only for the holder's
/// identity, which a contender may lose without losing the verdict.
#[cfg(unix)]
#[test]
fn a_contender_is_locked_out_whatever_state_the_holder_marker_is_in() {
    let (dir, store) = provisioned("contender-marker");
    let held = open(&store, schemas(), sites()).expect("the holder opens the store");
    let marker = store.join(marrow_lifecycle::LOCK_FILE);

    for (tag, body) in [
        ("empty", b"".as_slice()),
        ("magic-and-version-only", b"MWSL\x01"),
        ("truncated-pending", b"MWSL\x01\x01\x00\x00"),
        ("truncated-bound", b"MWSL\x01\x02\x00\x00\x00\x01"),
        (
            "unknown-tag",
            b"MWSL\x01\x7f\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x01",
        ),
        ("foreign-magic", b"XXXX\x01\x01"),
        ("garbage", b"not a marker at all"),
    ] {
        std::fs::write(&marker, body).expect("rewrite the holder's marker");
        match open(&store, schemas(), sites()) {
            Err(OpenError::Lock(error)) => assert_eq!(
                error.code(),
                "store.locked",
                "a contender meeting a {tag} holder marker must be told the store is locked",
            ),
            Ok(_) => panic!("a contender opened a held store whose marker is {tag}"),
            Err(other) => panic!("a {tag} holder marker preempted exclusion with {other}"),
        }
    }

    std::fs::hard_link(&marker, dir.path.join("marker-alias")).expect("add a second marker link");
    match open(&store, schemas(), sites()) {
        Err(OpenError::Lock(error)) => assert_eq!(
            error.code(),
            "store.locked",
            "a second link to the holder's marker must not preempt exclusion",
        ),
        Ok(_) => panic!("a contender opened a held store through a multiply-linked marker"),
        Err(other) => panic!("a multiply-linked holder marker preempted exclusion with {other}"),
    }
    drop(held);
}

/// Exclusion rests on the store directory node, and replacing every node inside it that a
/// holder locks does not divide it.
///
/// A holder locks two nodes that a writer inside the store directory can replace: the `lock`
/// marker and the engine file. Each alone is refused by the other — a deleted marker is
/// stopped at the engine, a replaced engine at the marker — so a single fault reaches the
/// exclusion verdict either way. Replacing both at once leaves neither, which is a state a
/// naive whole-directory restore over a live store also produces. The directory node itself
/// is the one node in the store that no replacement of its own children changes, and the
/// owner locks it before it opens any name inside it, so the compound fault reaches the same
/// verdict a single one does.
#[cfg(unix)]
#[test]
fn replacing_every_replaceable_node_a_holder_locks_admits_no_second_owner() {
    let (_dir, store) = provisioned("compound-fault");
    let held = open(&store, schemas(), sites()).expect("the holder opens the store");

    // A fresh engine node published over the held one under its own name: a whole store
    // engine byte for byte, and an inode no holder locks.
    let (_donor, donor_store) = provisioned("compound-fault-donor");
    let fresh = store.join("fresh-engine");
    std::fs::copy(donor_store.join(marrow_lifecycle::ENGINE_FILE), &fresh)
        .expect("copy a fresh engine into the held store directory");
    std::fs::rename(&fresh, store.join(marrow_lifecycle::ENGINE_FILE))
        .expect("publish the fresh engine under the engine's name");
    std::fs::remove_file(store.join(marrow_lifecycle::LOCK_FILE)).expect("remove the marker");

    match open(&store, schemas(), sites()) {
        Err(OpenError::Lock(error)) => assert_eq!(
            error.code(),
            "store.locked",
            "replacing both of the holder's replaceable locked nodes must still yield the \
             exclusion verdict",
        ),
        Ok(_) => panic!(
            "a second owner opened a held store after both of the holder's replaceable locked \
             nodes were replaced",
        ),
        Err(other) => panic!("the compound replacement preempted exclusion with {other}"),
    }
    drop(held);
}

/// A store directory this process cannot look inside refuses as a permission denial, never
/// as absence and never as corruption. A predicate that folds a denied look into "the
/// artifact is not there" reports a complete, intact, live-held store as a partially formed
/// one, which is a false factual claim about the store and — read as instructions — tells an
/// operator to remove it.
#[cfg(unix)]
#[test]
fn a_store_directory_that_denies_access_refuses_as_a_permission_denial() {
    use std::os::unix::fs::PermissionsExt;

    for holder in [false, true] {
        let tag = if holder { "denied-held" } else { "denied-idle" };
        let (_dir, store) = provisioned(tag);
        let held = holder.then(|| open(&store, schemas(), sites()).expect("the holder opens"));
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o000))
            .expect("deny access to the store directory");

        // A process holding the mode-override capability is bound by none of those bits, so
        // only a process they do bind can observe this refusal at all.
        if std::fs::read_dir(&store).is_err() {
            match open(&store, schemas(), sites()) {
                Ok(_) => panic!("a store directory that cannot be looked inside was opened"),
                Err(error) => assert_eq!(
                    error.code(),
                    "store.permission_denied",
                    "a denied look at a {tag} store must report the denial, not what it could \
                     not see: {error}",
                ),
            }
        }

        let _ = std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700));
        drop(held);
    }
}

/// The whole family, enumerated: every door by which a contender can learn about a held
/// store, and the verdict each one yields. A door either reaches the store directory node
/// the owner locks — and then the verdict is exactly `store.locked`, whatever the marker's
/// bytes, its node kind, its mode, its link count, or the store's other artifacts say — or
/// it cannot reach that node at all, and then it refuses. No door admits a second owner.
///
/// Two doors get past the marker and are stopped behind the directory node: a marker deleted
/// under the holder, and a fresh node renamed over its name. Both let a contender take a
/// marker lock the holder is not holding, at the cost of leaving an unclean obligation
/// behind in an intact store. The marker is a cooperating owner's own custody, not a defence
/// against an actor who can rewrite the store directory.
///
/// Only the two doors that take the store directory node itself out of reach — removing it,
/// and denying this process the read the lock's open requires — refuse instead, and each
/// refuses as what it is rather than as a claim about the store's contents.
#[cfg(unix)]
#[test]
fn no_door_into_a_held_store_admits_a_second_owner() {
    use std::os::unix::fs::PermissionsExt;

    /// The verdict a door is allowed to produce. `Locked` is the exclusion verdict a
    /// contender is owed; `Refused` is a door the contender cannot see through at all, where
    /// it must still refuse rather than proceed.
    enum Verdict {
        Locked,
        Refused(&'static [&'static str]),
    }

    /// One door: how a held store is damaged, and the verdict the next open must reach.
    struct Door {
        name: &'static str,
        damage: Box<dyn Fn(&Path)>,
        expected: Verdict,
    }

    let door = |name, damage: Box<dyn Fn(&Path)>, expected| Door {
        name,
        damage,
        expected,
    };
    fn chmod_marker(store: &Path, mode: u32) {
        std::fs::set_permissions(
            store.join(marrow_lifecycle::LOCK_FILE),
            std::fs::Permissions::from_mode(mode),
        )
        .expect("chmod the marker");
    }
    let doors = vec![
        door(
            "the marker deleted under the holder",
            Box::new(|store: &Path| {
                std::fs::remove_file(store.join(marrow_lifecycle::LOCK_FILE)).expect("remove");
            }),
            Verdict::Locked,
        ),
        door(
            "a fresh node renamed over the marker's name",
            Box::new(|store: &Path| {
                let decoy = store.join("decoy");
                std::fs::write(&decoy, b"").expect("write the decoy");
                std::fs::rename(&decoy, store.join(marrow_lifecycle::LOCK_FILE)).expect("swap");
            }),
            Verdict::Locked,
        ),
        door(
            "the store directory stripped of write access",
            Box::new(|store: &Path| {
                std::fs::set_permissions(store, std::fs::Permissions::from_mode(0o500))
                    .expect("chmod the store directory");
            }),
            Verdict::Locked,
        ),
        door(
            "the engine deleted under the holder",
            Box::new(|store: &Path| {
                std::fs::remove_file(store.join(marrow_lifecycle::ENGINE_FILE)).expect("remove");
            }),
            Verdict::Locked,
        ),
        door(
            "a symbolic link standing in for the marker",
            Box::new(|store: &Path| {
                let marker = store.join(marrow_lifecycle::LOCK_FILE);
                std::fs::remove_file(&marker).expect("remove the marker");
                std::os::unix::fs::symlink(store.join("elsewhere"), &marker).expect("link");
            }),
            Verdict::Locked,
        ),
        door(
            "a directory standing in for the marker",
            Box::new(|store: &Path| {
                let marker = store.join(marrow_lifecycle::LOCK_FILE);
                std::fs::remove_file(&marker).expect("remove the marker");
                std::fs::create_dir(&marker).expect("a directory in place of the marker");
            }),
            Verdict::Locked,
        ),
        door(
            "a marker whose own mode denies every open",
            Box::new(|store: &Path| chmod_marker(store, 0o000)),
            Verdict::Locked,
        ),
        door(
            "a marker whose own mode denies the write the open requires",
            Box::new(|store: &Path| chmod_marker(store, 0o400)),
            Verdict::Locked,
        ),
        door(
            "a marker whose own mode denies the read the open requires",
            Box::new(|store: &Path| chmod_marker(store, 0o200)),
            Verdict::Locked,
        ),
        door(
            "the store directory stripped of the read its own lock needs",
            Box::new(|store: &Path| {
                std::fs::set_permissions(store, std::fs::Permissions::from_mode(0o300))
                    .expect("chmod the store directory");
            }),
            // A process that may read it regardless (a privileged test runner) reaches the
            // directory node and meets the holder; one that may not cannot reach it at all,
            // and is told that rather than anything about the store.
            Verdict::Refused(&["store.permission_denied", "store.locked"]),
        ),
        door(
            "the store directory removed under the holder",
            Box::new(|store: &Path| {
                std::fs::remove_dir_all(store).expect("remove the store directory");
            }),
            Verdict::Refused(&["store.io"]),
        ),
    ];

    for Door {
        name,
        damage,
        expected,
    } in doors
    {
        let (_dir, store) = provisioned("held-store-doors");
        let held = open(&store, schemas(), sites()).expect("the holder opens the store");
        damage(&store);

        match open(&store, schemas(), sites()) {
            Ok(_) => panic!("{name} admitted a second owner of a held store"),
            Err(error) => match expected {
                Verdict::Locked => assert_eq!(
                    error.code(),
                    "store.locked",
                    "{name} must yield the exclusion verdict, got {error}",
                ),
                Verdict::Refused(codes) => assert!(
                    codes.contains(&error.code()),
                    "{name} must refuse as one of {codes:?}, got {} ({error})",
                    error.code(),
                ),
            },
        }
        let _ = std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700));
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
        bytes.len() as u64,
        marrow_lifecycle::MAX_ENVELOPE_FILE_BYTES,
        "the largest envelope the encoder produces is the ceiling admission applies",
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
        bytes.len() as u64,
        marrow_lifecycle::MAX_HEAD_FILE_BYTES,
        "the largest head the encoder produces is the ceiling admission applies",
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
/// owner holds, so admission refuses it rather than reading through it — as the store
/// directory not holding the artifact, which is what its multiply-linked sibling below also
/// reports.
///
/// The engine is in the family even though admission never reads its bytes. Its opener
/// resolves it by path, which follows a link, so the completeness verdict is what has to
/// refuse it: without that, a store whose engine bytes live outside the directory the owner
/// holds would open.
#[cfg(unix)]
#[test]
fn a_symbolic_link_standing_in_for_an_artifact_is_refused() {
    for artifact in [
        marrow_lifecycle::ENVELOPE_FILE,
        marrow_lifecycle::HEAD_FILE,
        marrow_lifecycle::ENGINE_FILE,
    ] {
        let (dir, store) = provisioned(&format!("symlink-{artifact}"));
        let target = dir.path.join(format!("{artifact}-elsewhere"));
        let path = store.join(artifact);
        std::fs::rename(&path, &target).expect("move the artifact outside the store directory");
        std::os::unix::fs::symlink(&target, &path).expect("link the artifact name to it");

        match open(&store, schemas(), sites()) {
            Ok(_) => panic!("admission followed a symbolic link standing in for the {artifact}"),
            Err(error) => assert_eq!(
                error.code(),
                "store.corruption",
                "a linked {artifact} is a substitution refusal, not contention or I/O: {error}",
            ),
        }
    }
}

/// An artifact carrying a second hard link is refused. A one-link regular file is the whole
/// of the artifact's reachability under the owner's directory; a second link is a name the
/// owner does not hold, through which the bytes admission just checked can be rewritten. It
/// reports as the same refusal its symbolic-link sibling above reaches.
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
            Err(error) => assert_eq!(
                error.code(),
                "store.corruption",
                "a multiply-linked {artifact} is a substitution refusal, not contention: {error}",
            ),
        }
    }
}

/// One refusal, one subject. An admission refusal composes the artifact it names with the
/// rejection that artifact reached, so what reaches a user is a single sentence about a
/// single thing rather than two subjects stacked.
#[test]
fn an_admission_refusal_reads_as_one_sentence_about_one_artifact() {
    for (artifact, damage, expected) in [
        (
            "head",
            Box::new(|bytes: &mut Vec<u8>| bytes[7] ^= 0xFF) as Box<dyn Fn(&mut Vec<u8>)>,
            "the store head does not match its sealing digest",
        ),
        (
            "envelope",
            Box::new(|bytes: &mut Vec<u8>| bytes[4] = 0x7F),
            "the store envelope records version 127, which this build does not read",
        ),
    ] {
        let (_dir, store) = provisioned(&format!("one-subject-{artifact}"));
        let path = store.join(artifact);
        let mut bytes = std::fs::read(&path).expect("read artifact");
        damage(&mut bytes);
        std::fs::write(&path, &bytes).expect("write damaged artifact");

        match open(&store, schemas(), sites()) {
            Ok(_) => panic!("a damaged {artifact} was admitted"),
            Err(error) => assert_eq!(error.to_string(), expected),
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

/// An artifact rewritten in place, to a different length, while admission is reading it is
/// never admitted as a spliced artifact. The pre-read length check, the bounded read one
/// byte past the ceiling, the identity and length recheck, and the artifact's own sealing
/// digest together admit only bytes that were a whole artifact: every outcome here is one of
/// the two whole heads the mutator writes, or a typed refusal.
///
/// The interleaving is real but not scheduled, so this drives the invariant rather than
/// pinning a particular window; the deterministic substitution cases above pin the
/// identity and link checks on their own.
#[test]
fn an_artifact_rewritten_under_a_read_is_never_admitted_spliced() {
    let (_dir, store) = provisioned("torn-read");
    let short = head(1, vec![0x11; 32]).encode();
    let long = head(64, vec![0x22; 64 * 1024]).encode();
    assert_ne!(
        short.len(),
        long.len(),
        "the two forms must differ in length"
    );
    let path = head_path(&store);
    let stop = std::sync::atomic::AtomicBool::new(false);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            use std::io::Write;
            let mut which = false;
            while !stop.load(Ordering::Relaxed) {
                // Rewrite in place rather than renaming, so a reader's pinned handle can
                // genuinely observe a body mid-write.
                let bytes = if which { &short } else { &long };
                which = !which;
                if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(&path) {
                    let _ = file.set_len(bytes.len() as u64);
                    let _ = file.write_all(bytes);
                }
            }
        });

        for _ in 0..50 {
            match open(&store, schemas(), sites()) {
                Ok(opened) => {
                    let admitted = opened.head.encode();
                    assert!(
                        admitted == short || admitted == long,
                        "admission returned a head that was never wholly written",
                    );
                }
                Err(error) => assert!(
                    matches!(
                        error.code(),
                        "store.corruption" | "store.limit" | "store.format_version" | "store.io",
                    ),
                    "a torn read must be a typed refusal, got {error}",
                ),
            }
        }
        stop.store(true, Ordering::Relaxed);
    });
}

/// An open aimed at a directory that is not a store writes nothing into it — not even the
/// owner lock the acquisition would otherwise create. Exclusion is still decided before
/// anything a store *contains* is read: a directory that has been opened as a store before
/// carries the lock entry, and an open of one of those takes the lock first, whatever state
/// its artifacts are in.
#[test]
fn an_open_of_a_directory_that_is_not_a_store_writes_nothing_into_it() {
    let dir = TempDir::new("refused-publishes-nothing");
    let store = dir.store();
    std::fs::create_dir_all(&store).expect("an existing but empty store directory");
    std::fs::write(store.join("notes.txt"), b"unrelated").expect("an unrelated file");

    for _ in 0..2 {
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
        assert_eq!(
            names,
            vec!["notes.txt".to_string()],
            "a refused open wrote into a directory that is not a store",
        );
    }

    // A store that has been opened before keeps its lock entry, and losing an artifact does
    // not move the completeness verdict ahead of the owner: the lock is still taken first.
    let (_dir, complete) = provisioned("refused-after-first-open");
    drop(open(&complete, schemas(), sites()).expect("the first open creates the lock entry"));
    std::fs::remove_file(complete.join(marrow_lifecycle::HEAD_FILE)).expect("remove the head");
    assert!(matches!(
        open(&complete, schemas(), sites()),
        Err(OpenError::Incomplete),
    ));
    assert!(
        complete.join(marrow_lifecycle::LOCK_FILE).exists(),
        "an open that took the owner lock keeps the entry it locked",
    );
}
