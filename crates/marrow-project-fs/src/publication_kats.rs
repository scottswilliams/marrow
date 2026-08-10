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
    AdmittedDir, EntryName, FsIdentity, JournalKind, PendingName, encode_header, encode_record,
};
use marrow_project::{IdentityAnchor, IdentityKind, LedgerPublicationPlan};

use crate::capture::capture_project;
use crate::overlay::OverlaySnapshot;
use crate::publication::header::RowHeader;
use crate::publication::{
    IdsPublication, IdsPublicationMarker, IdsPublishOutcome, IdsRefusal,
    ProjectMetadataWriteGuard, ids_publication_marker,
};

const MANIFEST: &[u8] = b"edition = \"2026\"\n";

fn serialized() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    let guard = project.guard();

    let outcome = guard.publish_ids(plan).expect("the publication runs");
    assert!(matches!(
        outcome,
        IdsPublishOutcome::Settled(IdsPublication::Published)
    ));
    assert!(project.read_meta("ids").is_some_and(|bytes| !bytes.is_empty()));
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
    let guard = project.guard();
    let settled = guard.publish_ids(first).expect("the first publication runs");
    assert!(matches!(settled, IdsPublishOutcome::Settled(IdsPublication::Published)));
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

    let guard = project.guard();
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

    let guard = project.guard();
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

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::Published));
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

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::Published));
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
    Crash::new(&project, None, b"successor").installing().plant();

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::Published));
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
    Crash::new(&project, None, b"successor").installing().plant();

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::Published));
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

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::Published));
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

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::Published));
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

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::Published));
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

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::ConcurrentChange));
    assert_eq!(
        project.read_meta("ids").as_deref(),
        Some(&b"another writer's generation"[..]),
        "a reverted publication leaves the artifact exactly as it found it"
    );
    assert!(!project.exists("ids.publish.stage"));
    assert!(!project.exists("ids.pending"));
}

/// A publication that reached `Installing` and found the destination taken
/// settles as a concurrent change without touching the artifact.
#[test]
fn recovery_reverts_when_the_destination_was_taken() {
    let _serial = serialized();
    let project = Project::new("destination-taken");
    project.write_meta("ids", b"another writer's generation");
    project.write_meta("ids.publish.stage", b"successor");
    Crash::new(&project, None, b"successor").installing().plant();

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::ConcurrentChange));
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

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::Published));
    assert_eq!(project.read_meta("ids").as_deref(), Some(&b"successor"[..]));
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

    let settled = project.guard().recover_ids().expect("recovery runs");
    assert_eq!(settled, Some(IdsPublication::Published));
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
        .err()
        .expect("capture refuses under a live claim");
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
        .err()
        .expect("capture refuses under an unclaimed create");
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
        include_str!("publication/protocol.rs"),
        include_str!("publication/header.rs"),
        include_str!("publication/marker.rs"),
    ] {
        for forbidden in ["read_dir", "ReadDir", "glob", "remove_dir_all"] {
            assert!(
                !source.contains(forbidden),
                "the publication owner must not reach for `{forbidden}`"
            );
        }
    }
}
