//! The accepted catalog's canonical JSON round-trip and validation invariants: a
//! committed snapshot serializes and parses back identically, identity collisions and a
//! non-SHA or mismatched digest fail closed, and an unrepresentable lifecycle is rejected.
//! This is the on-the-wire contract the engine-resident store and the backup catalog
//! section both persist through.

use std::hash::{Hash, Hasher};

use marrow_catalog::{
    CATALOG_INVALID, CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata,
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
        accepted_struct: None,
    }
}

/// Wrap `entries` in a catalog at a fixed epoch so a fixture lists only what it cares about.
fn catalog(entries: Vec<CatalogEntry>) -> CatalogMetadata {
    CatalogMetadata::new(7, entries)
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
        let json = metadata.to_json_pretty();
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
            lifecycle: CatalogLifecycle::Deprecated,
            accepted_key_shape: None,
            accepted_struct: None,
        },
    ]);

    let json = metadata.to_json_pretty();
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

    let json = metadata.to_json_pretty();
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
    let json = metadata.to_json_pretty().replacen(
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
    let json = metadata.to_json_pretty();
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
    );

    let parsed = CatalogMetadata::from_json(&metadata.to_json_pretty()).expect("catalog parses");

    assert_eq!(parsed.entries[0].stable_id, branch_a_id);
    assert_eq!(parsed.entries[1].stable_id, branch_b_id);
    assert!(parsed.digest.starts_with("sha256:"));
}
