//! The accepted catalog's canonical JSON round-trip and validation invariants: a
//! committed snapshot serializes and parses back identically, identity collisions and a
//! non-SHA or mismatched digest fail closed, and an unrepresentable lifecycle is rejected.
//! This is the on-the-wire contract the engine-resident store and the backup catalog
//! section both persist through.

use std::hash::{Hash, Hasher};

use marrow_catalog::{
    CATALOG_INVALID, CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogLock,
    CatalogMetadata, LOCK_CORRUPT, LockEntry, LockLedgerTombstone,
};

/// An `Active` catalog entry with a `cat_`-shaped stable id minted deterministically from
/// `label`, so a fixture names a member readably and the assertions agree on its id.
fn entry(kind: CatalogEntryKind, path: &str, label: &str, aliases: &[&str]) -> CatalogEntry {
    literal_entry(kind, path, &derived_id(label), aliases)
}

/// An `Active` catalog entry with a caller-chosen literal stable id, for cases that pin a
/// specific id rather than a label-derived one.
fn literal_entry(
    kind: CatalogEntryKind,
    path: &str,
    stable_id: &str,
    aliases: &[&str],
) -> CatalogEntry {
    CatalogEntry {
        kind,
        path: path.to_string(),
        stable_id: stable_id.to_string(),
        aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
        lifecycle: CatalogLifecycle::Active,
        accepted_key_shape: None,
        accepted_index_shape: None,
        accepted_struct: None,
        applied_transform: None,
    }
}

/// Wrap `entries` in a catalog at a fixed epoch so a fixture lists only what it cares about.
fn catalog(entries: Vec<CatalogEntry>) -> CatalogMetadata {
    CatalogMetadata::new(7, entries).expect("catalog builds")
}

/// Mint a deterministic `cat_<32 hex>` stable id from a readable label.
fn derived_id(label: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    label.hash(&mut hasher);
    let first = hasher.finish();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (label, "catalog-json-fixture").hash(&mut hasher);
    let second = hasher.finish();
    format!("cat_{first:016x}{second:016x}")
}

#[test]
fn rejects_alias_and_stable_id_collisions() {
    for metadata in [
        catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(
                CatalogEntryKind::Resource,
                "library::Book",
                "res-library",
                &["books::Book"],
            ),
        ]),
        catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "res-book", &[]),
        ]),
    ] {
        let json = metadata.to_json_pretty().expect("catalog renders");
        let error = CatalogMetadata::from_json(&json).expect_err("collision is rejected");

        assert_eq!(error.code, CATALOG_INVALID);
    }
}

#[test]
fn round_trips_stable_ids_aliases_lifecycle_epoch_and_digest() {
    let metadata = catalog(vec![
        entry(
            CatalogEntryKind::Resource,
            "books::Book",
            "res-book",
            &["library::Book"],
        ),
        CatalogEntry {
            kind: CatalogEntryKind::EnumMember,
            path: "books::Status::archived".to_string(),
            stable_id: derived_id("enum-member-archived"),
            aliases: vec!["books::Status::inactive".to_string()],
            lifecycle: CatalogLifecycle::Reserved,
            accepted_key_shape: None,
            accepted_index_shape: None,
            accepted_struct: None,
            applied_transform: None,
        },
    ]);

    let json = metadata.to_json_pretty().expect("catalog renders");
    let parsed = CatalogMetadata::from_json(&json).expect("catalog parses");

    assert_eq!(parsed.epoch, 7);
    assert!(
        parsed.digest.starts_with("sha256:"),
        "catalog digest must be collision-resistant: {}",
        parsed.digest
    );
    assert_eq!("sha256:".len() + 64, parsed.digest.len());
    assert_eq!(parsed.digest, metadata.digest);
    assert_eq!(parsed.entries, metadata.entries);
}

#[test]
fn digest_is_independent_of_entry_order() {
    let enum_entry = entry(CatalogEntryKind::Enum, "books::Status", "enum-status", &[]);
    let active = entry(
        CatalogEntryKind::EnumMember,
        "books::Status::active",
        "member-active",
        &[],
    );
    let archived = entry(
        CatalogEntryKind::EnumMember,
        "books::Status::archived",
        "member-archived",
        &[],
    );

    let source_order = catalog(vec![enum_entry.clone(), active.clone(), archived.clone()]);
    let reordered = catalog(vec![archived, enum_entry, active]);

    assert_eq!(
        source_order.digest, reordered.digest,
        "catalog digest must bind identity entries, not their source/member order"
    );
}

#[test]
fn digest_golden_covers_ordering_and_optional_fields() {
    let mut store = literal_entry(
        CatalogEntryKind::Store,
        "books::^books",
        "cat_11111111111111111111111111111111",
        &[],
    );
    store.accepted_key_shape = Some("int,string".to_string());
    let mut index = literal_entry(
        CatalogEntryKind::StoreIndex,
        "books::^books::byTitle",
        "cat_22222222222222222222222222222222",
        &[],
    );
    index.accepted_index_shape =
        Some("unique=false;keys=cat_33333333333333333333333333333333".to_string());
    let mut member = literal_entry(
        CatalogEntryKind::ResourceMember,
        "books::Book::title",
        "cat_33333333333333333333333333333333",
        &["library::Book::name"],
    );
    member.accepted_struct = Some("leaf:string".to_string());
    let reserved = CatalogEntry {
        lifecycle: CatalogLifecycle::Reserved,
        ..literal_entry(
            CatalogEntryKind::EnumMember,
            "books::Status::archived",
            "cat_44444444444444444444444444444444",
            &["books::Status::inactive"],
        )
    };

    let metadata =
        CatalogMetadata::new(11, vec![reserved, member, index, store]).expect("catalog builds");

    assert_eq!(
        metadata.digest,
        "sha256:dba9bff05d704d6a311f0cdadb6e2a21c5240a5c8d48c19df281e1ad92e17b99"
    );
}

#[test]
fn round_trips_reserved_lifecycle() {
    let metadata = catalog(vec![CatalogEntry {
        lifecycle: CatalogLifecycle::Reserved,
        ..entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::oldTitle",
            "member-old-title",
            &[],
        )
    }]);

    let json = metadata.to_json_pretty().expect("catalog renders");
    assert!(json.contains("\"lifecycle\": \"reserved\""), "{json}");
    let parsed = CatalogMetadata::from_json(&json).expect("catalog parses");

    assert_eq!(parsed.entries[0].lifecycle, CatalogLifecycle::Reserved);
}

#[test]
fn non_sha_digest_is_rejected() {
    let metadata = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    let json = metadata
        .to_json_pretty()
        .expect("catalog renders")
        .replacen(
            &format!("\"digest\": \"{}\"", metadata.digest),
            "\"digest\": \"weak:0000000000000000\"",
            1,
        );

    let error = CatalogMetadata::from_json(&json).expect_err("non-SHA digest rejected");

    assert_eq!(error.code, CATALOG_INVALID);
}

/// A retired identity is dropped from the catalog, never carried as a `removed` lifecycle
/// marker: `removed` is not a representable [`CatalogLifecycle`], so a catalog whose
/// lifecycle text reads `removed` fails closed with `CATALOG_INVALID`. The contrast against
/// the byte-identical `active` catalog, which parses clean, pins the rejection to the
/// lifecycle value rather than the digest or another field.
#[test]
fn removed_lifecycle_is_rejected() {
    let metadata = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    let json = metadata.to_json_pretty().expect("catalog renders");
    CatalogMetadata::from_json(&json).expect("active lifecycle parses clean");

    let field = "\"lifecycle\": \"active\"";
    assert!(
        json.contains(field),
        "on-disk lifecycle spelling drifted: {json}"
    );
    let removed = json.replacen(field, "\"lifecycle\": \"removed\"", 1);

    let error = CatalogMetadata::from_json(&removed).expect_err("removed lifecycle rejected");
    assert_eq!(error.code, CATALOG_INVALID);
}

#[test]
fn deprecated_lifecycle_is_rejected() {
    let metadata = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    let json = metadata.to_json_pretty().expect("catalog renders");
    CatalogMetadata::from_json(&json).expect("active lifecycle parses clean");

    let deprecated = json.replacen(
        "\"lifecycle\": \"active\"",
        "\"lifecycle\": \"deprecated\"",
        1,
    );

    let error = CatalogMetadata::from_json(&deprecated).expect_err("deprecated lifecycle rejected");
    assert_eq!(error.code, CATALOG_INVALID);
}

#[test]
fn git_conflict_markers_report_a_typed_lock_corruption() {
    let lock = sample_lock();
    let clean = lock.to_lock_json_pretty().expect("lock renders");
    let conflicted = format!("<<<<<<< HEAD\n{clean}\n=======\n{clean}\n>>>>>>> branch\n");

    let error =
        CatalogLock::from_lock_json(&conflicted).expect_err("conflict markers are rejected");

    assert_eq!(error.code, LOCK_CORRUPT);
}

#[test]
fn parallel_additions_merge_without_regenerating_ids() {
    let branch_a_id = "cat_11111111111111111111111111111111";
    let branch_b_id = "cat_22222222222222222222222222222222";
    let metadata = CatalogMetadata::new(
        9,
        vec![
            literal_entry(
                CatalogEntryKind::Resource,
                "branch_a::Book",
                branch_a_id,
                &[],
            ),
            literal_entry(
                CatalogEntryKind::Resource,
                "branch_b::Magazine",
                branch_b_id,
                &[],
            ),
        ],
    )
    .expect("catalog builds");

    let parsed = CatalogMetadata::from_json(&metadata.to_json_pretty().expect("catalog renders"))
        .expect("catalog parses");

    assert_eq!(parsed.entries[0].stable_id, branch_a_id);
    assert_eq!(parsed.entries[1].stable_id, branch_b_id);
    assert!(parsed.digest.starts_with("sha256:"));
}

#[test]
fn rejects_hostile_catalog_json_families() {
    let metadata = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    let json = metadata.to_json_pretty().expect("catalog renders");

    let duplicate_digest = json.replacen(
        "\"entries\": [",
        &format!("\"digest\": \"{}\",\n  \"entries\": [", metadata.digest),
        1,
    );
    let lying_version = json.replacen("{", "{\n  \"version\": 999,", 1);
    let truncated = json
        .strip_suffix("}")
        .expect("pretty catalog ends in a JSON object")
        .to_string();
    let valid_sha_wrong_content = json.replacen("books::Book", "books::Magazine", 1);

    for (label, hostile) in [
        ("duplicate digest key", duplicate_digest),
        ("lying version", lying_version),
        ("truncated JSON", truncated),
        (
            "valid checksum shape with wrong content",
            valid_sha_wrong_content,
        ),
    ] {
        let error = CatalogMetadata::from_json(&hostile).expect_err(label);
        assert_eq!(error.code, CATALOG_INVALID, "{label}");
    }

    let null_path = catalog(vec![literal_entry(
        CatalogEntryKind::Resource,
        "books::Book\0Shadow",
        "cat_33333333333333333333333333333333",
        &[],
    )]);
    let error = CatalogMetadata::from_json(&null_path.to_json_pretty().expect("catalog renders"))
        .expect_err("null byte path must fail closed");
    assert_eq!(error.code, CATALOG_INVALID);

    let null_alias = catalog(vec![literal_entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "cat_44444444444444444444444444444444",
        &["books::Book\0Alias"],
    )]);
    let error = CatalogMetadata::from_json(&null_alias.to_json_pretty().expect("catalog renders"))
        .expect_err("null byte alias must fail closed");
    assert_eq!(error.code, CATALOG_INVALID);

    let mut store_with_null_shape = literal_entry(
        CatalogEntryKind::Store,
        "books::^books",
        "cat_55555555555555555555555555555555",
        &[],
    );
    store_with_null_shape.accepted_key_shape = Some("int\0str".to_string());
    let error = CatalogMetadata::from_json(
        &catalog(vec![store_with_null_shape])
            .to_json_pretty()
            .expect("catalog renders"),
    )
    .expect_err("null byte accepted key shape must fail closed");
    assert_eq!(error.code, CATALOG_INVALID);

    let mut index_with_null_shape = literal_entry(
        CatalogEntryKind::StoreIndex,
        "books::^books::byTitle",
        "cat_77777777777777777777777777777777",
        &[],
    );
    index_with_null_shape.accepted_index_shape = Some("unique=false\0".to_string());
    let error = CatalogMetadata::from_json(
        &catalog(vec![index_with_null_shape])
            .to_json_pretty()
            .expect("catalog renders"),
    )
    .expect_err("null byte accepted index shape must fail closed");
    assert_eq!(error.code, CATALOG_INVALID);

    let index_without_shape = literal_entry(
        CatalogEntryKind::StoreIndex,
        "books::^books::byTitle",
        "cat_88888888888888888888888888888888",
        &[],
    );
    let error = CatalogMetadata::from_json(
        &catalog(vec![index_without_shape])
            .to_json_pretty()
            .expect("catalog renders"),
    )
    .expect_err("store index without accepted shape must fail closed");
    assert_eq!(error.code, CATALOG_INVALID);

    let mut member_with_null_struct = literal_entry(
        CatalogEntryKind::ResourceMember,
        "books::Book::title",
        "cat_66666666666666666666666666666666",
        &[],
    );
    member_with_null_struct.accepted_struct = Some("leaf:str\0".to_string());
    let error = CatalogMetadata::from_json(
        &catalog(vec![member_with_null_struct])
            .to_json_pretty()
            .expect("catalog renders"),
    )
    .expect_err("null byte accepted structural signature must fail closed");
    assert_eq!(error.code, CATALOG_INVALID);
}

/// A `Store` catalog entry carrying an accepted key shape, for fingerprint fixtures.
fn store_entry(path: &str, stable_id: &str, key_shape: &str) -> CatalogEntry {
    let mut entry = literal_entry(CatalogEntryKind::Store, path, stable_id, &[]);
    entry.accepted_key_shape = Some(key_shape.to_string());
    entry
}

/// A `StoreIndex` catalog entry carrying an accepted index shape, for fingerprint fixtures.
fn index_entry(path: &str, stable_id: &str, index_shape: &str) -> CatalogEntry {
    let mut entry = literal_entry(CatalogEntryKind::StoreIndex, path, stable_id, &[]);
    entry.accepted_index_shape = Some(index_shape.to_string());
    entry
}

/// A `ResourceMember` catalog entry carrying an accepted structural signature, for fixtures.
fn member_entry(path: &str, stable_id: &str, struct_sig: &str) -> CatalogEntry {
    let mut entry = literal_entry(CatalogEntryKind::ResourceMember, path, stable_id, &[]);
    entry.accepted_struct = Some(struct_sig.to_string());
    entry
}

/// A well-formed `sha256:`-prefixed producing source digest for lock fixtures.
fn sample_source_digest() -> String {
    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string()
}

/// A lock with several fingerprinted entries, a non-empty cross-lifecycle ledger, a monotonic
/// `epoch_high_water` above every tombstone, and a producing `source_digest`.
fn sample_lock() -> CatalogLock {
    let entries = vec![
        LockEntry::from_catalog_entry(&store_entry(
            "books::^books",
            "cat_11111111111111111111111111111111",
            "int,string",
        )),
        LockEntry::from_catalog_entry(&index_entry(
            "books::^books::byTitle",
            "cat_22222222222222222222222222222222",
            "unique=false;keys=cat_33333333333333333333333333333333",
        )),
        LockEntry::from_catalog_entry(&member_entry(
            "books::Book::title",
            "cat_33333333333333333333333333333333",
            "leaf:string",
        )),
    ];
    let ledger = vec![
        LockLedgerTombstone {
            kind: CatalogEntryKind::ResourceMember,
            path: "books::Book::subtitle".to_string(),
            id: "cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            lifecycle: CatalogLifecycle::Reserved,
            high_water: 4,
        },
        LockLedgerTombstone {
            kind: CatalogEntryKind::ResourceMember,
            path: "books::Book::blurb".to_string(),
            id: "cat_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            lifecycle: CatalogLifecycle::Reserved,
            high_water: 6,
        },
    ];
    CatalogLock::new(entries, ledger, 9, sample_source_digest()).expect("sample lock builds")
}

#[test]
fn lock_round_trips_fingerprints_ledger_epoch_high_water_and_source_digest() {
    let lock = sample_lock();
    let json = lock.to_lock_json_pretty().expect("lock renders");
    let parsed = CatalogLock::from_lock_json(&json).expect("lock parses");

    assert_eq!(parsed.entries, lock.entries);
    assert_eq!(parsed.ledger, lock.ledger);
    assert_eq!(parsed.epoch_high_water, lock.epoch_high_water);
    assert_eq!(parsed.epoch_high_water, 9);
    assert_eq!(parsed.source_digest, lock.source_digest);

    for lock_entry in &parsed.entries {
        assert!(
            lock_entry.shape_fingerprint.starts_with("sha256:"),
            "fingerprint must be a sha256 hash: {}",
            lock_entry.shape_fingerprint
        );
        assert_eq!(
            "sha256:".len() + 64,
            lock_entry.shape_fingerprint.len(),
            "fingerprint must be 32 hex bytes: {}",
            lock_entry.shape_fingerprint
        );
    }

    // The `(kind, path)` adoption anchor round-trips verbatim — it is committed text, not folded
    // into the fingerprint — so a fresh checkout can match a source declaration to its committed id.
    let store = parsed
        .entries
        .iter()
        .find(|entry| entry.path == "books::^books")
        .expect("store entry round-trips its path");
    assert_eq!(store.kind, CatalogEntryKind::Store);
    assert_eq!(store.stable_id, "cat_11111111111111111111111111111111");
    let member = parsed
        .entries
        .iter()
        .find(|entry| entry.path == "books::Book::title")
        .expect("member entry round-trips its path");
    assert_eq!(member.kind, CatalogEntryKind::ResourceMember);

    // The path text IS committed (it is the adoption anchor), but the FULL accepted SHAPE text is
    // still folded into the opaque fingerprint, never spelled out: a reader of the lock learns
    // identity and a shape-change signal, not the shape grammar itself.
    for forbidden in ["leaf:", "keyed-group:", "unique="] {
        assert!(
            !json.contains(forbidden),
            "lock projection leaked full-shape text `{forbidden}`: {json}"
        );
    }
}

#[test]
fn fingerprints_separate_shapes_and_ignore_renames() {
    let key_int = LockEntry::from_catalog_entry(&store_entry(
        "a::^s",
        "cat_11111111111111111111111111111111",
        "int",
    ));
    let key_int_string = LockEntry::from_catalog_entry(&store_entry(
        "a::^s",
        "cat_11111111111111111111111111111111",
        "int,string",
    ));
    assert_ne!(
        key_int.shape_fingerprint, key_int_string.shape_fingerprint,
        "store key arity must change the fingerprint"
    );

    let struct_int = LockEntry::from_catalog_entry(&member_entry(
        "a::B::m",
        "cat_22222222222222222222222222222222",
        "leaf:int",
    ));
    let struct_bool = LockEntry::from_catalog_entry(&member_entry(
        "a::B::m",
        "cat_22222222222222222222222222222222",
        "leaf:bool",
    ));
    assert_ne!(
        struct_int.shape_fingerprint, struct_bool.shape_fingerprint,
        "struct leaf type must change the fingerprint"
    );

    let index_nonunique = LockEntry::from_catalog_entry(&index_entry(
        "a::^s::byX",
        "cat_33333333333333333333333333333333",
        "unique=false;keys=cat_44444444444444444444444444444444",
    ));
    let index_unique = LockEntry::from_catalog_entry(&index_entry(
        "a::^s::byX",
        "cat_33333333333333333333333333333333",
        "unique=true;keys=cat_44444444444444444444444444444444",
    ));
    assert_ne!(
        index_nonunique.shape_fingerprint, index_unique.shape_fingerprint,
        "index uniqueness must change the fingerprint"
    );

    let original = LockEntry::from_catalog_entry(&member_entry(
        "books::Book::title",
        "cat_55555555555555555555555555555555",
        "leaf:string",
    ));
    let renamed = LockEntry::from_catalog_entry(&member_entry(
        "library::Tome::name",
        "cat_55555555555555555555555555555555",
        "leaf:string",
    ));
    assert_eq!(
        original.shape_fingerprint, renamed.shape_fingerprint,
        "a pure rename preserving shape must preserve the fingerprint"
    );
}

#[test]
fn from_lock_json_rejects_hostile_families() {
    let lock = sample_lock();
    let clean = lock.to_lock_json_pretty().expect("lock renders");
    CatalogLock::from_lock_json(&clean).expect("the clean baseline parses");

    let fingerprint = &lock.entries[0].shape_fingerprint;

    // Non-sha256 fingerprint: wrong length.
    let short_fingerprint = clean.replacen(fingerprint.as_str(), "sha256:0123456789abcdef", 1);
    // Non-sha256 fingerprint: missing prefix.
    let unprefixed_fingerprint = clean.replacen(
        fingerprint.as_str(),
        &fingerprint.replacen("sha256:", "weak:", 1),
        1,
    );
    // A ledger entry reissuing an id that is also an active LockEntry.
    let reissued_active_id = clean.replacen(
        "cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "cat_11111111111111111111111111111111",
        1,
    );
    // epoch_high_water below a tombstone's recorded high-water (tombstone high_water 6 > 5).
    let low_epoch_high_water = clean.replacen("\"epochHighWater\": 9", "\"epochHighWater\": 5", 1);
    // A non-sha256 source_digest.
    let non_sha_source_digest = clean.replacen(
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "weak:0000",
        1,
    );
    // Duplicate ledger id (reissued tombstone, never silently deduped).
    let duplicate_ledger_id = clean.replacen(
        "cat_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        1,
    );
    // A ledger tombstone recording the active lifecycle, which the ledger never holds.
    let active_tombstone = clean.replacen(
        "      \"id\": \"cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\n      \"lifecycle\": \"reserved\"",
        "      \"id\": \"cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\n      \"lifecycle\": \"active\"",
        1,
    );
    // An injected unknown JSON field at the root (serde deny_unknown_fields).
    let unknown_field = clean.replacen("{", "{\n  \"authoritative\": true,", 1);
    // An unknown key inside a nested LockEntry object (serde deny_unknown_fields).
    let unknown_entry_field = clean.replacen(
        "      \"stableId\": \"cat_11111111111111111111111111111111\",",
        "      \"stableId\": \"cat_11111111111111111111111111111111\",\n      \"authoritative\": true,",
        1,
    );
    // An unknown key inside a nested LockLedgerTombstone object (serde deny_unknown_fields).
    let unknown_tombstone_field = clean.replacen(
        "      \"id\": \"cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",",
        "      \"id\": \"cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\n      \"authoritative\": true,",
        1,
    );

    for (label, hostile) in [
        ("short fingerprint", short_fingerprint),
        ("unprefixed fingerprint", unprefixed_fingerprint),
        ("ledger reissues an active id", reissued_active_id),
        ("epoch high water below tombstone", low_epoch_high_water),
        ("non-sha source digest", non_sha_source_digest),
        ("duplicate ledger id", duplicate_ledger_id),
        ("active lifecycle tombstone", active_tombstone),
        ("unknown root field", unknown_field),
        ("unknown entry field", unknown_entry_field),
        ("unknown tombstone field", unknown_tombstone_field),
    ] {
        assert_ne!(
            hostile, clean,
            "{label}: the mutation must actually change the JSON, or the case tests nothing"
        );
        let error = CatalogLock::from_lock_json(&hostile).expect_err(label);
        assert_eq!(error.code, LOCK_CORRUPT, "{label}");
    }
}

/// The `(kind, path)` adoption anchor is the lock's identity key, so the codec fails closed on an
/// empty path and on a duplicate `(kind, path)` — either would leave first-run adoption unable to
/// resolve a source declaration to exactly one committed id.
#[test]
fn lock_rejects_empty_and_duplicate_path_anchors() {
    let empty_path = LockEntry::from_catalog_entry(&literal_entry(
        CatalogEntryKind::Resource,
        "",
        "cat_11111111111111111111111111111111",
        &[],
    ));
    let empty_error = CatalogLock::new(vec![empty_path], Vec::new(), 1, sample_source_digest())
        .expect_err("an empty entry path is rejected");
    assert_eq!(empty_error.code, LOCK_CORRUPT);
    assert!(
        empty_error.message.contains("path must not be empty"),
        "expected the path guard message, got: {}",
        empty_error.message
    );

    let duplicate = vec![
        LockEntry::from_catalog_entry(&literal_entry(
            CatalogEntryKind::Resource,
            "books::Book",
            "cat_22222222222222222222222222222222",
            &[],
        )),
        LockEntry::from_catalog_entry(&literal_entry(
            CatalogEntryKind::Resource,
            "books::Book",
            "cat_33333333333333333333333333333333",
            &[],
        )),
    ];
    let duplicate_error = CatalogLock::new(duplicate, Vec::new(), 1, sample_source_digest())
        .expect_err("a duplicate (kind, path) anchor is rejected");
    assert_eq!(duplicate_error.code, LOCK_CORRUPT);
    assert!(
        duplicate_error.message.contains("appears twice"),
        "expected the anchor uniqueness message, got: {}",
        duplicate_error.message
    );
}

/// A ledger tombstone carries the retired `(kind, path)` so the committed lock fully represents a
/// reserved path, and projects to and from a Reserved catalog entry as mutual inverses — the round
/// trip a fresh checkout relies on to reconstruct a reserved store row from the lock alone.
#[test]
fn ledger_tombstone_round_trips_a_reserved_entry_with_kind_and_path() {
    let reserved = CatalogEntry {
        lifecycle: CatalogLifecycle::Reserved,
        ..literal_entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::subtitle",
            "cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            &[],
        )
    };
    let tombstone = LockLedgerTombstone::from_reserved_entry(&reserved, 7);
    assert_eq!(tombstone.kind, CatalogEntryKind::ResourceMember);
    assert_eq!(tombstone.path, "books::Book::subtitle");
    assert_eq!(tombstone.id, "cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(tombstone.lifecycle, CatalogLifecycle::Reserved);
    assert_eq!(tombstone.high_water, 7);
    assert_eq!(
        tombstone.reserved_catalog_entry(),
        reserved,
        "the tombstone reconstructs the reserved entry verbatim"
    );

    // The lock JSON round-trips the new `(kind, path)` fields.
    let lock = CatalogLock::new(Vec::new(), vec![tombstone], 7, sample_source_digest())
        .expect("lock with a kind/path tombstone builds");
    let parsed = CatalogLock::from_lock_json(&lock.to_lock_json_pretty().expect("renders"))
        .expect("lock parses");
    assert_eq!(parsed.ledger, lock.ledger);
}

/// A reserved `(kind, path)` and a live active `(kind, path)` are mutually exclusive: the same
/// path cannot be both a retired tombstone and an active entry, and one path cannot be reserved
/// twice. Either collision fails closed, so adoption never reconstructs a reserved row that
/// shadows or duplicates a live declaration.
#[test]
fn lock_rejects_a_reserved_path_that_collides_with_a_live_or_reserved_path() {
    let active = LockEntry::from_catalog_entry(&literal_entry(
        CatalogEntryKind::ResourceMember,
        "books::Book::title",
        "cat_11111111111111111111111111111111",
        &[],
    ));
    let shadowing = LockLedgerTombstone {
        kind: CatalogEntryKind::ResourceMember,
        path: "books::Book::title".to_string(),
        id: "cat_22222222222222222222222222222222".to_string(),
        lifecycle: CatalogLifecycle::Reserved,
        high_water: 1,
    };
    let shadow_error = CatalogLock::new(vec![active], vec![shadowing], 1, sample_source_digest())
        .expect_err("a reserved path shadowing a live entry is rejected");
    assert_eq!(shadow_error.code, LOCK_CORRUPT);
    assert!(
        shadow_error.message.contains("is also a live active entry"),
        "expected the live-shadow guard message, got: {}",
        shadow_error.message
    );

    let first = LockLedgerTombstone {
        kind: CatalogEntryKind::ResourceMember,
        path: "books::Book::subtitle".to_string(),
        id: "cat_33333333333333333333333333333333".to_string(),
        lifecycle: CatalogLifecycle::Reserved,
        high_water: 1,
    };
    let second = LockLedgerTombstone {
        id: "cat_44444444444444444444444444444444".to_string(),
        ..first.clone()
    };
    let duplicate_error =
        CatalogLock::new(Vec::new(), vec![first, second], 1, sample_source_digest())
            .expect_err("a path reserved twice is rejected");
    assert_eq!(duplicate_error.code, LOCK_CORRUPT);
    assert!(
        duplicate_error.message.contains("is reserved twice"),
        "expected the reserved-twice guard message, got: {}",
        duplicate_error.message
    );
}

/// The id NUL guard fires at the [`CatalogLock::new`] boundary for a NUL embedded in either a
/// stable id or a ledger id, pinning the guard rather than serde's control-char rejection. The
/// message text proves the guard, not just the code, produced the failure.
#[test]
fn lock_new_rejects_nul_in_ids() {
    let nul_stable_entry = LockEntry::from_catalog_entry(&member_entry(
        "books::Book::title",
        "cat_5555555555555555555555555555555\u{0}",
        "leaf:string",
    ));
    let stable_error = CatalogLock::new(
        vec![nul_stable_entry],
        Vec::new(),
        1,
        sample_source_digest(),
    )
    .expect_err("a NUL in a stable id is rejected");
    assert_eq!(stable_error.code, LOCK_CORRUPT);
    assert!(
        stable_error
            .message
            .contains("entry stable id must not contain a NUL byte"),
        "expected the id NUL guard message, got: {}",
        stable_error.message
    );

    let nul_ledger = LockLedgerTombstone {
        kind: CatalogEntryKind::ResourceMember,
        path: "books::Book::subtitle".to_string(),
        id: "cat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{0}".to_string(),
        lifecycle: CatalogLifecycle::Reserved,
        high_water: 1,
    };
    let ledger_error = CatalogLock::new(Vec::new(), vec![nul_ledger], 1, sample_source_digest())
        .expect_err("a NUL in a ledger id is rejected");
    assert_eq!(ledger_error.code, LOCK_CORRUPT);
    assert!(
        ledger_error
            .message
            .contains("ledger id must not contain a NUL byte"),
        "expected the id NUL guard message, got: {}",
        ledger_error.message
    );
}

/// A ` ` escape in the lock JSON decodes into a real NUL inside the string, reaching
/// [`CatalogLock::from_lock_json`]'s id NUL guard rather than being rejected as a raw control
/// character. The escape lands in a stable id, so the id guard rejects it with [`LOCK_CORRUPT`].
#[test]
fn from_lock_json_rejects_escaped_nul_in_id() {
    let lock = CatalogLock::new(
        vec![LockEntry::from_catalog_entry(&member_entry(
            "books::Book::title",
            "cat_55555555555555555555555555555555",
            "leaf:string",
        ))],
        Vec::new(),
        1,
        sample_source_digest(),
    )
    .expect("baseline lock builds");
    let clean = lock.to_lock_json_pretty().expect("lock renders");

    // Replace the final hex digit of the stable id with an escaped NUL; serde decodes the escape
    // into a real NUL inside the decoded string, so validate() sees a NUL the id guard must reject.
    let escaped = clean.replacen(
        "cat_55555555555555555555555555555555",
        "cat_5555555555555555555555555555555\\u0000",
        1,
    );
    assert!(
        escaped.contains("\\u0000"),
        "the fixture must carry a JSON escape, not a raw NUL: {escaped}"
    );

    let error = CatalogLock::from_lock_json(&escaped).expect_err("an escaped NUL id is rejected");
    assert_eq!(error.code, LOCK_CORRUPT);
    assert!(
        error.message.contains("must not contain a NUL byte"),
        "expected the id NUL guard message, got: {}",
        error.message
    );
}
