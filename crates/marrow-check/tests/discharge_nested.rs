mod support;
mod support_discharge;

use marrow_check::evolution::{RepairDiagnostic, RepairReason, Verdict, preview};
use marrow_project::CatalogEntryKind;
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
fn assert_leaf_reshape_fails_closed(name: &str, value_decl: &str, expected_reason: RepairReason) {
    let value_id = hex_id(3);
    let root = temp_project(name, |root| {
        write(
            root,
            "src/books.mw",
            &format!(
                "module books\n\
                 resource Book at ^books(id: int)\n\
                 {value_decl}\
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
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_bytes_by_id(
        1,
        &value_id,
        encode_value(&Scalar::Str("draft".into())).unwrap(),
    );

    let reshaped_id = group_member_catalog_id(&place, "value");
    assert_eq!(
        reshaped_id, value_id,
        "a reshaped leaf keeps the member's accepted stable id"
    );
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    assert_fails_closed(&result, &diagnostics, &reshaped_id, expected_reason);
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
    let seed = Seed::new(&store, &place);
    // The old record carries `name.first` but predates required `name.last`.
    seed.record(1);
    seed.nested_member(1, "name", "first", Scalar::Str("Ada".into()));

    let result = witness(&program, &store);
    let last_id = nested_member_catalog_id(&place, "name", "last");

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
    let seed = Seed::new(&store, &place);
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
    let seed = Seed::new(&store, &place);
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

    assert!(result.is_activatable(), "{:#?}", result.verdicts);
    assert!(
        matches!(verdict_for(&result, &body_id), Verdict::DataProof),
        "{:#?}",
        result.verdicts
    );
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
        let accepted = accepted_catalog(
            3,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![member_entry(
                "books::Book::tags",
                &map_stable,
                "[int]string",
            )],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
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
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.keyed_leaf(
        1,
        "tags",
        SavedKey::Int(0),
        encode_value(&Scalar::Str("draft".into())).unwrap(),
    );

    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
/// accepted catalog id yet, so the presence scan must be proposal-aware to reach it at all.
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
        let accepted = accepted_catalog(
            3,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![member_entry("books::Book::title", &title_stable, "string")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Old records carry `title` but predate the brand-new required `pages`.
    seed.record(1);
    seed.member_by_id(1, &title_stable, Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member_by_id(2, &title_stable, Scalar::Str("Hyperion".into()));

    let pages_id = new_member_proposal_id(&program, "books::Book::pages");
    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
        let accepted = accepted_catalog(
            3,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![member_entry("books::Book::title", &title_stable, "string")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &title_stable, Scalar::Str("Dune".into()));

    let pages_id = new_member_proposal_id(&program, "books::Book::pages");
    let result = witness(&program, &store);

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
        let accepted = accepted_catalog(
            3,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![member_entry("books::Book::title", &title_stable, "string")],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    // No records seeded: the store is empty.
    let store = TreeStore::memory();

    let result = witness(&program, &store);

    assert!(
        result.is_activatable(),
        "a brand-new required member over an empty store must activate: {:#?}",
        result.verdicts
    );
}

/// A brand-new REQUIRED leaf added inside an EXISTING keyed layer over populated entries
/// fails closed with no default: the keyed layer already has entries that predate the new
/// leaf, so requiredness is unmet per existing entry. The new leaf has no bound facts id,
/// only a proposal-minted one, so the keyed scan must thread the resolved id to reach it.
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
                member_entry("policies::Policy::versions::note", &hex_id(4), "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
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
                member_entry("policies::Policy::versions::note", &hex_id(4), "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
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

    assert!(
        matches!(verdict_for(&result, &body_id), Verdict::Default { .. }),
        "a brand-new required keyed leaf with a default must backfill, got {:#?}",
        verdict_for(&result, &body_id)
    );
    assert!(result.is_activatable(), "{result:#?}");
}

/// A member that WAS a plain leaf becoming a GROUP over populated data fails closed: the new
/// group shape would orphan the old single-cell bytes, so it is steered to a transform.
#[test]
fn leaf_member_becoming_a_group_over_populated_data_fails_closed() {
    assert_leaf_reshape_fails_closed(
        "discharge-leaf-to-group",
        "\x20   value\n\x20       required first: string\n",
        RepairReason::TypeChangeRequiresTransform,
    );
}

/// A member that WAS a plain leaf becoming a KEYED LAYER over populated data fails closed: the
/// new keyed shape addresses entries by a key the old bytes were never written at, so it is
/// steered to a transform.
#[test]
fn leaf_member_becoming_a_keyed_layer_over_populated_data_fails_closed() {
    assert_leaf_reshape_fails_closed(
        "discharge-leaf-to-keyed",
        "\x20   value(version: int)\n\x20       required body: string\n",
        RepairReason::TypeChangeRequiresTransform,
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
    let program = checked(&root);
    let place = root_place(&program, "policies");
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

    let body_id = nested_member_catalog_id(&place, "versions", "body");
    assert_eq!(
        body_id, body_stable,
        "a retyped keyed-nested leaf keeps its accepted stable id"
    );
    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The entry's `body` cell was written as `int` `1`, a byte the new `bool` decoder accepts.
    seed.record(1);
    seed.keyed_member(1, "versions", SavedKey::Int(7), "body", Scalar::Int(1));

    let body_id = nested_member_catalog_id(&place, "versions", "body");
    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
    let program = checked(&root);
    let place = root_place(&program, "policies");
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

    let body_id = nested_member_catalog_id(&place, "versions", "body");
    let (result, diagnostics) = preview(&program, &store).expect("preview");

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
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
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
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
                group_entry("policies::Policy::versions", &versions_id),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The old record carries `versions.body` as an unkeyed-group sub-member cell.
    seed.record(1);
    seed.nested_member(1, "versions", "body", Scalar::Str("draft".into()));

    let layer_id = group_member_catalog_id(&place, "versions");
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    assert_fails_closed(
        &result,
        &diagnostics,
        &layer_id,
        RepairReason::KeyedLayerKeyShapeChange,
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
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
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
    assert_fails_closed(
        &result,
        &diagnostics,
        &group_id,
        RepairReason::KeyedLayerKeyShapeChange,
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
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The record exists (a sibling `marker` cell) but its old `value` leaf cell was never
    // populated, so the disappeared-leaf probe sees nothing; the new required `value.first`
    // sub-member is missing and must fail closed.
    seed.record(1);
    seed.member_by_id(1, &hex_id(5), Scalar::Str("seen".into()));

    // `value.first` is a brand-new required sub-member of the new group; its identity lives
    // only in the proposal, so the descend reaches it by its proposal-minted id.
    let first_id = new_member_proposal_id(&program, "books::Book::value::first");
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
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
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
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![member_entry(
                "policies::Policy::tags",
                &tags_id,
                "[int]string",
            )],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.keyed_leaf(
        1,
        "labels",
        SavedKey::Int(7),
        encode_value(&Scalar::Str("draft".into())).unwrap(),
    );

    let layer_id = keyed_leaf_catalog_id(&place, "labels");
    let (result, _diagnostics) = preview(&program, &store).expect("preview");

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
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
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
    let seed = Seed::new(&store, &place);
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
        let accepted = accepted_catalog(
            4,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "policies");
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

    let layer_id = group_member_catalog_id(&place, "versions");
    let (result, _diagnostics) = preview(&program, &store).expect("preview");

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

/// A keyed layer nested BELOW another keyed layer, re-keyed by KEY TYPE over
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
        let accepted = accepted_catalog(
            5,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
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
    let seed = Seed::new(&store, &place);
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

/// A keyed-layer ARITY change two levels deep fails closed. The accepted
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
        let accepted = accepted_catalog(
            5,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
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
    let seed = Seed::new(&store, &place);
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

/// A structurally-diverged INTERIOR member arbitrarily deep fails closed. A
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
        let accepted = accepted_catalog(
            6,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
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
    let seed = Seed::new(&store, &place);
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
        let accepted = accepted_catalog(
            5,
            "policies::Policy",
            "policies::^policies",
            Some("int"),
            vec![
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
    let seed = Seed::new(&store, &place);
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
