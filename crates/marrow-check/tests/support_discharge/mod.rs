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
//! This module is the single owner of that seeding-and-verdict plumbing.

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogMetadata};
use marrow_check::evolution::{EvolutionWitness, Verdict, preview};
use marrow_check::{CheckedProgram, CheckedSavedPlace};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, encode_value};

pub use marrow_check::evolution::{RepairDiagnostic, RepairReason};

// The saved-place fact lookups and the check/commit factories are owned by marrow-check
// behind its `test-support` feature, so the discharge suites resolve member, index, enum,
// and proposal catalog ids through the same helpers the apply and CLI evolution suites do
// rather than carrying a copy. The discharge fixtures use the same single-`src`-root config,
// so `checked`/`commit_then_check`/`root_place` resolve through this import too.
pub use marrow_check::test_support::{
    accepted_catalog_id, checked, commit_then_check, deep_member_catalog_id, enum_catalog_id,
    enum_member_catalog_id, group_member_catalog_id, keyed_leaf_catalog_id, member_catalog_id,
    nested_member_catalog_id, new_member_proposal_id, root_place,
};

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

/// A resource-member catalog entry recording an arbitrary accepted structural signature verbatim,
/// for a baseline shape the current `marrow-catalog` decoder does not classify into leaf, group,
/// or keyed group. Such a signature involves neither a leaf nor a keyed group on the accepted
/// side, so a divergence from it routes the backstop to the general structural arm rather than a
/// targeted shape rule.
pub fn struct_signature_entry(path: &str, stable_id: &str, signature: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_struct: Some(signature.to_string()),
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

/// A store-index catalog entry that records the declaration shape its derived cells were
/// accepted under. Hand-built accepted catalogs use this for source-declared indexes; dropped
/// index tests keep the same shape so only source absence drives the obligation.
pub fn store_index_entry(path: &str, stable_id: &str, accepted_index_shape: &str) -> CatalogEntry {
    CatalogEntry {
        accepted_index_shape: Some(accepted_index_shape.to_string()),
        ..entry(CatalogEntryKind::StoreIndex, path, stable_id)
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
    CatalogMetadata::new(epoch, entries).expect("catalog builds")
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
        CatalogId::new(
            accepted_catalog_id(&self.place.store_catalog_id, "store").expect("store catalog id"),
        )
        .expect("store catalog id")
    }

    pub fn record(&self, id: i64) {
        write_record_presence(self.store, &self.store_id(), &[SavedKey::Int(id)]);
    }

    pub fn member(&self, id: i64, member: &str, value: Scalar) {
        let member_id =
            CatalogId::new(member_catalog_id(self.place, member).expect("member catalog id"))
                .expect("member id");
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

    /// Seed a leaf inside a keyed layer entry, at the path the runtime writes:
    /// `[Member(layer_id), Key(entry_key), Member(leaf_id)]` under the record
    /// identity. The presence of any leaf marks the keyed entry as existing.
    pub fn keyed_member(&self, id: i64, layer: &str, entry: SavedKey, leaf: &str, value: Scalar) {
        let layer_id =
            CatalogId::new(group_member_catalog_id(self.place, layer).expect("layer catalog id"))
                .expect("layer id");
        let leaf_id = CatalogId::new(
            nested_member_catalog_id(self.place, layer, leaf).expect("keyed leaf catalog id"),
        )
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

    /// Seed a keyed-leaf value, at the path the runtime writes:
    /// `[Member(leaf_id), Key(entry_key)]` under the record identity. The keyed leaf is
    /// itself the value cell, so it sits directly under its entry key with no
    /// sub-member. The bytes are written exactly as the prior schema's writes did, so a
    /// retype case can seed a value of the old V type regardless of the current one.
    pub fn keyed_leaf(&self, id: i64, leaf: &str, entry: SavedKey, bytes: Vec<u8>) {
        let leaf_id =
            CatalogId::new(keyed_leaf_catalog_id(self.place, leaf).expect("leaf catalog id"))
                .expect("leaf id");
        self.store
            .write_data_value(
                &self.store_id(),
                &[SavedKey::Int(id)],
                &[
                    DataPathSegment::Member(leaf_id),
                    DataPathSegment::Key(entry),
                ],
                bytes,
            )
            .expect("write keyed-leaf value");
    }

    /// Seed a leaf inside an unkeyed group, at the nested member path the runtime
    /// writes: `[Member(group_id), Member(leaf_id)]` under the record identity.
    pub fn nested_member(&self, id: i64, group: &str, leaf: &str, value: Scalar) {
        let group_id =
            CatalogId::new(group_member_catalog_id(self.place, group).expect("group catalog id"))
                .expect("group id");
        let leaf_id = CatalogId::new(
            nested_member_catalog_id(self.place, group, leaf).expect("nested leaf catalog id"),
        )
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
            CatalogId::new(group_member_catalog_id(self.place, outer).expect("outer catalog id"))
                .expect("outer layer id");
        let inner_id = CatalogId::new(
            deep_member_catalog_id(self.place, &[outer, inner]).expect("inner catalog id"),
        )
        .expect("inner layer id");
        let leaf_id = CatalogId::new(
            deep_member_catalog_id(self.place, &[outer, inner, leaf])
                .expect("deep leaf catalog id"),
        )
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
            let layer_id = CatalogId::new(
                deep_member_catalog_id(self.place, &chain).expect("deep layer catalog id"),
            )
            .expect("deep layer id");
            path.push(DataPathSegment::Member(layer_id));
            path.push(DataPathSegment::Key(key.clone()));
        }
        chain.push(group);
        let group_id = CatalogId::new(
            deep_member_catalog_id(self.place, &chain).expect("deep group catalog id"),
        )
        .expect("deep group id");
        path.push(DataPathSegment::Member(group_id));
        chain.push(leaf);
        let leaf_id = CatalogId::new(
            deep_member_catalog_id(self.place, &chain).expect("deep leaf catalog id"),
        )
        .expect("deep leaf id");
        path.push(DataPathSegment::Member(leaf_id));
        let bytes = encode_value(&value).expect("encode value");
        self.store
            .write_data_value(&self.store_id(), &[SavedKey::Int(id)], &path, bytes)
            .expect("write deep group member value");
    }
}

fn write_record_presence(store: &TreeStore, store_id: &CatalogId, identity: &[SavedKey]) {
    store
        .write_record_presence(store_id, identity)
        .expect("write record presence");
}

pub fn seed_catalog_record(store: &TreeStore, store_id: &str, identity: &[SavedKey]) {
    let store_id = CatalogId::new(store_id).expect("accepted store catalog id");
    write_record_presence(store, &store_id, identity);
}

pub fn seed_catalog_member(
    store: &TreeStore,
    store_id: &str,
    identity: &[SavedKey],
    member_id: &str,
    value: Scalar,
) {
    let store_id = CatalogId::new(store_id).expect("accepted store catalog id");
    write_record_presence(store, &store_id, identity);
    store
        .write_data_value(
            &store_id,
            identity,
            &[DataPathSegment::Member(
                CatalogId::new(member_id).expect("accepted member catalog id"),
            )],
            encode_value(&value).expect("encode value"),
        )
        .expect("write member value");
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
