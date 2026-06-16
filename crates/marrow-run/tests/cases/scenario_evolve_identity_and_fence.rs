//! Tier-2 scenarios over the production evolve/apply/run pipeline for two durable
//! contracts that a multi-step language-database workflow leans on:
//!
//! - adding a sparse `Id(^store)` reference field to a resource that already holds
//!   saved records, evolving it, and proving the old records stay intact and the new
//!   field reads as absent until written;
//! - the activation fence after an `evolve apply` advances the store epoch: a real
//!   pre-evolution program still pinned to its accepted epoch is locked out of a
//!   write-capable open with the typed `run.store_evolved` fence before any write.
//!
//! Both run through the real check/commit/preview/apply path and assert typed
//! oracles: direct store reads of stored member values and typed `FenceError`
//! codes — never rendered prose.
use crate::evolution_apply_support;
use evolution_apply_support::{
    Seed, checked, commit_then_check, member_catalog_id, proposal_catalog_id, read_scalar,
    root_place, store_id_of, witness,
};

use marrow_run::evolution::{FenceError, apply, fence};
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

use std::path::Path;

/// The pre-evolution schema: a `Book` keyed store plus a separate `Author` store, with
/// no reference between them yet. Records seeded under this schema carry only `title`.
const REFERENCE_BASELINE: &str = "module lib\n\
     resource Author\n\
     \x20   name: string\n\
     store ^authors(id: int): Author\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     pub fn noop()\n\
     \x20   print(\"noop\")\n";

/// The evolved schema: `Book` gains a sparse `Id(^authors)` reference field. Adding a
/// sparse field is a non-defaulting evolution, so existing records need no backfill.
const REFERENCE_EVOLVED: &str = "module lib\n\
     resource Author\n\
     \x20   name: string\n\
     store ^authors(id: int): Author\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   authorId: Id(^authors)\n\
     store ^books(id: int): Book\n\
     pub fn noop()\n\
     \x20   print(\"noop\")\n";

fn write_source(root: &Path, source: &str) {
    evolution_apply_support::write(root, "src/lib.mw", source);
}

#[test]
fn adding_a_sparse_identity_field_by_evolution_preserves_old_records_and_admits_the_reference()
-> Result<(), Box<dyn std::error::Error>> {
    // A populated `Book` store evolves to gain a sparse `Id(^authors)` reference. The
    // evolution backfills nothing (a sparse add carries no data obligation), the old
    // record keeps its `title`, and the new reference field is absent on it — the sparse
    // contract, not a zero identity.
    let root = evolution_apply_support::temp_project("evolve-identity-sparse", |root| {
        write_source(root, REFERENCE_BASELINE);
    });

    // Commit the baseline schema and seed a book plus an author under it, exactly as the
    // runtime write path would: whole-record presence keyed by id, then member cells.
    let baseline = commit_then_check(&root).expect("committed fixture");
    let books = root_place(&baseline, "books")?;
    let authors = root_place(&baseline, "authors")?;
    let store = TreeStore::memory();
    let books_seed = Seed {
        store: &store,
        place: &books,
    };
    books_seed.record(1);
    books_seed.member(1, "title", Scalar::Str("Mort".into()));
    let authors_seed = Seed {
        store: &store,
        place: &authors,
    };
    authors_seed.record(1);
    authors_seed.member(1, "name", Scalar::Str("Ada".into()));

    // Evolve: add the sparse reference field, then discharge it through the production
    // preview/apply path against the live store.
    write_source(&root, REFERENCE_EVOLVED);
    let evolved = checked(&root).expect("checked fixture");
    let outcome = apply(&witness(&evolved, &store), &evolved, &store, false, None)
        .expect("apply sparse identity-field evolution");
    assert_eq!(
        outcome.receipt.records_backfilled, 0,
        "a sparse identity-field add backfills nothing"
    );

    // The old record is untouched: `title` is still readable under its bound member id.
    // `title` predates the evolution, so its id is bound in the accepted place; `authorId`
    // was minted in this proposal, so its store id comes from the proposal apply consumed.
    let books_evolved = root_place(&evolved, "books")?;
    let store_id = store_id_of(&books_evolved)?;
    let title_id = member_catalog_id(&books_evolved, "title")?;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &title_id, ScalarType::Str),
        Some(Scalar::Str("Mort".into())),
        "the pre-evolution record keeps its title across the evolution",
    );
    // The new reference field reads as absent (the sparse contract), not a zero identity.
    let author_ref_id = proposal_catalog_id(&evolved, "lib::Book::authorId")?;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &author_ref_id, ScalarType::Str),
        None,
        "the freshly added sparse reference field is absent until written",
    );

    Ok(())
}

#[test]
fn an_evolve_apply_advances_the_epoch_and_fences_the_pre_evolution_program_before_any_write()
-> Result<(), Box<dyn std::error::Error>> {
    // A real write-then-evolve sequence: a program is committed and its store seeded,
    // then a sparse-field evolution applies and advances the store epoch. The original
    // program — still pinned to its pre-evolution accepted epoch, a genuine stale
    // binding rather than a synthetic epoch — is fenced out of a write-capable open with
    // the typed `run.store_evolved` code before any write reaches the store.
    let root = evolution_apply_support::temp_project("evolve-fence-drift", |root| {
        write_source(root, REFERENCE_BASELINE);
    });
    let baseline = commit_then_check(&root).expect("committed fixture");
    let baseline_epoch = baseline
        .catalog
        .accepted_epoch
        .expect("baseline accepted epoch");
    let books = root_place(&baseline, "books")?;
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &books,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Mort".into()));

    // Evolve and apply against a fresh re-check of the evolved source. Apply advances the
    // store to the proposal epoch and re-stamps the durable shape under it.
    write_source(&root, REFERENCE_EVOLVED);
    let evolved = checked(&root).expect("checked fixture");
    let outcome = apply(&witness(&evolved, &store), &evolved, &store, false, None)
        .expect("apply advances the store epoch");
    assert_eq!(
        outcome.receipt.catalog_epoch,
        baseline_epoch + 1,
        "apply advances the store one epoch past the baseline",
    );

    // The original program, still pinned to the pre-evolution epoch, is fenced before any
    // write: the store moved past it. This is the documented `run.store_evolved` lockout.
    let error = fence(Some(baseline_epoch), &baseline.source_digest(), &store)
        .expect_err("the pre-evolution program is fenced out of the evolved store");
    assert_eq!(
        error,
        FenceError::StoreEvolved {
            stored: baseline_epoch + 1,
            accepted: baseline_epoch,
        }
    );
    assert_eq!(error.code(), "run.store_evolved");

    // The evolved program — pinned to the epoch apply stamped — passes the same fence, so
    // the lockout is precisely the stale binding and not a blanket post-apply refusal.
    fence(Some(baseline_epoch + 1), &evolved.source_digest(), &store)
        .expect("the evolved program is not fenced by the store it just advanced");

    Ok(())
}
