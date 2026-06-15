//! The accepted catalog's canonical JSON round-trip and validation invariants: a
//! committed snapshot serializes and parses back identically, identity collisions and a
//! non-SHA or mismatched digest fail closed, and an unrepresentable lifecycle is rejected.
//! This is the on-the-wire contract the engine-resident store and the backup catalog
//! section both persist through.

use std::hash::{Hash, Hasher};

use marrow_catalog::{
    CATALOG_INVALID, CATALOG_MERGE_CONFLICT, CatalogEntry, CatalogEntryKind, CatalogLifecycle,
    CatalogMetadata,
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
fn git_conflict_markers_report_a_typed_merge_conflict() {
    let metadata = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    let conflicted = format!(
        "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> branch\n",
        metadata.to_json_pretty().expect("catalog renders"),
        metadata.to_json_pretty().expect("catalog renders")
    );

    let error = CatalogMetadata::from_json(&conflicted).expect_err("conflict markers are rejected");

    assert_eq!(error.code, CATALOG_MERGE_CONFLICT);
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
