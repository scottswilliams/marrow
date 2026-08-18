//! Crate-internal behavior tests over the `.marrow/ids` publication owner.
//!
//! Every state in the closed map is built on a real filesystem the way a crash
//! would leave it — the durable header and phase records through the journal
//! owner's own encoders, the artifact and stage entries as real inodes — and
//! then driven through the production recovery entry. Nothing here reaches a
//! test-only production path: the only crate-private reach is the header
//! encoder, which is exactly the durable representation under test.
//!
//! Guard acquisition is serialized across this binary because the quarantine is
//! process-wide by construction; the temp roots are disjoint, so the
//! serialization costs a lock and nothing else.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use marrow_codes::Code;
use marrow_fs_journal::{
    AdmittedDir, CustodyError, EntryName, FsIdentity, JournalKind, PendingName, encode_header,
    encode_record,
};
use marrow_project::{IdentityAnchor, IdentityKind, LedgerPublicationPlan};

use crate::capture::capture_project;
use crate::overlay::OverlaySnapshot;
use crate::publication::header::RowHeader;
use crate::publication::{
    IdsPublication, IdsPublicationError, IdsPublicationMarker, IdsPublishOutcome, IdsRefusal,
    ProjectMetadataWriteGuard, ids_publication_marker,
};

const MANIFEST: &[u8] = b"edition = \"2026\"\n";

fn serialized() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Recover `project` and require it to settle as `expected`.
///
/// The planting stays in each kat, because the state a kat plants is its whole
/// subject. What is shared is only the drive-and-check that follows it.
fn assert_settled(project: &Project, expected: IdsPublication) {
    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(expected));
}

/// A temporary project root removed on drop.
struct Project {
    root: PathBuf,
}

impl Project {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "marrow-idpub01-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create temp project");
        fs::write(root.join("marrow.toml"), MANIFEST).expect("write manifest");
        fs::write(root.join("src/main.mw"), b"").expect("write source");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn meta(&self) -> PathBuf {
        self.root.join(".marrow")
    }

    fn write_meta(&self, name: &str, bytes: &[u8]) {
        fs::create_dir_all(self.meta()).expect("create metadata directory");
        fs::write(self.meta().join(name), bytes).expect("write metadata entry");
    }

    fn read_meta(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.meta().join(name)).ok()
    }

    fn quarantine(&self) -> PathBuf {
        self.meta().join("ids.publish.quarantine")
    }

    /// The one state an interrupted removal can leave: the object under the
    /// quarantine name, its own name absent.
    ///
    /// A removal's only entry mutations are one rename in and either an unlink
    /// or one rename back, all within the metadata directory. Before the first
    /// there is nothing to find; after the last there is nothing to find; the
    /// interval between them is this.
    fn move_into_quarantine(&self, name: &str) {
        fs::rename(self.meta().join(name), self.quarantine())
            .expect("move the object to the quarantine name");
    }

    fn exists(&self, name: &str) -> bool {
        fs::symlink_metadata(self.meta().join(name)).is_ok()
    }

    /// One publication plan minting `anchor`, admitted against whatever the
    /// project currently carries. The plan comes from the pure owner through a
    /// real capture, so it binds the exact captured artifact state.
    fn plan(&self, anchor: &str, id: u8) -> LedgerPublicationPlan {
        let input = capture_project(self.path(), OverlaySnapshot::empty())
            .expect("the fixture project captures");
        input
            .admit_identity_mints_with(
                IdentityAnchor::new(IdentityKind::Product, anchor),
                Vec::new(),
                |count| {
                    Ok::<_, std::convert::Infallible>(
                        (0..count)
                            .map(|index| {
                                let mut bytes = [0u8; 16];
                                bytes[0] = id;
                                bytes[15] = u8::try_from(index).expect("one candidate");
                                marrow_project::DurableIdentityId::from_bytes(bytes)
                            })
                            .collect(),
                    )
                },
            )
            .expect("the mint is admitted")
    }

    fn guard(&self) -> ProjectMetadataWriteGuard {
        ProjectMetadataWriteGuard::acquire(self.path()).expect("the write guard is acquired")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn name(spelling: &str) -> EntryName {
    EntryName::admit(spelling).expect("an admitted fixture spelling")
}

fn identity_of(path: &Path) -> FsIdentity {
    let dir = AdmittedDir::admit_trusted_root(path.parent().expect("a parent"))
        .expect("admit the fixture parent");
    dir.stat_entry(&name(
        path.file_name()
            .and_then(|raw| raw.to_str())
            .expect("a UTF-8 fixture name"),
    ))
    .expect("stat the fixture entry")
    .expect("the fixture entry exists")
    .identity()
}

/// Build a durable kind-1 journal on disk the way a crash between two syncs
/// leaves it: the exact header the protocol would have written, followed by the
/// phase records it had reached, under the single `.pending` name.
struct Crash<'a> {
    project: &'a Project,
    base: Option<Vec<u8>>,
    next: Vec<u8>,
    records: Vec<(u8, Vec<u8>)>,
    tail: Vec<u8>,
    parent_override: Option<FsIdentity>,
    inode_override: Option<FsIdentity>,
    base_inode: Option<FsIdentity>,
    next_inode: Option<FsIdentity>,
}

impl<'a> Crash<'a> {
    fn new(project: &'a Project, base: Option<&[u8]>, next: &[u8]) -> Self {
        Self {
            project,
            base: base.map(<[u8]>::to_vec),
            next: next.to_vec(),
            records: vec![(1, Vec::new())],
            tail: Vec::new(),
            parent_override: None,
            inode_override: None,
            base_inode: None,
            next_inode: None,
        }
    }

    fn installing(mut self) -> Self {
        self.records.push((2, Vec::new()));
        self
    }

    fn settled(mut self, terminal: u8) -> Self {
        self.records.push((3, vec![terminal]));
        self
    }

    fn partial(mut self, record: Vec<u8>, keep: usize) -> Self {
        self.tail = record[..keep].to_vec();
        self
    }

    fn parent(mut self, identity: FsIdentity) -> Self {
        self.parent_override = Some(identity);
        self
    }

    fn inode(mut self, identity: FsIdentity) -> Self {
        self.inode_override = Some(identity);
        self
    }

    /// Record the displaced generation's inode now, before the fixture moves
    /// or removes the entry that carries it.
    fn witness_base(mut self, entry: &str) -> Self {
        self.base_inode = Some(identity_of(&self.project.meta().join(entry)));
        self
    }

    /// Record the successor's inode now, before the fixture moves the entry
    /// that carries it.
    fn witness_next(mut self, entry: &str) -> Self {
        self.next_inode = Some(identity_of(&self.project.meta().join(entry)));
        self
    }

    /// Write the journal. Whatever inodes the header witnesses must already
    /// exist, because the header binds them by identity.
    fn plant(self) {
        let meta = self.project.meta();
        let parent = self.parent_override.unwrap_or_else(|| identity_of(&meta));
        let base = self.base.as_ref().map(|_| {
            self.base_inode
                .unwrap_or_else(|| identity_of(&meta.join("ids")))
        });
        let next_inode = self
            .next_inode
            .unwrap_or_else(|| identity_of(&meta.join("ids.publish.stage")));

        // The journal must exist before its own inode can be witnessed, so it
        // is created empty, witnessed, then filled — the order the claim
        // protocol itself uses.
        let path = meta.join("ids.pending");
        fs::write(&path, b"").expect("create the planted journal");
        let journal_inode = self.inode_override.unwrap_or_else(|| identity_of(&path));

        let header = RowHeader {
            parent,
            journal_inode,
            base,
            next_inode,
            base_bytes: self.base.clone().unwrap_or_default(),
            next_bytes: self.next.clone(),
        };
        let mut bytes =
            encode_header(JournalKind::Ids, &header.encode()).expect("a lawful kind-1 header");
        for (sequence, (tag, payload)) in self.records.iter().enumerate() {
            bytes.extend_from_slice(
                &encode_record(
                    JournalKind::Ids,
                    u32::try_from(sequence).expect("few records"),
                    *tag,
                    payload,
                )
                .expect("a lawful kind-1 record"),
            );
        }
        bytes.extend_from_slice(&self.tail);
        fs::write(&path, &bytes).expect("write the planted journal");
        // The journal owner requires the fixed mode on a durable claim.
        set_mode(&path, 0o600);
    }
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set fixture mode");
}

fn installing_record() -> Vec<u8> {
    encode_record(JournalKind::Ids, 1, 2, &[]).expect("a lawful record")
}

fn settled_record(terminal: u8) -> Vec<u8> {
    encode_record(JournalKind::Ids, 2, 3, &[terminal]).expect("a lawful record")
}

// ===== Fresh publication =====================================================

#[test]
fn a_publication_installs_the_successor_over_an_absent_artifact() {
    let _serial = serialized();
    let project = Project::new("absent");
    let plan = project.plan("Book", 1);
    let mut guard = project.guard();

    let outcome = guard.publish_ids(plan).expect("the publication runs");
    assert!(matches!(
        outcome,
        IdsPublishOutcome::Settled(IdsPublication::Published)
    ));
    assert!(
        project
            .read_meta("ids")
            .is_some_and(|bytes| !bytes.is_empty())
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
    assert!(!project.exists("ids.pending.create"));
    assert_eq!(ids_publication_marker(project.path()), None);
}

#[test]
fn a_publication_replaces_the_exact_captured_generation() {
    let _serial = serialized();
    let project = Project::new("replace");
    let first = project.plan("Book", 1);
    let mut guard = project.guard();
    let settled = guard
        .publish_ids(first)
        .expect("the first publication runs");
    assert!(matches!(
        settled,
        IdsPublishOutcome::Settled(IdsPublication::Published)
    ));
    // The outcome holds the write owner until it is dropped: a second
    // publication cannot begin while an unsettled one is still live.
    drop(settled);
    let base = project.read_meta("ids").expect("the first generation");

    let second = project.plan("Shelf", 2);
    let outcome = guard.publish_ids(second).expect("the publication runs");
    assert!(matches!(
        outcome,
        IdsPublishOutcome::Settled(IdsPublication::Published)
    ));
    let next = project.read_meta("ids").expect("the second generation");
    assert_ne!(base, next);
    assert!(next.len() > base.len(), "the successor carries both rows");
    assert!(!project.exists("ids.publish.stage"));
}

#[test]
fn a_plan_admitted_against_another_generation_settles_as_a_concurrent_change() {
    let _serial = serialized();
    let project = Project::new("stale");
    let stale = project.plan("Book", 1);
    // Another writer commits a generation between admission and publication.
    project.write_meta("ids", b"marrow-ids v1\nend\n");
    let before = project.read_meta("ids").expect("the other writer's bytes");

    let mut guard = project.guard();
    let outcome = guard.publish_ids(stale).expect("the publication runs");
    assert!(matches!(
        outcome,
        IdsPublishOutcome::Settled(IdsPublication::ConcurrentChange)
    ));
    assert_eq!(project.read_meta("ids"), Some(before));
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

#[test]
fn a_publication_refuses_while_another_is_claimed() {
    let _serial = serialized();
    let project = Project::new("interrupted");
    // The plan is admitted before the marker exists; a front door cannot
    // capture the project once one does.
    let plan = project.plan("Book", 1);
    project.write_meta("ids.publish.stage", b"next");
    Crash::new(&project, None, b"next").plant();

    let mut guard = project.guard();
    let refusal = guard
        .publish_ids(plan)
        .expect_err("a claimed publication refuses a fresh one");
    assert_eq!(refusal.refusal(), IdsRefusal::Interrupted);
    assert_eq!(refusal.code(), Code::ProjectIdsPublicationPending);
}

// ===== The closed map, driven through recovery ===============================

/// `Prepared absent`: the artifact is absent and the successor is staged.
#[test]
fn recovery_installs_from_prepared_absent() {
    let _serial = serialized();
    let project = Project::new("prepared-absent");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor").plant();

    assert_settled(&project, IdsPublication::Published);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

/// `Prepared replace`: the bound generation is committed and the successor is
/// staged.
#[test]
fn recovery_installs_from_prepared_replace() {
    let _serial = serialized();
    let project = Project::new("prepared-replace");
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, Some(b"base"), b"successor").plant();

    assert_settled(&project, IdsPublication::Published);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(!project.exists("ids.publish.stage"));
}

/// `Installing absent`, the previous-map side: the phase advanced but the link
/// had not landed.
#[test]
fn recovery_installs_from_installing_absent_before_the_link() {
    let _serial = serialized();
    let project = Project::new("installing-absent-before");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor")
        .installing()
        .plant();

    assert_settled(&project, IdsPublication::Published);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(!project.exists("ids.publish.stage"));
}

/// `Installing absent`, the linked side: one inode under both names.
#[test]
fn recovery_settles_from_installing_absent_after_the_link() {
    let _serial = serialized();
    let project = Project::new("installing-absent-after");
    project.write_meta("ids.publish.stage", b"successor");
    fs::hard_link(
        project.meta().join("ids.publish.stage"),
        project.meta().join("ids"),
    )
    .expect("link the staged successor into place");
    Crash::new(&project, None, b"successor")
        .installing()
        .plant();

    assert_settled(&project, IdsPublication::Published);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(!project.exists("ids.publish.stage"));
}

/// `Installing replace`, the exchanged side: the successor is committed and the
/// displaced generation waits at the stage name.
#[test]
fn recovery_settles_from_installing_replace_after_the_exchange() {
    let _serial = serialized();
    let project = Project::new("installing-replace-after");
    // Build the post-exchange map directly: the header binds the base inode,
    // which is the one now sitting at the stage name.
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    let planted = Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .witness_base("ids")
        .witness_next("ids.publish.stage");
    // Swap the two entries so the map reads `target=next; stage=base`, with the
    // header still witnessing each inode by identity.
    let meta = project.meta();
    fs::rename(meta.join("ids"), meta.join("ids.swap")).expect("stage the swap");
    fs::rename(meta.join("ids.publish.stage"), meta.join("ids")).expect("install the successor");
    fs::rename(meta.join("ids.swap"), meta.join("ids.publish.stage")).expect("displace the base");
    planted.plant();

    assert_settled(&project, IdsPublication::Published);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(!project.exists("ids.publish.stage"));
}

/// `Settled installed`, before the exact cleanup ran.
#[test]
fn recovery_finishes_a_settled_installation() {
    let _serial = serialized();
    let project = Project::new("settled-installed");
    project.write_meta("ids.publish.stage", b"successor");
    fs::hard_link(
        project.meta().join("ids.publish.stage"),
        project.meta().join("ids"),
    )
    .expect("link the staged successor into place");
    Crash::new(&project, None, b"successor")
        .installing()
        .settled(0)
        .plant();

    assert_settled(&project, IdsPublication::Published);
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

/// `Settled installed`, after the cleanup ran and before the marker unlink.
#[test]
fn recovery_finishes_a_cleaned_installation() {
    let _serial = serialized();
    let project = Project::new("settled-clean");
    project.write_meta("ids.publish.stage", b"successor");
    let planted = Crash::new(&project, None, b"successor")
        .installing()
        .settled(0)
        .witness_next("ids.publish.stage");
    let meta = project.meta();
    fs::rename(meta.join("ids.publish.stage"), meta.join("ids")).expect("run the exact cleanup");
    planted.plant();

    assert_settled(&project, IdsPublication::Published);
    assert!(!project.exists("ids.pending"));
}

/// The reverted terminal: the successor was never installed, the artifact is
/// the other writer's, and only the stage is cleaned.
#[test]
fn recovery_settles_a_reverted_publication_as_a_concurrent_change() {
    let _serial = serialized();
    let project = Project::new("settled-reverted");
    project.write_meta("ids", b"another writer's generation");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor")
        .installing()
        .settled(1)
        .plant();

    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"another writer's generation"[..]),
        "a reverted publication leaves the artifact exactly as it found it"
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

/// The ledger is a committed file, so an ordinary Git operation — a checkout, a
/// `stash pop`, a pull — can replace it with another branch's generation while a
/// publication is claimed. That is the writer the destination-refusing arms
/// exist for, and its replace-arm reading is reverted: the successor is not
/// installed and every byte of the artifact is the other writer's, including
/// where the plan bound a generation of its own.
#[test]
fn a_generation_a_checkout_landed_reverts_the_publication() {
    let _serial = serialized();
    let project = Project::new("checkout-generation");
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .witness_base("ids")
        .witness_next("ids.publish.stage")
        .plant();
    // A checkout replaces the tracked entry rather than rewriting it in place,
    // so the ledger is a fresh inode carrying neither the successor nor the
    // generation the header binds.
    fs::remove_file(project.meta().join("ids")).expect("the checkout replaces the entry");
    project.write_meta("ids", b"another branch's generation");

    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"another branch's generation"[..]),
        "the publication left the other writer's bytes exactly as it found them"
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

/// The same checkout, one record earlier. A publication interrupted before its
/// `Installing` record has mutated no artifact either, so the reverted reading
/// means exactly what it means in the `Installing` window and settles the same
/// way. The two windows classify one writer identically.
#[test]
fn a_generation_a_checkout_landed_before_the_installing_record_reverts_the_publication() {
    let _serial = serialized();
    let project = Project::new("checkout-generation-prepared");
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, Some(b"base"), b"successor")
        .witness_base("ids")
        .witness_next("ids.publish.stage")
        .plant();
    fs::remove_file(project.meta().join("ids")).expect("the checkout replaces the entry");
    project.write_meta("ids", b"another branch's generation");

    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"another branch's generation"[..]),
        "the publication left the other writer's bytes exactly as it found them"
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

/// The absent arm of the same window: the plan was admitted against no artifact
/// and one appeared before the `Installing` record. The destination is taken, so
/// the publication settles without installing rather than being retained.
#[test]
fn a_destination_taken_before_the_installing_record_reverts_the_publication() {
    let _serial = serialized();
    let project = Project::new("destination-taken-prepared");
    project.write_meta("ids", b"another writer's generation");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor").plant();

    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"another writer's generation"[..])
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

/// The window admits exactly two readings. A state only this publication's own
/// mutations could have produced — the successor already committed while no
/// `Installing` record exists — stays retained corruption, so the shared rule
/// widened the classification of an outside writer and nothing else.
#[test]
fn an_installed_successor_before_the_installing_record_is_retained() {
    let _serial = serialized();
    let project = Project::new("installed-before-installing");
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    let planted = Crash::new(&project, Some(b"base"), b"successor")
        .witness_base("ids")
        .witness_next("ids.publish.stage");
    let meta = project.meta();
    fs::rename(meta.join("ids"), meta.join("ids.swap")).expect("stage the swap");
    fs::rename(meta.join("ids.publish.stage"), meta.join("ids")).expect("install the successor");
    fs::rename(meta.join("ids.swap"), meta.join("ids.publish.stage"))
        .expect("land the displaced generation at the stage name");
    planted.plant();

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("an installed successor with no `Installing` record is retained corruption");
    assert_eq!(refusal.refusal(), IdsRefusal::Corrupt);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(project.exists("ids.pending"), "the marker keeps gating");
    assert_eq!(
        ids_publication_marker(project.path()),
        Some(IdsPublicationMarker::Claimed)
    );
}

/// A publication that reached `Installing` and found the destination taken
/// settles as a concurrent change without touching the artifact.
#[test]
fn recovery_reverts_when_the_destination_was_taken() {
    let _serial = serialized();
    let project = Project::new("destination-taken");
    project.write_meta("ids", b"another writer's generation");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor")
        .installing()
        .plant();

    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"another writer's generation"[..])
    );
    assert!(!project.exists("ids.publish.stage"));
}

// ===== Incomplete tails ======================================================

#[test]
fn an_incomplete_installing_record_is_truncated_to_the_unique_next_record() {
    let _serial = serialized();
    let project = Project::new("tail-installing");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor")
        .partial(installing_record(), 5)
        .plant();

    assert_settled(&project, IdsPublication::Published);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
}

/// The reverted side of the same derivation. The unique legal next record is
/// the terminal the artifact map names, so a crash part-way through the
/// terminal record of a publication that never installed resumes as reverted
/// rather than as an install.
#[test]
fn an_incomplete_terminal_record_over_a_reverted_map_resumes_as_reverted() {
    let _serial = serialized();
    let project = Project::new("tail-settled-reverted");
    project.write_meta("ids", b"another writer's generation");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor")
        .installing()
        .partial(settled_record(1), 6)
        .plant();

    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"another writer's generation"[..]),
        "a reverted publication leaves the artifact exactly as it found it"
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

#[test]
fn an_incomplete_terminal_record_is_truncated_against_the_artifact_map() {
    let _serial = serialized();
    let project = Project::new("tail-settled");
    project.write_meta("ids.publish.stage", b"successor");
    fs::hard_link(
        project.meta().join("ids.publish.stage"),
        project.meta().join("ids"),
    )
    .expect("link the staged successor into place");
    Crash::new(&project, None, b"successor")
        .installing()
        .partial(settled_record(0), 6)
        .plant();

    assert_settled(&project, IdsPublication::Published);
    assert!(!project.exists("ids.publish.stage"));
}

// ===== Retained states =======================================================

/// The post-death third inode: the successor is committed and something that is
/// neither the successor nor the bound generation holds the stage name.
#[test]
fn a_third_inode_beside_an_installed_successor_is_retained() {
    let _serial = serialized();
    let project = Project::new("third-inode");
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    let planted = Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .witness_base("ids")
        .witness_next("ids.publish.stage");
    let meta = project.meta();
    fs::rename(meta.join("ids"), meta.join("ids.swap")).expect("stage the swap");
    fs::rename(meta.join("ids.publish.stage"), meta.join("ids")).expect("install the successor");
    fs::remove_file(meta.join("ids.swap")).expect("drop the displaced generation");
    // A third inode, not the one the header bound, lands at the stage name.
    fs::write(meta.join("ids.publish.stage"), b"third").expect("plant a third inode");
    planted.plant();

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("a third inode is retained corruption");
    assert_eq!(refusal.refusal(), IdsRefusal::Corrupt);
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"third"[..]),
        "no exchange and no cleanup are authorized"
    );
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(project.exists("ids.pending"), "the marker keeps gating");
    assert_eq!(
        ids_publication_marker(project.path()),
        Some(IdsPublicationMarker::Claimed)
    );
}

/// The same third inode, arriving on the inode number the header bound.
///
/// An inode number is free for reuse the moment its last link goes, and ext4
/// and XFS hand it straight back to the next create in the same block group;
/// APFS draws from a counter that never repeats. The bound generation here is
/// unlinked and the third entry lands on its number, which the fixture spells
/// directly by witnessing the number off the entry that carries it — the state
/// a recycling filesystem produces on its own, reproduced on any filesystem.
/// It is the same state and the same observable outcome as the Linux failure,
/// not the same panic text: these kats name their own expectations.
/// The stage name then maps to the bound number under a run that is not the
/// bound one, and that is not the displaced generation coming back.
///
/// The foreign run is the bound run's own length, so the resolution that
/// separates them is the byte comparison and not the size it is filtered by.
#[test]
fn a_third_inode_on_the_bound_generation_s_recycled_number_is_retained() {
    let _serial = serialized();
    let project = Project::new("third-inode-recycled");
    project.write_meta("ids", b"successor");
    project.write_meta("ids.publish.stage", b"fake");
    Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .witness_base("ids.publish.stage")
        .witness_next("ids")
        .plant();

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("a recycled number under a foreign run is retained corruption");
    assert_eq!(refusal.refusal(), IdsRefusal::Corrupt);
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"fake"[..]),
        "the foreign entry on the recycled number is not cleaned away",
    );
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(project.exists("ids.pending"), "the marker keeps gating");
}

/// The checkout writer, arriving on the inode number the header bound. The
/// ledger is a committed file, so the entry a checkout replaces is unlinked and
/// the replacement can be handed the same number. The run it carries is not the
/// bound one, so the plan's generation is gone and the publication is reverted
/// exactly as it is when the number differs.
///
/// As above, the landed run is the bound run's own length, so the byte
/// comparison is what separates them.
#[test]
fn a_generation_on_the_bound_generation_s_recycled_number_reverts_the_publication() {
    let _serial = serialized();
    let project = Project::new("checkout-recycled");
    project.write_meta("ids", b"kept");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .witness_base("ids")
        .witness_next("ids.publish.stage")
        .plant();

    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"kept"[..]),
        "the publication left the other writer's bytes exactly as it found them"
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

/// The crash window inside the reverted cleanup. That cleanup is authorized by
/// a target naming neither bound run, so the state it leaves behind — terminal
/// recorded, stage swept, target still naming neither bound run — is a state
/// the protocol produces itself. It must finish, not retain: the terminal
/// record already says which way this publication went, and no mutation is
/// still owed. Nothing here is unlinked but the marker.
#[test]
fn a_reverted_terminal_over_a_drifted_target_finishes_after_its_own_cleanup() {
    let _serial = serialized();
    let project = Project::new("reverted-clean-drifted");
    project.write_meta("ids", b"kept");
    project.write_meta("ids.publish.stage", b"successor");
    let planted = Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .settled(1)
        .witness_base("ids")
        .witness_next("ids.publish.stage");
    fs::remove_file(project.meta().join("ids.publish.stage"))
        .expect("the reverted cleanup already ran");
    planted.plant();

    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"kept"[..]),
        "the other writer's bytes are left exactly as they were found"
    );
    assert!(!project.exists("ids.pending"), "the publication finished");
}

/// A removal never deletes what the durable record does not name, and an
/// interrupted removal's parked object is judged by that same proof rather than
/// inheriting it. A stranger under the quarantine name is therefore left
/// exactly where it is and the publication is retained for an operator, rather
/// than swept so the cleanup can proceed.
#[test]
fn a_stranger_parked_under_the_quarantine_name_is_neither_swept_nor_deleted() {
    let _serial = serialized();
    let project = Project::new("quarantine-stranger");
    project.write_meta("ids.publish.stage", b"successor");
    fs::hard_link(
        project.meta().join("ids.publish.stage"),
        project.meta().join("ids"),
    )
    .expect("link the staged successor into place");
    project.write_meta("ids.publish.quarantine", b"someone else's file");
    Crash::new(&project, None, b"successor")
        .installing()
        .settled(0)
        .plant();

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("a parked stranger retains the publication");
    assert_eq!(refusal.refusal(), IdsRefusal::Custody);
    assert_eq!(
        project.read_meta("ids.publish.quarantine").as_deref(),
        Some(&b"someone else's file"[..]),
        "the parked stranger is not deleted to clear the way"
    );
    assert!(
        project.exists("ids.publish.stage"),
        "the cleanup this removal came for did not run either"
    );
    assert!(project.exists("ids.pending"), "the marker keeps gating");
}

/// A crash between a removal's move and its unlink leaves the object in the
/// quarantine name with the stage name absent. The terminal record says
/// which run that cleanup was entitled to remove, and this object is it, so
/// reconciliation finishes the removal rather than putting the object back to
/// take it again.
#[test]
fn an_object_left_in_quarantine_by_an_interrupted_removal_is_finished_not_stranded() {
    let _serial = serialized();
    let project = Project::new("quarantine-mid-removal");
    project.write_meta("ids.publish.stage", b"successor");
    fs::hard_link(
        project.meta().join("ids.publish.stage"),
        project.meta().join("ids"),
    )
    .expect("link the staged successor into place");
    Crash::new(&project, None, b"successor")
        .installing()
        .settled(0)
        .plant();
    project.move_into_quarantine("ids.publish.stage");

    assert_settled(&project, IdsPublication::Published);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
    assert!(
        !project.quarantine().exists(),
        "the quarantine name is clear again"
    );
}

/// The retry path reconciles too. An accepted cleanup that moved its object
/// into quarantine and then failed to unlink it hands back a pending
/// publication; consuming that value drives the same classification a fresh
/// process would, so it must not read the quarantined object's absent name as
/// a cleanup already finished. Driven here through the fixed state that retry
/// sees, entered by the same production call the CLI makes.
#[test]
fn a_publication_retried_over_an_occupied_quarantine_does_not_read_the_cleanup_as_done() {
    let _serial = serialized();
    let project = Project::new("retry-occupied-quarantine");
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    let planted = Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .settled(0)
        .witness_base("ids")
        .witness_next("ids.publish.stage");
    let meta = project.meta();
    fs::rename(meta.join("ids"), meta.join("ids.swap")).expect("stage the swap");
    fs::rename(meta.join("ids.publish.stage"), meta.join("ids")).expect("install the successor");
    fs::rename(meta.join("ids.swap"), meta.join("ids.publish.stage"))
        .expect("displace the base to the stage name");
    planted.plant();
    // The accepted cleanup moved the displaced generation into quarantine and
    // did not get to unlink it.
    project.move_into_quarantine("ids.publish.stage");

    assert_settled(&project, IdsPublication::Published);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(
        !project.exists("ids.publish.stage"),
        "the displaced generation the cleanup came for is gone, not stranded"
    );
    assert!(!project.exists("ids.pending"));
    assert!(!project.quarantine().exists());
}

/// A terminal already decided is not re-opened by putting its cleanup's object
/// back. After a reverted terminal and a post-move crash, an outside writer can
/// leave the target reading as the bound generation; restoring the successor to
/// the stage name would manufacture a `Prepared` reading the terminal record
/// contradicts, and the publication would never settle. The object the cleanup
/// was entitled to remove is removed instead, which is what the terminal
/// already committed to.
#[test]
fn a_reverted_cleanup_interrupted_over_a_mutated_target_finishes_rather_than_wedging() {
    let _serial = serialized();
    let project = Project::new("reverted-cleanup-mutated-target");
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    let planted = Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .settled(1)
        .witness_base("ids")
        .witness_next("ids.publish.stage");
    planted.plant();
    // The reverted cleanup took the successor; the crash landed before its
    // unlink, and the target still reads as the bound generation.
    project.move_into_quarantine("ids.publish.stage");

    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"base"[..]),
        "a reverted publication leaves the artifact exactly as it found it"
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
    assert!(!project.quarantine().exists());
}

/// A publication attempted over an interrupted removal refuses without
/// touching it, and the recovery that follows still settles.
///
/// Only recovery has the header and terminal that make the quarantined object
/// judgeable. A publication that reconciled before classifying would put a
/// successor the reverted cleanup had already taken back under the stage name,
/// manufacture a reading the terminal contradicts, and leave the project in a
/// state no later command could settle. Refusing first costs one refusal and
/// keeps the state settleable.
#[test]
fn a_publication_over_an_interrupted_removal_refuses_and_leaves_it_settleable() {
    let _serial = serialized();
    let project = Project::new("publish-over-interrupted-removal");
    // Captured before the marker is planted: capture refuses while one is live.
    let plan = project.plan("Book", 1);
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .settled(1)
        .witness_base("ids")
        .witness_next("ids.publish.stage")
        .plant();
    project.move_into_quarantine("ids.publish.stage");

    let refusal = project
        .guard()
        .publish_ids(plan)
        .expect_err("a durably claimed project refuses a fresh publication");
    assert_eq!(refusal.refusal(), IdsRefusal::Interrupted);
    assert_eq!(
        fs::read(project.quarantine()).ok().as_deref(),
        Some(&b"successor"[..]),
        "the refused publication put nothing back and took nothing away"
    );
    assert!(!project.exists("ids.publish.stage"));

    // The state the refusal preserved is the one recovery settles.
    assert_settled(&project, IdsPublication::ConcurrentChange);
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"base"[..]));
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
    assert!(!project.quarantine().exists());
}

/// A fresh publication reconciles on its own entry. The state is reached with
/// no marker at all, so recovery's arms never run and only the publication
/// path's own reconciliation can find the stranded object.
#[test]
fn a_fresh_publication_reconciles_an_orphaned_quarantine_before_it_stages() {
    let _serial = serialized();
    let project = Project::new("publish-orphaned-quarantine");
    project.write_meta("ids.publish.stage", b"orphaned successor");
    project.move_into_quarantine("ids.publish.stage");
    let plan = project.plan("Book", 1);

    let refusal = project
        .guard()
        .publish_ids(plan)
        .expect_err("the restored transient is a retained manual state");
    assert_eq!(refusal.refusal(), IdsRefusal::UnclaimedIncomplete);
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"orphaned successor"[..]),
        "the orphan is put back under its own name, not deleted"
    );
    assert!(
        !project.quarantine().exists(),
        "the quarantine name is clear again"
    );
}

/// Reconciliation refuses rather than forces when the name it would restore to
/// has been taken. Both objects survive, the refusal is typed, and repeating
/// the command repeats it — and once the outside writer's entry is gone the
/// very same state completes.
#[test]
fn a_taken_stage_name_makes_reconciliation_refuse_without_losing_either_object() {
    let _serial = serialized();
    let project = Project::new("reconcile-collision");
    project.write_meta("ids", b"successor");
    project.write_meta("ids.publish.stage", b"someone else's file");
    Crash::new(&project, None, b"successor")
        .installing()
        .settled(0)
        .witness_next("ids")
        .plant();
    project.move_into_quarantine("ids.publish.stage");
    // An outside writer takes the name the quarantined object came from.
    project.write_meta("ids.publish.stage", b"a checkout's own entry");

    for attempt in 0..2 {
        let refusal = project
            .guard()
            .recover_ids()
            .expect_err("a taken destination refuses rather than overwriting");
        assert_eq!(
            refusal.refusal(),
            IdsRefusal::Custody,
            "attempt {attempt} refused with the wrong class"
        );
        assert_eq!(
            project.read_meta("ids.publish.stage").as_deref(),
            Some(&b"a checkout's own entry"[..]),
            "the outside writer's entry is never displaced"
        );
        assert_eq!(
            fs::read(project.quarantine()).ok().as_deref(),
            Some(&b"someone else's file"[..]),
            "the quarantined object is never deleted to clear the way"
        );
    }

    // The outside writer departs; the same state now completes.
    fs::remove_file(project.meta().join("ids.publish.stage")).expect("the writer departs");
    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("the restored stranger retains the publication");
    assert_eq!(refusal.refusal(), IdsRefusal::Corrupt);
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"someone else's file"[..]),
        "the quarantined object is restored once its name is free"
    );
    assert!(!project.quarantine().exists());
}

/// A crash after a removal rejected an object and before it moved that object
/// back leaves a stranger at the quarantine name. Reconciliation returns
/// it to the name it was taken from — it is not this protocol's object to
/// delete — and the map then reads the state that stranger creates and retains
/// the project.
#[test]
fn a_stranger_left_in_quarantine_by_a_rejected_removal_is_put_back_not_deleted() {
    let _serial = serialized();
    let project = Project::new("quarantine-rejected");
    project.write_meta("ids", b"successor");
    project.write_meta("ids.publish.stage", b"someone else's file");
    Crash::new(&project, None, b"successor")
        .installing()
        .settled(0)
        .witness_next("ids")
        .plant();
    project.move_into_quarantine("ids.publish.stage");

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("the restored stranger retains the publication");
    assert_eq!(refusal.refusal(), IdsRefusal::Corrupt);
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"someone else's file"[..]),
        "the stranger is put back under the name it was taken from, not deleted"
    );
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(project.exists("ids.pending"), "the marker keeps gating");
    assert!(
        !project.quarantine().exists(),
        "the quarantine name is clear again"
    );
}

#[test]
fn a_substituted_journal_inode_is_retained() {
    let _serial = serialized();
    let project = Project::new("substituted-inode");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor")
        .inode(FsIdentity::new(7, 7))
        .plant();

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("substituted evidence is retained corruption");
    assert_eq!(refusal.refusal(), IdsRefusal::Corrupt);
    assert!(!project.exists("ids"), "no artifact was written");
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"successor"[..])
    );
    assert!(project.exists("ids.pending"));
}

#[test]
fn a_foreign_parent_witness_is_retained() {
    let _serial = serialized();
    let project = Project::new("foreign-parent");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor")
        .parent(FsIdentity::new(9, 9))
        .plant();

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("a foreign parent witness is retained corruption");
    assert_eq!(refusal.refusal(), IdsRefusal::Corrupt);
    assert!(!project.exists("ids"));
    assert!(project.exists("ids.pending"));
}

#[test]
fn a_malformed_header_is_retained() {
    let _serial = serialized();
    let project = Project::new("malformed-header");
    project.write_meta("ids.publish.stage", b"successor");
    let mut bytes = encode_header(JournalKind::Ids, &[0u8; 40]).expect("a lawful frame header");
    bytes.extend_from_slice(&encode_record(JournalKind::Ids, 0, 1, &[]).expect("a lawful record"));
    project.write_meta("ids.pending", &bytes);
    set_mode(&project.meta().join("ids.pending"), 0o600);

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("a malformed header is retained corruption");
    assert_eq!(refusal.refusal(), IdsRefusal::Corrupt);
    assert_eq!(refusal.code(), Code::ProjectIdsPublicationPending);
    assert!(!project.exists("ids"));
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"successor"[..])
    );
}

#[test]
fn a_never_claimed_create_is_the_manual_unclaimed_state() {
    let _serial = serialized();
    let project = Project::new("unclaimed");
    project.write_meta("ids.publish.stage", b"successor");
    project.write_meta("ids.pending.create", b"partial");
    set_mode(&project.meta().join("ids.pending.create"), 0o600);

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("a never-claimed create is manual");
    assert_eq!(refusal.refusal(), IdsRefusal::UnclaimedIncomplete);
    assert_eq!(refusal.code(), Code::ProjectIdsPublicationPending);
    assert_eq!(
        project.read_meta("ids.pending.create").as_deref(),
        Some(&b"partial"[..]),
        "every byte is retained"
    );
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"successor"[..])
    );
}

#[test]
fn a_staged_successor_with_no_marker_is_the_manual_unclaimed_state() {
    let _serial = serialized();
    let project = Project::new("stage-only");
    project.write_meta("ids.publish.stage", b"successor");

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("a stage with no marker is manual");
    assert_eq!(refusal.refusal(), IdsRefusal::UnclaimedIncomplete);
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"successor"[..])
    );
    assert_eq!(
        ids_publication_marker(project.path()),
        None,
        "the artifact was never touched, so capture is not gated"
    );
}

#[test]
fn a_clear_project_recovers_nothing() {
    let _serial = serialized();
    let project = Project::new("clear");
    assert_eq!(project.guard().recover_ids().expect("recovery runs"), None);
}

// ===== The read-only front-door gate =========================================

#[test]
fn capture_refuses_while_a_claim_exists() {
    let project = Project::new("gate-claimed");
    project.write_meta("ids", b"marrow-ids v1\nend\n");
    project.write_meta("ids.pending", b"");

    assert_eq!(
        ids_publication_marker(project.path()),
        Some(IdsPublicationMarker::Claimed)
    );
    let failure = capture_project(project.path(), OverlaySnapshot::empty())
        .expect_err("capture refuses under a live claim");
    assert_eq!(
        failure.presentation(project.path()).code(),
        Code::ProjectIdsPublicationPending
    );
}

#[test]
fn capture_refuses_while_an_unclaimed_create_exists() {
    let project = Project::new("gate-unclaimed");
    project.write_meta("ids.pending.create", b"");

    assert_eq!(
        ids_publication_marker(project.path()),
        Some(IdsPublicationMarker::Unclaimed)
    );
    let failure = capture_project(project.path(), OverlaySnapshot::empty())
        .expect_err("capture refuses under an unclaimed create");
    assert_eq!(
        failure.presentation(project.path()).code(),
        Code::ProjectIdsPublicationPending
    );
}

#[test]
fn capture_is_unaffected_by_a_staged_successor_alone() {
    let project = Project::new("gate-stage-only");
    project.write_meta("ids.publish.stage", b"successor");
    assert!(capture_project(project.path(), OverlaySnapshot::empty()).is_ok());
}

/// The probe spells the two marker names itself so it can answer without
/// admitting a directory. This pins those spellings against the journal owner's
/// own derivation, so the two cannot drift apart.
#[test]
fn the_probe_spells_the_journal_owner_s_marker_names() {
    let derived = PendingName::derive(&name("ids")).expect("the fixed base name derives");
    let project = Project::new("probe-names");

    project.write_meta(derived.pending().as_str(), b"");
    assert_eq!(
        ids_publication_marker(project.path()),
        Some(IdsPublicationMarker::Claimed)
    );
    fs::remove_file(project.meta().join(derived.pending().as_str())).expect("remove the marker");

    project.write_meta(derived.claim().as_str(), b"");
    assert_eq!(
        ids_publication_marker(project.path()),
        Some(IdsPublicationMarker::Unclaimed)
    );
}

// ===== Absence gates =========================================================

/// No broad temporary sweep exists: the owner names its entries and never
/// enumerates the directory it publishes into.
#[test]
fn the_publication_owner_enumerates_no_directory() {
    for source in [
        include_str!("publication.rs"),
        include_str!("publication/ignore.rs"),
        include_str!("publication/protocol.rs"),
        include_str!("publication/header.rs"),
        include_str!("publication/marker.rs"),
        // The journal owner is under the same ban for its own sake: nothing in
        // this protocol's namespace is enumerated anywhere. Classification's
        // independence from artifacts is not this gate's subject —
        // `MarkerStats` carries that decision by value, so a classification has
        // no directory to enumerate in the first place.
        include_str!("../../marrow-fs-journal/src/journal.rs"),
    ] {
        for forbidden in ["read_dir", "ReadDir", "glob", "remove_dir_all"] {
            assert!(
                !source.contains(forbidden),
                "the publication owner must not reach for `{forbidden}`"
            );
        }
    }
}

/// The claim's preclaim arm cannot contain a link attempt, and cannot run
/// caller code that could contain one.
///
/// Which arm a claim refusal names is decided by which function the refusal
/// came from, so the boundary is only as good as the split: a step that moves
/// from the commit into the preflight would silently start reporting
/// possibly-durable states as clean, and the publication owner would clean up
/// under a marker that may exist. The preflight's body is therefore required to
/// spell no link, and the commit is required to spell one.
///
/// Scanning the body alone is not enough, because a callback invoked inside the
/// preflight would be a link the body does not spell: a caller that captured
/// the same admitted directory could link the marker itself, and every refusal
/// after that would still be reported as leaving no marker. The header is a
/// value, built before the claim from what the plan already knows, so no caller
/// code runs inside the preflight at all — that escape is unrepresentable
/// rather than refused, and there is no runtime state left to test. What this
/// gate keeps is the shape that makes it so: the preflight's parameters carry
/// no callable.
#[test]
fn the_claim_s_preclaim_arm_reaches_no_link() {
    let source = include_str!("../../marrow-fs-journal/src/journal.rs");
    let body = |name: &str| -> &str {
        let start = source
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("the claim family still defines `{name}`"));
        let rest = &source[start..];
        let open = rest.find('{').expect("the function has a body");
        let mut depth = 0usize;
        for (offset, byte) in rest[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &rest[open..=open + offset];
                    }
                }
                _ => {}
            }
        }
        panic!("`{name}` has no closing brace");
    };

    assert!(
        !body("claim_preflight").contains(".link("),
        "the preclaim arm attempted a link; every refusal it returns claims to leave no \
         marker, which a link attempt makes false"
    );
    assert!(
        body("claim_commit").contains(".link("),
        "the commit arm no longer performs the link, so the boundary this gate checks is \
         somewhere else and the arms no longer mean what they say"
    );

    // The parameter list, not the body: a callable parameter is caller code
    // running inside the preflight, and no scan of the body can see what it
    // does.
    let signature = {
        let start = source
            .find("fn claim_preflight(")
            .expect("the claim family still defines `claim_preflight`");
        let rest = &source[start..];
        &rest[..rest.find(')').expect("the parameter list closes")]
    };
    for callable in [
        "impl Fn",
        "impl FnOnce",
        "impl FnMut",
        "dyn Fn",
        "fn(",
        "F:",
    ] {
        assert!(
            !signature.contains(callable),
            "`claim_preflight` takes `{callable}`, so caller code runs between the claim \
             file's creation and the link; a callback that links the marker itself would \
             make every refusal below it wrongly report that none exists"
        );
    }
}

/// An unclassifiable map under an incomplete terminal record is a typed
/// refusal, not a panic. Deriving the unique next record reads the artifact map
/// with no live journal, so a reader that reached for one would fail here
/// instead of retaining the state.
#[test]
fn an_off_map_state_under_an_incomplete_tail_refuses_without_panicking() {
    let _serial = serialized();
    let project = Project::new("off-map-tail");
    project.write_meta("ids.publish.stage", b"successor");
    let planted = Crash::new(&project, None, b"successor")
        .installing()
        .partial(settled_record(0), 6)
        .witness_next("ids.publish.stage");
    // Neither the artifact nor the stage is where any phase could have left it.
    fs::remove_file(project.meta().join("ids.publish.stage")).expect("remove the stage");
    planted.plant();

    let refusal = project
        .guard()
        .recover_ids()
        .expect_err("an unclassifiable map is retained corruption");
    assert_eq!(refusal.refusal(), IdsRefusal::Corrupt);
    assert!(!project.exists("ids"), "no artifact was written");
    assert!(project.exists("ids.pending"), "the marker keeps gating");
}

// ===== The write owner's exclusion ===========================================

/// The write lock is what serializes publication, so a second owner on one
/// project reports the contention it is actually in.
#[test]
fn a_second_write_owner_on_one_project_reports_contention() {
    let _serial = serialized();
    let project = Project::new("contended");
    let held = project.guard();

    let refusal = ProjectMetadataWriteGuard::acquire(project.path())
        .expect_err("a second write owner refuses while the first holds the lock");
    assert_eq!(refusal.refusal(), IdsRefusal::Contended);
    assert_eq!(refusal.code(), Code::IoWrite);
    assert!(
        refusal
            .to_string()
            .contains("holds the project-metadata write lock"),
        "the refusal names the contention: {refusal}"
    );

    drop(held);
    ProjectMetadataWriteGuard::acquire(project.path())
        .expect("the released lock is reacquired by the next owner");
}

/// A clone carries no lock, so a first publication races an absent `.marrow`
/// and an absent lock entry. The rendezvous is created rather than owned: at
/// most one guard is ever live, every loser reports the contention rather than
/// a filesystem refusal for the directory the winner created, and exactly one
/// publication installs its successor.
#[test]
fn concurrent_first_publications_serialize_on_the_write_lock() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Barrier, Mutex};

    let _serial = serialized();
    const THREADS: usize = 4;
    const ROUNDS: usize = 24;

    for round in 0..ROUNDS {
        let project = Project::new(&format!("first-publish-race-{round}"));
        assert!(!project.meta().exists(), "the round starts from a clone");
        let plans: Vec<LedgerPublicationPlan> = (0..THREADS)
            .map(|seat| project.plan("Book", 1 + u8::try_from(seat).expect("few seats")))
            .collect();

        let live = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let published = AtomicUsize::new(0);
        let refusals: Mutex<Vec<(IdsRefusal, String)>> = Mutex::new(Vec::new());
        let start = Barrier::new(THREADS);

        std::thread::scope(|scope| {
            for plan in plans {
                scope.spawn(|| {
                    start.wait();
                    match ProjectMetadataWriteGuard::acquire(project.path()) {
                        Ok(mut guard) => {
                            peak.fetch_max(
                                live.fetch_add(1, Ordering::SeqCst) + 1,
                                Ordering::SeqCst,
                            );
                            if let Ok(IdsPublishOutcome::Settled(IdsPublication::Published)) =
                                guard.publish_ids(plan)
                            {
                                published.fetch_add(1, Ordering::SeqCst);
                            }
                            live.fetch_sub(1, Ordering::SeqCst);
                        }
                        Err(refusal) => refusals
                            .lock()
                            .expect("collect")
                            .push((refusal.refusal(), refusal.to_string())),
                    }
                });
            }
        });

        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "two write owners were live at once in round {round}"
        );
        assert_eq!(
            published.load(Ordering::SeqCst),
            1,
            "round {round} installed a successor {} times",
            published.load(Ordering::SeqCst)
        );
        for (refusal, message) in refusals.lock().expect("read the refusals").iter() {
            assert_eq!(
                *refusal,
                IdsRefusal::Contended,
                "a loser of the first-publication race reported {refusal:?} rather than \
                 the contention it is in (round {round}): {message}"
            );
        }
        assert_eq!(
            project.read_meta("publish.lock").as_deref(),
            Some(&b""[..]),
            "the race leaves exactly one zero-byte lock entry"
        );
        assert_eq!(
            project.read_meta(".gitignore").as_deref(),
            Some(WRITTEN_IGNORE),
            "the race leaves exactly one written ignore entry in round {round}"
        );
        assert!(!project.exists("ids.publish.stage"));
        assert!(!project.exists("ids.pending"));
    }
}

// ===== The lock's own ignore entry ===========================================
//
// The lock is machine-local runtime state, and a lock that travelled with a
// checkout would exclude nobody: an ordinary Git operation deletes and
// recreates a tracked entry, replacing the inode a holder is excluding on. The
// entry that keeps it untracked is written by the owner that creates the lock,
// so a project carries no hand-written line and a fresh clone is correct
// without one.

/// The exact artifact the write owner keeps beside the lock. The front-door
/// metadata snapshot in `crates/marrow/tests/durable_identity.rs` pins the same
/// bytes, so a change here is a change a developer's checkout would see.
const WRITTEN_IGNORE: &[u8] = b"\
# Machine-written by Marrow. The cooperative write lock is machine-local runtime\n\
# state, and the other entries are a publication in flight or the debris an\n\
# interrupted one left. No checkout carries any of them; only `ids` is committed.\n\
publish.lock\n\
ids.publish.stage\n\
ids.publish.quarantine\n\
ids.pending\n\
ids.pending.create\n";

/// The exact entry an earlier build of this owner wrote: its header line above
/// the lock's name alone. A project that published before the four transient
/// names joined the block carries this on disk.
const PREVIOUS_FORMAT_IGNORE: &[u8] = b"\
# Machine-written by Marrow. The cooperative project-metadata write lock is\n\
# machine-local runtime state that no checkout carries.\n\
publish.lock\n";

/// Exactly what the previous format becomes: the four names it lacks, appended
/// under the header already there. A second header above a stale first block
/// would leave a developer's checkout carrying two Marrow-written comments, the
/// first of them describing a name set the file no longer has.
const UPGRADED_IGNORE: &[u8] = b"\
# Machine-written by Marrow. The cooperative project-metadata write lock is\n\
# machine-local runtime state that no checkout carries.\n\
publish.lock\n\
ids.publish.stage\n\
ids.publish.quarantine\n\
ids.pending\n\
ids.pending.create\n";

/// An entry this owner wrote under an earlier name set is completed in place:
/// the names it lacks are appended, and the comment it already carries is not
/// written a second time. The bytes are pinned exactly, and a second
/// acquisition adds nothing to them.
///
/// This is also what holds the comment's stable opening in place. The owner
/// tells its own entry from a developer's file by that opening alone, so a
/// reword that reached it would stop recognizing every entry written before the
/// reword — and this kat, whose planted entry carries the earlier wording, is
/// where that shows up.
#[test]
fn an_entry_written_under_an_earlier_name_set_gains_only_the_missing_names() {
    let _serial = serialized();
    let project = Project::new("ignore-upgrade");
    project.write_meta(".gitignore", PREVIOUS_FORMAT_IGNORE);

    drop(project.guard());
    assert_ignore_entry(&project, UPGRADED_IGNORE, "completing the previous format");

    drop(project.guard());
    assert_ignore_entry(&project, UPGRADED_IGNORE, "a second acquisition");
}

/// Assert the ignore entry's exact bytes, rendering both sides as the text they
/// are so a mismatch reads as the entry a developer's checkout would carry.
fn assert_ignore_entry(project: &Project, expected: &[u8], what: &str) {
    let found = project.read_meta(".gitignore").unwrap_or_default();
    assert_eq!(
        found,
        expected,
        "{what} left\n{}\nrather than\n{}",
        String::from_utf8_lossy(&found),
        String::from_utf8_lossy(expected)
    );
}

/// The first acquisition writes the entry; every later one leaves it byte for
/// byte alone, because the owner appends only what is missing.
#[test]
fn the_write_owner_writes_the_ignore_entry_once() {
    let _serial = serialized();
    let project = Project::new("ignore-once");
    assert!(!project.meta().exists(), "the project starts from a clone");

    for acquisition in 0..3 {
        let guard = project.guard();
        assert_eq!(
            project.read_meta(".gitignore").as_deref(),
            Some(WRITTEN_IGNORE),
            "acquisition {acquisition} did not leave the exact ignore entry"
        );
        drop(guard);
    }
}

/// The entry is completed rather than rewritten. The empty file a crash between
/// the create and the fill leaves is finished, a line this owner did not write
/// is kept, and neither grows a second copy.
#[test]
fn the_ignore_entry_is_completed_rather_than_rewritten() {
    let _serial = serialized();
    let crashed = Project::new("ignore-crashed");
    crashed.write_meta(".gitignore", b"");
    drop(crashed.guard());
    assert_eq!(
        crashed.read_meta(".gitignore").as_deref(),
        Some(WRITTEN_IGNORE),
        "the empty entry a crash leaves is finished by the next acquisition"
    );

    let edited = Project::new("ignore-edited");
    edited.write_meta(".gitignore", b"notes");
    drop(edited.guard());
    let mut expected = b"notes\n".to_vec();
    expected.extend_from_slice(WRITTEN_IGNORE);
    assert_eq!(
        edited.read_meta(".gitignore"),
        Some(expected.clone()),
        "an unterminated foreign line keeps its own line"
    );
    drop(edited.guard());
    assert_eq!(
        edited.read_meta(".gitignore"),
        Some(expected),
        "a second acquisition adds nothing to an entry that already names the lock"
    );
}

/// The written block names every entry the protocol can leave beside the
/// committed ledger, not the lock alone. The expectation is read from the
/// module's own fixed entry-name table and cross-checked against the pure
/// owner's ledger spelling, this adapter's stage spelling, and the journal
/// owner's derived marker names, so a protocol that grows an eighth name fails
/// here until the block covers it.
///
/// The uncovered names are not theoretical debris: crash debris under a
/// transient name that no ignore entry covers is offered by `git add -A`,
/// committed, and then recreated by every checkout of the result — and a
/// committed `ids.pending` makes every read-only front door refuse on every
/// clone, because a marker is exactly what says the ledger is indeterminate.
#[test]
fn the_written_ignore_names_every_transient_the_protocol_can_leave() {
    let _serial = serialized();
    let documented = documented_entry_names();
    assert_eq!(
        documented.len(),
        7,
        "the module documents {documented:?}; the table is the source of this expectation"
    );

    let ledger = marrow_project::IDS_ENTRY;
    let derived = PendingName::derive(&name(ledger)).expect("the fixed base name derives");
    for spelling in [
        ledger,
        crate::publication::stage_spelling(),
        crate::publication::quarantine_spelling(),
        derived.pending().as_str(),
        derived.claim().as_str(),
        ".gitignore",
    ] {
        assert!(
            documented.iter().any(|entry| entry == spelling),
            "the fixed entry-name table does not document `{spelling}`, so it cannot be the \
             list this kat reads: {documented:?}"
        );
    }

    let project = Project::new("ignore-covers-transients");
    drop(project.guard());
    let written = project.read_meta(".gitignore").expect("the ignore entry");

    for entry in documented {
        // The ledger is the one committed artifact, and the ignore entry
        // carries itself into the checkout that needs it.
        if entry == ledger || entry == ".gitignore" {
            continue;
        }
        assert!(
            written
                .split(|byte| *byte == b'\n')
                .any(|line| line == entry.as_bytes()),
            "the written ignore entry does not name `{entry}`, which the protocol can leave \
             in `.marrow`: {}",
            String::from_utf8_lossy(&written)
        );
    }
}

/// The fixed entry names the publication module documents, read from the first
/// fenced block of its own module documentation.
fn documented_entry_names() -> Vec<String> {
    let source = include_str!("publication.rs");
    let (_, rest) = source
        .split_once("//! ```text\n")
        .expect("the fixed entry-name table opens the module documentation");
    let (table, _) = rest
        .split_once("//! ```")
        .expect("the fixed entry-name table closes");
    table
        .lines()
        .filter_map(|line| {
            line.trim_start()
                .strip_prefix("//!")
                .and_then(|body| body.split_whitespace().next())
                .map(str::to_owned)
        })
        .collect()
}

/// A name a developer's entry already carries is recognized in the spellings
/// version control actually produces: a CRLF checkout leaves each line with a
/// trailing carriage return, and a leading `/` anchors a pattern to the ignore
/// file's own directory, which is exactly where these entries are. Both name
/// the same entry, so neither may earn a second copy — an entry that grew one
/// on every checkout style would grow one forever.
#[test]
fn a_carriage_return_or_anchored_spelling_earns_no_second_copy() {
    let _serial = serialized();
    for (tag, render) in [
        (
            "crlf",
            (|entry: &str| format!("{entry}\r\n")) as fn(&str) -> String,
        ),
        ("anchored", |entry: &str| format!("/{entry}\n")),
        ("anchored-crlf", |entry: &str| format!("/{entry}\r\n")),
    ] {
        let project = Project::new(&format!("ignore-{tag}"));
        let planted: String = String::from_utf8_lossy(WRITTEN_IGNORE)
            .lines()
            .map(|line| {
                if line.starts_with('#') {
                    format!("{line}\n")
                } else {
                    render(line)
                }
            })
            .collect();
        project.write_meta(".gitignore", planted.as_bytes());

        drop(project.guard());

        assert_eq!(
            project.read_meta(".gitignore"),
            Some(planted.into_bytes()),
            "the {tag} spelling of every name earned a second copy of at least one of them"
        );
    }
}

/// Whether the ignore entry already carries every name is a read-only
/// question, so the owner asks it read-only. A developer's checkout can carry
/// that entry unwritable, and an owner that opened it for writing to decide a
/// question needing no write would refuse every publication — and every
/// recovery, which takes the same guard — of a project whose ignore entry is
/// already complete.
#[test]
fn a_complete_unwritable_ignore_entry_leaves_the_owner_working() {
    let _serial = serialized();
    let project = Project::new("ignore-unwritable");
    project.write_meta(".gitignore", WRITTEN_IGNORE);
    let path = project.meta().join(".gitignore");
    withhold_write(&path);

    let mut guard = ProjectMetadataWriteGuard::acquire(project.path())
        .expect("a complete ignore entry needs no write, so acquisition must not demand one");
    guard
        .recover_ids()
        .expect("recovery takes the same guard and must reach the same conclusion");
    drop(guard);

    assert_eq!(
        project.read_meta(".gitignore").as_deref(),
        Some(WRITTEN_IGNORE),
        "the read-only decision wrote to the entry it only had to read"
    );
    set_mode(&path, 0o600);
}

/// Withhold write access to `path`, failing loudly when the mode binds nothing.
///
/// A withheld write that binds nothing would leave its kat asserting an
/// acquisition no unwritable entry was ever in the way of. Mode bits do not
/// bind a process holding the mode-override capability (`root`, or
/// `CAP_DAC_OVERRIDE` on Linux), and a filesystem that carries no mode bits does
/// not enforce them at all.
fn withhold_write(path: &Path) {
    set_mode(path, 0o444);
    let binds = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .is_err();
    assert!(
        binds,
        "mode 0444 on {} did not refuse a read-write open, so this kat never ran. Run the \
         suite as a process the mode bits bind, on a filesystem that carries them.",
        path.display()
    );
}

/// Withhold read access to `path` with `mode`, failing loudly when the mode
/// binds nothing.
///
/// The same vacuity this file's write-withholding helper guards against applies
/// here: an unreadable entry that binds nothing would leave its kat asserting an
/// acquisition nothing was ever in the way of.
fn withhold_read(path: &Path, mode: u32) {
    set_mode(path, mode);
    let binds = fs::OpenOptions::new().read(true).open(path).is_err();
    assert!(
        binds,
        "mode {mode:04o} on {} did not refuse a read-only open, so this kat never ran. Run the \
         suite as a process the mode bits bind, on a filesystem that carries them.",
        path.display()
    );
}

/// An ignore entry this owner cannot read refuses the acquisition.
///
/// Every removal this protocol performs rests on its transients never being
/// tracked, and this entry is what establishes that in a project no index gate
/// of ours can see. An entry whose contents cannot be read cannot be shown to
/// name them, so the contract is not established and nothing is staged or
/// claimed. Proceeding would publish transients an ordinary `git add -A`
/// offers and a later checkout writes — precisely the writer the protocol
/// claims not to have.
///
/// The entry is left exactly as found: the refusal is a refusal to proceed,
/// not an attempt to repair.
#[test]
fn an_unreadable_ignore_entry_refuses_the_acquisition() {
    let _serial = serialized();
    for mode in [0o000, 0o200] {
        let project = Project::new(&format!("ignore-unreadable-{mode:04o}"));
        project.write_meta(".gitignore", PREVIOUS_FORMAT_IGNORE);
        let path = project.meta().join(".gitignore");
        withhold_read(&path, mode);

        let refusal = ProjectMetadataWriteGuard::acquire(project.path()).expect_err(
            "an ignore entry that cannot be read leaves the untracked contract unestablished",
        );
        assert_eq!(refusal.refusal(), IdsRefusal::UntrackedContract);

        set_mode(&path, 0o600);
        assert_eq!(
            project.read_meta(".gitignore").as_deref(),
            Some(PREVIOUS_FORMAT_IGNORE),
            "the entry the owner could not read at mode {mode:04o} is not the entry it left"
        );
    }
}

/// An ignore entry past the size this owner reads refuses the acquisition, for
/// the same reason an unreadable one does: the part that would show it names
/// the transients was never read, so the contract is unestablished rather than
/// established-or-not-known.
#[test]
fn an_oversized_ignore_entry_refuses_the_acquisition() {
    let _serial = serialized();
    let project = Project::new("ignore-oversized");
    let mut oversized = b"# a developer's own very long ignore file\n".to_vec();
    oversized.resize(64 * 1024, b'#');
    oversized.push(b'\n');
    project.write_meta(".gitignore", &oversized);

    let refusal = ProjectMetadataWriteGuard::acquire(project.path())
        .expect_err("an ignore entry past the read bound leaves the contract unestablished");
    assert_eq!(refusal.refusal(), IdsRefusal::UntrackedContract);
    assert_eq!(
        project.read_meta(".gitignore").as_deref(),
        Some(&oversized[..]),
        "the entry the owner would not read is not the entry it left"
    );
}

/// An ignore entry that negates one of this owner's names refuses the
/// acquisition, however complete the rest of it is and however the negation is
/// spelled.
///
/// Git takes the last matching line, so a `!` line re-including a transient
/// leaves it tracked no matter how many positive lines precede it. A negation
/// need not spell the name to reach it, so each shape that can is refused:
/// the exact name, the anchored name, a suffix wildcard, a prefix wildcard,
/// bare `*`, a single-character wildcard, and a character set. Appending the
/// name again would change nothing, and this owner does not rewrite a line a
/// developer wrote.
#[test]
fn an_ignore_entry_negating_a_transient_refuses_the_acquisition() {
    let _serial = serialized();
    for (tag, spelling) in [
        ("exact", "!ids.publish.stage\n"),
        ("anchored", "!/ids.publish.quarantine\n"),
        ("prefix-glob", "!ids.publish.*\n"),
        ("suffix-glob", "!*.stage\n"),
        ("everything", "!*\n"),
        ("single-char", "!ids.pendin?\n"),
        ("char-set", "!ids.publish.[sq]*\n"),
        ("trailing-slash", "!ids.publish.stage/\n"),
        // `**` matches zero directories, so both of these name the entry
        // sitting directly in `.marrow`.
        ("double-star-exact", "!**/ids.publish.stage\n"),
        ("double-star-glob", "!**/*.stage\n"),
        // A POSIX class inside a set: the members are not read, and any set
        // carrying one is taken as reaching the name.
        ("posix-class", "!ids.publish.[[:alpha:]]*\n"),
        ("negated-posix-class", "!ids.publish.[![:digit:]]*\n"),
    ] {
        let project = Project::new(&format!("ignore-negated-{tag}"));
        let mut planted = WRITTEN_IGNORE.to_vec();
        planted.extend_from_slice(spelling.as_bytes());
        project.write_meta(".gitignore", &planted);

        let refusal = ProjectMetadataWriteGuard::acquire(project.path())
            .err()
            .unwrap_or_else(|| {
                panic!("the {tag} negation re-includes a transient and must refuse")
            });
        assert_eq!(
            refusal.refusal(),
            IdsRefusal::UntrackedContract,
            "the {tag} negation refused with the wrong class"
        );
        assert_eq!(
            project.read_meta(".gitignore").as_deref(),
            Some(&planted[..]),
            "the developer's own line is not rewritten away"
        );
    }
}

/// A negation that cannot reach one of this owner's names does not refuse.
///
/// The check exists to catch a transient being re-included, not to make any
/// developer's ignore file unusable. A pattern naming another suffix, another
/// directory, or a path below this one reaches nothing here, and a project
/// carrying one publishes exactly as it did before.
#[test]
fn an_ignore_entry_negating_something_else_leaves_the_owner_working() {
    let _serial = serialized();
    for (tag, spelling) in [
        ("other-suffix", "!*.md\n"),
        ("subdirectory", "!docs/*.md\n"),
        ("anchored-elsewhere", "!/build\n"),
        ("below-this-directory", "!ids.publish.stage/inner\n"),
        ("other-name", "!notes.txt\n"),
        // A real component survives the `**`, so the pattern still needs a
        // subdirectory these entries never sit in.
        ("double-star-subdirectory", "!**/docs/notes.md\n"),
        ("double-star-other-suffix", "!**/*.md\n"),
    ] {
        let project = Project::new(&format!("ignore-negation-elsewhere-{tag}"));
        let mut planted = WRITTEN_IGNORE.to_vec();
        planted.extend_from_slice(spelling.as_bytes());
        project.write_meta(".gitignore", &planted);

        let mut guard =
            ProjectMetadataWriteGuard::acquire(project.path()).unwrap_or_else(|error| {
                panic!("the {tag} negation reaches no transient and must not refuse: {error:?}")
            });
        guard
            .recover_ids()
            .expect("recovery takes the same guard and reaches the same conclusion");
        drop(guard);
        assert_eq!(
            project.read_meta(".gitignore").as_deref(),
            Some(&planted[..]),
            "the {tag} entry already names every transient, so nothing is appended"
        );
    }
}

/// An incomplete ignore entry this owner cannot write refuses the acquisition.
///
/// The names are missing, so the entry does not keep this project's transients
/// untracked, and an entry that cannot be written cannot be completed. That is
/// the same unestablished contract an unreadable or oversized entry leaves, and
/// it gets the same answer. An entry that already names every transient is a
/// different case entirely: nothing needs writing, and a read-only complete
/// entry keeps working.
#[test]
fn an_incomplete_unwritable_ignore_entry_refuses_the_acquisition() {
    let _serial = serialized();
    let project = Project::new("ignore-unwritable-incomplete");
    project.write_meta(".gitignore", PREVIOUS_FORMAT_IGNORE);
    let path = project.meta().join(".gitignore");
    withhold_write(&path);

    let refusal = ProjectMetadataWriteGuard::acquire(project.path())
        .expect_err("an entry that cannot be completed leaves the contract unestablished");
    assert_eq!(refusal.refusal(), IdsRefusal::UntrackedContract);

    set_mode(&path, 0o600);
    assert_eq!(
        project.read_meta(".gitignore").as_deref(),
        Some(PREVIOUS_FORMAT_IGNORE),
        "the entry the owner could not complete is not the entry it left"
    );
}

/// Only a withheld write is read as a convenience the owner leaves alone. A
/// node kind this owner never wrote is a corrupted metadata directory, not a
/// permissions case, so it stays the typed refusal it already was — including
/// the FIFO, which an owner that blocked on classifying it would hang on under
/// the write lock rather than refuse.
#[test]
fn a_non_regular_ignore_entry_is_still_refused() {
    let _serial = serialized();
    for kind in ["directory", "fifo"] {
        let project = Project::new(&format!("ignore-{kind}"));
        fs::create_dir_all(project.meta()).expect("create the metadata directory");
        let path = project.meta().join(".gitignore");
        match kind {
            "directory" => fs::create_dir(&path).expect("plant the directory"),
            _ => {
                let planted = std::process::Command::new("mkfifo")
                    .arg(&path)
                    .status()
                    .expect("run mkfifo");
                assert!(planted.success(), "mkfifo planted the non-regular node");
            }
        }

        let refusal = ProjectMetadataWriteGuard::acquire(project.path())
            .err()
            .unwrap_or_else(|| panic!("a {kind} at the ignore entry's name was admitted"));
        assert_eq!(
            refusal.refusal(),
            IdsRefusal::Custody,
            "a {kind} at the ignore entry's name reported {refusal:?}"
        );
        assert_eq!(
            refused_operation(&refusal),
            "open file",
            "a {kind} at the ignore entry's name was refused by another operation"
        );
    }
}

/// Acquisitions racing one fresh project write one ignore entry between them.
/// The entry is installed under the write lock, so however the seats interleave
/// exactly one of them appends and the rest find the lock already named.
#[test]
fn concurrent_acquisitions_write_one_ignore_entry() {
    use std::sync::Barrier;

    let _serial = serialized();
    const THREADS: usize = 8;
    const ROUNDS: usize = 12;
    /// Each seat needs one uncontended acquisition; the bound turns a livelock
    /// into a failure rather than a hung suite. A seat that yields between
    /// attempts retries about once per seat ahead of it, so the bound sits far
    /// above what contention costs and nothing here measures anything: on the
    /// run this was written against the worst seat reached attempt 8 with the
    /// yield and 10 without, on a machine and scheduler neither number is a
    /// property of.
    const ATTEMPTS: usize = 10_000;

    for round in 0..ROUNDS {
        let project = Project::new(&format!("ignore-race-{round}"));
        assert!(!project.meta().exists(), "the round starts from a clone");
        let barrier = Barrier::new(THREADS);
        let start = &barrier;
        let root = project.path();

        std::thread::scope(|scope| {
            for seat in 0..THREADS {
                scope.spawn(move || {
                    start.wait();
                    for attempt in 0..ATTEMPTS {
                        match ProjectMetadataWriteGuard::acquire(root) {
                            Ok(guard) => {
                                drop(guard);
                                return;
                            }
                            Err(refusal) => {
                                assert_eq!(
                                    refusal.refusal(),
                                    IdsRefusal::Contended,
                                    "seat {seat} attempt {attempt} of round {round} reported \
                                     {refusal:?} rather than the contention it is in"
                                );
                                // The lock is uncontended for a few syscalls at
                                // a time, so a seat that spun would burn its
                                // whole slice racing the holder it is waiting
                                // for.
                                std::thread::yield_now();
                            }
                        }
                    }
                    panic!("seat {seat} of round {round} never acquired the write lock");
                });
            }
        });

        assert_eq!(
            project.read_meta(".gitignore").as_deref(),
            Some(WRITTEN_IGNORE),
            "round {round} wrote the ignore entry more than once"
        );
    }
}

// ===== One spelling of the ledger's location =================================

/// `.marrow` and the ledger's entry name belong to the pure owner. This adapter
/// derives every publication name from those spellings, so a rename there moves
/// these names with it instead of leaving a second live ledger location behind.
#[test]
fn the_publication_names_derive_from_the_pure_owner_s_spellings() {
    let _serial = serialized();
    let project = Project::new("one-spelling");
    let plan = project.plan("Book", 1);
    assert!(matches!(
        project
            .guard()
            .publish_ids(plan)
            .expect("the publication runs"),
        IdsPublishOutcome::Settled(IdsPublication::Published)
    ));

    let joined = project
        .path()
        .join(marrow_project::META_DIR)
        .join(marrow_project::IDS_ENTRY);
    assert_eq!(
        joined,
        project.path().join(marrow_project::IDS_FILE),
        "the pure owner's joined path and its two parts are one location"
    );
    assert!(
        fs::read(&joined).is_ok_and(|bytes| !bytes.is_empty()),
        "the publication installed the successor at the pure owner's path"
    );

    for source in [
        include_str!("publication.rs"),
        include_str!("publication/ignore.rs"),
        include_str!("publication/protocol.rs"),
        include_str!("publication/header.rs"),
        include_str!("publication/marker.rs"),
    ] {
        // The opening quote alone, so a derived spelling — `"ids.publish.stage"`,
        // `"ids.pending"`, `".marrow/ids"` — is caught as well as the bare name.
        // Matching the closed literal `"ids"` would have let every suffixed
        // spelling this owner derives through.
        for literal in ["\".marrow", "\"ids"] {
            assert!(
                !source.contains(literal),
                "the publication owner spells a name opening {literal} itself; `.marrow` and \
                 the ledger's entry name have one owner in `marrow_project`, and every \
                 transient is derived from them"
            );
        }
    }
}

/// The terminal has one decision point. A mutation that finds its destination
/// taken, an exchange that returned a successor back out, the phase driver, and
/// the tail derivation all name their terminal by classifying the artifact map,
/// so every `Terminal` a publication can settle into is constructed in exactly
/// one place. A second construction is how a successor gets recorded as
/// installed over a map that says nothing was installed.
#[test]
fn the_settled_terminal_is_decided_in_exactly_one_place() {
    let protocol = include_str!("publication/protocol.rs");
    let decisions = protocol.matches("Ok(Terminal::").count();
    assert_eq!(
        decisions, 2,
        "the protocol builds a terminal outcome in {decisions} places; the closed map \
         classifier `terminal_of_map` is the only one that may"
    );
    let classifier = protocol
        .split_once("fn terminal_of_map")
        .map(|(_, body)| body)
        .expect("the classifier is present");
    assert_eq!(
        classifier.matches("Ok(Terminal::").count(),
        2,
        "both terminal decisions must live inside `terminal_of_map`"
    );
}

// ===== Faulted mutations =====================================================
//
// Every entry mutation the protocol performs — the stage's `create`, the absent
// arm's `link`, the replace arm's `exchange`, the `rename-noreplace` that opens
// a validated removal, and the marker's `unlink` — happens in the admitted
// metadata directory, so a directory whose owner bits withhold write refuses
// all five. A removal's own `unlink` is not separately reachable here: a
// removal never reaches it because the move to the quarantine name refuses
// first.
//
// A removal's renames are within one directory, which is the rename atomicity
// this protocol already rests on for its exchange and its destination-refusing
// link, so the quarantine adds no filesystem property to the documented
// file-and-directory-`fsync` envelope. Within that envelope an interrupted
// removal leaves exactly one state — the object at the quarantine name, its
// own name absent — because before its first rename and after its last there
// is nothing to find. Sudden power loss remains outside the envelope here
// exactly as it is everywhere else in this protocol. That is a real
// refusal from the production path rather than an injected one, and it needs no
// seam: the fault is applied from outside the protocol, between two production
// calls, and the phase the planted journal has reached selects which mutation
// meets it.
//
// Two operations are outside what withdrawing directory write can reach: the
// stage's `append` and every `sync`. Both act on a descriptor this process
// already holds, and mode bits are checked when a name is resolved rather than
// when an open file is written or flushed, so no change to those bits refuses
// them. That is a statement about mode bits, not about the operations: a full
// filesystem, an exceeded quota, a revoked mount, a media error, and a lowered
// `RLIMIT_FSIZE` each refuse a write or a flush on an already-open descriptor.
// Installing one of those from inside this binary needs privileged setup this
// suite does not assume, or — for the file-size limit, whose refusal arrives as
// `SIGXFSZ` first — a signal disposition this workspace's `forbid(unsafe_code)`
// rules out. So the paths they would exercise are untested here rather than
// unreachable: the two `?` operators in `stage_successor` around `file.append`
// and `file.sync`, the `meta.sync()` calls that follow each mutation, and the
// `discard_stage` precedence rule that only a fault on those reaches.

/// Withdraw write access to the metadata directory, restoring it on drop.
///
/// A withdrawal that binds nothing would leave every kat below asserting a
/// refusal that never happened, so it panics instead of reporting green. Mode
/// bits do not bind a process holding the mode-override capability (`root`, or
/// `CAP_DAC_OVERRIDE` on Linux), and a filesystem that carries no mode bits
/// does not enforce them at all; `withdrawn_directory_write_binds_this_process`
/// is the control that names the cause once.
struct RefusingMeta {
    path: PathBuf,
}

impl RefusingMeta {
    fn apply(project: &Project) -> Self {
        let path = project.meta();
        set_mode(&path, 0o500);
        let probe = path.join("write-probe");
        if fs::write(&probe, b"").is_ok() {
            fs::remove_file(&probe).ok();
            set_mode(&path, 0o700);
            panic!(
                "withdrawing owner write from {} did not refuse a write inside it, so this \
                 fault kat would assert a refusal that never happened. Run the suite as a \
                 process the mode bits bind, on a filesystem that carries them.",
                path.display()
            );
        }
        Self { path }
    }
}

impl Drop for RefusingMeta {
    fn drop(&mut self) {
        set_mode(&self.path, 0o700);
    }
}

/// The custody operation a refusal names, read from the typed error rather than
/// from its rendered message. A fault kat that asserted only the refusal class
/// would be satisfied by a refusal from any earlier operation in the same call,
/// including the guard's own lock and the marker claim.
fn refused_operation(error: &IdsPublicationError) -> &'static str {
    let source = std::error::Error::source(error).expect("a custody refusal carries its source");
    let custody = source
        .downcast_ref::<CustodyError>()
        .expect("a custody refusal's source is the custody error");
    match custody {
        CustodyError::UnqualifiedPlatform { .. } => "platform",
        CustodyError::Unsupported { op }
        | CustodyError::AlreadyExists { op }
        | CustodyError::NotFound { op }
        | CustodyError::SymlinkRefused { op }
        | CustodyError::NotADirectory { op }
        | CustodyError::WrongNodeKind { op, .. }
        | CustodyError::IdentityDrift { op }
        | CustodyError::ModeDenied { op, .. }
        | CustodyError::Io { op, .. } => op,
    }
}

/// The control for the four faulted-mutation kats: withdrawing owner write from
/// the metadata directory must actually refuse a write inside it. Without it a
/// suite running where mode bits bind nothing would report four green checks
/// that observed no refusal at all.
#[test]
fn withdrawn_directory_write_binds_this_process() {
    let _serial = serialized();
    let project = Project::new("mode-control");
    fs::create_dir_all(project.meta()).expect("create the metadata directory");
    let _fault = RefusingMeta::apply(&project);
}

/// A refused stage creation is an ordinary refusal: nothing was staged and
/// nothing was claimed, so the project is exactly as it was.
#[test]
fn a_refused_stage_creation_stages_and_claims_nothing() {
    let _serial = serialized();
    let project = Project::new("fault-create");
    let plan = project.plan("Book", 1);
    let mut guard = project.guard();
    let _fault = RefusingMeta::apply(&project);

    let refusal = guard
        .publish_ids(plan)
        .expect_err("a refused create is an ordinary refusal");
    assert_eq!(refusal.refusal(), IdsRefusal::Custody);
    assert_eq!(refusal.code(), Code::IoWrite);
    assert_eq!(
        refused_operation(&refusal),
        "create file",
        "the stage creation is the operation that refused: {refusal}"
    );
    assert!(!project.exists("ids"), "no artifact was written");
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
    assert!(!project.exists("ids.pending.create"));
    assert_eq!(ids_publication_marker(project.path()), None);
}

/// A refused link retains the claimed publication: the artifact is untouched,
/// the successor is still staged, and the marker keeps gating until a later
/// recovery can finish.
#[test]
fn a_refused_link_retains_the_claimed_publication() {
    let _serial = serialized();
    let project = Project::new("fault-link");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor")
        .installing()
        .plant();
    let mut guard = project.guard();
    let _fault = RefusingMeta::apply(&project);

    let refusal = guard
        .recover_ids()
        .expect_err("a refused link cannot settle the publication");
    assert_eq!(refusal.refusal(), IdsRefusal::Custody);
    assert_eq!(
        refused_operation(&refusal),
        "link",
        "the absent arm's link is the operation that refused: {refusal}"
    );
    assert!(!project.exists("ids"), "no artifact was written");
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"successor"[..])
    );
    assert_eq!(
        ids_publication_marker(project.path()),
        Some(IdsPublicationMarker::Claimed),
        "the marker keeps gating capture"
    );
}

/// A refused exchange leaves the bound generation committed. The replace arm
/// mutates in one atomic step, so a refusal means it did not happen at all.
#[test]
fn a_refused_exchange_retains_the_bound_generation() {
    let _serial = serialized();
    let project = Project::new("fault-exchange");
    project.write_meta("ids", b"base");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, Some(b"base"), b"successor")
        .installing()
        .plant();
    let mut guard = project.guard();
    let _fault = RefusingMeta::apply(&project);

    let refusal = guard
        .recover_ids()
        .expect_err("a refused exchange cannot settle the publication");
    assert_eq!(refusal.refusal(), IdsRefusal::Custody);
    assert_eq!(
        refused_operation(&refusal),
        "exchange",
        "the replace arm's exchange is the operation that refused: {refusal}"
    );
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"base"[..]));
    assert_eq!(
        project.read_meta("ids.publish.stage").as_deref(),
        Some(&b"successor"[..])
    );
    assert!(project.exists("ids.pending"));
}

/// A refused stage cleanup stops before the marker is unlinked. The successor
/// is installed and the terminal is recorded, so the publication is finishable
/// — but only the final marker unlink ends it, and that has not happened. The
/// cleanup removes nothing it has not first moved to the quarantine name, so
/// that move is the act a refusing metadata directory stops, and the alias it
/// would have removed is still under its own name.
#[test]
fn a_refused_stage_cleanup_keeps_the_publication_unfinished() {
    let _serial = serialized();
    let project = Project::new("fault-unlink");
    project.write_meta("ids.publish.stage", b"successor");
    fs::hard_link(
        project.meta().join("ids.publish.stage"),
        project.meta().join("ids"),
    )
    .expect("link the staged successor into place");
    Crash::new(&project, None, b"successor")
        .installing()
        .settled(0)
        .plant();
    let mut guard = project.guard();
    let fault = RefusingMeta::apply(&project);

    let refusal = guard
        .recover_ids()
        .expect_err("a refused cleanup cannot finish the publication");
    assert_eq!(refusal.refusal(), IdsRefusal::Custody);
    assert_eq!(
        refused_operation(&refusal),
        "rename-noreplace",
        "the stage cleanup's first act is the move to the quarantine name, and it is \
         the operation that refused: {refusal}"
    );
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
    assert!(project.exists("ids.publish.stage"), "the alias is retained");
    assert!(
        project.exists("ids.pending"),
        "the publication is unfinished"
    );

    // Restoring the directory lets the same recovery finish it: the refusal
    // interrupted the protocol without moving it off its map.
    drop(fault);
    assert_eq!(
        guard.recover_ids().expect("recovery runs"),
        Some(IdsPublication::Published)
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}
