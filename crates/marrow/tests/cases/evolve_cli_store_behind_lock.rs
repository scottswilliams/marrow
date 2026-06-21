//! Tier-2 end-to-end guard over the `marrow` binary: an `evolve apply` whose local store is
//! BEHIND an ahead committed `marrow.lock` must never regress that lock. The committed lock's
//! epoch high-water and its append-only retired-id ledger are monotonic by contract, so a stale
//! local checkout cannot rewind the high-water a teammate already committed or erase a committed
//! tombstone — which would let a later checkout reissue a retired id to a different entity.
//!
//! Oracles are typed: the decoded `CatalogLock` epoch high-water and ledger tombstone ids before
//! and after the apply, never a substring of human-rendered prose.
use std::fs;

use crate::support;
use crate::support_evolve;
use marrow_catalog::{CatalogLock, LockLedgerTombstone};
use support::{marrow, write};
use support_evolve::{REQUIRED_BASELINE_SOURCE, commit_catalog, native_books_project, store_epoch};

/// The retired-id tombstone an ahead committed lock carries: a reserved id at a retired
/// `(kind, path)`, recorded at the lock's high-water. A fresh checkout seeded from the lock
/// reserves this id so it is never reissued.
fn ahead_tombstone(high_water: u64) -> LockLedgerTombstone {
    LockLedgerTombstone {
        kind: marrow_catalog::CatalogEntryKind::Resource,
        path: "books::Retired".to_string(),
        id: format!("cat_{:032x}", 0xdead_u64),
        lifecycle: marrow_catalog::CatalogLifecycle::Reserved,
        high_water,
    }
}

#[test]
fn evolve_apply_over_a_store_behind_an_ahead_lock_never_regresses_the_lock() {
    // A committed project at epoch 1: the store and a real epoch-1 committed lock exist.
    let root = native_books_project(
        "evolve-apply-store-behind-ahead-lock",
        REQUIRED_BASELINE_SOURCE,
    );
    let program = commit_catalog(&root);
    let dir = root.to_str().expect("project path utf-8");
    assert_eq!(store_epoch(&root), Some(1));

    let real_lock_bytes =
        fs::read_to_string(root.join("marrow.lock")).expect("read committed epoch-1 lock");
    let real_lock = CatalogLock::from_lock_json(&real_lock_bytes).expect("parse committed lock");

    // A teammate has since advanced and committed the lock far past the local store: high-water 9
    // with a retired-id tombstone. The local store is left BEHIND at epoch 1. This is the exact
    // desynced state Finding A reproduces — the store snapshot alone would rewind the lock.
    let ahead_lock = CatalogLock::new(
        real_lock.entries.clone(),
        vec![ahead_tombstone(9)],
        9,
        real_lock.source_digest.clone(),
    )
    .expect("ahead lock builds");
    write(
        &root,
        "marrow.lock",
        &ahead_lock
            .to_lock_json_pretty()
            .expect("ahead lock renders"),
    );

    // Evolve the source so apply has real activation work to do over the behind store.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    let lock_before_apply =
        fs::read_to_string(root.join("marrow.lock")).expect("read ahead lock before apply");

    // The decided semantics: a desynced apply (a store behind a committed lock whose high-water
    // this single activation cannot reach) FAILS CLOSED. This local apply would land the store at
    // epoch 2, far below the committed high-water of 9, so it cannot reconstruct the teammate's
    // intermediate activations (including the retire that minted the tombstone). Minting a fresh
    // epoch-2 activation here would collide with the teammate's already-committed epoch-2 identity.
    let apply = marrow(&["evolve", "apply", "--format", "json", dir]);
    assert_eq!(
        apply.status.code(),
        Some(1),
        "a store behind an unreachable committed lock must fail closed, not regress it: {apply:?}"
    );
    let record = support::json(apply.stdout);
    assert_eq!(
        record["code"],
        serde_json::json!("run.store_behind"),
        "the desynced apply surfaces the typed store-behind fence: {record}"
    );
    // The apply-desync remedy must be actionable, not circular: it tells the operator to
    // reconcile the local store with the team's up-to-date store, and must never advise
    // re-running the apply that just refused (the run-path remedy that would mislead here).
    let message = record["message"]
        .as_str()
        .expect("remedy message is a string");
    assert!(
        message.contains("Reconcile the local store") && message.contains("up-to-date store"),
        "the apply-desync remedy points at reconciling the local store, not re-running apply: {message}"
    );
    assert!(
        !message.contains("Run `marrow evolve apply`"),
        "the apply-desync remedy must not be circular by telling the operator to re-run apply: {message}"
    );

    // The catastrophe oracle: the committed lock is byte-identical after the refused apply. Its
    // epoch high-water never regressed below 9 and the retired-id tombstone is intact, so a fresh
    // checkout still reserves the retired id and can never reissue it to a different entity.
    let after_bytes = fs::read_to_string(root.join("marrow.lock")).expect("read lock after apply");
    assert_eq!(
        after_bytes, lock_before_apply,
        "a fail-closed apply must not touch the committed lock"
    );
    let after = CatalogLock::from_lock_json(&after_bytes).expect("lock parses");
    assert!(
        after.epoch_high_water >= 9,
        "the committed epoch high-water must never regress below the ahead lock's 9: {}",
        after.epoch_high_water
    );
    let retired_id = format!("cat_{:032x}", 0xdead_u64);
    assert!(
        after.ledger.iter().any(|stone| stone.id == retired_id),
        "the committed retired-id tombstone must survive, never be erased: {:?}",
        after.ledger
    );
    assert!(
        after
            .entries
            .iter()
            .all(|entry| entry.stable_id != retired_id),
        "a retired id must never be reissued to an active entry"
    );

    let _ = program;
}
