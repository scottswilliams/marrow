//! Shared apply harness for the evolution-apply integration suites.
//!
//! Each case checks a source-driven fixture through the production pipeline, seeds a
//! store at the member catalog ids the checked saved place names, runs the read-only
//! `preview` to produce the witness apply consumes, then drives the production `apply`
//! entry and asserts the written data, the metadata stamp, and the drift/rollback
//! contracts. The witness is the only input that crosses the check->run boundary, so
//! every drift dimension is exercised by mutating the witness or the store and proving
//! apply aborts before committing a write.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use marrow_check::evolution::{EvolutionWitness, preview};
use marrow_check::{CheckedProgram, CheckedSavedPlace};
use marrow_run::evolution::{ApplyOutcome, apply, current_engine_profile};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{CommitMetadata, DataPathSegment, EngineProfile, TreeStore};
use marrow_store::value::{Scalar, decode_value, encode_value};

// The fact-lookup family and the check/commit factories are owned by marrow-check
// behind its `test-support` feature, so the apply suites use the same helpers the
// discharge suites do rather than carrying a copy. The `config`/`checked`/
// `commit_then_check`/`root_place` names the apply tests call resolve through this glob.
pub use marrow_check::test_support::{
    checked, commit_then_check, group_member_catalog_id, index_catalog_id, member_catalog_id,
    nested_member_catalog_id, proposal_catalog_id, root_place, store_id_of, test_config as config,
};

// The before/after `module books` evolution sources live in the repo-root corpus, so
// the apply suites and the CLI evolution suites load one canonical fixture rather than
// re-declaring the same shape as an inline string per crate.
const BOOKS_BASELINE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_required_baseline.mw"
));
const BOOKS_REQUIRED_DEFAULT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_required_default.mw"
));
const BOOKS_SUBTITLE_BASELINE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_subtitle_baseline.mw"
));
const BOOKS_RETIRE_SUBTITLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_retire_subtitle.mw"
));

/// A temporary project directory removed when the value is dropped.
///
/// Derefs to its root [`Path`], so it passes straight into the check, preview, and
/// apply entries and any other `&Path` consumer without an explicit accessor. The
/// drop removes the directory even when an assertion panics, so a failing test never
/// leaks its temp dir.
pub struct TempProject {
    root: PathBuf,
}

impl Deref for TempProject {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

pub fn temp_project(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    TempProject { root }
}

pub fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

pub fn index_has_children(store: &TreeStore, index: &CatalogId) -> bool {
    store
        .index_first_child(index, &[])
        .expect("read first index child")
        .is_some()
}

/// Seeds saved data for an accepted place through the same tree shape runtime writes use.
pub struct Seed<'a> {
    pub store: &'a TreeStore,
    pub place: &'a CheckedSavedPlace,
}

impl Seed<'_> {
    pub fn store_id(&self) -> CatalogId {
        store_id_of(self.place).expect("store catalog id")
    }

    pub fn record(&self, id: i64) {
        let store_id = self.store_id();
        write_identity_presence(self.store, &store_id, &[SavedKey::Int(id)]);
    }

    pub fn member(&self, id: i64, member: &str, value: Scalar) {
        self.member_by_id(
            id,
            &member_catalog_id(self.place, member).expect("member catalog id"),
            value,
        );
    }

    pub fn member_by_id(&self, id: i64, member_catalog_id: &str, value: Scalar) {
        let store_id = self.store_id();
        let member_id = CatalogId::new(member_catalog_id).expect("member id");
        let identity = [SavedKey::Int(id)];
        let path = [DataPathSegment::Member(member_id)];
        self.write_value(&store_id, &identity, &path, value);
    }

    pub fn singleton_member(&self, member: &str, value: Scalar) {
        let store_id = self.store_id();
        write_identity_presence(self.store, &store_id, &[]);
        let member_id =
            CatalogId::new(member_catalog_id(self.place, member).expect("member catalog id"))
                .expect("member id");
        let path = [DataPathSegment::Member(member_id)];
        self.write_value(&store_id, &[], &path, value);
    }

    pub fn nested_member_by_id(
        &self,
        id: i64,
        group_member_id: &str,
        member_id: &str,
        value: Scalar,
    ) {
        let store_id = self.store_id();
        let identity = [SavedKey::Int(id)];
        let path = [
            DataPathSegment::Member(CatalogId::new(group_member_id).expect("group member id")),
            DataPathSegment::Member(CatalogId::new(member_id).expect("member id")),
        ];
        self.write_value(&store_id, &identity, &path, value);
    }

    pub fn delete_member_by_id(&self, id: i64, member_id: &str) {
        let store_id = self.store_id();
        let identity = [SavedKey::Int(id)];
        let path = [DataPathSegment::Member(
            CatalogId::new(member_id).expect("member id"),
        )];
        self.store
            .delete_data_subtree(&store_id, &identity, &path)
            .expect("delete member subtree");
    }

    fn write_value(
        &self,
        store_id: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        value: Scalar,
    ) {
        let bytes = encode_value(&value).expect("encode value");
        self.store
            .write_data_value(store_id, identity, path, bytes)
            .expect("write member value");
    }
}

fn write_identity_presence(store: &TreeStore, store_id: &CatalogId, identity: &[SavedKey]) {
    store
        .write_record_presence(store_id, identity)
        .expect("write record presence");
}

pub const INT: marrow_store::value::ScalarType = marrow_store::value::ScalarType::Int;

pub fn witness(program: &CheckedProgram, store: &TreeStore) -> EvolutionWitness {
    preview(program, store).expect("preview").0
}

/// Build a commit-metadata stamp with empty changed-id partitions, deriving the layout
/// epoch and engine-profile digest from `profile`. The apply suites seed predecessor
/// stamps that differ only in commit id, catalog epoch, source digest, and engine
/// profile, so those are the only inputs the caller supplies.
fn commit_metadata(
    commit_id: u64,
    catalog_epoch: u64,
    source_digest: String,
    profile: EngineProfile,
) -> CommitMetadata {
    CommitMetadata {
        commit_id,
        catalog_epoch,
        layout_epoch: profile.layout_epoch(),
        source_digest,
        engine_profile_digest: profile.digest_bytes(),
        changed_root_catalog_ids: Vec::new(),
        changed_index_catalog_ids: Vec::new(),
    }
}

/// Write a predecessor commit stamp the apply suites can fence or resume against.
pub fn stamp_commit(
    store: &TreeStore,
    commit_id: u64,
    catalog_epoch: u64,
    source_digest: String,
    profile: EngineProfile,
) {
    store
        .write_commit_metadata(&commit_metadata(
            commit_id,
            catalog_epoch,
            source_digest,
            profile,
        ))
        .expect("stamp commit metadata");
}

/// Stamp a clean predecessor commit at the program's accepted epoch under this binary's
/// engine profile, the steady state a same-name index change starts from.
pub fn stamp_clean_commit(store: &TreeStore, program: &CheckedProgram) {
    stamp_commit(
        store,
        1,
        program.catalog.accepted_epoch.expect("accepted epoch"),
        program.source_digest(),
        current_engine_profile(),
    );
}

pub fn applied_proposal_default_fixture(
    name: &str,
    records: i64,
) -> (
    TempProject,
    CheckedProgram,
    CheckedSavedPlace,
    TreeStore,
    String,
    ApplyOutcome,
) {
    let root = temp_project(name, |root| {
        write(root, "src/books.mw", BOOKS_BASELINE);
    });
    let accepted = commit_then_check(&root).expect("committed fixture");
    let accepted_place = root_place(&accepted, "books").expect("accepted books place");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    for id in 1..=records {
        seed.record(id);
        seed.member(id, "title", Scalar::Str(format!("Book {id}")));
    }
    write(&root, "src/books.mw", BOOKS_REQUIRED_DEFAULT);
    let program = checked(&root).expect("checked fixture");
    let pages_id = proposal_catalog_id(&program, "books::Book::pages").expect("pages proposal id");
    let w = witness(&program, &store);
    let outcome = apply(&w, &program, &store, false, None).expect("apply proposal default");
    (root, program, accepted_place, store, pages_id, outcome)
}

/// Read a member cell value as a scalar for backfill assertions.
pub fn read_scalar(
    store: &TreeStore,
    store_id: &CatalogId,
    id: i64,
    member_id: &str,
    scalar: marrow_store::value::ScalarType,
) -> Option<Scalar> {
    let member = CatalogId::new(member_id).expect("member id");
    let bytes = store
        .read_data_value(
            store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(member)],
        )
        .expect("read member");
    bytes.map(|bytes| decode_value(&bytes, scalar).expect("decode value"))
}

/// Build a fixture whose accepted catalog carries a `subtitle` member current source
/// retires, with two records populated. Source first declares `subtitle` and is
/// accepted, so the member binds a real stable id and old records carry data under it;
/// source then drops the member with an `evolve retire`, so the accepted snapshot still
/// names it. Returns the project root, the re-checked retiring program, the place, the
/// seeded memory store, and the retired member's bound stable id.
pub fn destructive_retire_fixture(
    name: &str,
) -> (
    TempProject,
    CheckedProgram,
    CheckedSavedPlace,
    TreeStore,
    String,
) {
    let root = temp_project(name, |root| {
        write(root, "src/books.mw", BOOKS_SUBTITLE_BASELINE);
    });
    // Commit the schema that still declares `subtitle`, so the member binds a stable id.
    let accepted = commit_then_check(&root).expect("committed fixture");
    let accepted_place = root_place(&accepted, "books").expect("accepted books place");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle").expect("subtitle catalog id");

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
    write(&root, "src/books.mw", BOOKS_RETIRE_SUBTITLE);
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books").expect("checked books place");
    (root, program, place, store, subtitle_id)
}
