use crate::support;
use crate::support_discharge;
use marrow_check::evolution::{RepairDiagnostic, RepairReason, Verdict, preview};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

use support::catalog::write_catalog;
use support::{temp_project, write};
use support_discharge::*;

/// A NESTED keyed-layer member whose key TYPE changes over populated entries fails closed.
/// The accepted catalog records `versions` as a keyed group keyed by `version: int`; source
/// re-keys it `version: string`. Each existing entry is addressed by the old `int` key bytes,
/// which sit in the data path itself, so the new `string` shape addresses no existing entry —
/// the same orphaning hazard a store identity-key change has, one level down. v0.1 cannot
/// migrate a keyed-layer key shape, so the layer member fails closed rather than activating
/// over entries the new key shape cannot reach.
#[test]
fn keyed_layer_key_type_change_over_populated_entries_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let root = temp_project("discharge-keyed-layer-keytype", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   versions(version: string)\n\
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
    let place = root_place(&program, "policies")?;
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

    let layer_id = group_member_catalog_id(&place, "versions")?;
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

    Ok(())
}

/// A plain unkeyed GROUP reshaped into a KEYED LAYER over populated data fails closed. The
/// accepted catalog records `versions` as an unkeyed group (no key params); source now keys it
/// `versions(version: int)`. The old group's sub-member cells sit directly under the group node
/// with no entry key, so the new keyed shape — which addresses every value under an entry key —
/// reads none of them. The reshape is a structural divergence the snapshot cannot satisfy, so
/// the layer member fails closed.
#[test]
fn plain_group_reshaped_to_keyed_layer_over_populated_data_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let root = temp_project("discharge-group-to-keyed", |root| {
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
                group_entry("policies::Policy::versions", &versions_id),
                member_entry("policies::Policy::versions::body", &body_id, "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "policies")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The old record carries `versions.body` as an unkeyed-group sub-member cell.
    seed.record(1);
    seed.nested_member(1, "versions", "body", Scalar::Str("draft".into()));

    let layer_id = group_member_catalog_id(&place, "versions")?;
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    assert_fails_closed(
        &result,
        &diagnostics,
        &layer_id,
        RepairReason::KeyedLayerKeyShapeChange,
    );

    Ok(())
}

/// A KEYED LAYER reshaped into a plain unkeyed GROUP over populated data fails closed — the
/// inverse reshape. The accepted catalog records `versions` keyed by `version: int`; source
/// drops the key, making it a plain group. Every existing entry sits under an entry key the
/// plain group shape never reads, so the reshape is a structural divergence that fails closed.
#[test]
fn keyed_layer_reshaped_to_plain_group_over_populated_data_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let root = temp_project("discharge-keyed-to-group", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   versions\n\
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
    let place = root_place(&program, "policies")?;
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

    let group_id = group_member_catalog_id(&place, "versions")?;
    let (result, diagnostics) = preview(&program, &store).expect("preview");
    assert_fails_closed(
        &result,
        &diagnostics,
        &group_id,
        RepairReason::KeyedLayerKeyShapeChange,
    );

    Ok(())
}

/// The default-deny backstop catches a structural transition no targeted classifier addresses.
/// Here a member moves from a keyed group keyed by `version: int` to a keyed group keyed by the
/// SAME `version: int` but with an added key column `lang: string` — a keyed-layer arity change.
/// Each existing entry is addressed by a one-column key the two-column shape cannot read. No
/// leaf-token classifier fires (both shapes are non-leaf groups), so the structural signature
/// backstop is what fails it closed.
#[test]
fn keyed_layer_arity_change_fails_closed_via_backstop() -> Result<(), Box<dyn std::error::Error>> {
    let versions_id = hex_id(3);
    let body_id = hex_id(4);
    let root = temp_project("discharge-keyed-arity", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   versions(version: int, lang: string)\n\
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
    let place = root_place(&program, "policies")?;
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

    let layer_id = group_member_catalog_id(&place, "versions")?;
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

    Ok(())
}
