use crate::support;
use crate::support_discharge;
use marrow_catalog::CatalogEntryKind;
use marrow_check::evolution::{RepairDiagnostic, RepairReason, Verdict, preview};
use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SUPPORTED_DATE_MIN_DAYS, Scalar};

use support::catalog::write_catalog;
use support::{temp_project, write};
use support_discharge::*;

/// A required leaf inside an unkeyed group is required for the containing resource.
/// An old record that lacks `name.last` must discharge to a fail-closed repair, and
/// the nested leaf's catalog id must appear among the affected ids so apply
/// re-verifies it.
#[test]
fn required_nested_group_leaf_missing_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-nested-required", |root| {
        write(
            root,
            "src/people.mw",
            "module people\n\
             resource Person\n\
             \x20   name\n\
             \x20       required first: string\n\
             \x20       required last: string\n\
             store ^people(id: int): Person\n\
             pub fn add(): Id(^people)\n\
             \x20   return nextId(^people)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "people")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The old record carries `name.first` but predates required `name.last`.
    seed.record(1);
    seed.nested_member(1, "name", "first", Scalar::Str("Ada".into()));

    let result = witness(&program, &store);
    let last_id = nested_member_catalog_id(&place, "name", "last")?;

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

    Ok(())
}

/// A required leaf inside a keyed layer is required for each entry that exists. An
/// old keyed entry that lacks a newly-required leaf must discharge to a blocking
/// verdict, never an empty pass: the witness alone must be non-activatable, and the
/// keyed leaf's catalog id must appear among the affected ids so apply re-verifies it.
#[test]
fn required_keyed_layer_leaf_missing_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-keyed-required", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   status: string\n\
             \x20   versions(version: int)\n\
             \x20       note: string\n\
             \x20       required body: string\n\
             store ^policies(id: int): Policy\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "policies")?;
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

    let body_id = nested_member_catalog_id(&place, "versions", "body")?;
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

    Ok(())
}

#[test]
fn malformed_temporal_keyed_layer_entry_faults_discharge() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_project("discharge-keyed-malformed-date", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   versions(day: date)\n\
             \x20       required body: string\n\
             store ^policies(id: int): Policy\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "policies")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.keyed_member(
        1,
        "versions",
        SavedKey::Date(SUPPORTED_DATE_MIN_DAYS - 1),
        "body",
        Scalar::Str("bad".into()),
    );

    let err = preview(&program, &store).expect_err("malformed date key must fault discharge");

    assert!(matches!(err, StoreError::Corruption { .. }), "{err:?}");

    Ok(())
}

/// A keyed layer whose every existing entry already carries its required leaf
/// discharges to a proof, not a block: the per-entry scan must not fail open in
/// either direction.
#[test]
fn keyed_layer_leaf_present_in_every_entry_proves() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-keyed-present", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   status: string\n\
             \x20   versions(version: int)\n\
             \x20       required body: string\n\
             store ^policies(id: int): Policy\n\
             pub fn add(): Id(^policies)\n\
             \x20   return nextId(^policies)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "policies")?;
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

    let body_id = nested_member_catalog_id(&place, "versions", "body")?;
    let result = witness(&program, &store);

    assert!(result.is_activatable(), "{:#?}", result.verdicts);
    assert!(
        matches!(verdict_for(&result, &body_id), Verdict::DataProof),
        "{:#?}",
        result.verdicts
    );

    Ok(())
}

/// A brand-new required scalar member added over a populated store with no `evolve default`
/// and no transform fails closed: the existing records lack it, and there is nothing to
/// backfill them with, so the add-required-field obligation is unmet. The new member has no
/// accepted catalog id yet, so the presence scan must be proposal-aware to reach it at all.
#[test]
fn brand_new_required_member_over_populated_store_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let title_stable = hex_id(3);
    let root = temp_project("discharge-new-required-no-default", |root| {
        // `pages` is brand-new in source and not in the accepted catalog.
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             store ^books(id: int): Book\n\
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
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Old records carry `title` but predate the brand-new required `pages`.
    seed.record(1);
    seed.member_by_id(1, &title_stable, Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member_by_id(2, &title_stable, Scalar::Str("Hyperion".into()));

    let pages_id = new_member_proposal_id(&program, "books::Book::pages")?;
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

    Ok(())
}

/// A brand-new required member added WITH an `evolve default` over a populated store is the
/// Default backfill obligation: the default fills every old record, so it stays activatable.
/// This is the add-required-field-with-default path the proposal-aware scan must still reach
/// for a not-yet-accepted member, not only for an already-accepted one.
#[test]
fn brand_new_required_member_with_default_backfills() -> Result<(), Box<dyn std::error::Error>> {
    let title_stable = hex_id(3);
    let root = temp_project("discharge-new-required-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             store ^books(id: int): Book\n\
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
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_by_id(1, &title_stable, Scalar::Str("Dune".into()));

    let pages_id = new_member_proposal_id(&program, "books::Book::pages")?;
    let result = witness(&program, &store);

    match verdict_for(&result, &pages_id) {
        Verdict::Default { value } => {
            assert_eq!(value.scalar_type, marrow_store::value::ScalarType::Int);
            assert_eq!(
                value.encoded,
                marrow_store::value::encode_value(&Scalar::Int(0))?
            );
        }
        other => panic!("expected default for the brand-new required member, got {other:#?}"),
    }
    assert!(result.is_activatable(), "{result:#?}");

    Ok(())
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
             resource Book\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             store ^books(id: int): Book\n\
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
    let program = checked(&root).expect("checked fixture");
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
fn brand_new_required_keyed_leaf_over_populated_layer_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-new-keyed-required-no-default", |root| {
        // `body` is brand-new required inside the existing `versions` keyed layer; the
        // accepted catalog carries the layer and a sibling `note`, but not `body`.
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       note: string\n\
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
                member_entry("policies::Policy::versions::note", &hex_id(4), "string"),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "policies")?;
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

    let body_id = new_member_proposal_id(&program, "policies::Policy::versions::body")?;
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

    Ok(())
}

/// A brand-new required leaf added inside an existing keyed layer WITH an `evolve default`
/// backfills every existing entry, staying activatable: the keyed proposal-aware path must
/// reach the Default obligation for a not-yet-accepted keyed leaf the same way the unkeyed
/// path does.
#[test]
fn brand_new_required_keyed_leaf_with_default_backfills() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_project("discharge-new-keyed-required-default", |root| {
        write(
            root,
            "src/policies.mw",
            "module policies\n\
             resource Policy\n\
             \x20   versions(version: int)\n\
             \x20       note: string\n\
             \x20       required body: string\n\
             store ^policies(id: int): Policy\n\
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
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "policies")?;
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

    let body_id = new_member_proposal_id(&program, "policies::Policy::versions::body")?;
    let result = witness(&program, &store);

    assert!(
        matches!(verdict_for(&result, &body_id), Verdict::Default { .. }),
        "a brand-new required keyed leaf with a default must backfill, got {:#?}",
        verdict_for(&result, &body_id)
    );
    assert!(result.is_activatable(), "{result:#?}");

    Ok(())
}
