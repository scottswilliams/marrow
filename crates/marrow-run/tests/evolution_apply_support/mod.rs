//! Shared apply harness for the evolution-apply integration suites.
//!
//! Each case checks a source-driven fixture through the production pipeline, seeds a
//! store at the member catalog ids the checked saved place names, runs the read-only
//! `preview` to produce the witness apply consumes, then drives the production `apply`
//! entry and asserts the staged data, the metadata stamp, and the drift/rollback
//! contracts. The witness is the only input that crosses the check->run boundary, so
//! every drift dimension is exercised by mutating the witness or the store and proving
//! apply aborts before staging a write.
//!
//! Each invariant-focused split file includes this module, so not every binary
//! exercises every helper; the crate-wide `dead_code` allowance keeps the shared
//! surface intact.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::evolution::{EvolutionWitness, preview};
use marrow_check::{CheckedProgram, CheckedSavedPlace, check_project};
use marrow_run::evolution::apply;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, decode_value, encode_value};

// The fact-lookup family and the check/commit factories are owned by marrow-check
// behind its `test-support` feature, so the apply suites query the same helpers the
// discharge suites do rather than carrying a copy. The `config`/`checked`/
// `commit_then_check`/`root_place` names the apply tests call resolve through this glob.
// Each split binary uses a subset, so unused re-exports are expected, as with the
// crate-wide `dead_code` allowance.
#[allow(unused_imports)]
pub use marrow_check::test_support::{
    checked, commit_then_check, group_member_catalog_id, index_catalog_id, member_catalog_id,
    nested_member_catalog_id, proposal_catalog_id, root_place, store_id_of, test_config as config,
};

// The before/after `module books` evolution sources live in the repo-root corpus, so
// the apply suites and the CLI evolution suites load one canonical fixture rather than
// re-declaring the same shape as an inline string per crate.
const BOOKS_BASELINE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_required_baseline.mw");
const BOOKS_REQUIRED_DEFAULT: &str =
    include_str!("../../../../fixtures/v01/evolution/books_required_default.mw");
const BOOKS_SUBTITLE_BASELINE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_subtitle_baseline.mw");
const BOOKS_RETIRE_SUBTITLE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_retire_subtitle.mw");

pub fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
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

/// A minimal seeded store rooted at one single-key-identity saved place, identical to
/// the discharge harness: a record is its `id` key; a member is seeded at the bound
/// member catalog id exactly as the runtime write path does.
pub struct Seed<'a> {
    pub store: &'a TreeStore,
    pub place: &'a CheckedSavedPlace,
}

impl Seed<'_> {
    pub fn store_id(&self) -> CatalogId {
        store_id_of(self.place)
    }

    pub fn record(&self, id: i64) {
        self.store
            .write_node(&self.store_id(), &[SavedKey::Int(id)])
            .expect("write node");
    }

    pub fn member(&self, id: i64, member: &str, value: Scalar) {
        self.member_by_id(id, &member_catalog_id(self.place, member), value);
    }

    pub fn member_by_id(&self, id: i64, member_catalog_id: &str, value: Scalar) {
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

pub const INT: marrow_store::value::ScalarType = marrow_store::value::ScalarType::Int;

pub fn witness(program: &CheckedProgram, store: &TreeStore) -> EvolutionWitness {
    preview(program, store).expect("preview").0
}

pub fn applied_proposal_default_fixture(
    name: &str,
    records: i64,
) -> (
    PathBuf,
    CheckedProgram,
    CheckedSavedPlace,
    TreeStore,
    String,
) {
    let root = temp_project(name, |root| {
        write(root, "src/books.mw", BOOKS_BASELINE);
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
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
    let program = checked(&root);
    let pages_id = proposal_catalog_id(&program, "books::Book::pages");
    let w = witness(&program, &store);
    apply(&w, &program, &store, false, None).expect("apply proposal default");
    (root, program, accepted_place, store, pages_id)
}

/// Read a member cell value as a scalar for backfill assertions.
pub fn read_scalar(
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

/// Build a fixture whose accepted catalog carries a `subtitle` member current source
/// retires, with two records populated. Source first declares `subtitle` and is
/// accepted, so the member binds a real stable id and old records carry data under it;
/// source then drops the member with an `evolve retire`, so the accepted snapshot still
/// names it. Returns the project root, the re-checked retiring program, the place, the
/// seeded memory store, and the retired member's bound stable id.
pub fn destructive_retire_fixture(
    name: &str,
) -> (
    PathBuf,
    CheckedProgram,
    CheckedSavedPlace,
    TreeStore,
    String,
) {
    let root = temp_project(name, |root| {
        write(root, "src/books.mw", BOOKS_SUBTITLE_BASELINE);
    });
    // Commit the schema that still declares `subtitle`, so the member binds a stable id.
    let accepted = commit_then_check(&root);
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
    write(&root, "src/books.mw", BOOKS_RETIRE_SUBTITLE);
    let (_report, program) = check_project(&root, &config()).expect("check retiring source");
    let place = root_place(&program, "books");
    (root, program, place, store, subtitle_id)
}
