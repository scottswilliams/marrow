//! Data-attached discharge over a real checked project and a live store snapshot.
//!
//! Each case checks a source-driven fixture through the production pipeline, seeds
//! a `TreeStore::memory()` at the member catalog ids the checked saved place names,
//! then runs the read-only discharge/preview entry and asserts the verdicts, the
//! witness counts, and the composed fingerprints. The data-only cases commit the
//! catalog proposal first (so the schema is already the accepted catalog) and exercise
//! an old store snapshot that predates a new member or index; the catalog-evolution
//! cases pin a hand-built accepted catalog the current source has moved away from.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::evolution::{EvolutionWitness, RepairDiagnostic, RepairReason, Verdict, preview};
use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, check_project,
    checked_saved_root_place,
};
use marrow_project::{
    CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata, parse_config,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
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

/// A valid `cat_<32 lowercase hex>` stable id keyed by a small fixture number, so a
/// hand-built accepted catalog uses ids the store can address.
fn hex_id(n: u8) -> String {
    format!("cat_{n:032x}")
}

fn entry(kind: CatalogEntryKind, path: &str, stable_id: &str) -> CatalogEntry {
    CatalogEntry {
        kind,
        path: path.to_string(),
        stable_id: stable_id.to_string(),
        aliases: Vec::new(),
        lifecycle: CatalogLifecycle::Active,
        accepted_key_shape: None,
        accepted_struct: None,
    }
}

/// A resource-member catalog entry that records the identity-aware leaf token its durable bytes
/// were accepted as (a scalar name, `enum:<enum-stable-id>`, or `id:<store-stable-id>:<arity>`)
/// as the structural signature `leaf:<token>`, the one durable field that carries it, so a
/// discharge can detect a later type change by referent identity and the default-deny backstop
/// sees a leaf member's baseline. The hand-built accepted catalogs use this for members the test
/// then retypes in source.
fn member_entry(path: &str, stable_id: &str, accepted_leaf: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_struct: Some(format!("leaf:{accepted_leaf}")),
        ..entry(CatalogEntryKind::ResourceMember, path, stable_id)
    }
}

/// A keyed-group resource-member catalog entry that records the per-keyed-layer key shape its
/// durable entries are keyed under, as the structural signature `keyed-group:[<shape>]`. A keyed
/// group holds no single leaf cell, so its signature carries no leaf token; the backstop compares
/// it against the current shape to fail a re-key or a group<->keyed-group reshape closed.
fn keyed_group_entry(path: &str, stable_id: &str, key_shape: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_struct: Some(format!("keyed-group:[{key_shape}]")),
        ..entry(CatalogEntryKind::ResourceMember, path, stable_id)
    }
}

/// A plain unkeyed-group resource-member catalog entry, recording the structural signature
/// `group` so the backstop has a baseline to compare a reshape into a keyed layer against. An
/// unkeyed group holds no single leaf cell, so its signature carries no leaf token.
fn group_entry(path: &str, stable_id: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_struct: Some("group".to_string()),
        ..entry(CatalogEntryKind::ResourceMember, path, stable_id)
    }
}

/// A store catalog entry that records the identity-key shape its durable records are keyed
/// under (`<scalar>,<scalar>,...`), so a discharge can detect a later key-shape change the
/// new schema cannot address. The hand-built accepted catalogs use this for stores the test
/// then re-keys in source.
fn store_entry(path: &str, stable_id: &str, accepted_key_shape: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_key_shape: Some(accepted_key_shape.to_string()),
        ..entry(CatalogEntryKind::Store, path, stable_id)
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

/// Check the source once with no committed catalog, freeze its baseline through the
/// production commit path, then re-check. The returned program's schema is fully
/// committed, so its bound catalog ids address the store; the data-only cases then
/// exercise an old snapshot against that committed schema.
fn commit_then_check(root: &Path) -> CheckedProgram {
    let (report, program) = check_project(root, &config()).expect("check for commit");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let (report, program) = marrow_check::commit_pending_identity(root, &config(), &program)
        .expect("commit catalog")
        .expect("a catalog proposal to commit");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
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
        CatalogId::new(accepted_catalog_id(&self.place.store_catalog_id, "store"))
            .expect("store catalog id")
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
        let bytes = encode_value(&value).expect("encode value");
        self.member_bytes_by_id(id, member_catalog_id, bytes);
    }

    /// Seed arbitrary leaf bytes under a member id, exactly as the prior schema's
    /// writes did. Lets a retype case seed bytes written under the old type (a scalar,
    /// an enum member, or an identity payload) regardless of the member's current type.
    fn member_bytes_by_id(&self, id: i64, member_catalog_id: &str, bytes: Vec<u8>) {
        let member_id = CatalogId::new(member_catalog_id).expect("member id");
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

    /// Seed a keyed-leaf-layer (`map[K, V]`) value, at the path the runtime writes:
    /// `[Member(map_id), Key(entry_key)]` under the record identity. The map field is
    /// itself the leaf, so the value cell sits directly under its entry key with no
    /// sub-member. The bytes are written exactly as the prior schema's writes did, so a
    /// retype case can seed a value of the old V type regardless of the current one.
    fn keyed_leaf(&self, id: i64, map: &str, entry: SavedKey, bytes: Vec<u8>) {
        let map_id = CatalogId::new(keyed_leaf_catalog_id(self.place, map)).expect("map id");
        self.store
            .write_data_value(
                &self.store_id(),
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(map_id), DataPathSegment::Key(entry)],
                bytes,
            )
            .expect("write keyed-leaf value");
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

    /// Seed a leaf two keyed layers deep, at the path the runtime writes:
    /// `[Member(outer), Key(outer_key), Member(inner), Key(inner_key), Member(leaf)]`
    /// under the record identity. The presence of the leaf marks the inner keyed entry
    /// (and its enclosing outer entry) as existing, so a re-key of the inner layer
    /// over this data has populated entries the new key shape cannot reach.
    fn deep_keyed_member(&self, id: i64, layers: [(&str, SavedKey); 2], leaf: &str, value: Scalar) {
        let [(outer, outer_key), (inner, inner_key)] = layers;
        let outer_id =
            CatalogId::new(group_member_catalog_id(self.place, outer)).expect("outer layer id");
        let inner_id = CatalogId::new(deep_member_catalog_id(self.place, &[outer, inner]))
            .expect("inner layer id");
        let leaf_id = CatalogId::new(deep_member_catalog_id(self.place, &[outer, inner, leaf]))
            .expect("deep leaf id");
        let bytes = encode_value(&value).expect("encode value");
        self.store
            .write_data_value(
                &self.store_id(),
                &[SavedKey::Int(id)],
                &[
                    DataPathSegment::Member(outer_id),
                    DataPathSegment::Key(outer_key),
                    DataPathSegment::Member(inner_id),
                    DataPathSegment::Key(inner_key),
                    DataPathSegment::Member(leaf_id),
                ],
                bytes,
            )
            .expect("write deep keyed member value");
    }

    /// Seed a leaf inside an unkeyed group reached through a chain of keyed layers, at the path
    /// the runtime writes: each `(layer, key)` pair descends a keyed layer by its entry key, then
    /// the group and the leaf are plain member segments. The presence of the leaf marks the deep
    /// unkeyed group as populated, so a reshape of that group over this data orphans it.
    fn deep_group_member(
        &self,
        id: i64,
        layers: &[(&str, SavedKey)],
        group: &str,
        leaf: &str,
        value: Scalar,
    ) {
        let mut chain: Vec<&str> = Vec::new();
        let mut path = Vec::new();
        for (layer, key) in layers {
            chain.push(layer);
            let layer_id =
                CatalogId::new(deep_member_catalog_id(self.place, &chain)).expect("deep layer id");
            path.push(DataPathSegment::Member(layer_id));
            path.push(DataPathSegment::Key(key.clone()));
        }
        chain.push(group);
        let group_id =
            CatalogId::new(deep_member_catalog_id(self.place, &chain)).expect("deep group id");
        path.push(DataPathSegment::Member(group_id));
        chain.push(leaf);
        let leaf_id =
            CatalogId::new(deep_member_catalog_id(self.place, &chain)).expect("deep leaf id");
        path.push(DataPathSegment::Member(leaf_id));
        let bytes = encode_value(&value).expect("encode value");
        self.store
            .write_data_value(&self.store_id(), &[SavedKey::Int(id)], &path, bytes)
            .expect("write deep group member value");
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
    let member = place
        .root_members
        .iter()
        .find(|member| {
            member.name == name && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .unwrap_or_else(|| panic!("checked member `{name}`"));
    accepted_catalog_id(&member.catalog_id, name)
}

fn index_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    let index = place
        .indexes
        .iter()
        .find(|index| index.name == name)
        .unwrap_or_else(|| panic!("checked index `{name}`"));
    accepted_catalog_id(&index.catalog_id, name)
}

fn group_member<'a>(place: &'a CheckedSavedPlace, group: &str) -> &'a CheckedSavedMember {
    place
        .root_members
        .iter()
        .find(|member| member.name == group && matches!(member.kind, CheckedSavedMemberKind::Group))
        .unwrap_or_else(|| panic!("checked group member `{group}`"))
}

fn group_member_catalog_id(place: &CheckedSavedPlace, group: &str) -> String {
    accepted_catalog_id(&group_member(place, group).catalog_id, group)
}

/// The catalog id of a top-level keyed-leaf-layer (`map[K, V]`) member: a `Field` that
/// carries key params, so it is the leaf its entries' values are stored under.
fn keyed_leaf_catalog_id(place: &CheckedSavedPlace, map: &str) -> String {
    let member = place
        .root_members
        .iter()
        .find(|member| {
            member.name == map
                && !member.key_params.is_empty()
                && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .unwrap_or_else(|| panic!("checked keyed-leaf member `{map}`"));
    accepted_catalog_id(&member.catalog_id, map)
}

fn nested_member_catalog_id(place: &CheckedSavedPlace, group: &str, leaf: &str) -> String {
    let member = group_member(place, group)
        .group_members
        .iter()
        .find(|member| member.name == leaf)
        .unwrap_or_else(|| panic!("checked nested member `{group}.{leaf}`"));
    accepted_catalog_id(&member.catalog_id, leaf)
}

/// The catalog id of a member reached by an arbitrary name chain from the record root, each
/// segment a layer or group whose sub-members hold the next. Resolves members nested through
/// more than one keyed layer, which the single-level [`nested_member_catalog_id`] cannot reach.
fn deep_member_catalog_id(place: &CheckedSavedPlace, chain: &[&str]) -> String {
    let mut members = &place.root_members;
    let mut found = None;
    for segment in chain {
        let member = members
            .iter()
            .find(|member| member.name == *segment)
            .unwrap_or_else(|| panic!("checked nested member `{}`", chain.join(".")));
        found = Some(member);
        members = &member.group_members;
    }
    let member = found.unwrap_or_else(|| panic!("empty member chain"));
    accepted_catalog_id(&member.catalog_id, &chain.join("."))
}

/// The proposal-minted stable id of a brand-new resource member at the given module-qualified
/// catalog path. A member current source adds but the accepted catalog does not yet carry has
/// no bound facts id, so its identity lives only in the catalog proposal; the proposal-aware
/// presence scan keys its verdict by this id.
fn new_member_proposal_id(program: &CheckedProgram, path: &str) -> String {
    program
        .catalog
        .proposal
        .as_ref()
        .expect("a catalog proposal")
        .entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::ResourceMember && entry.path == path)
        .unwrap_or_else(|| panic!("proposal entry for `{path}`"))
        .stable_id
        .clone()
}

/// The stable catalog id the checked program bound to the enum named `name`, so a
/// hand-built accepted catalog records the identity-aware leaf token (`enum:<id>`) the
/// discharge compares against, not a source spelling.
fn enum_catalog_id(program: &CheckedProgram, name: &str) -> String {
    let enum_fact = program
        .facts
        .enums()
        .iter()
        .find(|enum_fact| enum_fact.name == name)
        .unwrap_or_else(|| panic!("checked enum `{name}`"));
    accepted_catalog_id(&enum_fact.catalog_id, name)
}

/// The stable catalog ids of the enum's members, keyed by member name, so a test can
/// seed a stored enum value (its enum id plus the selected member id) the way the
/// runtime write path does.
fn enum_member_catalog_id(program: &CheckedProgram, enum_name: &str, member: &str) -> String {
    let enum_id = program
        .facts
        .enums()
        .iter()
        .find(|enum_fact| enum_fact.name == enum_name)
        .unwrap_or_else(|| panic!("checked enum `{enum_name}`"))
        .id;
    let member_fact = program
        .facts
        .enum_members()
        .iter()
        .find(|member_fact| member_fact.enum_id == enum_id && member_fact.name == member)
        .unwrap_or_else(|| panic!("checked enum member `{enum_name}::{member}`"));
    accepted_catalog_id(&member_fact.catalog_id, member)
}

fn accepted_catalog_id(id: &Option<String>, label: &str) -> String {
    id.clone()
        .unwrap_or_else(|| panic!("accepted catalog id for `{label}`"))
}

/// Encode a stored enum value as the runtime does: the enum's stable catalog id paired
/// with the selected member's stable catalog id.
fn enum_value_bytes(enum_catalog_id: &str, member_catalog_id: &str) -> Vec<u8> {
    let value = marrow_store::tree::TreeEnumMember::new(
        CatalogId::new(enum_catalog_id).expect("enum catalog id"),
        CatalogId::new(member_catalog_id).expect("enum member catalog id"),
    );
    marrow_store::tree::encode_tree_enum_member(&value).expect("encode enum member")
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
    let program = commit_then_check(&root);
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
    let program = commit_then_check(&root);
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

/// A constant temporal or bytes default written through a validating constructor
/// (`date("...")`, `duration("...")`, `bytes("...")`) is a constant the checker
/// evaluates against the same canonical form a stored value must satisfy, so each
/// discharges to a `Default` backfill rather than forcing a transform.
#[test]
fn constant_temporal_and_bytes_defaults_discharge_as_default() {
    let root = temp_project("discharge-temporal-bytes-default", |root| {
        write(
            root,
            "src/events.mw",
            "module events\n\
             resource Event at ^events(id: int)\n\
             \x20   required title: string\n\
             \x20   required day: date\n\
             \x20   required span: duration\n\
             \x20   required payload: bytes\n\
             evolve\n\
             \x20   default Event.day = date(\"2020-01-01\")\n\
             \x20   default Event.span = duration(\"PT3600S\")\n\
             \x20   default Event.payload = bytes(\"hi\")\n\
             pub fn add(title: string): Id(^events)\n\
             \x20   return nextId(^events)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "events");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // Old records carry `title` but predate the three new required members.
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Launch".into()));

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    let expect_default = |member: &str, expected: Scalar| {
        let member_id = member_catalog_id(&place, member);
        match verdict_for(&result, &member_id) {
            Verdict::Default { value } => {
                assert_eq!(value.scalar_type, expected.ty(), "{member} type");
                assert_eq!(
                    value.encoded,
                    encode_value(&expected).unwrap(),
                    "{member} encoded value",
                );
            }
            other => panic!("expected default for `{member}`, got {other:#?}"),
        }
    };
    // `2020-01-01` is 18262 days after the Unix epoch; one hour is 3.6e12 ns.
    expect_default("day", Scalar::Date(18262));
    expect_default("span", Scalar::Duration(3_600_000_000_000));
    expect_default("payload", Scalar::Bytes(b"hi".to_vec()));
}

/// A `bytes` default takes the argument string's raw UTF-8 bytes, so every string is a
/// valid canonical `bytes` value. The argument is read as a string, not a bytes literal:
/// a backslash-x sequence contributes its literal characters, never a decoded byte. This
/// pins the any-string contract `bytes(string)` carries at runtime.
#[test]
fn bytes_default_accepts_any_string_as_raw_utf8() {
    let root = temp_project("discharge-bytes-default-any-string", |root| {
        write(
            root,
            "src/events.mw",
            "module events\n\
             resource Event at ^events(id: int)\n\
             \x20   required title: string\n\
             \x20   required payload: bytes\n\
             evolve\n\
             \x20   default Event.payload = bytes(\"a\\\\x00b\")\n\
             pub fn add(title: string): Id(^events)\n\
             \x20   return nextId(^events)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "events");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Launch".into()));

    let (result, _diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let member_id = member_catalog_id(&place, "payload");
    match verdict_for(&result, &member_id) {
        Verdict::Default { value } => {
            assert_eq!(value.scalar_type, marrow_store::value::ScalarType::Bytes);
            assert_eq!(
                value.encoded,
                b"a\\x00b".to_vec(),
                "the bytes default is the argument string's literal UTF-8, not a decoded escape",
            );
        }
        other => panic!("expected a bytes default, got {other:#?}"),
    }
}

/// A temporal constructor default whose string is not the canonical saved form is not
/// a value the store could read back, so it fails closed as out of range rather than
/// silently normalizing or accepting it. The validation is exactly the canonical-form
/// boundary stored values pass.
#[test]
fn non_canonical_temporal_default_fails_closed() {
    let root = temp_project("discharge-noncanonical-temporal-default", |root| {
        write(
            root,
            "src/events.mw",
            "module events\n\
             resource Event at ^events(id: int)\n\
             \x20   required title: string\n\
             \x20   required day: date\n\
             evolve\n\
             \x20   default Event.day = date(\"2020-2-30\")\n\
             pub fn add(title: string): Id(^events)\n\
             \x20   return nextId(^events)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "events");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Launch".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let day_id = member_catalog_id(&place, "day");
    assert!(
        matches!(
            verdict_for(&result, &day_id),
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
            .any(|diagnostic| diagnostic.message.contains("out of range")),
        "{diagnostics:#?}"
    );
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
    let program = commit_then_check(&root);
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
        diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("constant") && diagnostic.message.contains("transform")
        }),
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
    let program = commit_then_check(&root);
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
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("1")
                && diagnostic.message.contains("2")
                && diagnostic.message.contains("pages")),
        "{diagnostics:#?}"
    );
}

/// A present required member is not proven unless its bytes decode under the
/// current leaf type. Presence alone would let an old scalar type's bytes activate
/// as the new type and fault later at read time.
#[test]
fn required_present_member_with_incompatible_bytes_repairs() {
    let root = temp_project("discharge-required-invalid-bytes", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: int\n\
             pub fn add(title: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("not an int".into()));

    let (witness, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let title_id = member_catalog_id(&place, "title");
    assert!(!witness.is_activatable(), "{witness:#?}");
    assert!(
        matches!(
            verdict_for(&witness, &title_id),
            Verdict::RepairRequired {
                reason: RepairReason::InvalidStoredValue
            }
        ),
        "{:#?}",
        witness.verdicts
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("title")
                && diagnostic.message.contains("decode")),
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
                member_entry("books::Book::title", &title_id, "string"),
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
    // The rename moves catalog identity only: the cells stay under the same stable id,
    // so the obligation is catalog-only, not a re-proof of the carried-over data.
    assert!(
        matches!(verdict_for(&result, &heading_id), Verdict::CatalogOnly),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.records_to_backfill, 0);
}

/// A member that is BOTH renamed and retyped is transform-required, not a catalog-only
/// move: the rename preserves identity, but the leaf type changed over stored data, so a
/// transform is owed. Here `title: string` data (`Dune`) is renamed onto `count: int`.
/// The type-change steer fires ahead of the rename classification.
#[test]
fn rename_and_retype_requires_transform() {
    let title_id = hex_id(3);
    let root = temp_project("discharge-rename-retype", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required count: int\n\
             evolve\n\
             \x20   rename Book.title -> Book.count\n\
             pub fn add(count: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            6,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                member_entry("books::Book::title", &title_id, "string"),
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
    // The renamed member keeps the old stable id; seed a string under it.
    seed.record(1);
    seed.member_by_id(1, &title_id, Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let count_id = member_catalog_id(&place, "count");
    assert_eq!(count_id, title_id, "rename preserves the stable id");
    assert!(
        matches!(
            verdict_for(&result, &count_id),
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform
            }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(!result.is_activatable(), "{result:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("count")
                && diagnostic.message.contains("transform")),
        "{diagnostics:#?}"
    );
}

/// A store whose identity-key type changed over saved data fails closed. The accepted
/// catalog keyed `^books` records under an `int` identity; source re-keys it to `string`.
/// v0.1 has no graceful store-key migration: re-keying would orphan every record addressed
/// by the old key shape, so the store obligation is `RepairRequired`, never activatable.
#[test]
fn store_identity_key_type_change_fails_closed() {
    let store_id = hex_id(2);
    let root = temp_project("discharge-store-key-type", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: string)\n\
             \x20   required title: string\n\
             pub fn add(id: string, title: string)\n\
             \x20   ^books(id).title = title\n",
        );
        let accepted = CatalogMetadata::new(
            7,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &store_id, "int"),
                member_entry("books::Book::title", &hex_id(3), "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    assert_eq!(
        place.store_catalog_id.as_deref(),
        Some(store_id.as_str()),
        "store keeps its stable id"
    );
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // One record keyed under the old `int` shape, addressed by the preserved store id.
    seed.record(1);

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(
            verdict_for(&result, &store_id),
            Verdict::RepairRequired {
                reason: RepairReason::StoreKeyShapeChange
            }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(!result.is_activatable(), "{result:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == store_id
                && diagnostic.message.contains("identity key")),
        "{diagnostics:#?}"
    );
}

/// A store whose identity-key arity changed (a single key becomes composite) fails closed
/// the same way a key-type change does: the old records are addressed by a narrower key
/// tuple the new schema cannot read, so the store obligation is `RepairRequired`.
#[test]
fn store_identity_key_arity_change_fails_closed() {
    let store_id = hex_id(2);
    let root = temp_project("discharge-store-key-arity", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(shelf: int, id: int)\n\
             \x20   required title: string\n\
             pub fn add(shelf: int, id: int, title: string)\n\
             \x20   ^books(shelf, id).title = title\n",
        );
        let accepted = CatalogMetadata::new(
            8,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &store_id, "int"),
                member_entry("books::Book::title", &hex_id(3), "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    assert_eq!(
        place.store_catalog_id.as_deref(),
        Some(store_id.as_str()),
        "store keeps its stable id"
    );
    let store = TreeStore::memory();

    let (result, _) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(
            verdict_for(&result, &store_id),
            Verdict::RepairRequired {
                reason: RepairReason::StoreKeyShapeChange
            }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(!result.is_activatable(), "{result:#?}");
}

/// An unchanged store identity-key shape places no store obligation: re-running over a
/// store whose accepted key shape still matches source proceeds, so the store id carries
/// no `RepairRequired` verdict.
#[test]
fn store_identity_key_shape_unchanged_is_no_store_repair() {
    let store_id = hex_id(2);
    let root = temp_project("discharge-store-key-unchanged", |root| {
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
            9,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &store_id, "int"),
                member_entry("books::Book::title", &hex_id(3), "string"),
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
    seed.record(1);
    seed.member_by_id(1, &hex_id(3), Scalar::Str("Dune".into()));

    let (result, _) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result
            .verdicts
            .iter()
            .any(|obligation| obligation.catalog_id.as_str() == store_id
                && matches!(obligation.verdict, Verdict::RepairRequired { .. })),
        "an unchanged key shape places no store repair: {:#?}",
        result.verdicts
    );
}

/// Discharge a member that survives under the same name but with a changed leaf type
/// over populated data. The accepted catalog records the old leaf token in its structural
/// signature; source declares `value: {new_type}`; one record is seeded with `old_value`
/// written under the old type. Returns the member catalog id and the preview result so the
/// caller asserts the verdict and diagnostic.
fn retype_preview(
    name: &str,
    accepted_leaf: &str,
    new_type: &str,
    old_value: Scalar,
) -> (
    String,
    EvolutionWitness,
    Vec<marrow_check::evolution::RepairDiagnostic>,
) {
    let value_id = hex_id(3);
    let root = temp_project(name, |root| {
        write(
            root,
            "src/books.mw",
            &format!(
                "module books\n\
                 resource Book at ^books(id: int)\n\
                 \x20   required value: {new_type}\n\
                 pub fn add(value: {new_type}): Id(^books)\n\
                 \x20   return nextId(^books)\n"
            ),
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                member_entry("books::Book::value", &value_id, accepted_leaf),
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
    // Seed the old-type bytes under the preserved member id, exactly as the prior
    // schema's writes did.
    seed.record(1);
    seed.member_by_id(1, &value_id, old_value);

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();
    let value_id = member_catalog_id(&place, "value");
    (value_id, result, diagnostics)
}

/// Assert a populated retype is steered to a transform: a fail-closed
/// `TypeChangeRequiresTransform`, never a silent `DataProof`, and a diagnostic naming
/// the member and a required transform.
fn assert_retype_steered(
    value_id: &str,
    result: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
) {
    assert!(
        matches!(
            verdict_for(result, value_id),
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform
            }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(
        !matches!(verdict_for(result, value_id), Verdict::DataProof),
        "a retype must not be blessed as a data proof"
    );
    assert!(!result.is_activatable(), "{result:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == value_id
                && diagnostic.message.contains("value")
                && diagnostic.message.contains("transform")),
        "{diagnostics:#?}"
    );
}

/// An `int` member retyped to `bool` over a record stored as `1`. The new `bool` decoder
/// would read those bytes as `true`, so a presence-only proof would silently coerce the
/// value; the retype is steered to a transform instead.
#[test]
fn retype_int_to_bool_with_overlapping_byte_is_transform_required() {
    let (value_id, result, diagnostics) =
        retype_preview("discharge-retype-int-bool", "int", "bool", Scalar::Int(1));
    assert_retype_steered(&value_id, &result, &diagnostics);
}

/// A `string` member retyped to `bytes`. Every stored string is also valid bytes, so the
/// new decoder accepts the old data; the reinterpret is steered to a transform.
#[test]
fn retype_string_to_bytes_is_transform_required() {
    let (value_id, result, diagnostics) = retype_preview(
        "discharge-retype-str-bytes",
        "string",
        "bytes",
        Scalar::Str("hi".into()),
    );
    assert_retype_steered(&value_id, &result, &diagnostics);
}

/// An `int` member retyped to `decimal` over a record stored as `5`. The canonical
/// decimal text overlaps the integer text, so the new decoder reads the old bytes; the
/// retype is steered to a transform rather than blessed.
#[test]
fn retype_int_to_decimal_with_overlapping_text_is_transform_required() {
    let (value_id, result, diagnostics) = retype_preview(
        "discharge-retype-int-decimal",
        "int",
        "decimal",
        Scalar::Int(5),
    );
    assert_retype_steered(&value_id, &result, &diagnostics);
}

/// An OPTIONAL member retyped over populated data is steered to a transform too: the
/// reinterpret hole is not limited to required leaves. An optional `int` stored as `1`
/// retyped to `bool` would silently read `true`, so it fails closed with a transform
/// steer rather than the no-op an optional add would otherwise be.
#[test]
fn retype_optional_member_with_data_is_transform_required() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-retype-optional", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   value: bool\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                member_entry("books::Book::value", &value_id, "int"),
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
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Int(1));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert_retype_steered(&value_id, &result, &diagnostics);
}

/// An optional member retyped with NO stored data is harmless: there are no bytes to
/// reinterpret, so it stays a no-op rather than forcing a transform.
#[test]
fn retype_optional_member_without_data_is_no_op() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-retype-optional-empty", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   value: bool\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                member_entry("books::Book::title", &hex_id(5), "string"),
                member_entry("books::Book::value", &value_id, "int"),
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
    // A record exists and carries the unchanged required `title`, but no `value` cell —
    // so the retyped optional member has no bytes to reinterpret.
    seed.record(1);
    seed.member_by_id(1, &hex_id(5), Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert!(
        matches!(verdict_for(&result, &value_id), Verdict::NoOp),
        "{:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A member whose declared type is unchanged still proves cleanly: a populated required
/// member whose accepted leaf matches the source leaf is a `DataProof`, with no false
/// type-change positive.
#[test]
fn unchanged_type_still_proves_data() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-unchanged-type", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required value: int\n\
             pub fn add(value: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                member_entry("books::Book::value", &value_id, "int"),
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
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Int(7));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert!(
        matches!(verdict_for(&result, &value_id), Verdict::DataProof),
        "{:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A brand-new member — one the accepted catalog never recorded — is unaffected by the
/// type-change check: it carries no accepted leaf, so its optional sparse addition stays
/// a no-op rather than reading as a retype.
#[test]
fn brand_new_member_is_not_a_retype() {
    let root = temp_project("discharge-new-member", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   rank: int\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the baseline so `title` is accepted, then a fresh check adds `rank`.
    let program = commit_then_check(&root);
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

    let rank_id = member_catalog_id(&place, "rank");
    assert!(
        matches!(verdict_for(&result, &rank_id), Verdict::NoOp),
        "{:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A populated scalar member retyped to an enum is steered to a transform: the leaf
/// kind changed (`int` -> `Status`), so the stored integer bytes must not be reread as an
/// enum member. Retype detection is total over leaf kind, not scalar-only.
#[test]
fn retype_scalar_to_enum_is_transform_required() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-retype-scalar-enum", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book at ^books(id: int)\n\
             \x20   required value: Status\n\
             pub fn add(value: Status): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                member_entry("books::Book::value", &value_id, "int"),
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
    // Seed the integer bytes the old `int` schema wrote under the preserved member id.
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Int(1));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert_retype_steered(&value_id, &result, &diagnostics);
}

/// A populated scalar member retyped to a store identity is steered to a transform: the
/// leaf kind changed (`int` -> `Id(^books)`), so the stored integer must not be reread as
/// a reference payload.
#[test]
fn retype_scalar_to_identity_is_transform_required() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-retype-scalar-identity", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required value: Id(^books)\n\
             pub fn add(value: Id(^books)): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                member_entry("books::Book::value", &value_id, "int"),
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
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Int(1));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert_retype_steered(&value_id, &result, &diagnostics);
}

/// A populated enum member retyped to a store identity is steered to a transform: a change
/// between two non-scalar leaf kinds (`Status` -> `Id(^books)`) is a retype like any other,
/// so the stored enum-member payload must not be reread as a reference.
#[test]
fn retype_enum_to_identity_is_transform_required() {
    let value_id = hex_id(3);
    let enum_stable = hex_id(7);
    let root = temp_project("discharge-retype-enum-identity", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book at ^books(id: int)\n\
             \x20   required value: Id(^books)\n\
             pub fn add(value: Id(^books)): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        // The accepted catalog records the member's leaf as the enum's stable identity; the
        // source now types it `Id(^books)`, so the identity-aware tokens differ and it is a
        // retype across two non-scalar leaf kinds.
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                entry(CatalogEntryKind::Enum, "books::Status", &enum_stable),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::draft",
                    &hex_id(8),
                ),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::shipped",
                    &hex_id(9),
                ),
                member_entry(
                    "books::Book::value",
                    &value_id,
                    &format!("enum:{enum_stable}"),
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
    // Seed an identity payload — valid bytes for the NEW type — so the case turns on the
    // declared-type change, not on a decode failure: even bytes the new decoder accepts
    // must steer to a transform when the leaf kind changed.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, encode_identity_payload(&[SavedKey::Int(1)]));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert_retype_steered(&value_id, &result, &diagnostics);
}

/// A populated leaf member with NO recorded accepted leaf type fails closed: the prior
/// type is unknown, so the stored bytes cannot be proven safe to reread and the obligation
/// is steered to a transform rather than silently coerced through a data proof.
#[test]
fn populated_member_with_unknown_accepted_leaf_fails_closed() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-unknown-accepted-leaf", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required value: bool\n\
             pub fn add(value: bool): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        // The accepted member entry exists but records no structural signature, so its leaf
        // token reads back as unknown: an entry minted before signatures were recorded. Its
        // prior type cannot be proven.
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::value",
                    &value_id,
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
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Bool(true));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert_retype_steered(&value_id, &result, &diagnostics);
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
    let store_id = CatalogId::new(accepted_catalog_id(&place.store_catalog_id, "store")).unwrap();
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
    let program = commit_then_check(&root);
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
    let program = commit_then_check(&root);
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
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("byIsbn")),
        "{diagnostics:#?}"
    );
}

/// A checked transform computing a new member from a sibling discharges to an
/// applyable transform verdict carrying the read-member catalog ids. The read member
/// `price` decodes under its current type, so the transform is activatable and the
/// verdict names the ids apply reads to build the `old` binding.
#[test]
fn transform_from_decodable_sibling_is_applyable() {
    let root = temp_project("discharge-transform-applyable", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "price", Scalar::Int(3));
    seed.member(1, "priceCents", Scalar::Int(300));

    let result = witness(&program, &store);

    let cents_id = member_catalog_id(&place, "priceCents");
    let price_id = member_catalog_id(&place, "price");
    fs::remove_dir_all(&root).ok();

    assert!(result.is_activatable(), "{result:#?}");
    match verdict_for(&result, &cents_id) {
        Verdict::Transform { reads } => assert!(
            reads.iter().any(|id| id.as_str() == price_id),
            "transform reads must name `price`: {reads:#?}"
        ),
        other => panic!("expected transform, got {other:#?}"),
    }
}

/// A transform body whose read member does not decode under its current type fails
/// closed: the read member's stored bytes were written under an incompatible type, so
/// reading `old.<member>` is unsound. The transform target discharges to a blocking
/// repair (it cannot be recomputed) and the witness is not activatable.
#[test]
fn transform_undecodable_read_member_fails_closed() {
    let root = temp_project("discharge-transform-undecodable", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // `price` was written as a string under its old type; it cannot decode as the
    // current `int`, so reading `old.price` is unsound and the transform must block.
    seed.record(1);
    seed.member(1, "price", Scalar::Str("not-an-int".into()));
    seed.member(1, "priceCents", Scalar::Int(0));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    let cents_id = member_catalog_id(&place, "priceCents");
    fs::remove_dir_all(&root).ok();

    assert!(!result.is_activatable(), "{result:#?}");
    assert!(
        matches!(
            verdict_for(&result, &cents_id),
            Verdict::RepairRequired {
                reason: RepairReason::UndecodableTransformInput
            }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("transform") && diagnostic.message.contains("decode")
        }),
        "{diagnostics:#?}"
    );
}

/// A transform body that performs a saved write is impure and rejected at check time.
#[test]
fn transform_saved_write_is_check_error() {
    let root = temp_project("discharge-transform-impure-write", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       ^books(1).price = 9\n\
             \x20       return old.price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected an impure-transform error: {:#?}",
        report.diagnostics
    );
}

/// A transform body whose result type does not match the target member type is a
/// check error.
#[test]
fn transform_return_type_mismatch_is_check_error() {
    let root = temp_project("discharge-transform-rettype", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return \"a string\"\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report.has_errors(),
        "expected a return-type error: {:#?}",
        report.diagnostics
    );
}

/// Reading the transform's own target via `old.<target>` is a check error: the target
/// is the value being replaced, so its old bytes are not a sound input.
#[test]
fn transform_reading_own_target_is_check_error() {
    let root = temp_project("discharge-transform-readself", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.priceCents * 2\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a read-own-target error: {:#?}",
        report.diagnostics
    );
}

/// Reading another transform's target via `old.<member>` is a check error: that
/// member's old bytes are about to be rewritten by its own transform, so they are not
/// a sound input for this one.
#[test]
fn transform_reading_other_transform_target_is_check_error() {
    let root = temp_project("discharge-transform-readother", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required a: int\n\
             \x20   required b: int\n\
             evolve\n\
             \x20   transform Book.a\n\
             \x20       return 1\n\
             \x20   transform Book.b\n\
             \x20       return old.a + 1\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a read-other-transform-target error: {:#?}",
        report.diagnostics
    );
}

/// A transform body that directly reads a saved root (`^books(1).price`) is impure
/// and rejected at check time. Such a read escapes the per-record `old` model and the
/// decodability proof: it would let one record's value be written to every record. A
/// transform body may only read `old`.
#[test]
fn transform_reading_saved_root_is_check_error() {
    let root = temp_project("discharge-transform-savedread", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return ^books(1).price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a saved-read impurity error: {:#?}",
        report.diagnostics
    );
}

/// Reading `old.<member>` of a member a `default` in the same evolve block rewrites is
/// a check error: `old` exposes the pre-evolution value, not the post-default value the
/// developer intends, so the transform would compute from a value the same evolution is
/// changing.
#[test]
fn transform_reading_same_block_default_target_is_check_error() {
    let root = temp_project("discharge-transform-readdefault", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required base: int\n\
             \x20   required total: int\n\
             evolve\n\
             \x20   default Book.base = 10\n\
             \x20   transform Book.total\n\
             \x20       return old.base + 1\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a read-of-same-block-default-target error: {:#?}",
        report.diagnostics
    );
}

/// A transform target must be a top-level saved resource member: read resolution and
/// the per-record write address only handle a plain top-level field, so a nested target
/// (`Book.name.first`) is rejected at check time rather than silently mis-resolving.
#[test]
fn transform_of_nested_member_is_check_error() {
    let root = temp_project("discharge-transform-nested", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   name\n\
             \x20       required first: string\n\
             \x20       required last: string\n\
             evolve\n\
             \x20   transform Book.name.first\n\
             \x20       return \"x\"\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a nested-target error: {:#?}",
        report.diagnostics
    );
}

/// The witness composes the existing fingerprints: the accepted and proposal
/// catalog epoch/digest, the store engine profile + commit id, and the affected
/// catalog ids.
#[test]
fn witness_composes_catalog_and_store_fingerprints() {
    // Commit a first schema, then add an optional member so the next check proposes
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
    let accepted = commit_then_check(&root);
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
    // Adding `subtitle` without committing the new identity is exactly the pending
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

/// Dropping a sparse source field that nothing else depends on is a legal no-op. The
/// accepted entry lingers as data under its stable id, so the verdict is a no-op, not
/// an error and not a distinct deprecation outcome.
#[test]
fn dropped_sparse_field_is_no_op_not_error() {
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
        matches!(verdict_for(&result, &subtitle_id), Verdict::NoOp),
        "{:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("subtitle")),
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
            .any(|diagnostic| diagnostic.message.contains("byIsbn")
                && diagnostic.message.contains("retire")),
        "{diagnostics:#?}"
    );
}

#[test]
fn dropped_field_ignores_same_named_index_on_another_resource() {
    let book_subtitle_id = hex_id(5);
    let root = temp_project("discharge-f12-index-owner", |root| {
        write(
            root,
            "src/media.mw",
            "module media\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             resource Movie at ^movies(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             \x20   index bySubtitle(subtitle) unique\n",
        );
        let accepted = CatalogMetadata::new(
            12,
            vec![
                entry(CatalogEntryKind::Resource, "media::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "media::^books", &hex_id(2)),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Book::subtitle",
                    &book_subtitle_id,
                ),
                entry(CatalogEntryKind::Resource, "media::Movie", &hex_id(6)),
                entry(CatalogEntryKind::Store, "media::^movies", &hex_id(7)),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Movie::title",
                    &hex_id(8),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Movie::subtitle",
                    &hex_id(9),
                ),
                entry(
                    CatalogEntryKind::StoreIndex,
                    "media::^movies::bySubtitle",
                    &hex_id(10),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    checked(&root);
    write(
        &root,
        "src/media.mw",
        "module media\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         resource Movie at ^movies(id: int)\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         \x20   index bySubtitle(subtitle) unique\n",
    );
    let (_report, program) = check_project(&root, &config()).expect("check");
    let store = TreeStore::memory();
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(verdict_for(&result, &book_subtitle_id), Verdict::NoOp),
        "{result:#?}"
    );
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("bySubtitle")),
        "{diagnostics:#?}"
    );
}

/// Dropping a source index while keeping the member it covered is an index-subtree
/// deletion, not a silent no-op: the index binding is gone but its cells would linger.
/// The accepted catalog carries a member `isbn` and an index `byIsbn(isbn)`; current
/// source keeps `isbn` and drops the index, so discharge classifies the dropped index
/// id as `IndexDropped` and tags it as a changed index id apply deletes.
#[test]
fn dropped_index_is_index_dropped() {
    let index_id = hex_id(5);
    let root = temp_project("discharge-drop-index", |root| {
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
            13,
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
                    &hex_id(4),
                ),
                entry(
                    CatalogEntryKind::StoreIndex,
                    "books::^books::byIsbn",
                    &index_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    // The accepted catalog matches source, so this first check is clean. Then drop the
    // index from source while keeping `isbn`, so only the index binding disappears.
    checked(&root);
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required isbn: string\n\
         pub fn add(title: string, isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let (_report, program) = check_project(&root, &config()).expect("check");
    let store = TreeStore::memory();
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(verdict_for(&result, &index_id), Verdict::IndexDropped),
        "{:#?}",
        result.verdicts
    );
    assert!(
        result
            .changed_index_catalog_ids
            .iter()
            .any(|id| id.as_str() == index_id),
        "dropped index id is tagged as a changed index id: {:#?}",
        result.changed_index_catalog_ids
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
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
    let program = commit_then_check(&root);
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
    let program = commit_then_check(&root);
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
    let program = commit_then_check(&root);
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
    let program = commit_then_check(&root);
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
    let program = commit_then_check(&root);
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
    let program = commit_then_check(&root);
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
    let program = commit_then_check(&root);
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

/// The shape and evolution digests of a single-file source, computed against the
/// source's own accepted catalog so its evolve default and transform targets bind to
/// real stable ids. The shape digest is the one the store stamps and the fence
/// enforces; the evolution digest is the one the witness records.
fn digests(name: &str, source: &str) -> (String, String) {
    let root = temp_project(name, |root| write(root, "src/books.mw", source));
    let program = commit_then_check(&root);
    let store = TreeStore::memory();
    let witness = witness(&program, &store);
    fs::remove_dir_all(&root).ok();
    (witness.source_digest, witness.evolution_digest)
}

/// The store-stamp shape digest.
fn source_digest(name: &str, source: &str) -> String {
    digests(name, source).0
}

/// The witness evolution digest.
fn evolution_digest(name: &str, source: &str) -> String {
    digests(name, source).1
}

/// The store-stamp shape digest binds the durable shape, not the transient evolve
/// block: editing only the evolve decision surface (a default value, a transform body)
/// leaves the shape digest unchanged, so a consumed block is deletable without reading
/// as schema drift. A change that touches the shape — a module const a transform reads,
/// an optional/required toggle — drifts it, because the store must satisfy that shape.
#[test]
fn shape_digest_binds_shape_and_not_the_evolve_block() {
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

    let base_digest = source_digest("shape-base", base);
    assert_eq!(
        base_digest,
        source_digest("shape-default", changed_default),
        "a changed evolve default value must not drift the shape digest"
    );
    assert_eq!(
        base_digest,
        source_digest("shape-transform", changed_transform),
        "a changed transform body must not drift the shape digest"
    );
    let const_transform = "module books\n\
         const Scale = 1\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         evolve\n\
         \x20   transform Book.pages\n\
         \x20       return Scale\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let changed_const = "module books\n\
         const Scale = 2\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         evolve\n\
         \x20   transform Book.pages\n\
         \x20       return Scale\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    assert_ne!(
        source_digest("shape-const-base", const_transform),
        source_digest("shape-const", changed_const),
        "a changed module const is part of the shape and must drift the shape digest"
    );
    assert_ne!(
        base_digest,
        source_digest("shape-required", required_pages),
        "an optional->required toggle must drift the shape digest"
    );

    // The witness evolution digest, in contrast, binds the evolve decision surface, so a
    // changed default and a changed transform body each drift it. This is what keeps
    // apply fencing a transform-body edit between preview and apply.
    let base_evolution = evolution_digest("evolution-base", base);
    assert_ne!(
        base_evolution,
        evolution_digest("evolution-default", changed_default),
        "a changed default value must drift the evolution digest"
    );
    assert_ne!(
        base_evolution,
        evolution_digest("evolution-transform", changed_transform),
        "a changed transform body must drift the evolution digest"
    );
}

/// The shape digest binds the whole durable shape, with no enumeration gap. It is
/// computed from the canonical normalized rendering of every shape declaration, so any
/// change to a member type, a required flag, an identity key, an index, a keyed-layer
/// key at any nesting depth, or a top-level keyed-leaf key must drift it, while a pure
/// whitespace reformat of the same declarations must leave it unchanged.
///
/// The evolve decision surface — a default value, a transform body — is *not* shape:
/// editing it leaves the shape digest unchanged but drifts the evolution digest the
/// witness records. The two are asserted together so the boundary is explicit.
///
/// The single baseline carries every dimension once. Each variant edits exactly one
/// fact at the same catalog path, so a digest that still matched the baseline would
/// prove that fact is unbound.
#[test]
fn source_digest_binds_the_durable_shape() {
    let base = durable_fixture(DurableFixture::default());
    let base_digest = source_digest("durable-base", &base);

    let shape_cases: [(&str, DurableFixture, &str); 8] = [
        (
            "member-type",
            DurableFixture {
                count_type: "string",
                ..DurableFixture::default()
            },
            "a member scalar-type change must drift the shape digest",
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
            "an identity-key scalar-type change must drift the shape digest",
        ),
        (
            "index-unique",
            DurableFixture {
                index_unique: false,
                ..DurableFixture::default()
            },
            "an index uniqueness flip must drift the shape digest",
        ),
        (
            "index-columns",
            DurableFixture {
                index_columns: "count, id",
                ..DurableFixture::default()
            },
            "an index key-columns change must drift the shape digest",
        ),
        (
            "keyed-group-arity",
            DurableFixture {
                versions_keys: "version: int, draft: int",
                ..DurableFixture::default()
            },
            "a keyed-group key arity change must drift the shape digest",
        ),
        (
            "keyed-group-type",
            DurableFixture {
                versions_keys: "version: string",
                ..DurableFixture::default()
            },
            "a keyed-group key scalar-type change must drift the shape digest",
        ),
        (
            "keyed-leaf-type",
            DurableFixture {
                tags_keys: "pos: string",
                ..DurableFixture::default()
            },
            "a top-level keyed-leaf key scalar-type change must drift the shape digest",
        ),
        (
            "optional-toggle",
            DurableFixture {
                count_required: false,
                ..DurableFixture::default()
            },
            "an optional->required toggle must drift the shape digest",
        ),
    ];

    for (name, fixture, message) in shape_cases {
        let digest = source_digest(&format!("durable-{name}"), &durable_fixture(fixture));
        assert_ne!(base_digest, digest, "{message}");
    }

    // The evolve decision surface does not change the shape, so the shape digest is
    // stable, but the evolution digest the witness records must drift.
    let base_evolution = evolution_digest("durable-evolution-base", &base);
    let evolve_cases: [(&str, DurableFixture, &str); 2] = [
        (
            "default-value",
            DurableFixture {
                default_value: "1",
                ..DurableFixture::default()
            },
            "an evolve default value change",
        ),
        (
            "transform-body",
            DurableFixture {
                transform_body: "return \"y\"",
                ..DurableFixture::default()
            },
            "an evolve transform body change",
        ),
    ];
    for (name, fixture, change) in evolve_cases {
        let source = durable_fixture(fixture);
        assert_eq!(
            base_digest,
            source_digest(&format!("durable-shape-{name}"), &source),
            "{change} must not drift the shape digest"
        );
        assert_ne!(
            base_evolution,
            evolution_digest(&format!("durable-evolution-{name}"), &source),
            "{change} must drift the evolution digest"
        );
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

/// The shape digest is derived by re-formatting each declaration through the frozen
/// normalized formatter, so it must depend on the declared shape alone, not on the
/// author's source layout. A formatter-internal layout change — blank lines between and
/// inside declarations, and wider indentation, all of which the normalized formatter
/// collapses — must leave the digest exactly where it was. This pins the activation
/// fence to the declared shape rather than to incidental whitespace, so reformatting a
/// committed source never reads as schema drift.
///
/// The messy source is hand-written rather than produced by the formatter so the input
/// is genuinely non-canonical: the formatter could not emit it, and only the normalized
/// rendering brings it back to the canonical baseline.
#[test]
fn formatter_internal_layout_change_does_not_move_shape_digest() {
    let canonical = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    // Extra blank lines between and inside declarations, plus eight-space indentation,
    // none of which the normalized formatter preserves.
    let messy = "module books\n\
         \n\
         \n\
         resource Book at ^books(id: int)\n\
         \n\
         \x20       required title: string\n\
         \n\
         \x20       pages: int\n\
         \n\
         \n\
         pub fn add(title: string): Id(^books)\n\
         \x20       return nextId(^books)\n";

    assert_eq!(
        source_digest("layout-canonical", canonical),
        source_digest("layout-messy", messy),
        "a formatter-internal layout change must not move the shape digest"
    );
}

/// An enum's members are durable shape: each is a catalog entry a stored snapshot binds.
/// Adding, removing, or reordering a member drifts the shape digest, because the stored
/// shape no longer matches. A pure layout reformat of the same members — blank lines
/// between them and wider indentation, all of which the normalized formatter collapses —
/// must leave the digest exactly where it was. This proves the frozen-anchor claim for
/// enum members directly, not just for resource declarations.
#[test]
fn enum_member_shape_drifts_digest_but_layout_does_not() {
    let base = "module books\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         fn s(): bool\n\
         \x20   return true\n";
    let base_digest = source_digest("enum-base", base);

    let added_member = "module books\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         \x20   deleted\n\
         fn s(): bool\n\
         \x20   return true\n";
    assert_ne!(
        base_digest,
        source_digest("enum-added", added_member),
        "adding an enum member must drift the shape digest"
    );

    let reordered = "module books\n\
         enum Status\n\
         \x20   archived\n\
         \x20   active\n\
         fn s(): bool\n\
         \x20   return true\n";
    assert_ne!(
        base_digest,
        source_digest("enum-reordered", reordered),
        "reordering enum members must drift the shape digest"
    );

    // Extra blank lines and eight-space indentation around the same members, none of
    // which the normalized formatter preserves.
    let messy = "module books\n\
         \n\
         enum Status\n\
         \n\
         \x20       active\n\
         \n\
         \x20       archived\n\
         \n\
         fn s(): bool\n\
         \x20       return true\n";
    assert_eq!(
        base_digest,
        source_digest("enum-messy", messy),
        "an enum-member layout change must not move the shape digest"
    );
}

/// A frozen golden over a fixed canonical shape. The shape digest is stamped into every
/// store and enforced by the activation fence, so the canonical rendering it hashes must
/// not move silently: a formatter change that altered the normalized text of an unchanged
/// shape — different indentation, blank-line policy, or token spacing — would move every
/// committed snapshot's digest and read live stores as schema drift. The other digest
/// tests only compare digests within one run, so both sides would shift together and hide
/// such a change. This pins the exact value, so a deliberate formatter change surfaces as
/// a golden move to review rather than slipping through. Update the golden only alongside
/// a reviewed change to the durable rendering.
#[test]
fn shape_digest_is_a_frozen_golden() {
    let source = "module books\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         fn s(): bool\n\
         \x20   return true\n";
    assert_eq!(
        source_digest("golden-shape", source),
        "sha256:531be928b3fe8d46135633888c6ec346e4cb219928a57777cb60bc16d9d88eb9",
        "the canonical shape rendering moved; update the golden only with a reviewed \
         change to the durable rendering"
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

/// A pure enum rename (`Status` -> `State`) is NOT a retype. The member keeps referencing
/// the same enum stable identity, so the identity-aware leaf token is unchanged across the
/// rename and a populated record discharges as a clean `DataProof`, never a false
/// `TypeChangeRequiresTransform`. This is the regression the spelling-based comparison
/// caused: `Type::Display` rendered `Status` and `State` as different, blocking a legal
/// rename.
#[test]
fn enum_rename_is_not_a_retype() {
    let value_id = hex_id(3);
    let enum_stable = hex_id(7);
    let draft_member = hex_id(8);
    let shipped_member = hex_id(9);
    let root = temp_project("discharge-enum-rename", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum State\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book at ^books(id: int)\n\
             \x20   required value: State\n\
             evolve\n\
             \x20   rename Status -> State\n\
             pub fn add(value: State): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        // The accepted catalog records the enum under the OLD spelling `Status` with the
        // stable id the rename preserves, and the member's accepted leaf token references
        // that same enum identity.
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                entry(CatalogEntryKind::Enum, "books::Status", &enum_stable),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::draft",
                    &draft_member,
                ),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::shipped",
                    &shipped_member,
                ),
                member_entry(
                    "books::Book::value",
                    &value_id,
                    &format!("enum:{enum_stable}"),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    // The rename preserves the enum's stable id, so the bound enum id matches the accepted.
    assert_eq!(
        enum_catalog_id(&program, "State"),
        enum_stable,
        "rename preserves the enum stable id"
    );
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // Seed the stored `draft` value written under the prior member identity. The decisive
    // check is the leaf token: the enum's stable id is preserved across the rename, so this
    // is not a retype and the populated record proves cleanly.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, enum_value_bytes(&enum_stable, &draft_member));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert!(
        matches!(verdict_for(&result, &value_id), Verdict::DataProof),
        "a pure enum rename must not read as a retype: {:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// Redefining an enum under the same source spelling fails closed when a stored
/// member is no longer a member of the current enum. The enum keeps its stable
/// identity, so the leaf token is unchanged and this is not a retype, but the
/// stored value cannot be reread as the current enum.
#[test]
fn enum_member_removed_fails_closed() {
    let value_id = hex_id(3);
    let enum_stable = hex_id(7);
    let root = temp_project("discharge-enum-redefine", |root| {
        // Current source declares `Status` with only `draft`: `shipped` was removed.
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             resource Book at ^books(id: int)\n\
             \x20   required value: Status\n\
             pub fn add(value: Status): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                entry(CatalogEntryKind::Enum, "books::Status", &enum_stable),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::draft",
                    &hex_id(8),
                ),
                member_entry(
                    "books::Book::value",
                    &value_id,
                    &format!("enum:{enum_stable}"),
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
    // Seed a stored value whose member id is NOT a member of the current `Status`: a
    // removed `shipped`. A made-up member catalog id stands in for the retired member.
    let removed_member = hex_id(9);
    seed.record(1);
    seed.member_bytes_by_id(
        1,
        &value_id,
        enum_value_bytes(&enum_stable, &removed_member),
    );

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert!(
        matches!(
            verdict_for(&result, &value_id),
            Verdict::RepairRequired {
                reason: RepairReason::InvalidStoredValue
            }
        ),
        "a stored enum value no longer a member of the current enum must fail closed: {:#?}",
        result.verdicts
    );
    assert!(!result.is_activatable(), "{result:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == value_id),
        "{diagnostics:#?}"
    );
}

/// A REQUIRED enum leaf is presence- and decode-scanned exactly like a required scalar: a
/// record missing its enum cell fails closed. Before the total-scan fix, a required
/// non-scalar leaf was never scanned, so a missing required enum slipped through.
#[test]
fn required_enum_leaf_missing_fails_closed() {
    let root = temp_project("discharge-required-enum-missing", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required state: Status\n\
             pub fn add(title: string, state: Status): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the baseline so the enum and member ids are accepted, then exercise an old
    // snapshot that predates the required enum member.
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // The record carries `title` but no `state` cell: the required enum is missing.
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let state_id = member_catalog_id(&place, "state");
    assert!(
        matches!(
            verdict_for(&result, &state_id),
            Verdict::RepairRequired {
                reason: RepairReason::MissingRequiredMember
            }
        ),
        "a missing required enum leaf must fail closed: {:#?}",
        result.verdicts
    );
    assert!(!result.is_activatable(), "{result:#?}");
    assert!(!diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A REQUIRED identity leaf is presence- and decode-scanned like a required
/// scalar: a record missing its identity cell fails closed.
#[test]
fn required_identity_leaf_missing_fails_closed() {
    let root = temp_project("discharge-required-identity-missing", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Author at ^authors(id: int)\n\
             \x20   required name: string\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required author: Id(^authors)\n\
             pub fn add(title: string, author: Id(^authors)): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // The record carries `title` but no `author` cell: the required identity is missing.
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let author_id = member_catalog_id(&place, "author");
    assert!(
        matches!(
            verdict_for(&result, &author_id),
            Verdict::RepairRequired {
                reason: RepairReason::MissingRequiredMember
            }
        ),
        "a missing required identity leaf must fail closed: {:#?}",
        result.verdicts
    );
    assert!(!result.is_activatable(), "{result:#?}");
    assert!(!diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A present, valid required enum leaf discharges as a clean `DataProof`: the total scan
/// proves the cell present and decodable, and the stored member is a member of the current
/// enum.
#[test]
fn required_enum_leaf_present_proves_data() {
    let root = temp_project("discharge-required-enum-present", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book at ^books(id: int)\n\
             \x20   required state: Status\n\
             pub fn add(state: Status): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    let state_id = member_catalog_id(&place, "state");
    let enum_id = enum_catalog_id(&program, "Status");
    let draft = enum_member_catalog_id(&program, "Status", "draft");
    seed.record(1);
    seed.member_bytes_by_id(1, &state_id, enum_value_bytes(&enum_id, &draft));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(verdict_for(&result, &state_id), Verdict::DataProof),
        "a present valid required enum leaf proves cleanly: {:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A retype from one enum to a DIFFERENT enum (`Status` -> `Kind`) is a real retype: the
/// identity-aware token differs (each names a distinct enum stable id), so a populated
/// record is steered to a transform. Identity awareness must not over-collapse: distinct
/// enums are distinct leaf types.
#[test]
fn retype_enum_a_to_enum_b_is_transform_required() {
    let value_id = hex_id(3);
    let status_stable = hex_id(7);
    let root = temp_project("discharge-retype-enum-enum", |root| {
        // Source now types `value: Kind`; the accepted catalog had it as enum `Status`.
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Kind\n\
             \x20   alpha\n\
             \x20   beta\n\
             resource Book at ^books(id: int)\n\
             \x20   required value: Kind\n\
             pub fn add(value: Kind): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &hex_id(2)),
                entry(CatalogEntryKind::Enum, "books::Status", &status_stable),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::draft",
                    &hex_id(8),
                ),
                member_entry(
                    "books::Book::value",
                    &value_id,
                    &format!("enum:{status_stable}"),
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
    // Seed a stored value of the OLD enum `Status`; its bytes must not be reread as `Kind`.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, enum_value_bytes(&status_stable, &hex_id(8)));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "value");
    assert_retype_steered(&value_id, &result, &diagnostics);
}

/// A pure store rename behind an identity leaf (`Id(^books)` -> `Id(^library)`) is NOT a
/// retype: the referenced store keeps its stable identity, so the identity-aware token is
/// unchanged and a populated record discharges cleanly. The spelling-based comparison
/// rendered `Id(^books)` and `Id(^library)` as different and falsely blocked the rename.
#[test]
fn store_rename_behind_identity_leaf_is_not_a_retype() {
    let value_id = hex_id(3);
    let store_stable = hex_id(2);
    let root = temp_project("discharge-store-rename", |root| {
        // The store root is renamed `^books` -> `^library`; a self-referential identity
        // leaf follows it. The resource's own store is renamed in lockstep.
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^library(id: int)\n\
             \x20   required parent: Id(^library)\n\
             evolve\n\
             \x20   rename ^books -> ^library\n\
             pub fn add(parent: Id(^library)): Id(^library)\n\
             \x20   return nextId(^library)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                entry(CatalogEntryKind::Store, "books::^books", &store_stable),
                member_entry(
                    "books::Book::parent",
                    &value_id,
                    &format!("id:{store_stable}:1"),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "library");
    assert_eq!(
        place.store_catalog_id.as_deref(),
        Some(store_stable.as_str()),
        "store rename preserves the store stable id"
    );
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // Seed a valid identity payload for the renamed store.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, encode_identity_payload(&[SavedKey::Int(1)]));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    let value_id = member_catalog_id(&place, "parent");
    assert!(
        matches!(verdict_for(&result, &value_id), Verdict::DataProof),
        "a pure store rename behind an identity leaf must not read as a retype: {:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A keyed-leaf-layer (`map[K, V]`) VALUE type change over a populated map fails closed,
/// exactly as a top-level leaf retype does: the stored bytes were written under the old V
/// type, so the new type's decoder would silently reinterpret them. The map field is the
/// leaf, so its V type carries an identity-aware accepted leaf token the discharge compares
/// against; a populated re-typed map value is steered to a transform rather than activated.
#[test]
fn keyed_leaf_map_value_retype_over_populated_map_fails_closed() {
    let map_stable = hex_id(3);
    let root = temp_project("discharge-map-value-retype", |root| {
        // The map value type changes `string` -> `int`; its entries were written as strings.
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   tags(pos: int): int\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            3,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &hex_id(2), "int"),
                member_entry("books::Book::tags", &map_stable, "[int]string"),
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
    // One record with a map entry whose value was stored as a `string`.
    seed.record(1);
    seed.keyed_leaf(
        1,
        "tags",
        SavedKey::Int(0),
        encode_value(&Scalar::Str("draft".into())).unwrap(),
    );

    let map_id = keyed_leaf_catalog_id(&place, "tags");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a populated map value-type change must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &map_id),
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform
            }
        ),
        "the map value retype must steer to a transform, got {:#?}",
        verdict_for(&result, &map_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == map_id),
        "a fail-closed diagnostic must name the map value, got {diagnostics:#?}"
    );
}

/// A keyed-leaf-layer (`map[K, V]`) whose value type is unchanged proves cleanly over a
/// populated map: the stored value decodes under the current V type, so there is no
/// reinterpretation hazard and the change is activatable. This pins that recording an
/// accepted leaf token for map values does not block an honest no-change map.
#[test]
fn keyed_leaf_map_value_unchanged_proves() {
    let root = temp_project("discharge-map-value-unchanged", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   tags(pos: int): string\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the schema so the map's catalog id addresses the store; then exercise an
    // unchanged re-preview over a populated map.
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.keyed_leaf(
        1,
        "tags",
        SavedKey::Int(0),
        encode_value(&Scalar::Str("draft".into())).unwrap(),
    );

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        result.is_activatable(),
        "an unchanged map value must stay activatable: {:#?}",
        result.verdicts
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A brand-new required scalar member added over a populated store with no `evolve default`
/// and no transform fails closed: the existing records lack it, and there is nothing to
/// backfill them with, so the add-required-field obligation is unmet. The new member has no
/// accepted catalog id yet, so the presence scan must be proposal-aware to reach it at all —
/// keying off the accepted ids alone orphaned the requiredness and silently activated.
#[test]
fn brand_new_required_member_over_populated_store_fails_closed() {
    let title_stable = hex_id(3);
    let root = temp_project("discharge-new-required-no-default", |root| {
        // `pages` is brand-new in source and not in the accepted catalog.
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
        let accepted = CatalogMetadata::new(
            3,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &hex_id(2), "int"),
                member_entry("books::Book::title", &title_stable, "string"),
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
    // Old records carry `title` but predate the brand-new required `pages`.
    seed.record(1);
    seed.member_by_id(1, &title_stable, Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member_by_id(2, &title_stable, Scalar::Str("Hyperion".into()));

    let pages_id = new_member_proposal_id(&program, "books::Book::pages");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a brand-new required member with no default over a populated store must block: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &pages_id),
            Verdict::RepairRequired {
                reason: RepairReason::MissingRequiredMember
            }
        ),
        "the brand-new required member must fail closed, got {:#?}",
        verdict_for(&result, &pages_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == pages_id),
        "a fail-closed diagnostic must name the new required member, got {diagnostics:#?}"
    );
}

/// A brand-new required member added WITH an `evolve default` over a populated store is the
/// Default backfill obligation: the default fills every old record, so it stays activatable.
/// This is the add-required-field-with-default path the proposal-aware scan must still reach
/// for a not-yet-accepted member, not only for an already-accepted one.
#[test]
fn brand_new_required_member_with_default_backfills() {
    let title_stable = hex_id(3);
    let root = temp_project("discharge-new-required-default", |root| {
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
        let accepted = CatalogMetadata::new(
            3,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &hex_id(2), "int"),
                member_entry("books::Book::title", &title_stable, "string"),
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
    seed.record(1);
    seed.member_by_id(1, &title_stable, Scalar::Str("Dune".into()));

    let pages_id = new_member_proposal_id(&program, "books::Book::pages");
    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    match verdict_for(&result, &pages_id) {
        Verdict::Default { value } => {
            assert_eq!(value.scalar_type, marrow_store::value::ScalarType::Int);
            assert_eq!(
                value.encoded,
                marrow_store::value::encode_value(&Scalar::Int(0)).unwrap()
            );
        }
        other => panic!("expected default for the brand-new required member, got {other:#?}"),
    }
    assert!(result.is_activatable(), "{result:#?}");
}

/// A brand-new required member added over an EMPTY store is activatable with no default:
/// requiredness is checked only against records that exist, and there are none. This pins
/// that the proposal-aware scan does not over-fire on a store with nothing to backfill.
#[test]
fn brand_new_required_member_over_empty_store_activates() {
    let title_stable = hex_id(3);
    let root = temp_project("discharge-new-required-empty", |root| {
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
        let accepted = CatalogMetadata::new(
            3,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &hex_id(2), "int"),
                member_entry("books::Book::title", &title_stable, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    // No records seeded: the store is empty.
    let store = TreeStore::memory();

    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    assert!(
        result.is_activatable(),
        "a brand-new required member over an empty store must activate: {:#?}",
        result.verdicts
    );
}

/// A stored enum value that names a member which has become a `category` (gained children,
/// so it is no longer selectable) fails closed: a category is unselectable, so a value naming
/// it is not a valid value of the current enum. The enum-member validity check must admit
/// only SELECTABLE members, not every catalog member, or a stored value of a now-grouping
/// member would be silently accepted.
#[test]
fn stored_enum_value_naming_now_category_member_fails_closed() {
    let root = temp_project("discharge-enum-now-category", |root| {
        // `tiger` was a selectable leaf when the value was written; source now gives it
        // children, making it a category and unselectable. A stored `tiger` is invalid.
        write(
            root,
            "src/zoo.mw",
            "module zoo\n\
             enum Cat\n\
             \x20   category tiger\n\
             \x20       bengal\n\
             \x20       siberian\n\
             \x20   housecat\n\
             resource Pet at ^pets(id: int)\n\
             \x20   required kind: Cat\n\
             pub fn add(): Id(^pets)\n\
             \x20   return nextId(^pets)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "pets");
    let kind_id = member_catalog_id(&place, "kind");
    let cat_enum_id = enum_catalog_id(&program, "Cat");
    let tiger_member_id = enum_member_catalog_id(&program, "Cat", "tiger");

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // A record whose `kind` was stored as `Cat::tiger`, now a category.
    seed.record(1);
    seed.member_bytes_by_id(
        1,
        &kind_id,
        enum_value_bytes(&cat_enum_id, &tiger_member_id),
    );

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a stored value naming a now-category member must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &kind_id),
            Verdict::RepairRequired {
                reason: RepairReason::InvalidStoredValue
            }
        ),
        "the enum member must fail closed as an invalid stored value, got {:#?}",
        verdict_for(&result, &kind_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == kind_id),
        "a fail-closed diagnostic must name the enum member, got {diagnostics:#?}"
    );
}

/// A brand-new REQUIRED leaf added inside an EXISTING keyed layer over populated entries
/// fails closed with no default: the keyed layer already has entries that predate the new
/// leaf, so requiredness is unmet per existing entry. The new leaf has no bound facts id,
/// only a proposal-minted one, so the keyed scan must thread the resolved id to reach it;
/// before this fix the keyed scan recorded only bound ids and silently activated the
/// brand-new required leaf over populated entries.
#[test]
fn brand_new_required_keyed_leaf_over_populated_layer_fails_closed() {
    let root = temp_project("discharge-new-keyed-required-no-default", |root| {
        // `body` is brand-new required inside the existing `versions` keyed layer; the
        // accepted catalog carries the layer and a sibling `note`, but not `body`.
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       note: string\n\
             \x20       required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "policies::Policy::versions",
                    &hex_id(3),
                ),
                member_entry("policies::Policy::versions::note", &hex_id(4), "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // An existing keyed entry that predates required `body`: a sibling `note` marks the
    // entry as existing while `body` is absent.
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(7),
        "note",
        Scalar::Str("draft".into()),
    );

    let body_id = new_member_proposal_id(&program, "policies::Policy::versions::body");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a brand-new required keyed leaf over a populated layer must block: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &body_id),
            Verdict::RepairRequired {
                reason: RepairReason::MissingRequiredMember
            }
        ),
        "the brand-new required keyed leaf must fail closed, got {:#?}",
        verdict_for(&result, &body_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == body_id),
        "a fail-closed diagnostic must name the new required keyed leaf, got {diagnostics:#?}"
    );
}

/// A brand-new required leaf added inside an existing keyed layer WITH an `evolve default`
/// backfills every existing entry, staying activatable: the keyed proposal-aware path must
/// reach the Default obligation for a not-yet-accepted keyed leaf the same way the unkeyed
/// path does.
#[test]
fn brand_new_required_keyed_leaf_with_default_backfills() {
    let root = temp_project("discharge-new-keyed-required-default", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       note: string\n\
             \x20       required body: string\n\
             evolve\n\
             \x20   default Policy.versions.body = \"\"\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "policies::Policy::versions",
                    &hex_id(3),
                ),
                member_entry("policies::Policy::versions::note", &hex_id(4), "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
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
        SavedKey::Int(7),
        "note",
        Scalar::Str("draft".into()),
    );

    let body_id = new_member_proposal_id(&program, "policies::Policy::versions::body");
    let result = witness(&program, &store);
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(verdict_for(&result, &body_id), Verdict::Default { .. }),
        "a brand-new required keyed leaf with a default must backfill, got {:#?}",
        verdict_for(&result, &body_id)
    );
    assert!(result.is_activatable(), "{result:#?}");
}

/// A leaf retyped from a tokenizable scalar to a non-tokenizable `sequence` over populated
/// data fails closed: a leaf position whose new declared type produces no leaf token still
/// changed type, so the populated old bytes cannot be silently reread. The retype check must
/// be total over the new side; before this fix a `sequence` (or unknown) new type produced no
/// token, so the member was dropped from the leaf map and its retype escaped detection.
#[test]
fn retype_scalar_to_sequence_over_populated_data_fails_closed() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-retype-scalar-sequence", |root| {
        // `value` was `string`; source now types it `sequence[string]`, a non-tokenizable
        // leaf position. Its old bytes were written as a single string.
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required value: sequence[string]\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &hex_id(2), "int"),
                member_entry("books::Book::value", &value_id, "string"),
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
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Str("draft".into()));

    let value_id = member_catalog_id(&place, "value");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a populated leaf retyped to a sequence must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &value_id),
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform
            }
        ),
        "a non-tokenizable retype over populated data must steer to a transform, got {:#?}",
        verdict_for(&result, &value_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == value_id),
        "a fail-closed diagnostic must name the retyped leaf, got {diagnostics:#?}"
    );
}

/// An OPTIONAL enum leaf whose enum dropped a selectable member fails closed when a stored
/// value names the removed member: an optional enum leaf is normally scanned only on a
/// retype, so a member-removal under an UNCHANGED enum identity slipped through. The enum
/// keeps its stable identity (not a retype), but its selectable-member set shrank this
/// cycle, which forces a presence/validity scan of every leaf referencing it.
#[test]
fn optional_enum_leaf_with_dropped_member_fails_closed() {
    let value_id = hex_id(3);
    let enum_stable = hex_id(7);
    let root = temp_project("discharge-optional-enum-dropped", |root| {
        // `Status` keeps its identity but drops selectable `shipped`; `state` is OPTIONAL.
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             resource Book at ^books(id: int)\n\
             \x20   state: Status\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &hex_id(2), "int"),
                entry(CatalogEntryKind::Enum, "books::Status", &enum_stable),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::draft",
                    &hex_id(8),
                ),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::shipped",
                    &hex_id(9),
                ),
                member_entry(
                    "books::Book::state",
                    &value_id,
                    &format!("enum:{enum_stable}"),
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
    // A record whose optional `state` was stored as the now-removed `shipped`.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, enum_value_bytes(&enum_stable, &hex_id(9)));

    let value_id = member_catalog_id(&place, "state");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "an optional enum leaf storing a dropped member must block: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &value_id),
            Verdict::RepairRequired {
                reason: RepairReason::InvalidStoredValue
            }
        ),
        "the optional enum leaf must fail closed as an invalid stored value, got {:#?}",
        verdict_for(&result, &value_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == value_id),
        "a fail-closed diagnostic must name the optional enum leaf, got {diagnostics:#?}"
    );
}

/// An optional enum leaf whose enum is UNCHANGED proves cleanly over a stored value: the
/// shrank-enum trigger must not over-fire and force a scan (or a block) when no selectable
/// member was dropped. This pins that an honest optional enum stays a no-op.
#[test]
fn optional_enum_leaf_with_unchanged_enum_proves() {
    let root = temp_project("discharge-optional-enum-unchanged", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book at ^books(id: int)\n\
             \x20   state: Status\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the baseline so the enum and member ids are accepted, then re-preview the
    // unchanged enum over a populated optional leaf.
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    let state_id = member_catalog_id(&place, "state");
    let enum_id = enum_catalog_id(&program, "Status");
    let shipped = enum_member_catalog_id(&program, "Status", "shipped");
    seed.record(1);
    seed.member_bytes_by_id(1, &state_id, enum_value_bytes(&enum_id, &shipped));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        result.is_activatable(),
        "an unchanged optional enum must stay activatable: {:#?}",
        result.verdicts
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A member that WAS a plain leaf becoming a GROUP over populated data fails closed. The
/// accepted catalog recorded `value` as a `string` leaf; source now declares `value` as a
/// group of sub-fields, so the current declaration produces no leaf token at the member's
/// path. The disappearance of the leaf token is itself a retype: old bytes still live under
/// the member cell the group now occupies, and the new group shape would orphan them, so the
/// change is steered to a transform rather than silently activated.
#[test]
fn leaf_member_becoming_a_group_over_populated_data_fails_closed() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-leaf-to-group", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   value\n\
             \x20       required first: string\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &hex_id(2), "int"),
                member_entry("books::Book::value", &value_id, "string"),
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
    // The old record carries a `string` cell at the member position the group now occupies.
    seed.record(1);
    seed.member_bytes_by_id(
        1,
        &value_id,
        encode_value(&Scalar::Str("draft".into())).unwrap(),
    );

    let group_id = group_member_catalog_id(&place, "value");
    assert_eq!(
        group_id, value_id,
        "a leaf becoming a group keeps the member's accepted stable id"
    );
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a leaf becoming a group over populated data must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &group_id),
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform
            }
        ),
        "a leaf-to-group shape change must steer to a transform, got {:#?}",
        verdict_for(&result, &group_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == group_id),
        "a fail-closed diagnostic must name the now-group member, got {diagnostics:#?}"
    );
}

/// A member that WAS a plain leaf becoming a KEYED LAYER over populated data fails closed.
/// The accepted catalog recorded `value` as a `string` leaf; source now declares `value` as a
/// keyed group (`value(version: int)`), so the current declaration produces no leaf token at
/// the member's path. The old single-cell bytes live under the member position the keyed layer
/// now occupies; the new keyed shape addresses entries by a key those bytes were never written
/// at, so the change is steered to a transform rather than silently activated.
#[test]
fn leaf_member_becoming_a_keyed_layer_over_populated_data_fails_closed() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-leaf-to-keyed", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   value(version: int)\n\
             \x20       required body: string\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &hex_id(2), "int"),
                member_entry("books::Book::value", &value_id, "string"),
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
    // The old record carries a `string` cell at the member position the keyed layer now occupies.
    seed.record(1);
    seed.member_bytes_by_id(
        1,
        &value_id,
        encode_value(&Scalar::Str("draft".into())).unwrap(),
    );

    let layer_id = group_member_catalog_id(&place, "value");
    assert_eq!(
        layer_id, value_id,
        "a leaf becoming a keyed layer keeps the member's accepted stable id"
    );
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a leaf becoming a keyed layer over populated data must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &layer_id),
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform
            }
        ),
        "a leaf-to-keyed-layer shape change must steer to a transform, got {:#?}",
        verdict_for(&result, &layer_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == layer_id),
        "a fail-closed diagnostic must name the now-keyed-layer member, got {diagnostics:#?}"
    );
}

/// A leaf nested inside a populated KEYED GROUP, retyped, fails closed PER ENTRY. The accepted
/// catalog records the nested leaf `versions.body` as a `string`; source retypes it `int`. An
/// existing keyed entry carries a `string` value the new `int` decoder would silently
/// reinterpret. A retyped leaf below a keyed layer has no static path (its path needs an entry
/// key), so it must be probed through the per-entry keyed descent, not a flat subtree check,
/// or the old per-entry bytes are missed and it fails open.
#[test]
fn retype_of_leaf_nested_in_populated_keyed_group_fails_closed() {
    let body_stable = hex_id(4);
    let root = temp_project("discharge-keyed-nested-retype", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       required body: int\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "policies::Policy::versions",
                    &hex_id(3),
                ),
                member_entry("policies::Policy::versions::body", &body_stable, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // One keyed entry whose `body` cell was written under the old `string` type.
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(7),
        "body",
        Scalar::Str("draft".into()),
    );

    let body_id = nested_member_catalog_id(&place, "versions", "body");
    assert_eq!(
        body_id, body_stable,
        "a retyped keyed-nested leaf keeps its accepted stable id"
    );
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a populated keyed-nested retype must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &body_id),
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform
            }
        ),
        "a keyed-nested retype over populated entries must steer to a transform, got {:#?}",
        verdict_for(&result, &body_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == body_id),
        "a fail-closed diagnostic must name the retyped keyed-nested leaf, got {diagnostics:#?}"
    );
}

/// A keyed-nested retype whose old bytes happen to DECODE under the new type is the sharp
/// soundness hazard: a presence-only or per-entry validity proof would silently bless them.
/// An `int` keyed-nested leaf retyped to `bool` over an entry stored as `1` would read back as
/// `true`; the per-entry retype probe counts the entry as populated regardless of validity and
/// fails the change closed, so the overlapping bytes are never reinterpreted.
#[test]
fn retype_of_keyed_nested_leaf_with_overlapping_byte_fails_closed() {
    let body_stable = hex_id(4);
    let root = temp_project("discharge-keyed-nested-overlap", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       required body: bool\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "policies::Policy::versions",
                    &hex_id(3),
                ),
                member_entry("policies::Policy::versions::body", &body_stable, "int"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // The entry's `body` cell was written as `int` `1`, a byte the new `bool` decoder accepts.
    seed.record(1);
    seed.keyed_member(1, "versions", SavedKey::Int(7), "body", Scalar::Int(1));

    let body_id = nested_member_catalog_id(&place, "versions", "body");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "an overlapping-byte keyed-nested retype must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &body_id),
            Verdict::RepairRequired {
                reason: RepairReason::TypeChangeRequiresTransform
            }
        ),
        "an overlapping-byte keyed-nested retype must steer to a transform, got {:#?}",
        verdict_for(&result, &body_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == body_id),
        "a fail-closed diagnostic must name the retyped keyed-nested leaf, got {diagnostics:#?}"
    );
}

/// A leaf nested inside a populated keyed group whose type is UNCHANGED proves cleanly: the
/// per-entry retype probe must not fail closed on an honest no-change keyed-nested leaf.
#[test]
fn unchanged_leaf_nested_in_populated_keyed_group_proves() {
    let body_stable = hex_id(4);
    let root = temp_project("discharge-keyed-nested-unchanged", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "policies::Policy::versions",
                    &hex_id(3),
                ),
                member_entry("policies::Policy::versions::body", &body_stable, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
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
        SavedKey::Int(7),
        "body",
        Scalar::Str("draft".into()),
    );

    let body_id = nested_member_catalog_id(&place, "versions", "body");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(result.is_activatable(), "{:#?}", result.verdicts);
    assert!(
        matches!(verdict_for(&result, &body_id), Verdict::DataProof),
        "an unchanged keyed-nested leaf must prove, got {:#?}",
        verdict_for(&result, &body_id)
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A NESTED keyed-layer member whose key TYPE changes over populated entries fails closed.
/// The accepted catalog records `versions` as a keyed group keyed by `version: int`; source
/// re-keys it `version: string`. Each existing entry is addressed by the old `int` key bytes,
/// which sit in the data path itself, so the new `string` shape addresses no existing entry —
/// the same orphaning hazard a store identity-key change has, one level down. v0.1 cannot
/// migrate a keyed-layer key shape, so the layer member fails closed rather than activating
/// over entries the new key shape cannot reach.
#[test]
fn keyed_layer_key_type_change_over_populated_entries_fails_closed() {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let root = temp_project("discharge-keyed-layer-keytype", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: string)\n\
             \x20       required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // One existing keyed entry under the old `int` key shape.
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(7),
        "body",
        Scalar::Str("draft".into()),
    );

    let layer_id = group_member_catalog_id(&place, "versions");
    assert_eq!(
        layer_id, versions_id,
        "a re-keyed keyed layer keeps its accepted stable id"
    );
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a keyed-layer key-type change over populated entries must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &layer_id),
            Verdict::RepairRequired {
                reason: RepairReason::KeyedLayerKeyShapeChange
            }
        ),
        "a keyed-layer key-shape change must fail closed, got {:#?}",
        verdict_for(&result, &layer_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == layer_id),
        "a fail-closed diagnostic must name the re-keyed layer, got {diagnostics:#?}"
    );
}

/// A plain unkeyed GROUP reshaped into a KEYED LAYER over populated data fails closed. The
/// accepted catalog records `versions` as an unkeyed group (no key params); source now keys it
/// `versions(version: int)`. The old group's sub-member cells sit directly under the group node
/// with no entry key, so the new keyed shape — which addresses every value under an entry key —
/// reads none of them. The reshape is a structural divergence the snapshot cannot satisfy, so
/// the layer member fails closed.
#[test]
fn plain_group_reshaped_to_keyed_layer_over_populated_data_fails_closed() {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let root = temp_project("discharge-group-to-keyed", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                group_entry("policies::Policy::versions", &versions_id),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // The old record carries `versions.body` as an unkeyed-group sub-member cell.
    seed.record(1);
    seed.nested_member(1, "versions", "body", Scalar::Str("draft".into()));

    let layer_id = group_member_catalog_id(&place, "versions");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a group reshaped to a keyed layer over populated data must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &layer_id),
            Verdict::RepairRequired {
                reason: RepairReason::KeyedLayerKeyShapeChange
            }
        ),
        "a group-to-keyed-layer reshape must fail closed, got {:#?}",
        verdict_for(&result, &layer_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == layer_id),
        "a fail-closed diagnostic must name the reshaped member, got {diagnostics:#?}"
    );
}

/// A KEYED LAYER reshaped into a plain unkeyed GROUP over populated data fails closed — the
/// inverse reshape. The accepted catalog records `versions` keyed by `version: int`; source
/// drops the key, making it a plain group. Every existing entry sits under an entry key the
/// plain group shape never reads, so the reshape is a structural divergence that fails closed.
#[test]
fn keyed_layer_reshaped_to_plain_group_over_populated_data_fails_closed() {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let root = temp_project("discharge-keyed-to-group", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions\n\
             \x20       required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // One existing keyed entry under the old `int` key shape.
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(7),
        "body",
        Scalar::Str("draft".into()),
    );

    let group_id = group_member_catalog_id(&place, "versions");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a keyed layer reshaped to a plain group over populated data must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &group_id),
            Verdict::RepairRequired {
                reason: RepairReason::KeyedLayerKeyShapeChange
            }
        ),
        "a keyed-layer-to-group reshape must fail closed, got {:#?}",
        verdict_for(&result, &group_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == group_id),
        "a fail-closed diagnostic must name the reshaped member, got {diagnostics:#?}"
    );
}

/// A leaf becoming a group that adds a brand-new REQUIRED sub-member fails closed over a record
/// whose old leaf cell is UNPOPULATED. The old leaf disappearing is handled by the disappeared-
/// leaf probe, but that probe alone only fails closed when the old cell holds bytes. The new
/// group's brand-new required sub-member must ALSO be presence-scanned, so a record that exists
/// but has no value at the old leaf position is caught for the missing required sub-member
/// rather than fixed up by the empty disappeared-leaf probe and silently activated.
#[test]
fn leaf_to_group_adding_required_submember_over_empty_cell_fails_closed() {
    let value_id = hex_id(3);
    let root = temp_project("discharge-leaf-to-group-required", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   marker: string\n\
             \x20   value\n\
             \x20       required first: string\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "books::Book", &hex_id(1)),
                store_entry("books::^books", &hex_id(2), "int"),
                member_entry("books::Book::marker", &hex_id(5), "string"),
                member_entry("books::Book::value", &value_id, "string"),
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
    // The record exists (a sibling `marker` cell) but its old `value` leaf cell was never
    // populated, so the disappeared-leaf probe sees nothing; the new required `value.first`
    // sub-member is missing and must fail closed.
    seed.record(1);
    seed.member_by_id(1, &hex_id(5), Scalar::Str("seen".into()));

    // `value.first` is a brand-new required sub-member of the new group; its identity lives
    // only in the proposal, so the descend reaches it by its proposal-minted id.
    let first_id = new_member_proposal_id(&program, "books::Book::value::first");
    let (result, _diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a leaf-to-group adding a required sub-member over an unpopulated old cell must block: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &first_id),
            Verdict::RepairRequired {
                reason: RepairReason::MissingRequiredMember
            }
        ),
        "the brand-new required sub-member must be scanned and fail closed, got {:#?}",
        verdict_for(&result, &first_id)
    );
}

/// The default-deny backstop catches a structural transition no targeted classifier addresses.
/// Here a member moves from a keyed group keyed by `version: int` to a keyed group keyed by the
/// SAME `version: int` but with an added key column `lang: string` — a keyed-layer arity change.
/// Each existing entry is addressed by a one-column key the two-column shape cannot read. No
/// leaf-token classifier fires (both shapes are non-leaf groups), so the structural signature
/// backstop is what fails it closed.
#[test]
fn keyed_layer_arity_change_fails_closed_via_backstop() {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let root = temp_project("discharge-keyed-arity", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int, lang: string)\n\
             \x20       required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // One existing entry under the old one-column key shape.
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(7),
        "body",
        Scalar::Str("draft".into()),
    );

    let layer_id = group_member_catalog_id(&place, "versions");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a keyed-layer arity change over populated entries must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &layer_id),
            Verdict::RepairRequired { .. }
        ),
        "a keyed-layer arity change must fail closed via the backstop, got {:#?}",
        verdict_for(&result, &layer_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == layer_id),
        "a fail-closed diagnostic must name the structurally-diverged member, got {diagnostics:#?}"
    );
}

/// NEGATIVE GUARD: the structural backstop must not over-fire. An identity-preserving rename of
/// a keyed layer (same stable id, same key shape, only the source spelling moved) keeps its
/// structural signature unchanged, so it stays activatable — the rename is a catalog-only move,
/// not a structural divergence. A keyed-leaf map carries the rename cleanly: its stable id and
/// `[int]string` signature are preserved, so the backstop sees no divergence.
#[test]
fn renamed_keyed_layer_with_unchanged_shape_does_not_overfire() {
    let tags_id = hex_id(3);
    let root = temp_project("discharge-keyed-rename", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   labels(pos: int): string\n\
             evolve\n\
             \x20   rename Policy.tags -> Policy.labels\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                member_entry("policies::Policy::tags", &tags_id, "[int]string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.keyed_leaf(
        1,
        "labels",
        SavedKey::Int(7),
        encode_value(&Scalar::Str("draft".into())).unwrap(),
    );

    let layer_id = keyed_leaf_catalog_id(&place, "labels");
    let (result, _diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        result.is_activatable(),
        "an identity-preserving keyed-layer rename must not be failed closed by the backstop: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(verdict_for(&result, &layer_id), Verdict::CatalogOnly),
        "a renamed keyed-leaf map with an unchanged shape is a catalog-only move, got {:#?}",
        verdict_for(&result, &layer_id)
    );
}

/// NEGATIVE GUARD: reordering keyed-layer sub-members keeps every member's structural signature
/// unchanged, so the backstop stays silent and the change activates. The signature is identity-
/// aware and per member, not order-sensitive.
#[test]
fn reordered_keyed_layer_members_do_not_overfire() {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let note_id = hex_id(5);
    let root = temp_project("discharge-keyed-reorder", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       required body: string\n\
             \x20       required note: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::note", &note_id, "string"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
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
        SavedKey::Int(7),
        "body",
        Scalar::Str("draft".into()),
    );
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(7),
        "note",
        Scalar::Str("seen".into()),
    );

    let layer_id = group_member_catalog_id(&place, "versions");
    let (result, _diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        result.is_activatable(),
        "reordering keyed-layer members must not be failed closed by the backstop: {:#?}",
        result.verdicts
    );
    assert!(
        !result
            .verdicts
            .iter()
            .any(|obligation| obligation.catalog_id.as_str() == layer_id
                && matches!(obligation.verdict, Verdict::RepairRequired { .. })),
        "reordering places no structural repair on the layer: {:#?}",
        result.verdicts
    );
}

/// NEGATIVE GUARD: adding an optional member alongside an unchanged keyed layer activates. A
/// brand-new optional member is not present in the accepted snapshot, so the backstop never
/// considers it, and the unchanged keyed layer keeps its signature.
#[test]
fn optional_add_beside_unchanged_keyed_layer_does_not_overfire() {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let root = temp_project("discharge-keyed-optional-add", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   tag: string\n\
             \x20   versions(version: int)\n\
             \x20       required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            4,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
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
        SavedKey::Int(7),
        "body",
        Scalar::Str("draft".into()),
    );

    let layer_id = group_member_catalog_id(&place, "versions");
    let (result, _diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        result.is_activatable(),
        "adding an optional member beside an unchanged keyed layer must activate: {:#?}",
        result.verdicts
    );
    assert!(
        !result
            .verdicts
            .iter()
            .any(|obligation| obligation.catalog_id.as_str() == layer_id
                && matches!(obligation.verdict, Verdict::RepairRequired { .. })),
        "an unchanged keyed layer places no structural repair: {:#?}",
        result.verdicts
    );
}

/// DEPTH-TOTAL (a): a keyed layer nested BELOW another keyed layer, re-keyed by KEY TYPE over
/// populated entries, fails closed. The accepted catalog records the inner layer `revisions`
/// keyed by `rev: int`; source re-keys it `rev: string`. The inner layer's own structural
/// signature diverged, but it sits below the outer keyed layer `versions`, whose own shape did
/// not change — so the backstop must descend through the unchanged outer layer per entry to
/// reach the diverged inner layer, find its populated entries under the old `int` key, and fail
/// it closed. Without depth-total descent the divergence below a keyed ancestor activates
/// silently over entries the new inner key shape addresses none of.
#[test]
fn nested_keyed_layer_rekey_below_keyed_ancestor_fails_closed() {
    let versions_id = hex_id(3);
    let revisions_id = hex_id(4);
    let body_id = hex_id(5);
    let root = temp_project("discharge-nested-keyed-rekey", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       revisions(rev: string)\n\
             \x20           required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            5,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                keyed_group_entry(
                    "policies::Policy::versions::revisions",
                    &revisions_id,
                    "int",
                ),
                member_entry(
                    "policies::Policy::versions::revisions::body",
                    &body_id,
                    "string",
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // One existing inner entry under the old `int` rev key, two layers deep.
    seed.record(1);
    seed.deep_keyed_member(
        1,
        [
            ("versions", SavedKey::Int(7)),
            ("revisions", SavedKey::Int(2)),
        ],
        "body",
        Scalar::Str("draft".into()),
    );

    let revisions_layer_id = deep_member_catalog_id(&place, &["versions", "revisions"]);
    assert_eq!(
        revisions_layer_id, revisions_id,
        "the re-keyed nested layer keeps its accepted stable id"
    );
    let body_member_id = deep_member_catalog_id(&place, &["versions", "revisions", "body"]);
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a nested keyed-layer key-type change over populated entries must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &revisions_layer_id),
            Verdict::RepairRequired {
                reason: RepairReason::KeyedLayerKeyShapeChange
            }
        ),
        "a re-keyed layer below a keyed ancestor must fail closed, got {:#?}",
        verdict_for(&result, &revisions_layer_id)
    );
    // The enclosing re-keyed layer fails closed, so its interior required leaf must not also emit
    // a misleading data proof over entries the new key shape orphans.
    assert!(
        !result
            .verdicts
            .iter()
            .any(|obligation| obligation.catalog_id.as_str() == body_member_id),
        "a deeper required leaf under a failed-closed layer must not be re-judged, got {:#?}",
        result.verdicts
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == revisions_layer_id),
        "a fail-closed diagnostic must name the re-keyed nested layer, got {diagnostics:#?}"
    );
}

/// DEPTH-TOTAL (b): a keyed-layer ARITY change two levels deep fails closed. The accepted
/// catalog records the inner layer `revisions` keyed by one column `rev: int`; source makes it
/// composite `rev: int, draft: int`. The inner layer's signature diverged below the unchanged
/// outer keyed layer, so the backstop must descend to it per entry and fail it closed: every
/// existing inner entry is addressed by the old one-column key the new composite shape cannot
/// reach.
#[test]
fn nested_keyed_layer_arity_change_two_levels_deep_fails_closed() {
    let versions_id = hex_id(3);
    let revisions_id = hex_id(4);
    let body_id = hex_id(5);
    let root = temp_project("discharge-nested-keyed-arity", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       revisions(rev: int, draft: int)\n\
             \x20           required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            5,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                keyed_group_entry(
                    "policies::Policy::versions::revisions",
                    &revisions_id,
                    "int",
                ),
                member_entry(
                    "policies::Policy::versions::revisions::body",
                    &body_id,
                    "string",
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.deep_keyed_member(
        1,
        [
            ("versions", SavedKey::Int(7)),
            ("revisions", SavedKey::Int(2)),
        ],
        "body",
        Scalar::Str("draft".into()),
    );

    let revisions_layer_id = deep_member_catalog_id(&place, &["versions", "revisions"]);
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a nested keyed-layer arity change over populated entries must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &revisions_layer_id),
            Verdict::RepairRequired { .. }
        ),
        "a nested keyed-layer arity change must fail closed via the backstop, got {:#?}",
        verdict_for(&result, &revisions_layer_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == revisions_layer_id),
        "a fail-closed diagnostic must name the structurally-diverged nested layer, got {diagnostics:#?}"
    );
}

/// DEPTH-TOTAL (c): a structurally-diverged INTERIOR member arbitrarily deep fails closed. A
/// plain unkeyed group `meta` nested under two keyed layers is reshaped into a keyed layer, so
/// its signature moves from `group` to `keyed-group:[int]` with no leaf token on either side —
/// a structural divergence no leaf-type, store-key, or per-entry leaf classifier claims, reached
/// only by descending through the two unchanged keyed ancestors. Its old sub-member cells sit
/// directly under the group node with no entry key, so the new keyed shape reads none of them;
/// the member fails closed over the populated entry rather than activating.
#[test]
fn deep_interior_member_structural_divergence_fails_closed() {
    let versions_id = hex_id(3);
    let revisions_id = hex_id(4);
    let meta_id = hex_id(5);
    let body_id = hex_id(6);
    let root = temp_project("discharge-deep-interior-divergence", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       revisions(rev: int)\n\
             \x20           meta(tag: int)\n\
             \x20               required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            6,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                keyed_group_entry(
                    "policies::Policy::versions::revisions",
                    &revisions_id,
                    "int",
                ),
                group_entry("policies::Policy::versions::revisions::meta", &meta_id),
                member_entry(
                    "policies::Policy::versions::revisions::meta::body",
                    &body_id,
                    "string",
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // The old `meta.body` cell sits as an unkeyed-group sub-member two keyed layers deep, with
    // no `tag` entry key, so the reshaped keyed `meta` addresses none of it.
    seed.record(1);
    seed.deep_group_member(
        1,
        &[
            ("versions", SavedKey::Int(7)),
            ("revisions", SavedKey::Int(2)),
        ],
        "meta",
        "body",
        Scalar::Str("draft".into()),
    );

    let meta_member_id = deep_member_catalog_id(&place, &["versions", "revisions", "meta"]);
    assert_eq!(
        meta_member_id, meta_id,
        "the reshaped deep interior member keeps its accepted stable id"
    );
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        !result.is_activatable(),
        "a structurally-diverged interior member arbitrarily deep must block activation: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(
            verdict_for(&result, &meta_member_id),
            Verdict::RepairRequired { .. }
        ),
        "a deep interior structural divergence must fail closed, got {:#?}",
        verdict_for(&result, &meta_member_id)
    );
    assert!(
        diagnostics
            .iter()
            .any(|RepairDiagnostic { catalog_id, .. }| catalog_id.as_str() == meta_member_id),
        "a fail-closed diagnostic must name the deep diverged member, got {diagnostics:#?}"
    );
}

/// NEGATIVE GUARD: an UNCHANGED nested keyed layer must still activate. With depth-total descent
/// the backstop now reaches interior members below keyed ancestors, so it must not over-fire on
/// a nested layer whose signature is unchanged: every member keeps its identity and shape, so
/// the deep required leaf proves per entry and nothing fails closed.
#[test]
fn unchanged_nested_keyed_layer_does_not_overfire() {
    let versions_id = hex_id(3);
    let revisions_id = hex_id(4);
    let body_id = hex_id(5);
    let root = temp_project("discharge-nested-keyed-unchanged", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy at ^policies(id: int)\n\
             \x20   versions(version: int)\n\
             \x20       revisions(rev: int)\n\
             \x20           required body: string\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = CatalogMetadata::new(
            5,
            vec![
                entry(CatalogEntryKind::Resource, "policies::Policy", &hex_id(1)),
                store_entry("policies::^policies", &hex_id(2), "int"),
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                keyed_group_entry(
                    "policies::Policy::versions::revisions",
                    &revisions_id,
                    "int",
                ),
                member_entry(
                    "policies::Policy::versions::revisions::body",
                    &body_id,
                    "string",
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.deep_keyed_member(
        1,
        [
            ("versions", SavedKey::Int(7)),
            ("revisions", SavedKey::Int(2)),
        ],
        "body",
        Scalar::Str("draft".into()),
    );

    let revisions_layer_id = deep_member_catalog_id(&place, &["versions", "revisions"]);
    let body_member_id = deep_member_catalog_id(&place, &["versions", "revisions", "body"]);
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    fs::remove_dir_all(&root).ok();

    assert!(
        result.is_activatable(),
        "an unchanged nested keyed layer must activate: {:#?}",
        result.verdicts
    );
    assert!(
        matches!(verdict_for(&result, &body_member_id), Verdict::DataProof),
        "the unchanged deep required leaf must prove per entry, got {:#?}",
        verdict_for(&result, &body_member_id)
    );
    assert!(
        !result
            .verdicts
            .iter()
            .any(
                |obligation| obligation.catalog_id.as_str() == revisions_layer_id
                    && matches!(obligation.verdict, Verdict::RepairRequired { .. })
            ),
        "an unchanged nested keyed layer places no structural repair: {:#?}",
        result.verdicts
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}
