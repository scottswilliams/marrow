mod support;
mod support_discharge;

use marrow_catalog::CatalogEntryKind;
use marrow_check::evolution::{EvolutionWitness, RepairDiagnostic, RepairReason, Verdict, preview};
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

use support::catalog::write_catalog;
use support::{temp_project, write};
use support_discharge::*;

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
                 resource Book\n\
                 \x20   required value: {new_type}\n\
                 store ^books(id: int): Book\n\
                 pub fn add(value: {new_type}): Id(^books)\n\
                 \x20   return nextId(^books)\n"
            ),
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![member_entry("books::Book::value", &value_id, accepted_leaf)],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &value_id, old_value);

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    let value_id = member_catalog_id(&place, "value");
    (value_id, result, diagnostics)
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
             resource Book\n\
             \x20   required heading: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.title -> Book.heading\n\
             pub fn add(heading: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            5,
            "books::Book",
            "books::^books",
            None,
            vec![member_entry("books::Book::title", &title_id, "string")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The renamed member keeps its accepted stable id; seed data under it.
    seed.record(1);
    seed.member_by_id(1, &title_id, Scalar::Str("Dune".into()));

    let result = witness(&program, &store);

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
             resource Book\n\
             \x20   required count: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.title -> Book.count\n\
             pub fn add(count: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            6,
            "books::Book",
            "books::^books",
            None,
            vec![member_entry("books::Book::title", &title_id, "string")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The renamed member keeps the old stable id; seed a string under it.
    seed.record(1);
    seed.member_by_id(1, &title_id, Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
            .any(|diagnostic| diagnostic.catalog_id.as_str() == count_id),
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
             resource Book\n\
             \x20   value: bool\n\
             store ^books(id: int): Book\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![member_entry("books::Book::value", &value_id, "int")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Int(1));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
             resource Book\n\
             \x20   required title: string\n\
             \x20   value: bool\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![
                member_entry("books::Book::title", &hex_id(5), "string"),
                member_entry("books::Book::value", &value_id, "int"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // A record exists and carries the unchanged required `title`, but no `value` cell —
    // so the retyped optional member has no bytes to reinterpret.
    seed.record(1);
    seed.member_by_id(1, &hex_id(5), Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
             resource Book\n\
             \x20   required value: int\n\
             store ^books(id: int): Book\n\
             pub fn add(value: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![member_entry("books::Book::value", &value_id, "int")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Int(7));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
             resource Book\n\
             \x20   required title: string\n\
             \x20   rank: int\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the baseline so `title` is accepted, then a fresh check adds `rank`.
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
             resource Book\n\
             \x20   required value: Status\n\
             store ^books(id: int): Book\n\
             pub fn add(value: Status): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![member_entry("books::Book::value", &value_id, "int")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Seed the integer bytes the old `int` schema wrote under the preserved member id.
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Int(1));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
             resource Book\n\
             \x20   required value: Id(^books)\n\
             store ^books(id: int): Book\n\
             pub fn add(value: Id(^books)): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![member_entry("books::Book::value", &value_id, "int")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Int(1));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
             resource Book\n\
             \x20   required value: Id(^books)\n\
             store ^books(id: int): Book\n\
             pub fn add(value: Id(^books)): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        // The accepted catalog records the member's leaf as the enum's stable identity; the
        // source now types it `Id(^books)`, so the identity-aware tokens differ and it is a
        // retype across two non-scalar leaf kinds.
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![
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
    let seed = Seed::new(&store, &place);
    // Seed an identity payload — valid bytes for the NEW type — so the case turns on the
    // declared-type change, not on a decode failure: even bytes the new decoder accepts
    // must steer to a transform when the leaf kind changed.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, encode_identity_payload(&[SavedKey::Int(1)]));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
             resource Book\n\
             \x20   required value: bool\n\
             store ^books(id: int): Book\n\
             pub fn add(value: bool): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        // The accepted member entry exists but records no structural signature, so its leaf
        // token reads back as unknown: an entry minted before signatures were recorded. Its
        // prior type cannot be proven.
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::value",
                &value_id,
            )],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Bool(true));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let value_id = member_catalog_id(&place, "value");
    assert_retype_steered(&value_id, &result, &diagnostics);
}

/// A leaf retyped from a tokenizable scalar to a non-tokenizable `sequence` over populated
/// data fails closed: a leaf position whose new declared type produces no leaf token still
/// changed type, so the populated old bytes cannot be silently reread. The retype check must
/// be total over the new side, so a leaf whose new type yields no token still counts as a
/// type change rather than dropping out of the leaf map undetected.
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
             resource Book\n\
             \x20   required value: sequence[string]\n\
             store ^books(id: int): Book\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![member_entry("books::Book::value", &value_id, "string")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &value_id, Scalar::Str("draft".into()));

    let value_id = member_catalog_id(&place, "value");
    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
