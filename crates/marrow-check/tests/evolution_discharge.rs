//! Data-attached discharge over a real checked project and a live store snapshot.
//!
//! Each case checks a source-driven fixture through the production pipeline, seeds
//! a `TreeStore::memory()` at the member catalog ids the checked saved place names,
//! then runs the read-only discharge/preview entry and asserts the verdicts, the
//! witness counts, and the composed fingerprints. The data-only cases accept the
//! catalog proposal first (so the schema is already accepted) and exercise an old
//! store snapshot that predates a new member or index; the catalog-evolution cases
//! pin a hand-built accepted catalog the current source has moved away from.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::evolution::{EvolutionWitness, RepairReason, Verdict, preview};
use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, check_project,
    checked_saved_root_place,
};
use marrow_project::{
    CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata, parse_config,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, encode_value};

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

fn config() -> marrow_project::ProjectConfig {
    parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config")
}

fn catalog_path(root: &Path) -> PathBuf {
    root.join("marrow.catalog.json")
}

/// A valid `cat_<16 lowercase hex>` stable id keyed by a small ordinal, so a
/// hand-built accepted catalog uses ids the store can address.
fn hex_id(n: u8) -> String {
    format!("cat_{n:016x}")
}

fn entry(kind: CatalogEntryKind, path: &str, stable_id: &str) -> CatalogEntry {
    CatalogEntry {
        kind,
        path: path.to_string(),
        stable_id: stable_id.to_string(),
        aliases: Vec::new(),
        lifecycle: CatalogLifecycle::Active,
    }
}

fn write_catalog(root: &Path, metadata: &CatalogMetadata) {
    fs::write(catalog_path(root), metadata.to_json_pretty()).expect("write catalog");
}

fn checked(root: &Path) -> CheckedProgram {
    let (report, program) = check_project(root, &config()).expect("check project");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

/// Check the source once with no accepted catalog, write its proposal as the
/// accepted catalog (the accept flow), then re-check. The returned program's schema
/// is fully accepted, so its bound catalog ids address the store; the data-only
/// cases then exercise an old snapshot against that accepted schema.
fn accept_then_check(root: &Path) -> CheckedProgram {
    let (report, program) = check_project(root, &config()).expect("check for accept");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program.catalog.proposal.expect("first-check proposal");
    write_catalog(root, &proposal);
    checked(root)
}

fn root_place(program: &CheckedProgram, root: &str) -> CheckedSavedPlace {
    checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .expect("checked saved root place")
}

/// A minimal seeded store rooted at one single-key-identity saved place. Each
/// record is its `id` key; a member is seeded with `write_data_value` at the bound
/// member catalog id, exactly as the runtime write path does.
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
        let member_id = CatalogId::new(member_catalog_id(self.place, member)).expect("member id");
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

    fn index_entry(&self, index: &str, key: Scalar, id: i64) {
        let index_id = CatalogId::new(index_catalog_id(self.place, index)).expect("index id");
        self.store
            .write_index_entry(
                &index_id,
                &[key.as_key().expect("index key")],
                &[SavedKey::Int(id)],
                Vec::new(),
            )
            .expect("write index entry");
    }

    /// Seed a leaf inside a keyed layer entry, at the path the runtime writes:
    /// `[Member(layer_id), Key(entry_key), Member(leaf_id)]` under the record
    /// identity. The presence of any leaf marks the keyed entry as existing.
    fn keyed_member(&self, id: i64, layer: &str, entry: SavedKey, leaf: &str, value: Scalar) {
        let layer_id =
            CatalogId::new(group_member_catalog_id(self.place, layer)).expect("layer id");
        let leaf_id = CatalogId::new(nested_member_catalog_id(self.place, layer, leaf))
            .expect("keyed leaf id");
        let bytes = encode_value(&value).expect("encode value");
        self.store
            .write_data_value(
                &self.store_id(),
                &[SavedKey::Int(id)],
                &[
                    DataPathSegment::Member(layer_id),
                    DataPathSegment::Key(entry),
                    DataPathSegment::Member(leaf_id),
                ],
                bytes,
            )
            .expect("write keyed member value");
    }

    /// Seed a leaf inside an unkeyed group, at the nested member path the runtime
    /// writes: `[Member(group_id), Member(leaf_id)]` under the record identity.
    fn nested_member(&self, id: i64, group: &str, leaf: &str, value: Scalar) {
        let group_id =
            CatalogId::new(group_member_catalog_id(self.place, group)).expect("group id");
        let leaf_id = CatalogId::new(nested_member_catalog_id(self.place, group, leaf))
            .expect("nested leaf id");
        let bytes = encode_value(&value).expect("encode value");
        self.store
            .write_data_value(
                &self.store_id(),
                &[SavedKey::Int(id)],
                &[
                    DataPathSegment::Member(group_id),
                    DataPathSegment::Member(leaf_id),
                ],
                bytes,
            )
            .expect("write nested member value");
    }
}

/// Discharge through the production preview entry and return the witness; the tests
/// assert the witness verdicts, counts, and fingerprints. Diagnostics are discarded
/// here; cases that assert on them call `preview` directly.
fn witness(program: &CheckedProgram, store: &TreeStore) -> EvolutionWitness {
    preview(program, store).expect("preview").0
}

fn verdict_for<'a>(witness: &'a EvolutionWitness, catalog_id: &str) -> &'a Verdict {
    witness
        .verdicts
        .iter()
        .find(|outcome| outcome.catalog_id.as_str() == catalog_id)
        .map(|outcome| &outcome.verdict)
        .unwrap_or_else(|| panic!("verdict for `{catalog_id}` among {:#?}", witness.verdicts))
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

/// Adding an optional sparse field over existing records is a no-op. The store
/// needs no rewrite and the witness records zero backfill.
#[test]
fn optional_sparse_add_needs_no_rewrite() {
    let root = temp_project("discharge-optional-add", |root| {
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
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let subtitle_id = member_catalog_id(&place, "subtitle");
    assert!(
        matches!(verdict_for(&result, &subtitle_id), Verdict::NoOp),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.records_to_backfill, 0);
    assert_eq!(result.counts.records_lacking_member, 0);
    assert_eq!(result.counts.scanned_records, 2);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A newly-required member with an `evolve default` discharges to a backfill plan:
/// the witness counts the records lacking it.
#[test]
fn required_with_default_backfills_old_records() {
    let root = temp_project("discharge-required-default", |root| {
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
    // Old records carry `title` but predate the new required `pages`.
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    let pages_id = member_catalog_id(&place, "pages");
    match verdict_for(&result, &pages_id) {
        // The constant evolve default `0` flows into the witness as the encoded
        // int the apply phase backfills with.
        Verdict::Default { value } => {
            assert_eq!(value.scalar_type, marrow_store::value::ScalarType::Int);
            assert_eq!(
                value.encoded,
                marrow_store::value::encode_value(&Scalar::Int(0)).unwrap()
            );
        }
        other => panic!("expected default, got {other:#?}"),
    }
    assert_eq!(result.counts.records_lacking_member, 2);
    assert_eq!(result.counts.records_to_backfill, 2);
}

/// An `evolve default` whose value is not a constant the checker can evaluate is not
/// a default at all; it fails closed with a diagnostic steering the developer to a
/// transform. A per-record-varying fill is a transform, not a default.
#[test]
fn non_constant_default_fails_closed_with_transform_hint() {
    let root = temp_project("discharge-nonconst-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             evolve\n\
             \x20   default Book.pages = 1 + 1\n\
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

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let pages_id = member_catalog_id(&place, "pages");
    assert!(
        matches!(
            verdict_for(&result, &pages_id),
            Verdict::RepairRequired {
                reason: RepairReason::MissingRequiredMember
            }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(
        diagnostics
            .iter()
            .any(|message| message.contains("constant") && message.contains("transform")),
        "{diagnostics:#?}"
    );
}

/// A newly-required member with no default and records missing it cannot attach
/// data. Activation fails and the diagnostic names the exact records.
#[test]
fn required_without_default_fails_naming_records() {
    let root = temp_project("discharge-required-no-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
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

    let (witness, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(!witness.is_activatable(), "{witness:#?}");
    assert!(
        diagnostics.iter().any(|message| message.contains("1")
            && message.contains("2")
            && message.contains("pages")),
        "{diagnostics:#?}"
    );
}

/// A rename declared with an `evolve rename` intent moves catalog identity only. No
/// record data moves and the verdict is catalog-only.
#[test]
fn rename_with_intent_is_catalog_only() {
    let title_id = hex_id(3);
    let root = temp_project("discharge-rename", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required heading: string\n\
             evolve\n\
             \x20   rename Book.title -> Book.heading\n\
             pub fn add(heading: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            5,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &title_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // The renamed member keeps its accepted stable id; seed data under it.
    seed.record(1);
    seed.member_by_id(1, &title_id, Scalar::Str("Dune".into()));

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    let heading_id = member_catalog_id(&place, "heading");
    assert_eq!(heading_id, title_id, "rename preserves the stable id");
    assert!(
        matches!(
            verdict_for(&result, &heading_id),
            Verdict::CatalogOnly | Verdict::DataProof
        ),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.records_to_backfill, 0);
}

/// Retiring a member whose source is gone, with populated records, is a destructive
/// decision. The verdict names the exact catalog id and the populated count.
#[test]
fn retire_of_populated_member_requires_scoped_approval() {
    let subtitle_id = hex_id(4);
    let root = temp_project("discharge-retire", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             evolve\n\
             \x20   retire Book.subtitle\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            6,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::subtitle",
                    &subtitle_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let store_id = CatalogId::new(place.store_catalog_id.clone()).unwrap();
    let subtitle = CatalogId::new(subtitle_id.clone()).unwrap();
    for (id, value) in [(1, "A"), (2, "B")] {
        store.write_node(&store_id, &[SavedKey::Int(id)]).unwrap();
        store
            .write_data_value(
                &store_id,
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(subtitle.clone())],
                encode_value(&Scalar::Str(value.into())).unwrap(),
            )
            .unwrap();
    }

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    match verdict_for(&result, &subtitle_id) {
        Verdict::DestructiveDecisionRequired { populated } => assert_eq!(*populated, 2),
        other => panic!("expected destructive decision, got {other:#?}"),
    }
}

/// A new unique index over clean (collision-free) data discharges to a derived
/// rebuild.
#[test]
fn new_unique_index_over_clean_data_rebuilds() {
    let root = temp_project("discharge-index-clean", |root| {
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
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));
    seed.index_entry("byIsbn", Scalar::Str("111".into()), 1);
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("222".into()));
    seed.index_entry("byIsbn", Scalar::Str("222".into()), 2);

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    let index_id = index_catalog_id(&place, "byIsbn");
    assert!(
        matches!(verdict_for(&result, &index_id), Verdict::DerivedRebuild),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.index_collisions, 0);
}

/// A unique index over colliding data fails activation and the witness counts the
/// collisions.
#[test]
fn new_unique_index_over_colliding_data_fails() {
    let root = temp_project("discharge-index-collide", |root| {
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
    // Two records claim the same unique key: a collision the index cannot publish.
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("dup".into()));
    seed.index_entry("byIsbn", Scalar::Str("dup".into()), 1);
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("dup".into()));
    seed.index_entry("byIsbn", Scalar::Str("dup".into()), 2);

    let (witness, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(!witness.is_activatable(), "{witness:#?}");
    assert!(witness.counts.index_collisions > 0, "{witness:#?}");
    assert!(
        diagnostics.iter().any(|message| message.contains("byIsbn")),
        "{diagnostics:#?}"
    );
}

/// A declared transform is non-applyable here. The verdict is a
/// typed-transform-required obligation and activation is blocked.
#[test]
fn transform_is_typed_transform_required() {
    let root = temp_project("discharge-transform", |root| {
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

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    assert!(!result.is_activatable(), "{result:#?}");
    assert!(
        result
            .verdicts
            .iter()
            .any(|outcome| matches!(outcome.verdict, Verdict::TypedTransformRequired)),
        "{:#?}",
        result.verdicts
    );
}

/// The witness composes the existing fingerprints: the accepted and proposal
/// catalog epoch/digest, the store engine profile + commit id, and the affected
/// catalog ids.
#[test]
fn witness_composes_catalog_and_store_fingerprints() {
    // Accept a first schema, then add an optional member so the next check proposes
    // a changed catalog: the witness must carry both fingerprints.
    let root = temp_project("discharge-witness", |root| {
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
    let accepted = accept_then_check(&root);
    let accepted_epoch = accepted.catalog.accepted_epoch.expect("accepted epoch");
    let accepted_digest = accepted.catalog.accepted_digest.clone().expect("digest");

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    // Adding `subtitle` without re-accepting is exactly the "accept the proposal"
    // signal: the check reports a catalog-intent diagnostic, yet the proposal still
    // forms, so the witness must carry both the accepted and proposal fingerprints.
    let (_report, program) = check_project(&root, &config()).expect("re-check with new member");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let (witness, _diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert_eq!(witness.accepted_catalog.epoch, accepted_epoch);
    assert_eq!(witness.accepted_catalog.digest, accepted_digest);
    let proposal = witness.proposal_catalog.clone().expect("proposal");
    assert_eq!(
        proposal.epoch,
        accepted_epoch + 1,
        "proposal advances the accepted epoch"
    );
    assert_eq!(
        Some(proposal.digest),
        program
            .catalog
            .proposal
            .as_ref()
            .map(|catalog| catalog.digest.clone())
    );
    // No commit metadata was stamped, so the witness records no commit id.
    assert_eq!(witness.store_commit_id, None);
    // The subtitle member the proposal newly adds is among the affected ids. Its
    // bound place id is empty until the proposal is accepted, so read the minted
    // stable id from the proposal entries.
    let subtitle_id = program
        .catalog
        .proposal
        .as_ref()
        .expect("proposal")
        .entries
        .iter()
        .find(|entry| entry.path == "books::Book::subtitle")
        .expect("subtitle entry")
        .stable_id
        .clone();
    assert!(
        witness
            .changed_root_catalog_ids
            .iter()
            .any(|id| id.as_str() == subtitle_id),
        "{witness:#?}"
    );
}

/// Dropping a sparse source field that nothing else depends on is a legal no-op.
/// The accepted entry lingers as data but the verdict is a deprecation, not an
/// error.
#[test]
fn dropped_sparse_field_is_deprecated_not_error() {
    let subtitle_id = hex_id(4);
    let root = temp_project("discharge-f12-drop", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            11,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::subtitle",
                    &subtitle_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let store = TreeStore::memory();

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(verdict_for(&result, &subtitle_id), Verdict::Deprecated),
        "{:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(
        !diagnostics
            .iter()
            .any(|message| message.contains("subtitle")),
        "{diagnostics:#?}"
    );
}

/// A dropped source field a unique index still reads is not a silent deprecation;
/// discharge requires a retire intent. The accepted catalog keeps a member `isbn`
/// and an index `byIsbn(isbn)`; current source drops the member but keeps the index,
/// so the proposal carries the lingering member with the index still reading it.
#[test]
fn dropped_field_an_index_needs_requires_retire() {
    let isbn_id = hex_id(4);
    let root = temp_project("discharge-f12-index", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(title: string, isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            12,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::isbn",
                    &isbn_id,
                ),
                entry(
                    CatalogEntryKind::StoreIndex,
                    "books::^books::byIsbn",
                    &hex_id(5),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    // The accepted catalog already matches source, so this first check is clean and
    // binds `isbn`. Now drop the `isbn` member from source while keeping the index,
    // so the index reads a member current source no longer declares.
    checked(&root);
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   index byIsbn(isbn) unique\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let (_report, program) = check_project(&root, &config()).expect("check");
    let store = TreeStore::memory();
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    // The dropped member an index still reads is its own typed failure: a retire is
    // required, distinct from a plain missing-required-member repair. The verdict
    // carries the index's catalog identity (its accepted stable id), prose-free; the
    // developer-facing name surfaces only in the diagnostic.
    match verdict_for(&result, &isbn_id) {
        Verdict::RepairRequired {
            reason: RepairReason::RetireRequired { index },
        } => assert_eq!(index.as_str(), hex_id(5)),
        other => panic!("expected RetireRequired, got {other:#?}"),
    }
    assert!(
        diagnostics
            .iter()
            .any(|message| message.contains("byIsbn") && message.contains("retire")),
        "{diagnostics:#?}"
    );
}

fn composite_index_project(name: &str) -> PathBuf {
    temp_project(name, |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required a: string\n\
             \x20   required b: string\n\
             \x20   index byPair(a, b) unique\n\
             pub fn add(a: string, b: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    })
}

/// A composite unique index over distinct full key tuples that happen to share
/// their first key column is collision-free: the discharge must derive the full
/// `(a, b)` tuple per record, not descend the first column alone.
#[test]
fn composite_unique_index_distinct_tuples_rebuild() {
    let root = composite_index_project("discharge-composite-clean");
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "a", Scalar::Str("shared".into()));
    seed.member(1, "b", Scalar::Str("one".into()));
    seed.record(2);
    seed.member(2, "a", Scalar::Str("shared".into()));
    seed.member(2, "b", Scalar::Str("two".into()));

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    let index_id = index_catalog_id(&place, "byPair");
    assert!(
        matches!(verdict_for(&result, &index_id), Verdict::DerivedRebuild),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.index_collisions, 0);
}

/// A composite unique index over a real duplicate full tuple `(a, b)` is a
/// collision the index cannot publish, even when the records also share their
/// first column. The verdict fails closed.
#[test]
fn composite_unique_index_duplicate_tuple_collides() {
    let root = composite_index_project("discharge-composite-collide");
    let program = accept_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "a", Scalar::Str("same".into()));
    seed.member(1, "b", Scalar::Str("same".into()));
    seed.record(2);
    seed.member(2, "a", Scalar::Str("same".into()));
    seed.member(2, "b", Scalar::Str("same".into()));

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    let index_id = index_catalog_id(&place, "byPair");
    assert!(
        matches!(
            verdict_for(&result, &index_id),
            Verdict::RepairRequired { .. }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(result.counts.index_collisions > 0, "{:#?}", result.counts);
}

/// A newly-declared single-column unique index has no index cells yet: the
/// discharge must derive each record's prospective key from its member value, not
/// from a (nonexistent) index entry. Distinct member values rebuild cleanly.
#[test]
fn new_unique_index_no_cells_clean_rebuilds() {
    let root = temp_project("discharge-new-index-clean", |root| {
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
    // Records exist with distinct member values, but no index cells were ever
    // written: the index is being declared over a pre-existing store.
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("222".into()));

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    let index_id = index_catalog_id(&place, "byIsbn");
    assert!(
        matches!(verdict_for(&result, &index_id), Verdict::DerivedRebuild),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.index_collisions, 0);
}

/// A newly-declared unique index over pre-existing records whose member values
/// collide fails closed, even though the store holds no index cells: the
/// prospective keys are derived from the records themselves.
#[test]
fn new_unique_index_no_cells_duplicate_collides() {
    let root = temp_project("discharge-new-index-collide", |root| {
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
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("dup".into()));
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("dup".into()));

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    let index_id = index_catalog_id(&place, "byIsbn");
    assert!(
        matches!(
            verdict_for(&result, &index_id),
            Verdict::RepairRequired { .. }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(result.counts.index_collisions > 0, "{:#?}", result.counts);
}

/// A required leaf inside an unkeyed group is required for the containing resource.
/// An old record that lacks `name.last` must discharge to a fail-closed repair, and
/// the nested leaf's catalog id must appear among the affected ids so apply
/// re-verifies it.
#[test]
fn required_nested_group_leaf_missing_fails_closed() {
    let root = temp_project("discharge-nested-required", |root| {
        write(
            root,
            "src/people.mw",
            "module people\n\
             resource Person at ^people(id: int)\n\
             \x20   name\n\
             \x20       required first: string\n\
             \x20       required last: string\n\
             pub fn add(): Id(^people)\n\
             \x20   return nextId(^people)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "people");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // The old record carries `name.first` but predates required `name.last`.
    seed.record(1);
    seed.nested_member(1, "name", "first", Scalar::Str("Ada".into()));

    let result = witness(&program, &store);
    let last_id = nested_member_catalog_id(&place, "name", "last");
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(
            verdict_for(&result, &last_id),
            Verdict::RepairRequired { .. }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(
        result
            .changed_root_catalog_ids
            .iter()
            .any(|id| id.as_str() == last_id),
        "{:#?}",
        result.changed_root_catalog_ids
    );
}

/// A required leaf inside a keyed layer is required for each entry that exists. An
/// old keyed entry that lacks a newly-required leaf must discharge to a blocking
/// verdict, never an empty pass: the witness alone must be non-activatable, and the
/// keyed leaf's catalog id must appear among the affected ids so apply re-verifies it.
#[test]
fn required_keyed_layer_leaf_missing_fails_closed() {
    let root = temp_project("discharge-keyed-required", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   status: string\n\
             \x20   versions(version: int)\n\
             \x20       note: string\n\
             \x20       required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // The record exists with one keyed entry that predates required `body`: a sibling
    // `note` cell marks the entry as existing while `body` is absent.
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(7),
        "note",
        Scalar::Str("draft".into()),
    );

    let body_id = nested_member_catalog_id(&place, "versions", "body");
    let (result, _diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a missing keyed-layer required leaf must block activation: {:#?}",
        result.verdicts
    );
    let verdict = verdict_for(&result, &body_id);
    assert!(
        !verdict.is_activatable(),
        "the keyed leaf verdict must be blocking, got {verdict:#?}"
    );
    assert!(
        result
            .changed_root_catalog_ids
            .iter()
            .any(|id| id.as_str() == body_id),
        "{:#?}",
        result.changed_root_catalog_ids
    );
}

/// A keyed layer whose every existing entry already carries its required leaf
/// discharges to a proof, not a block: the per-entry scan must not fail open in
/// either direction.
#[test]
fn keyed_layer_leaf_present_in_every_entry_proves() {
    let root = temp_project("discharge-keyed-present", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   status: string\n\
             \x20   versions(version: int)\n\
             \x20       required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
    });
    let program = accept_then_check(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(1),
        "body",
        Scalar::Str("v1".into()),
    );
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(2),
        "body",
        Scalar::Str("v2".into()),
    );
    seed.record(2);
    seed.keyed_member(
        2,
        "versions",
        SavedKey::Int(1),
        "body",
        Scalar::Str("only".into()),
    );

    let body_id = nested_member_catalog_id(&place, "versions", "body");
    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    assert!(result.is_activatable(), "{:#?}", result.verdicts);
    assert!(
        matches!(verdict_for(&result, &body_id), Verdict::DataProof),
        "{:#?}",
        result.verdicts
    );
}

/// The witness source digest for a single-file source, computed against the source's
/// own accepted catalog so its evolve default and transform targets bind to real
/// stable ids. The digest binds the durable surface and the evolve decision surface,
/// so it is the anchor apply re-derives.
fn source_digest(name: &str, source: &str) -> String {
    let root = temp_project(name, |root| write(root, "src/books.mw", source));
    let program = accept_then_check(&root);
    let store = TreeStore::memory();
    let digest = witness(&program, &store).source_digest;
    fs::remove_dir_all(&root).ok();
    digest
}

/// The source digest binds the evolve decision surface, not just the catalog
/// `(kind, path)` shape. A changed default value, a changed transform body, and an
/// optional/required toggle each drift the digest, so apply can detect a source that
/// no longer matches what was discharged.
#[test]
fn source_digest_binds_the_evolve_decision_surface() {
    let base = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         evolve\n\
         \x20   default Book.pages = 0\n\
         \x20   transform Book.title\n\
         \x20       return \"x\"\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let changed_default = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         evolve\n\
         \x20   default Book.pages = 1\n\
         \x20   transform Book.title\n\
         \x20       return \"x\"\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let changed_transform = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         evolve\n\
         \x20   default Book.pages = 0\n\
         \x20   transform Book.title\n\
         \x20       return \"y\"\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let required_pages = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         evolve\n\
         \x20   default Book.pages = 0\n\
         \x20   transform Book.title\n\
         \x20       return \"x\"\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";

    let base_digest = source_digest("digest-base", base);
    assert_ne!(
        base_digest,
        source_digest("digest-default", changed_default),
        "a changed default value must drift the digest"
    );
    assert_ne!(
        base_digest,
        source_digest("digest-transform", changed_transform),
        "a changed transform body must drift the digest"
    );
    assert_ne!(
        base_digest,
        source_digest("digest-required", required_pages),
        "an optional->required toggle must drift the digest"
    );
}

/// The source digest binds the whole durable surface and the evolve decision
/// surface, with no enumeration gap. It is computed from the canonical normalized
/// rendering of every durable and evolution declaration, so any change to a member
/// type, a required flag, an identity key, an index, a keyed-layer key at any nesting
/// depth, a top-level keyed-leaf key, an evolve default value, or a transform body
/// must drift the digest, while a pure whitespace reformat of the same declarations
/// must leave it unchanged.
///
/// The single baseline carries every dimension once. Each variant edits exactly one
/// durable fact at the same catalog path, so a digest that still matched the baseline
/// would prove that fact is unbound.
#[test]
fn source_digest_binds_the_durable_shape() {
    let base = durable_fixture(DurableFixture::default());
    let base_digest = source_digest("durable-base", &base);

    let cases: [(&str, DurableFixture, &str); 10] = [
        (
            "member-type",
            DurableFixture {
                count_type: "string",
                ..DurableFixture::default()
            },
            "a member scalar-type change must drift the digest",
        ),
        (
            "identity-type",
            DurableFixture {
                identity_type: "string",
                // A string identity has no default allocation policy, so the helper
                // reads rather than allocates. The function is not a durable fact the
                // digest binds, so changing it alongside the identity type keeps the
                // edit single in the durable surface.
                func: "pub fn lookup(id: string): string\n\
                       \x20   return ^books(id).isbn",
                ..DurableFixture::default()
            },
            "an identity-key scalar-type change must drift the digest",
        ),
        (
            "index-unique",
            DurableFixture {
                index_unique: false,
                ..DurableFixture::default()
            },
            "an index uniqueness flip must drift the digest",
        ),
        (
            "index-columns",
            DurableFixture {
                index_columns: "count, id",
                ..DurableFixture::default()
            },
            "an index key-columns change must drift the digest",
        ),
        (
            "keyed-group-arity",
            DurableFixture {
                versions_keys: "version: int, draft: int",
                ..DurableFixture::default()
            },
            "a keyed-group key arity change must drift the digest",
        ),
        (
            "keyed-group-type",
            DurableFixture {
                versions_keys: "version: string",
                ..DurableFixture::default()
            },
            "a keyed-group key scalar-type change must drift the digest",
        ),
        (
            "keyed-leaf-type",
            DurableFixture {
                tags_keys: "pos: string",
                ..DurableFixture::default()
            },
            "a top-level keyed-leaf key scalar-type change must drift the digest",
        ),
        (
            "default-value",
            DurableFixture {
                default_value: "1",
                ..DurableFixture::default()
            },
            "an evolve default value change must drift the digest",
        ),
        (
            "transform-body",
            DurableFixture {
                transform_body: "return \"y\"",
                ..DurableFixture::default()
            },
            "an evolve transform body change must drift the digest",
        ),
        (
            "optional-toggle",
            DurableFixture {
                count_required: false,
                ..DurableFixture::default()
            },
            "an optional->required toggle must drift the digest",
        ),
    ];

    for (name, fixture, message) in cases {
        let digest = source_digest(&format!("durable-{name}"), &durable_fixture(fixture));
        assert_ne!(base_digest, digest, "{message}");
    }

    // A pure whitespace and indentation reformat of the same declarations parses to
    // the same syntax tree, so the normalized rendering — and the digest — is stable.
    let reformatted = marrow_syntax::format_source(&base);
    assert_ne!(reformatted, base, "the reformat must change layout");
    assert_eq!(
        base_digest,
        source_digest("durable-reformatted", &reformatted),
        "a pure reformat must not drift the digest"
    );
}

/// One single-edit knob over the durable baseline. Each field maps to exactly one
/// durable fact the digest must bind; the default is the baseline source, and a case
/// flips a single field to assert that fact drifts the digest.
struct DurableFixture {
    identity_type: &'static str,
    count_type: &'static str,
    count_required: bool,
    index_unique: bool,
    index_columns: &'static str,
    versions_keys: &'static str,
    tags_keys: &'static str,
    default_value: &'static str,
    transform_body: &'static str,
    func: &'static str,
}

impl Default for DurableFixture {
    fn default() -> Self {
        Self {
            identity_type: "int",
            count_type: "int",
            count_required: true,
            index_unique: true,
            index_columns: "isbn, id",
            versions_keys: "version: int",
            tags_keys: "pos: int",
            default_value: "0",
            transform_body: "return \"x\"",
            func: "pub fn add(isbn: string): Id(^books)\n\
                   \x20   return nextId(^books)",
        }
    }
}

/// Render the durable baseline (or a single-edit variant) as one `.mw` source. The
/// resource carries a scalar member, a keyed group with a required leaf, a top-level
/// keyed-leaf map, and a unique index; the evolve block defaults the scalar member and
/// transforms `isbn`, so one fixture exercises every digest dimension.
fn durable_fixture(f: DurableFixture) -> String {
    let count_required = if f.count_required { "required " } else { "" };
    let index_unique = if f.index_unique { " unique" } else { "" };
    format!(
        "module books\n\
         resource Book at ^books(id: {identity})\n\
         \x20   {count_required}count: {count_type}\n\
         \x20   pages: int\n\
         \x20   required isbn: string\n\
         \x20   tags({tags}): string\n\
         \x20   versions({versions})\n\
         \x20       required body: string\n\
         \x20   index byIsbn({columns}){unique}\n\
         evolve\n\
         \x20   default Book.pages = {default}\n\
         \x20   transform Book.isbn\n\
         \x20       {transform}\n\
         {func}\n",
        identity = f.identity_type,
        count_type = f.count_type,
        tags = f.tags_keys,
        versions = f.versions_keys,
        columns = f.index_columns,
        unique = index_unique,
        default = f.default_value,
        transform = f.transform_body,
        func = f.func,
    )
}
