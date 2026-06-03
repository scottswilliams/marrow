//! Witness-validated apply over a real checked project and a live store snapshot.
//!
//! Each case checks a source-driven fixture through the production pipeline, seeds a
//! store at the member catalog ids the checked saved place names, runs the read-only
//! `preview` to produce the witness apply consumes, then drives the production `apply`
//! entry and asserts the staged data, the metadata stamp, and the drift/rollback
//! contracts. The witness is the only input that crosses the check->run boundary, so
//! every drift dimension is exercised by mutating the witness or the store and proving
//! apply aborts before staging a write.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::evolution::{EvolutionWitness, preview};
use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, ProjectConfig,
    check_project, checked_saved_root_place,
};
use marrow_run::evolution::{ApplyError, Approval, apply};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, decode_value, encode_value};

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn config() -> ProjectConfig {
    ProjectConfig {
        source_roots: vec!["src".into()],
        default_entry: None,
        store: None,
        tests: Vec::new(),
        accepted_catalog: "marrow.catalog.json".into(),
    }
}

fn catalog_path(root: &Path) -> PathBuf {
    root.join("marrow.catalog.json")
}

fn checked(root: &Path) -> CheckedProgram {
    let (report, program) = check_project(root, &config()).expect("check project");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

/// Check the source with no accepted catalog, write its proposal as the accepted
/// catalog (the accept flow), then re-check. The returned program's schema is fully
/// accepted, so its bound catalog ids address the store.
fn accept_then_check(root: &Path) -> CheckedProgram {
    let (report, program) = check_project(root, &config()).expect("check for accept");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program.catalog.proposal.expect("first-check proposal");
    fs::write(catalog_path(root), proposal.to_json_pretty()).expect("write catalog");
    checked(root)
}

fn root_place(program: &CheckedProgram, root: &str) -> CheckedSavedPlace {
    checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .expect("checked saved root place")
}

/// A minimal seeded store rooted at one single-key-identity saved place, identical to
/// the discharge harness: a record is its `id` key; a member is seeded at the bound
/// member catalog id exactly as the runtime write path does.
struct Seed<'a> {
    store: &'a TreeStore,
    place: &'a CheckedSavedPlace,
}

impl Seed<'_> {
    fn store_id(&self) -> CatalogId {
        CatalogId::new(self.place.store_catalog_id.clone()).expect("store catalog id")
    }

    fn record(&self, id: i64) {
        self.store
            .write_node(&self.store_id(), &[SavedKey::Int(id)])
            .expect("write node");
    }

    fn member(&self, id: i64, member: &str, value: Scalar) {
        self.member_by_id(id, &member_catalog_id(self.place, member), value);
    }

    fn member_by_id(&self, id: i64, member_catalog_id: &str, value: Scalar) {
        let member_id = CatalogId::new(member_catalog_id).expect("member id");
        let bytes = encode_value(&value).expect("encode value");
        self.store
            .write_data_value(
                &self.store_id(),
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(member_id)],
                bytes,
            )
            .expect("write member value");
    }
}

fn member_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    place
        .root_members
        .iter()
        .find(|member| {
            member.name == name && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .unwrap_or_else(|| panic!("checked member `{name}`"))
        .catalog_id
        .clone()
}

fn index_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    place
        .indexes
        .iter()
        .find(|index| index.name == name)
        .unwrap_or_else(|| panic!("checked index `{name}`"))
        .catalog_id
        .clone()
}

fn group_member<'a>(place: &'a CheckedSavedPlace, group: &str) -> &'a CheckedSavedMember {
    place
        .root_members
        .iter()
        .find(|member| member.name == group && matches!(member.kind, CheckedSavedMemberKind::Group))
        .unwrap_or_else(|| panic!("checked group member `{group}`"))
}

fn group_member_catalog_id(place: &CheckedSavedPlace, group: &str) -> String {
    group_member(place, group).catalog_id.clone()
}

fn nested_member_catalog_id(place: &CheckedSavedPlace, group: &str, leaf: &str) -> String {
    group_member(place, group)
        .group_members
        .iter()
        .find(|member| member.name == leaf)
        .unwrap_or_else(|| panic!("checked nested member `{group}.{leaf}`"))
        .catalog_id
        .clone()
}

fn witness(program: &CheckedProgram, store: &TreeStore) -> EvolutionWitness {
    preview(program, store).expect("preview").0
}

/// Read a member cell value as a scalar for backfill assertions.
fn read_scalar(
    store: &TreeStore,
    store_id: &CatalogId,
    id: i64,
    member_id: &str,
    scalar: marrow_store::value::ScalarType,
) -> Option<Scalar> {
    let member = CatalogId::new(member_id.to_string()).expect("member id");
    let bytes = store
        .read_data_value(
            store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(member)],
        )
        .expect("read member");
    bytes.map(|bytes| decode_value(&bytes, scalar).expect("decode value"))
}

/// A required-with-default change backfills exactly the records lacking the member
/// and stamps the proposal epoch. The applied store carries the encoded default at
/// each old record and a commit stamp at the proposal epoch.
#[test]
fn required_with_default_backfills_exactly_k_and_stamps_epoch() {
    let root = temp_project("apply-required-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             evolve\n\
             \x20   default Book.pages = 0\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));
    // One record already carries pages, so the backfill must touch only the two that
    // lack it; preview counts records_to_backfill = 2.
    seed.record(3);
    seed.member(3, "title", Scalar::Str("Neuromancer".into()));
    seed.member(3, "pages", Scalar::Int(271));

    let w = witness(&program, &store);
    // The full schema (including required `pages` and the evolve default) was already
    // accepted, so source proposes no catalog change: apply stamps the accepted epoch
    // while the data catches up.
    assert!(w.proposal_catalog.is_none());
    let target_epoch = w.accepted_catalog.epoch;

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.records_backfilled, 2);
    assert_eq!(outcome.catalog_epoch, target_epoch);

    let store_id = CatalogId::new(place.store_catalog_id.clone()).unwrap();
    let pages_id = member_catalog_id(&place, "pages");
    let int = marrow_store::value::ScalarType::Int;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, int),
        Some(Scalar::Int(0))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &pages_id, int),
        Some(Scalar::Int(0))
    );
    // The record that already had a value is untouched.
    assert_eq!(
        read_scalar(&store, &store_id, 3, &pages_id, int),
        Some(Scalar::Int(271))
    );

    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("a stamp");
    assert_eq!(commit.catalog_epoch, target_epoch);
    assert_eq!(
        store.read_catalog_epoch().expect("epoch"),
        Some(target_epoch)
    );

    // Idempotent re-apply: the same source against the now-applied store re-previews
    // to a no-op for pages (every record carries it) and re-applying succeeds.
    let resumed = witness(&program, &store);
    let second = apply(&resumed, &program, &store, false, None).expect("re-apply succeeds");
    assert_eq!(second.records_backfilled, 0);
    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, int),
        Some(Scalar::Int(0))
    );

    fs::remove_dir_all(&root).ok();
}

/// An optional sparse add is a no-op: apply stamps the proposal epoch with no data
/// step. The store is stamped but carries no new member cell.
#[test]
fn optional_add_stamps_epoch_without_data_step() {
    let root = temp_project("apply-optional-add", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let witness = witness(&program, &store);
    let proposal_epoch = witness.proposal_catalog.as_ref().map(|c| c.epoch);
    let outcome = apply(&witness, &program, &store, false, None).expect("apply");
    assert_eq!(outcome.records_backfilled, 0);

    let store_id = CatalogId::new(place.store_catalog_id.clone()).unwrap();
    let subtitle_id = member_catalog_id(&place, "subtitle");
    assert_eq!(
        read_scalar(
            &store,
            &store_id,
            1,
            &subtitle_id,
            marrow_store::value::ScalarType::Str
        ),
        None,
        "an optional add writes no data"
    );
    // The epoch was still stamped so old binaries are fenced.
    if let Some(epoch) = proposal_epoch {
        assert_eq!(store.read_catalog_epoch().expect("epoch"), Some(epoch));
    }
    fs::remove_dir_all(&root).ok();
}

/// A new unique index over clean data rebuilds its entries and stamps the epoch.
#[test]
fn new_index_rebuild_writes_entries_and_stamps() {
    let root = temp_project("apply-index-rebuild", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // Records exist with distinct member values but no index cells were written.
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("222".into()));

    let witness = witness(&program, &store);
    let outcome = apply(&witness, &program, &store, false, None).expect("apply");
    assert_eq!(outcome.indexes_rebuilt, 1);

    let index_id = CatalogId::new(index_catalog_id(&place, "byIsbn")).unwrap();
    let one = store
        .scan_index_tuple(&index_id, &[SavedKey::Str("111".into())], 2)
        .expect("scan");
    assert_eq!(one.entries.len(), 1, "the rebuilt index holds 111");
    assert_eq!(one.entries[0].identity, vec![SavedKey::Int(1)]);
    let two = store
        .scan_index_tuple(&index_id, &[SavedKey::Str("222".into())], 2)
        .expect("scan");
    assert_eq!(two.entries.len(), 1, "the rebuilt index holds 222");
    fs::remove_dir_all(&root).ok();
}

/// A new NON-UNIQUE index over existing records rebuilds its entries. The discharge
/// must issue a derived rebuild regardless of uniqueness, so apply writes the index
/// entries rather than stamping success over a silently empty index. A non-unique index
/// ends with the identity keys, so each record publishes one entry under its full key
/// tuple `(genre, id)`.
#[test]
fn new_non_unique_index_rebuild_writes_entries() {
    let root = temp_project("apply-nonunique-index", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required genre: string\n\
             \x20   index byGenre(genre, id)\n\
             pub fn add(genre: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "genre", Scalar::Str("scifi".into()));
    seed.record(2);
    seed.member(2, "genre", Scalar::Str("scifi".into()));

    let w = witness(&program, &store);
    let outcome = apply(&w, &program, &store, false, None).expect("apply");
    assert_eq!(outcome.indexes_rebuilt, 1, "the non-unique index rebuilds");

    // Each record contributes one entry under its full `(genre, id)` tuple. Before the
    // fix the index had no entries at all, so each tuple scan must now return its
    // record identity.
    let index_id = CatalogId::new(index_catalog_id(&place, "byGenre")).unwrap();
    for id in [1, 2] {
        let scan = store
            .scan_index_tuple(
                &index_id,
                &[SavedKey::Str("scifi".into()), SavedKey::Int(id)],
                8,
            )
            .expect("scan");
        let identities: Vec<_> = scan
            .entries
            .iter()
            .map(|entry| entry.identity.clone())
            .collect();
        assert_eq!(
            identities,
            vec![vec![SavedKey::Int(id)]],
            "a new non-unique index must hold record {id}, not be silently empty"
        );
    }
    fs::remove_dir_all(&root).ok();
}

/// A retire over populated data needs maintenance plus a scoped approval. With no
/// approval, apply refuses: the witness is non-activatable.
#[test]
fn destructive_retire_without_approval_aborts() {
    let (root, program, place, store, subtitle_id) =
        destructive_retire_fixture("apply-retire-noapproval");
    let witness = witness(&program, &store);
    assert!(!witness.is_activatable());

    let result = apply(&witness, &program, &store, true, None);
    assert!(
        matches!(result, Err(ApplyError::ApprovalRequired { .. })),
        "expected ApprovalRequired, got {result:#?}"
    );
    // The subtitle data is still present: nothing was dropped.
    let store_id = CatalogId::new(place.store_catalog_id.clone()).unwrap();
    assert!(
        store
            .data_subtree_exists(
                &store_id,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(
                    CatalogId::new(subtitle_id.clone()).unwrap()
                )]
            )
            .expect("exists"),
        "retire without approval must not drop data"
    );
    fs::remove_dir_all(&root).ok();
}

/// A retire with maintenance and a matching scoped approval drops the retired member
/// subtree and stamps the epoch.
#[test]
fn destructive_retire_with_matching_approval_deletes() {
    let (root, program, place, store, subtitle_id) =
        destructive_retire_fixture("apply-retire-approved");
    let witness = witness(&program, &store);

    let approval = Approval {
        catalog_ids: vec![CatalogId::new(subtitle_id.clone()).unwrap()],
        populated: 2,
    };
    let outcome = apply(&witness, &program, &store, true, Some(&approval)).expect("apply");
    assert_eq!(outcome.records_retired, 2);

    let store_id = CatalogId::new(place.store_catalog_id.clone()).unwrap();
    for id in [1, 2] {
        assert!(
            !store
                .data_subtree_exists(
                    &store_id,
                    &[SavedKey::Int(id)],
                    &[DataPathSegment::Member(
                        CatalogId::new(subtitle_id.clone()).unwrap()
                    )]
                )
                .expect("exists"),
            "approved retire drops the member subtree"
        );
    }
    fs::remove_dir_all(&root).ok();
}

/// A scoped approval whose populated count does not match the witness aborts: the
/// store changed under the developer's decision and the destructive drop is refused.
#[test]
fn destructive_retire_count_drift_aborts() {
    let (root, program, place, store, subtitle_id) =
        destructive_retire_fixture("apply-retire-countdrift");
    let witness = witness(&program, &store);

    let approval = Approval {
        catalog_ids: vec![CatalogId::new(subtitle_id.clone()).unwrap()],
        populated: 1, // witness recorded 2 populated records
    };
    let result = apply(&witness, &program, &store, true, Some(&approval));
    assert!(
        matches!(result, Err(ApplyError::ApprovalMismatch)),
        "expected ApprovalMismatch, got {result:#?}"
    );
    let store_id = CatalogId::new(place.store_catalog_id.clone()).unwrap();
    assert!(
        store
            .data_subtree_exists(
                &store_id,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(
                    CatalogId::new(subtitle_id).unwrap()
                )]
            )
            .expect("exists"),
        "a count-drifted approval must not drop data"
    );
    fs::remove_dir_all(&root).ok();
}

/// Maintenance off blocks a destructive retire even with a matching approval: a hard
/// drop requires the maintenance gate.
#[test]
fn destructive_retire_requires_maintenance() {
    let (root, _program, _place, store, subtitle_id) =
        destructive_retire_fixture("apply-retire-no-maint");
    let program = checked(&root);
    let place = root_place(&program, "books");
    let witness = witness(&program, &store);
    let approval = Approval {
        catalog_ids: vec![CatalogId::new(subtitle_id.clone()).unwrap()],
        populated: 2,
    };
    let result = apply(&witness, &program, &store, false, Some(&approval));
    assert!(
        matches!(result, Err(ApplyError::MaintenanceRequired)),
        "expected MaintenanceRequired, got {result:#?}"
    );
    let store_id = CatalogId::new(place.store_catalog_id.clone()).unwrap();
    assert!(
        store
            .data_subtree_exists(
                &store_id,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(
                    CatalogId::new(subtitle_id).unwrap()
                )]
            )
            .expect("exists")
    );
    fs::remove_dir_all(&root).ok();
}

/// A `transform` declaration is non-applyable here: the witness is not activatable
/// and apply refuses it with a typed not-activatable error, staging no write.
#[test]
fn transform_required_witness_is_refused() {
    let root = temp_project("apply-transform", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             evolve\n\
             \x20   transform Book.title\n\
             \x20       return \"x\"\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let witness = witness(&program, &store);
    assert!(!witness.is_activatable());
    let result = apply(&witness, &program, &store, true, None);
    assert!(
        matches!(result, Err(ApplyError::NotActivatable)),
        "expected NotActivatable, got {result:#?}"
    );
    fs::remove_dir_all(&root).ok();
}

/// Source-digest drift: the witness no longer matches what the source now discharges.
/// Apply aborts with a typed drift error before staging a write.
#[test]
fn source_digest_drift_aborts() {
    let root = temp_project("apply-source-drift", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             evolve\n\
             \x20   default Book.pages = 0\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let mut witness = witness(&program, &store);
    witness.source_digest = "fnv1a64:0000000000000000".to_string();
    let result = apply(&witness, &program, &store, false, None);
    assert!(
        matches!(result, Err(ApplyError::Drift)),
        "expected Drift, got {result:#?}"
    );
    // No stamp landed.
    assert_eq!(store.read_commit_metadata().expect("read"), None);
    fs::remove_dir_all(&root).ok();
}

/// Count drift: the witness backfill count no longer matches the live store, so apply
/// aborts before staging a write. Witness equality catches the count change because a
/// re-preview produces a different count.
#[test]
fn count_drift_aborts() {
    let root = temp_project("apply-count-drift", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             evolve\n\
             \x20   default Book.pages = 0\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let mut witness = witness(&program, &store);
    // Simulate a concurrent writer adding a record after the witness was taken: the
    // live re-preview now counts one more record to backfill.
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));
    // Tamper the witness count to a stale value so the only mismatch is the count.
    witness.counts.records_to_backfill = 999;
    let result = apply(&witness, &program, &store, false, None);
    assert!(
        matches!(result, Err(ApplyError::Drift)),
        "expected Drift, got {result:#?}"
    );
    assert_eq!(store.read_commit_metadata().expect("read"), None);
    fs::remove_dir_all(&root).ok();
}

/// Store-commit drift: a concurrent writer advanced the store commit id after the
/// witness pinned it, so apply aborts. The witness pins `store_commit_id`; tampering
/// it to a stale value models the store moving under the apply.
#[test]
fn store_commit_drift_aborts() {
    let root = temp_project("apply-commit-drift", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let mut witness = witness(&program, &store);
    // The witness was taken against a store with no commit stamp (None). Pin it to a
    // value the store does not hold, modelling a writer that committed after preview.
    witness.store_commit_id = Some(42);
    let result = apply(&witness, &program, &store, false, None);
    assert!(
        matches!(
            result,
            Err(ApplyError::Drift | ApplyError::StoreCommitDrift { .. })
        ),
        "expected drift, got {result:#?}"
    );
    assert_eq!(store.read_commit_metadata().expect("read"), None);
    fs::remove_dir_all(&root).ok();
}

/// A failed apply leaves no stamp and a resumed apply re-previews and succeeds
/// (idempotent). A read-only store handle fails the apply, so nothing lands; re-opening
/// the same file read-write and re-applying lands the change, proving the apply wiring
/// commits nothing on failure and that resume is a no-op for data a record already
/// carries. The byte-identical mid-plan rollback after a fault that strikes between two
/// staged writes is proven by the store's transaction-bracket test, which owns that
/// invariant; here the read-only handle aborts before the first write.
#[test]
fn failed_apply_rolls_back_and_resumes_idempotently() {
    let root = temp_project("apply-rollback", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             evolve\n\
             \x20   default Book.pages = 0\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");

    let store_path = root.join("data.marrow");
    {
        let store = TreeStore::open(&store_path).expect("open store");
        let seed = Seed {
            store: &store,
            place: &place,
        };
        seed.record(1);
        seed.member(1, "title", Scalar::Str("Dune".into()));
        seed.record(2);
        seed.member(2, "title", Scalar::Str("Hyperion".into()));
    }

    let store_id = CatalogId::new(place.store_catalog_id.clone()).unwrap();
    let pages_id = member_catalog_id(&place, "pages");
    let int = marrow_store::value::ScalarType::Int;

    // A read-only handle fails the apply commit; nothing must land.
    {
        let ro = TreeStore::open_read_only(&store_path).expect("open read only");
        let witness = witness(&program, &ro);
        let result = apply(&witness, &program, &ro, false, None);
        assert!(result.is_err(), "read-only apply must fail");
        assert_eq!(ro.read_commit_metadata().expect("read"), None, "no stamp");
        assert_eq!(
            read_scalar(&ro, &store_id, 1, &pages_id, int),
            None,
            "no partial backfill"
        );
    }

    // Resume against a writable handle: the same source re-previews to the same
    // witness shape and apply now succeeds, backfilling both records.
    {
        let rw = TreeStore::open(&store_path).expect("reopen store");
        let witness = witness(&program, &rw);
        let outcome = apply(&witness, &program, &rw, false, None).expect("resumed apply");
        assert_eq!(outcome.records_backfilled, 2);
        assert_eq!(
            read_scalar(&rw, &store_id, 1, &pages_id, int),
            Some(Scalar::Int(0))
        );
        assert_eq!(
            read_scalar(&rw, &store_id, 2, &pages_id, int),
            Some(Scalar::Int(0))
        );
        assert!(rw.read_commit_metadata().expect("read").is_some());
    }

    fs::remove_dir_all(&root).ok();
}

/// Retiring a member nested under an unkeyed group fails closed: apply does not yet
/// descend a group to drop nested cells, so a nested retire must be non-activatable
/// rather than counting zero populated cells and silently dropping nothing. The records
/// carry `meta.note` cells that a top-level retire path would never reach.
#[test]
fn nested_group_retire_fails_closed() {
    let root = temp_project("apply-nested-retire", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   meta\n\
             \x20       required note: string\n\
             \x20       keep: string\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Accept the schema that declares `meta.note`, so the nested leaf binds a stable id.
    let accepted = accept_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let meta_id = group_member_catalog_id(&accepted_place, "meta");
    let note_id = nested_member_catalog_id(&accepted_place, "meta", "note");

    let store = TreeStore::memory();
    // Seed two records each carrying a `meta.note` cell at the nested member path.
    for id in [1, 2] {
        store
            .write_node(
                &CatalogId::new(accepted_place.store_catalog_id.clone()).unwrap(),
                &[SavedKey::Int(id)],
            )
            .expect("write node");
        store
            .write_data_value(
                &CatalogId::new(accepted_place.store_catalog_id.clone()).unwrap(),
                &[SavedKey::Int(id)],
                &[
                    DataPathSegment::Member(CatalogId::new(meta_id.clone()).unwrap()),
                    DataPathSegment::Member(CatalogId::new(note_id.clone()).unwrap()),
                ],
                encode_value(&Scalar::Str(format!("note-{id}"))).expect("encode"),
            )
            .expect("write nested member");
    }

    // Retire the nested leaf; the accepted catalog still names it.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   meta\n\
         \x20       keep: string\n\
         evolve\n\
         \x20   retire Book.meta.note\n\
         pub fn add(): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let (report, program) = check_project(&root, &config()).expect("check retiring source");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let w = witness(&program, &store);
    assert!(
        !w.is_activatable(),
        "a nested retire must not be activatable: {w:#?}"
    );

    // Even under the maintenance gate with an approval, apply must refuse rather than
    // stamp success while the nested cells survive.
    let approval = Approval {
        catalog_ids: vec![CatalogId::new(note_id.clone()).unwrap()],
        populated: 0,
    };
    let result = apply(&w, &program, &store, true, Some(&approval));
    assert!(
        matches!(result, Err(ApplyError::NotActivatable)),
        "expected NotActivatable, got {result:#?}"
    );

    // The nested cells are untouched and no stamp landed.
    let store_id = CatalogId::new(accepted_place.store_catalog_id.clone()).unwrap();
    for id in [1, 2] {
        assert!(
            store
                .data_subtree_exists(
                    &store_id,
                    &[SavedKey::Int(id)],
                    &[
                        DataPathSegment::Member(CatalogId::new(meta_id.clone()).unwrap()),
                        DataPathSegment::Member(CatalogId::new(note_id.clone()).unwrap()),
                    ],
                )
                .expect("exists"),
            "a refused nested retire must not drop the nested cell"
        );
    }
    assert_eq!(
        store.read_commit_metadata().expect("read"),
        None,
        "no stamp"
    );
    fs::remove_dir_all(&root).ok();
}

/// Dropping a unique index from source stamps its deprecated catalog id in the commit
/// metadata's index partition, never among the data roots. The discharge already knows
/// the id is a store index by its catalog entry kind, so apply must not re-derive the
/// index set from current source (which no longer declares the index).
#[test]
fn dropped_index_id_stamped_as_index_not_root() {
    let root = temp_project("apply-drop-index", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Accept the schema that declares the index, so the index binds a stable id.
    let accepted = accept_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let index_id = index_catalog_id(&accepted_place, "byIsbn");

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));

    // Drop the index from source while keeping the member; the accepted catalog still
    // names it, so discharge deprecates the index. Apply stays activatable and stamps.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required isbn: string\n\
         pub fn add(isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    apply(&w, &program, &store, false, None).expect("apply succeeds");

    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("a stamp");
    assert!(
        commit
            .changed_index_catalog_ids
            .iter()
            .any(|id| id.as_str() == index_id),
        "dropped index id must be stamped as an index: {commit:#?}"
    );
    assert!(
        !commit
            .changed_root_catalog_ids
            .iter()
            .any(|id| id.as_str() == index_id),
        "dropped index id must not be stamped as a data root: {commit:#?}"
    );
    fs::remove_dir_all(&root).ok();
}

/// Build a fixture whose accepted catalog carries a `subtitle` member current source
/// retires, with two records populated. Source first declares `subtitle` and is
/// accepted, so the member binds a real stable id and old records carry data under it;
/// source then drops the member with an `evolve retire`, so the accepted snapshot still
/// names it. Returns the project root, the re-checked retiring program, the place, the
/// seeded memory store, and the retired member's bound stable id.
fn destructive_retire_fixture(
    name: &str,
) -> (
    PathBuf,
    CheckedProgram,
    CheckedSavedPlace,
    TreeStore,
    String,
) {
    let root = temp_project(name, |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Accept the schema that still declares `subtitle`, so the member binds a stable id.
    let accepted = accept_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    // Both records carry the still-required `title` and the retired `subtitle`, so the
    // only blocking obligation is the destructive retire.
    for (id, value) in [(1, "A"), (2, "B")] {
        seed.record(id);
        seed.member(id, "title", Scalar::Str(format!("title-{id}")));
        seed.member_by_id(id, &subtitle_id, Scalar::Str(value.into()));
    }

    // Now drop `subtitle` from source with a retire intent; the accepted catalog still
    // names it, so discharge classifies a destructive decision over the two records.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let (_report, program) = check_project(&root, &config()).expect("check retiring source");
    let place = root_place(&program, "books");
    (root, program, place, store, subtitle_id)
}
