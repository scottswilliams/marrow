//! Shared discharge harness for the data-attached evolution suites.
//!
//! Each discharge case checks a source-driven fixture through the production pipeline,
//! seeds a `TreeStore::memory()` at the member catalog ids the checked saved place names,
//! then runs the read-only discharge/preview entry and asserts the verdicts, the witness
//! counts, and the composed fingerprints. The data-only cases commit the catalog proposal
//! first (so the schema is already the accepted catalog) and exercise an old store snapshot
//! that predates a new member or index; the catalog-evolution cases pin a hand-built accepted
//! catalog the current source has moved away from.
//!
//! This module is the single owner of that seeding-and-verdict plumbing. Each discharge
//! binary includes it, so not every binary exercises every helper; the `dead_code`
//! allowance keeps the shared surface intact across the split.

#![allow(dead_code)]

use std::path::Path;

use marrow_check::evolution::{EvolutionWitness, Verdict, preview};
use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, check_project,
    checked_saved_root_place,
};
use marrow_project::{CatalogEntry, CatalogEntryKind, CatalogMetadata};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, encode_value};

pub use marrow_check::evolution::{RepairDiagnostic, RepairReason};

use crate::support::config;

/// A valid `cat_<32 lowercase hex>` stable id keyed by a small fixture number, so a
/// hand-built accepted catalog uses ids the store can address.
pub fn hex_id(n: u8) -> String {
    format!("cat_{n:032x}")
}

/// A bare catalog entry whose literal `stable_id` the store addresses by. The discharge
/// fixtures use specific `cat_` ids, so this passes the id through verbatim rather than
/// minting one, and carries no aliases.
pub fn entry(kind: CatalogEntryKind, path: &str, stable_id: &str) -> CatalogEntry {
    crate::support::catalog::entry(kind, path, stable_id, &[])
}

/// A resource-member catalog entry that records the identity-aware leaf token its durable bytes
/// were accepted as (a scalar name, `enum:<enum-stable-id>`, or `id:<store-stable-id>:<arity>`)
/// as the structural signature `leaf:<token>`, the one durable field that carries it, so a
/// discharge can detect a later type change by referent identity and the default-deny backstop
/// sees a leaf member's baseline. The hand-built accepted catalogs use this for members the test
/// then retypes in source.
pub fn member_entry(path: &str, stable_id: &str, accepted_leaf: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_struct: Some(format!("leaf:{accepted_leaf}")),
        ..entry(CatalogEntryKind::ResourceMember, path, stable_id)
    }
}

/// A keyed-group resource-member catalog entry that records the per-keyed-layer key shape its
/// durable entries are keyed under, as the structural signature `keyed-group:[<shape>]`. A keyed
/// group holds no single leaf cell, so its signature carries no leaf token; the backstop compares
/// it against the current shape to fail a re-key or a group<->keyed-group reshape closed.
pub fn keyed_group_entry(path: &str, stable_id: &str, key_shape: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_struct: Some(format!("keyed-group:[{key_shape}]")),
        ..entry(CatalogEntryKind::ResourceMember, path, stable_id)
    }
}

/// A plain unkeyed-group resource-member catalog entry, recording the structural signature
/// `group` so the backstop has a baseline to compare a reshape into a keyed layer against. An
/// unkeyed group holds no single leaf cell, so its signature carries no leaf token.
pub fn group_entry(path: &str, stable_id: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_struct: Some("group".to_string()),
        ..entry(CatalogEntryKind::ResourceMember, path, stable_id)
    }
}

/// A store catalog entry that records the identity-key shape its durable records are keyed
/// under (`<scalar>,<scalar>,...`), so a discharge can detect a later key-shape change the
/// new schema cannot address. The hand-built accepted catalogs use this for stores the test
/// then re-keys in source.
pub fn store_entry(path: &str, stable_id: &str, accepted_key_shape: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_key_shape: Some(accepted_key_shape.to_string()),
        ..entry(CatalogEntryKind::Store, path, stable_id)
    }
}

/// An accepted catalog whose header is the single resource and its store that nearly every
/// discharge fixture shares: the resource at `hex_id(1)` and its store at `hex_id(2)`. The
/// store records `key_shape` when the fixture pins an accepted identity-key shape (the re-key
/// cases compare against it) and carries none otherwise. `members` are the entries the test
/// actually evolves, appended after the header, so each site spells only its own evolution.
pub fn accepted_catalog(
    epoch: u64,
    resource_path: &str,
    store_path: &str,
    key_shape: Option<&str>,
    members: Vec<CatalogEntry>,
) -> CatalogMetadata {
    let store = match key_shape {
        Some(shape) => store_entry(store_path, &hex_id(2), shape),
        None => entry(CatalogEntryKind::Store, store_path, &hex_id(2)),
    };
    let mut entries = vec![
        entry(CatalogEntryKind::Resource, resource_path, &hex_id(1)),
        store,
    ];
    entries.extend(members);
    CatalogMetadata::new(epoch, entries)
}

pub fn checked(root: &Path) -> CheckedProgram {
    let (report, program) = check_project(root, &config()).expect("check project");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

/// Check the source once with no committed catalog, freeze its baseline through the
/// production commit path, then re-check. The returned program's schema is fully
/// committed, so its bound catalog ids address the store; the data-only cases then
/// exercise an old snapshot against that committed schema.
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

/// A minimal seeded store rooted at one single-key-identity saved place. Each
/// record is its `id` key; a member is seeded with `write_data_value` at the bound
/// member catalog id, exactly as the runtime write path does.
pub struct Seed<'a> {
    store: &'a TreeStore,
    place: &'a CheckedSavedPlace,
}

impl<'a> Seed<'a> {
    pub fn new(store: &'a TreeStore, place: &'a CheckedSavedPlace) -> Self {
        Seed { store, place }
    }

    pub fn store_id(&self) -> CatalogId {
        CatalogId::new(accepted_catalog_id(&self.place.store_catalog_id, "store"))
            .expect("store catalog id")
    }

    pub fn record(&self, id: i64) {
        self.store
            .write_node(&self.store_id(), &[SavedKey::Int(id)])
            .expect("write node");
    }

    pub fn member(&self, id: i64, member: &str, value: Scalar) {
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

    pub fn member_by_id(&self, id: i64, member_catalog_id: &str, value: Scalar) {
        let bytes = encode_value(&value).expect("encode value");
        self.member_bytes_by_id(id, member_catalog_id, bytes);
    }

    /// Seed arbitrary leaf bytes under a member id, exactly as the prior schema's
    /// writes did. Lets a retype case seed bytes written under the old type (a scalar,
    /// an enum member, or an identity payload) regardless of the member's current type.
    pub fn member_bytes_by_id(&self, id: i64, member_catalog_id: &str, bytes: Vec<u8>) {
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

    pub fn index_entry(&self, index: &str, key: Scalar, id: i64) {
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
    pub fn keyed_member(&self, id: i64, layer: &str, entry: SavedKey, leaf: &str, value: Scalar) {
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
    pub fn keyed_leaf(&self, id: i64, map: &str, entry: SavedKey, bytes: Vec<u8>) {
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
    pub fn nested_member(&self, id: i64, group: &str, leaf: &str, value: Scalar) {
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
    pub fn deep_keyed_member(
        &self,
        id: i64,
        layers: [(&str, SavedKey); 2],
        leaf: &str,
        value: Scalar,
    ) {
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
    pub fn deep_group_member(
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
pub fn witness(program: &CheckedProgram, store: &TreeStore) -> EvolutionWitness {
    preview(program, store).expect("preview").0
}

pub fn verdict_for<'a>(witness: &'a EvolutionWitness, catalog_id: &str) -> &'a Verdict {
    witness
        .verdicts
        .iter()
        .find(|outcome| outcome.catalog_id.as_str() == catalog_id)
        .map(|outcome| &outcome.verdict)
        .unwrap_or_else(|| panic!("verdict for `{catalog_id}` among {:#?}", witness.verdicts))
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

pub fn index_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
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

pub fn group_member_catalog_id(place: &CheckedSavedPlace, group: &str) -> String {
    accepted_catalog_id(&group_member(place, group).catalog_id, group)
}

/// The catalog id of a top-level keyed-leaf-layer (`map[K, V]`) member: a `Field` that
/// carries key params, so it is the leaf its entries' values are stored under.
pub fn keyed_leaf_catalog_id(place: &CheckedSavedPlace, map: &str) -> String {
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

pub fn nested_member_catalog_id(place: &CheckedSavedPlace, group: &str, leaf: &str) -> String {
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
pub fn deep_member_catalog_id(place: &CheckedSavedPlace, chain: &[&str]) -> String {
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
pub fn new_member_proposal_id(program: &CheckedProgram, path: &str) -> String {
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
pub fn enum_catalog_id(program: &CheckedProgram, name: &str) -> String {
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
pub fn enum_member_catalog_id(program: &CheckedProgram, enum_name: &str, member: &str) -> String {
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

pub fn accepted_catalog_id(id: &Option<String>, label: &str) -> String {
    id.clone()
        .unwrap_or_else(|| panic!("accepted catalog id for `{label}`"))
}

/// Encode a stored enum value as the runtime does: the enum's stable catalog id paired
/// with the selected member's stable catalog id.
pub fn enum_value_bytes(enum_catalog_id: &str, member_catalog_id: &str) -> Vec<u8> {
    let value = marrow_store::tree::TreeEnumMember::new(
        CatalogId::new(enum_catalog_id).expect("enum catalog id"),
        CatalogId::new(member_catalog_id).expect("enum member catalog id"),
    );
    marrow_store::tree::encode_tree_enum_member(&value).expect("encode enum member")
}

/// Assert an obligation fails closed: the witness is not activatable, the member's
/// verdict is `RepairRequired` for exactly `expected_reason` (which rules out a silent
/// `DataProof`), and a fail-closed diagnostic names the member. The verdict and
/// diagnostic dumps identify any mismatch, so call sites need no per-test prose.
pub fn assert_fails_closed(
    result: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    catalog_id: &str,
    expected_reason: RepairReason,
) {
    assert!(!result.is_activatable(), "{result:#?}");
    match verdict_for(result, catalog_id) {
        Verdict::RepairRequired { reason } => assert_eq!(
            *reason, expected_reason,
            "wrong fail-closed reason for `{catalog_id}` among {:#?}",
            result.verdicts
        ),
        other => panic!("`{catalog_id}` must fail closed, got {other:#?}"),
    }
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == catalog_id),
        "{diagnostics:#?}"
    );
}

/// A populated retype is steered to a transform: it fails closed with
/// `TypeChangeRequiresTransform` and a diagnostic naming the member.
pub fn assert_retype_steered(
    value_id: &str,
    result: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
) {
    assert_fails_closed(
        result,
        diagnostics,
        value_id,
        RepairReason::TypeChangeRequiresTransform,
    );
}
