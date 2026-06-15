use crate::support;
use crate::support_discharge;
use marrow_check::evolution::{RepairDiagnostic, RepairReason, Verdict, preview};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, encode_value};

use support::catalog::write_catalog;
use support::{temp_project, write};
use support_discharge::*;

/// NEGATIVE GUARD: the structural backstop must not over-fire. An identity-preserving rename of
/// a keyed layer (same stable id, same key shape, only the source spelling moved) keeps its
/// structural signature unchanged, so it stays activatable — the rename is a catalog-only move,
/// not a structural divergence. A keyed leaf carries the rename cleanly: its stable id and
/// `[int]string` signature are preserved, so the backstop sees no divergence.
#[test]
fn renamed_keyed_layer_with_unchanged_shape_does_not_overfire() {
    let tags_id = hex_id(3);
    let root = temp_project("discharge-keyed-rename", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   labels(pos: int): string\n\
             store ^policies(id: int): Policy\n\
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
    let program = checked(&root).expect("checked fixture");
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
        "a renamed keyed leaf with an unchanged shape is a catalog-only move, got {:#?}",
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
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       required body: string\n\
             \x20       required note: string\n\
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
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::note", &note_id, "string"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
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
             resource Policy\n\
             \x20   tag: string\n\
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
                keyed_group_entry("policies::Policy::versions", &versions_id, "int"),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
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
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       revisions(rev: string)\n\
             \x20           required body: string\n\
             store ^policies(id: int): Policy\n\
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
    let program = checked(&root).expect("checked fixture");
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
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       revisions(rev: int, draft: int)\n\
             \x20           required body: string\n\
             store ^policies(id: int): Policy\n\
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
    let program = checked(&root).expect("checked fixture");
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
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       revisions(rev: int)\n\
             \x20           meta(tag: int)\n\
             \x20               required body: string\n\
             store ^policies(id: int): Policy\n\
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
    let program = checked(&root).expect("checked fixture");
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
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       revisions(rev: int)\n\
             \x20           required body: string\n\
             store ^policies(id: int): Policy\n\
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
    let program = checked(&root).expect("checked fixture");
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
