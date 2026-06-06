mod support;
mod support_discharge;

use marrow_check::evolution::{RepairReason, Verdict, preview};
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

use support::catalog::write_catalog;
use support::{temp_project, write};
use support_discharge::*;

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
        let accepted = accepted_catalog(
            7,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![member_entry("books::Book::title", &hex_id(3), "string")],
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
    let seed = Seed::new(&store, &place);
    // One record keyed under the old `int` shape, addressed by the preserved store id.
    seed.record(1);

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
            .any(|diagnostic| diagnostic.catalog_id.as_str() == store_id),
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
        let accepted = accepted_catalog(
            8,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![member_entry("books::Book::title", &hex_id(3), "string")],
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
        let accepted = accepted_catalog(
            9,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![member_entry("books::Book::title", &hex_id(3), "string")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &hex_id(3), Scalar::Str("Dune".into()));

    let (result, _) = preview(&program, &store).expect("preview");

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
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![member_entry(
                "books::Book::parent",
                &value_id,
                &format!("id:{store_stable}:1"),
            )],
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
    let seed = Seed::new(&store, &place);
    // Seed a valid identity payload for the renamed store.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, encode_identity_payload(&[SavedKey::Int(1)]));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let value_id = member_catalog_id(&place, "parent");
    assert!(
        matches!(verdict_for(&result, &value_id), Verdict::DataProof),
        "a pure store rename behind an identity leaf must not read as a retype: {:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}
