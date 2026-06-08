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
use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, ProjectConfig,
    check_project, checked_saved_root_place,
};
use marrow_run::evolution::apply;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, decode_value, encode_value};

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

pub fn config() -> ProjectConfig {
    ProjectConfig {
        source_roots: vec!["src".into()],
        default_entry: None,
        store: None,
        tests: Vec::new(),
        accepted_catalog: "marrow.catalog.json".into(),
    }
}

pub fn checked(root: &Path) -> CheckedProgram {
    let (report, program) = check_project(root, &config()).expect("check project");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

/// Check the source with no committed catalog, freeze its proposal through the
/// production commit path, then re-check. The returned program's schema is fully
/// committed, so its bound catalog ids address the store.
pub fn commit_then_check(root: &Path) -> CheckedProgram {
    let (report, program) = check_project(root, &config()).expect("check for commit");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let (report, program) = marrow_check::commit_pending_identity(root, &config(), &program)
        .expect("commit catalog")
        .expect("a catalog proposal to commit");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

pub fn root_place(program: &CheckedProgram, root: &str) -> CheckedSavedPlace {
    checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .expect("checked saved root place")
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

pub fn member_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    let member = place
        .root_members
        .iter()
        .find(|member| {
            member.name == name && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .unwrap_or_else(|| panic!("checked member `{name}`"));
    accepted_catalog_id(&member.catalog_id, name)
}

pub fn proposal_catalog_id(program: &CheckedProgram, path: &str) -> String {
    program
        .catalog
        .proposal
        .as_ref()
        .expect("catalog proposal")
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("proposal catalog entry `{path}`"))
        .stable_id
        .clone()
}

pub fn index_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    let index = place
        .indexes
        .iter()
        .find(|index| index.name == name)
        .unwrap_or_else(|| panic!("checked index `{name}`"));
    accepted_catalog_id(&index.catalog_id, name)
}

pub fn group_member<'a>(place: &'a CheckedSavedPlace, group: &str) -> &'a CheckedSavedMember {
    place
        .root_members
        .iter()
        .find(|member| member.name == group && matches!(member.kind, CheckedSavedMemberKind::Group))
        .unwrap_or_else(|| panic!("checked group member `{group}`"))
}

pub fn group_member_catalog_id(place: &CheckedSavedPlace, group: &str) -> String {
    accepted_catalog_id(&group_member(place, group).catalog_id, group)
}

pub fn nested_member_catalog_id(place: &CheckedSavedPlace, group: &str, leaf: &str) -> String {
    let member = group_member(place, group)
        .group_members
        .iter()
        .find(|member| member.name == leaf)
        .unwrap_or_else(|| panic!("checked nested member `{group}.{leaf}`"));
    accepted_catalog_id(&member.catalog_id, leaf)
}

pub fn accepted_catalog_id(id: &Option<String>, label: &str) -> String {
    id.clone()
        .unwrap_or_else(|| panic!("accepted catalog id for `{label}`"))
}

/// The bound store catalog id for a committed place, ready to address store cells.
pub fn store_id_of(place: &CheckedSavedPlace) -> CatalogId {
    CatalogId::new(accepted_catalog_id(&place.store_catalog_id, "store")).expect("store catalog id")
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
    write(
        &root,
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
