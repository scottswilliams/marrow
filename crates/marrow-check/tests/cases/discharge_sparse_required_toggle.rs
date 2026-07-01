//! The sparse↔required evolution toggle keys on the `Slot.required` flag, never on a
//! `?` type token. Both directions share one accepted catalog whose `subtitle` leaf token
//! is a bare `string` with no requiredness marker: the accepted snapshot cannot tell the
//! two directions apart, so only the source `required` flag drives the verdict. Flipping
//! `subtitle` to required over a populated store that lacks it fences exactly as adding a
//! required field does; clearing it back to sparse auto-applies and rewrites no record.

use crate::support;
use crate::support_discharge;
use marrow_catalog::CatalogMetadata;
use marrow_check::CheckedSavedPlace;
use marrow_check::evolution::{RepairReason, Verdict, preview};
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

use support::catalog::write_catalog;
use support::{temp_project, write};
use support_discharge::*;

// Stable ids the shared accepted catalog binds; the store seeds and verdict lookups
// address members by these.
fn title_id() -> String {
    hex_id(3)
}

fn subtitle_id() -> String {
    hex_id(4)
}

/// The one accepted catalog both toggle directions share. It records `subtitle` as a plain
/// `string` leaf: the leaf token never carries `required`, so sparse and required accept
/// byte-identically and the accepted snapshot is the same regardless of direction.
fn accepted() -> CatalogMetadata {
    accepted_catalog(
        4,
        "books::Book",
        "books::^books",
        Some("int"),
        vec![
            member_entry("books::Book::title", &title_id(), "string"),
            member_entry("books::Book::subtitle", &subtitle_id(), "string"),
        ],
    )
}

/// The fixture source with `subtitle` declared as `subtitle_decl`. `?` never appears on a
/// field declaration; the two directions differ by the `required` keyword alone.
fn book_source(subtitle_decl: &str) -> String {
    format!(
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         {subtitle_decl}\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n"
    )
}

/// Two records that carry `title` but predate `subtitle` being populated, so the store is
/// populated and every record lacks the toggled field.
fn seed_records_lacking_subtitle(store: &TreeStore, place: &CheckedSavedPlace) {
    let seed = Seed::new(store, place);
    seed.record(1);
    seed.member_by_id(1, &title_id(), Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member_by_id(2, &title_id(), Scalar::Str("Hyperion".into()));
}

/// sparse `subtitle: string` → `required subtitle: string` over a populated store that lacks
/// `subtitle` fences: the flip is discharged exactly as adding a required field, so the scan
/// finds the records lacking it and fails closed with `MissingRequiredMember`.
#[test]
fn sparse_to_required_fences_populated_store() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("evolution-sparse-to-required", |root| {
        write(
            root,
            "src/books.mw",
            &book_source("\x20   required subtitle: string\n"),
        );
        write_catalog(root, &accepted());
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    assert_eq!(
        member_catalog_id(&place, "subtitle")?,
        subtitle_id(),
        "the toggled member keeps its accepted stable id across the flip"
    );

    let store = TreeStore::memory();
    seed_records_lacking_subtitle(&store, &place);

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &subtitle_id(),
        RepairReason::MissingRequiredMember,
    );
    assert!(
        result.counts.scanned_records >= 2,
        "the fence is against a populated store: {:#?}",
        result.counts
    );

    Ok(())
}

/// `required subtitle: string` → sparse `subtitle: string` over the same populated store
/// auto-applies with zero mutation: only the `required` flag is cleared (the accepted leaf
/// token is unchanged), so `subtitle` is a `NoOp` and no record is backfilled, transformed,
/// or re-addressed.
#[test]
fn required_to_sparse_auto_applies_zero_mutation() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("evolution-required-to-sparse", |root| {
        write(
            root,
            "src/books.mw",
            &book_source("\x20   subtitle: string\n"),
        );
        write_catalog(root, &accepted());
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;

    let store = TreeStore::memory();
    seed_records_lacking_subtitle(&store, &place);

    let result = witness(&program, &store);

    assert!(
        matches!(verdict_for(&result, &subtitle_id()), Verdict::NoOp),
        "{:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{:#?}", result.verdicts);
    assert_eq!(
        (
            result.counts.records_to_backfill,
            result.counts.records_to_transform,
            result.counts.records_to_readdress,
            result.counts.records_lacking_member,
        ),
        (0, 0, 0, 0),
        "the flip to sparse mutates zero records: {:#?}",
        result.counts
    );

    Ok(())
}
