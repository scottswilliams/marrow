use crate::support;
use crate::support_discharge;
use marrow_catalog::CatalogEntryKind;
use marrow_check::evolution::{RepairReason, Verdict, preview};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, encode_value};

use support::catalog::write_catalog;
use support::{temp_project, write};
use support_discharge::*;

/// Reshape the accepted `string` leaf `value` over a single populated record and assert the
/// change fails closed for `expected_reason`. The accepted catalog records `value` as a leaf;
/// `value_decl` re-declares it (as a group, a keyed layer, or another shape) so the current
/// declaration produces no leaf token at the member's path, while the old `string` cell still
/// lives under the member position the new shape now occupies. The reshaped member keeps its
/// accepted stable id, so the populated bytes the new shape cannot address steer the change to
/// a transform rather than a silent activation.
fn assert_leaf_reshape_fails_closed(
    name: &str,
    value_decl: &str,
    expected_reason: RepairReason,
) -> Result<(), Box<dyn std::error::Error>> {
    let value_id = hex_id(3);
    let root = temp_project(name, |root| {
        write(
            root,
            "src/books.mw",
            &format!(
                "module books\n\
                 resource Book\n\
                 {value_decl}\
                 store ^books(id: int): Book\n\
                 pub fn add(): Id(^books)\n\
                 \x20   return nextId(^books)\n"
            ),
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
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, encode_value(&Scalar::Str("draft".into()))?);

    let reshaped_id = group_member_catalog_id(&place, "value")?;
    assert_eq!(
        reshaped_id, value_id,
        "a reshaped leaf keeps the member's accepted stable id"
    );
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    assert_fails_closed(&result, &diagnostics, &reshaped_id, expected_reason);
    Ok(())
}

/// A keyed-leaf value type change over populated entries fails closed, exactly as a
/// top-level leaf retype does: the stored bytes were written under the old value type,
/// so the new type's decoder would silently reinterpret them. The keyed leaf carries an
/// identity-aware accepted leaf token the discharge compares against; a populated retyped
/// value is steered to a transform rather than activated.
#[test]
fn keyed_leaf_value_retype_over_populated_entries_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let leaf_stable = hex_id(3);
    let root = temp_project("discharge-keyed-leaf-value-retype", |root| {
        // The keyed-leaf value type changes `string` -> `int`; its entries were written as strings.
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   tags(pos: int): int\n\
             store ^books(id: int): Book\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            3,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![member_entry(
                "books::Book::tags",
                &leaf_stable,
                "[int]string",
            )],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // One record with a keyed-leaf entry whose value was stored as a `string`.
    seed.record(1);
    seed.keyed_leaf(
        1,
        "tags",
        SavedKey::Int(0),
        encode_value(&Scalar::Str("draft".into()))?,
    );

    let leaf_id = keyed_leaf_catalog_id(&place, "tags")?;
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    assert_retype_steered(&leaf_id, &result, &diagnostics);

    Ok(())
}

/// A keyed leaf whose value type is unchanged proves cleanly over populated entries: the
/// stored value decodes under the current value type, so there is no reinterpretation
/// hazard and the change is activatable. This pins that recording an accepted leaf token
/// for keyed leaves does not block an honest no-change case.
#[test]
fn keyed_leaf_value_unchanged_proves() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-keyed-leaf-value-unchanged", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   tags(pos: int): string\n\
             store ^books(id: int): Book\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the schema so the map's catalog id addresses the store; then exercise an
    // unchanged re-preview over a populated map.
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.keyed_leaf(
        1,
        "tags",
        SavedKey::Int(0),
        encode_value(&Scalar::Str("draft".into()))?,
    );

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        result.is_activatable(),
        "an unchanged keyed-leaf value must stay activatable: {:#?}",
        result.verdicts
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    Ok(())
}

/// A member that WAS a plain leaf becoming a GROUP over populated data fails closed: the new
/// group shape would orphan the old single-cell bytes, so it is steered to a transform.
#[test]
fn leaf_member_becoming_a_group_over_populated_data_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    assert_leaf_reshape_fails_closed(
        "discharge-leaf-to-group",
        "\x20   value\n\x20       required first: string\n",
        RepairReason::TypeChangeRequiresTransform,
    )?;

    Ok(())
}

/// A member that WAS a plain leaf becoming a KEYED LAYER over populated data fails closed: the
/// new keyed shape addresses entries by a key the old bytes were never written at, so it is
/// steered to a transform.
#[test]
fn leaf_member_becoming_a_keyed_layer_over_populated_data_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    assert_leaf_reshape_fails_closed(
        "discharge-leaf-to-keyed",
        "\x20   value(version: int)\n\x20       required body: string\n",
        RepairReason::TypeChangeRequiresTransform,
    )?;

    Ok(())
}

/// A leaf nested inside a populated KEYED GROUP, retyped, fails closed PER ENTRY. The accepted
/// catalog records the nested leaf `versions.body` as a `string`; source retypes it `int`. An
/// existing keyed entry carries a `string` value the new `int` decoder would silently
/// reinterpret. A retyped leaf below a keyed layer has no static path (its path needs an entry
/// key), so it must be probed through the per-entry keyed descent, not a flat subtree check,
/// or the old per-entry bytes are missed and it fails open.
#[test]
fn retype_of_leaf_nested_in_populated_keyed_group_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let body_stable = hex_id(4);
    let root = temp_project("discharge-keyed-nested-retype", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       required body: int\n\
             store ^policies(id: int): Policy\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
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
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "policies")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // One keyed entry whose `body` cell was written under the old `string` type.
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(7),
        "body",
        Scalar::Str("draft".into()),
    );

    let body_id = nested_member_catalog_id(&place, "versions", "body")?;
    assert_eq!(
        body_id, body_stable,
        "a retyped keyed-nested leaf keeps its accepted stable id"
    );
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    assert_retype_steered(&body_id, &result, &diagnostics);

    Ok(())
}

/// A keyed-nested retype whose old bytes happen to DECODE under the new type is the sharp
/// soundness hazard: a presence-only or per-entry validity proof would silently bless them.
/// An `int` keyed-nested leaf retyped to `bool` over an entry stored as `1` would read back as
/// `true`; the per-entry retype probe counts the entry as populated regardless of validity and
/// fails the change closed, so the overlapping bytes are never reinterpreted.
#[test]
fn retype_of_keyed_nested_leaf_with_overlapping_byte_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let body_stable = hex_id(4);
    let root = temp_project("discharge-keyed-nested-overlap", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       required body: bool\n\
             store ^policies(id: int): Policy\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
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
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "policies")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The entry's `body` cell was written as `int` `1`, a byte the new `bool` decoder accepts.
    seed.record(1);
    seed.keyed_member(1, "versions", SavedKey::Int(7), "body", Scalar::Int(1));

    let body_id = nested_member_catalog_id(&place, "versions", "body")?;
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    assert_retype_steered(&body_id, &result, &diagnostics);

    Ok(())
}

/// A leaf nested inside a populated keyed group whose type is UNCHANGED proves cleanly: the
/// per-entry retype probe must not fail closed on an honest no-change keyed-nested leaf.
#[test]
fn unchanged_leaf_nested_in_populated_keyed_group_proves() -> Result<(), Box<dyn std::error::Error>>
{
    let body_stable = hex_id(4);
    let root = temp_project("discharge-keyed-nested-unchanged", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       required body: string\n\
             store ^policies(id: int): Policy\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
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
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "policies")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Int(7),
        "body",
        Scalar::Str("draft".into()),
    );

    let body_id = nested_member_catalog_id(&place, "versions", "body")?;
    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(result.is_activatable(), "{:#?}", result.verdicts);
    assert!(
        matches!(verdict_for(&result, &body_id), Verdict::DataProof),
        "an unchanged keyed-nested leaf must prove, got {:#?}",
        verdict_for(&result, &body_id)
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    Ok(())
}

/// A leaf becoming a group that adds a brand-new REQUIRED sub-member fails closed over a record
/// whose old leaf cell is UNPOPULATED. The old leaf disappearing is handled by the disappeared-
/// leaf probe, but that probe alone only fails closed when the old cell holds bytes. The new
/// group's brand-new required sub-member must ALSO be presence-scanned, so a record that exists
/// but has no value at the old leaf position is caught for the missing required sub-member
/// rather than fixed up by the empty disappeared-leaf probe and silently activated.
#[test]
fn leaf_to_group_adding_required_submember_over_empty_cell_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let value_id = hex_id(3);
    let root = temp_project("discharge-leaf-to-group-required", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   marker: string\n\
             \x20   value\n\
             \x20       required first: string\n\
             store ^books(id: int): Book\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![
                member_entry("books::Book::marker", &hex_id(5), "string"),
                member_entry("books::Book::value", &value_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The record exists (a sibling `marker` cell) but its old `value` leaf cell was never
    // populated, so the disappeared-leaf probe sees nothing; the new required `value.first`
    // sub-member is missing and must fail closed.
    seed.record(1);
    seed.member_by_id(1, &hex_id(5), Scalar::Str("seen".into()));

    // `value.first` is a brand-new required sub-member of the new group; its identity lives
    // only in the proposal, so the descend reaches it by its proposal-minted id.
    let first_id = new_member_proposal_id(&program, "books::Book::value::first")?;
    let (result, _diagnostics) = preview(&program, &store).expect("preview");

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

    Ok(())
}
