//! Tier-2 end-to-end guard over the `marrow` binary: a fresh-checkout `evolve apply` of a pending
//! evolution must reach the SAME accepted epoch and catalog identity a present store reaches. A
//! fresh checkout seeds an empty store from the committed `marrow.lock`; if the drifted proposal is
//! frozen AT the lock's epoch high-water, the pending change is silently folded into the committed
//! epoch and the epoch never advances, diverging from the present-store path on identical committed
//! inputs. The drifted proposal must instead advance one epoch past the lock high-water — the same
//! epoch a present store discharges the change to — so the seed freezes the identical accepted
//! identity at the advanced epoch. The store is empty, so no data migrates through the intermediate
//! epoch either way.
//!
//! Oracles are typed: the decoded store commit epoch and the accepted-catalog digest, never a
//! substring of human-rendered prose.
use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::support;
use crate::support_evolve;
use support::{TempProject, marrow_bounded, temp_project_uncommitted, write};
use support_evolve::{accepted_catalog, commit_catalog, native_books_project, store_epoch};

const APPLY_DEADLINE: Duration = Duration::from_secs(20);

// A committed epoch-1 shape: a `title` and a sparse `subtitle: string`. The empty store baselines at
// epoch 1 and the committed lock projects the same shape.
const BASELINE_SOURCE: &str = "module books\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     store ^books(id: int): Book\n\
     pub fn add(title: string): Id(^books)\n\
     \x20   return nextId(^books)\n";

// The pending evolution: the sparse `subtitle` leaf changes type over an empty store. It keeps its
// committed identity, so a present store and a fresh checkout must converge on the same catalog.
const RETYPED_SOURCE: &str = "module books\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: int\n\
     store ^books(id: int): Book\n\
     pub fn add(title: string): Id(^books)\n\
     \x20   return nextId(^books)\n";

// A purely additive pending evolution: a new sparse field. It discharges against any store.
const ADDED_FIELD_SOURCE: &str = "module books\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     \x20   pages: int\n\
     store ^books(id: int): Book\n\
     pub fn add(title: string): Id(^books)\n\
     \x20   return nextId(^books)\n";

/// Commit an epoch-1 project carrying `BASELINE_SOURCE`, then advance its source to `evolved` so a
/// pending evolution is outstanding against the committed epoch-1 lock. Returns the project whose
/// store is still present at epoch 1 (the present-store arm).
fn present_store_with_pending(name: &str, evolved: &str) -> TempProject {
    let root = native_books_project(name, BASELINE_SOURCE);
    let _ = commit_catalog(&root);
    assert_eq!(store_epoch(&root), Some(1), "baseline commits at epoch 1");
    write(&root, "src/books.mw", evolved);
    root
}

/// A fresh checkout of `present`: the identical committed source and `marrow.lock`, with no store
/// body on disk. The next write-capable open seeds an empty store from the committed identity.
fn fresh_checkout_of(name: &str, present: &Path) -> TempProject {
    let checkout = temp_project_uncommitted(name, |_| {});
    for relative in ["marrow.json", "marrow.lock", "src/books.mw"] {
        let contents = fs::read_to_string(present.join(relative))
            .unwrap_or_else(|error| panic!("read {relative}: {error}"));
        write(&checkout, relative, &contents);
    }
    assert!(
        !checkout.join(".data").exists(),
        "a fresh checkout carries no store body"
    );
    checkout
}

fn apply(dir: &Path) -> std::process::Output {
    marrow_bounded(
        &[
            "evolve",
            "apply",
            "--format",
            "json",
            dir.to_str().expect("utf8"),
        ],
        APPLY_DEADLINE,
    )
}

/// The load-bearing invariant: from identical committed inputs, a present-store apply and a
/// fresh-checkout apply of a pending (leaf-retype) evolution reach the SAME epoch and catalog
/// identity. The fresh checkout must NOT under-advance by folding the delta into the seed baseline.
#[test]
fn fresh_checkout_apply_reaches_the_same_epoch_and_identity_as_the_present_store() {
    let present = present_store_with_pending("fresh-seed-present-retype", RETYPED_SOURCE);
    let fresh = fresh_checkout_of("fresh-seed-fresh-retype", &present);

    let present_apply = apply(&present);
    assert_eq!(
        present_apply.status.code(),
        Some(0),
        "present-store apply succeeds: {present_apply:?}"
    );
    let fresh_apply = apply(&fresh);
    assert_eq!(
        fresh_apply.status.code(),
        Some(0),
        "fresh-checkout apply succeeds: {fresh_apply:?}"
    );

    assert_eq!(
        store_epoch(&present),
        Some(2),
        "the present store discharges the pending evolution to epoch 2"
    );
    assert_eq!(
        store_epoch(&fresh),
        Some(2),
        "the fresh checkout discharges to epoch 2 too, not folded into the seed at epoch 1"
    );

    let present_catalog = accepted_catalog(&present);
    let fresh_catalog = accepted_catalog(&fresh);
    assert_eq!(
        fresh_catalog.digest, present_catalog.digest,
        "the fresh checkout reaches the identical accepted-catalog identity"
    );
    assert_eq!(
        fresh_catalog.entries, present_catalog.entries,
        "every accepted entry — identity, aliases, lifecycle, and shape — matches the present store"
    );
}

/// A second apply on each store is a no-op at the advanced epoch: the evolution is recognized as
/// already applied, so neither the present store nor the (now populated) fresh checkout re-fires it.
#[test]
fn a_second_apply_is_a_no_op_at_the_advanced_epoch() {
    let present = present_store_with_pending("fresh-seed-present-second", RETYPED_SOURCE);
    let fresh = fresh_checkout_of("fresh-seed-fresh-second", &present);

    assert_eq!(apply(&present).status.code(), Some(0));
    assert_eq!(apply(&fresh).status.code(), Some(0));
    assert_eq!(store_epoch(&present), Some(2));
    assert_eq!(store_epoch(&fresh), Some(2));

    let present_again = apply(&present);
    let fresh_again = apply(&fresh);
    assert_eq!(
        present_again.status.code(),
        Some(0),
        "a repeat present-store apply is a no-op: {present_again:?}"
    );
    assert_eq!(
        fresh_again.status.code(),
        Some(0),
        "a repeat fresh-checkout apply is a no-op: {fresh_again:?}"
    );
    assert_eq!(
        store_epoch(&present),
        Some(2),
        "the second present-store apply does not advance the epoch"
    );
    assert_eq!(
        store_epoch(&fresh),
        Some(2),
        "the second fresh-checkout apply does not advance the epoch"
    );
}

/// A purely additive pending evolution (a new sparse field) still discharges on a fresh checkout,
/// reaching the same advanced epoch as the present store. The fix that stops folding a drifted shape
/// must not stop an additive change from auto-adopting.
#[test]
fn an_additive_fresh_checkout_still_discharges_to_the_advanced_epoch() {
    let present = present_store_with_pending("fresh-seed-present-additive", ADDED_FIELD_SOURCE);
    let fresh = fresh_checkout_of("fresh-seed-fresh-additive", &present);

    assert_eq!(apply(&present).status.code(), Some(0));
    assert_eq!(apply(&fresh).status.code(), Some(0));
    assert_eq!(
        store_epoch(&present),
        Some(2),
        "the present store discharges the additive change to epoch 2"
    );
    assert_eq!(
        store_epoch(&fresh),
        Some(2),
        "the fresh checkout discharges the additive change to epoch 2 too"
    );
}

/// A fresh checkout with NO pending evolution — source matching the committed lock exactly — stays
/// at the committed epoch. The drift-seeding fix must not spuriously advance a clean adoption.
#[test]
fn a_clean_fresh_checkout_stays_at_the_committed_epoch() {
    let root = native_books_project("fresh-seed-clean-present", BASELINE_SOURCE);
    let _ = commit_catalog(&root);
    assert_eq!(store_epoch(&root), Some(1));
    let fresh = fresh_checkout_of("fresh-seed-clean-fresh", &root);

    let fresh_apply = apply(&fresh);
    assert_eq!(
        fresh_apply.status.code(),
        Some(0),
        "a clean fresh-checkout apply succeeds: {fresh_apply:?}"
    );
    assert_eq!(
        store_epoch(&fresh),
        Some(1),
        "a clean adoption seeds at the committed epoch and does not advance"
    );
}
