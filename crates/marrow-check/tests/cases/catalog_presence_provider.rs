//! The checker consumes the accepted catalog as a caller-supplied provider input, threaded
//! through the `analyze_project` parameter. These tests inject the snapshot directly and
//! prove that identity binds against it: the accepted ids carry forward onto live facts, and
//! a source-only check proposes a first epoch while writing nothing.
use crate::support;
use marrow_catalog::{
    CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogLock, CatalogMetadata, LockEntry,
};
use marrow_check::{CHECK_EVOLVE_TARGET, CHECK_LOCK_CORRUPT, ProjectSources, analyze_project};

use support::catalog::{catalog, derived_id, entry_for_label as entry};
use support::{config, temp_root, write};

/// The `books::Book` source one accepted snapshot already carries identity for.
fn books_source(root: &std::path::Path) {
    write(
        root,
        "src/books.mw",
        "module books\nresource Book\n    title: string\nstore ^books(id: int): Book\n",
    );
}

/// The accepted snapshot whose ids the binding must carry forward unchanged.
fn books_accepted() -> CatalogMetadata {
    catalog(vec![
        entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
        entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
        entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::title",
            "member-title",
            &[],
        ),
    ])
}

/// A committed lock projecting `entries` at epoch high-water `high_water`, with no ledger
/// tombstones. Each committed entry records the `(kind, path)` first-run adoption keys on,
/// so a fresh source entity at the same path adopts its committed id regardless of shape.
fn books_lock(entries: Vec<LockEntry>, high_water: u64) -> CatalogLock {
    CatalogLock::new(
        entries,
        Vec::new(),
        high_water,
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
    )
    .expect("lock builds")
}

/// The committed lock for `books_source`, projecting every entity the source declares at its
/// real accepted shape so adoption is exercised against SHAPED entries — a store carrying an
/// `int` key shape and a member carrying a `leaf:string` signature — not only the shapeless
/// resource. Path-keyed adoption must carry each committed id forward even though a freshly
/// built source pre-image records none of these shapes yet.
fn books_committed_lock(high_water: u64) -> CatalogLock {
    let resource = LockEntry::from_catalog_entry(&entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    ));
    let mut store_entry = entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]);
    store_entry.accepted_key_shape = Some("int".to_string());
    let store = LockEntry::from_catalog_entry(&store_entry);
    let mut member_entry = entry(
        CatalogEntryKind::ResourceMember,
        "books::Book::title",
        "member-title",
        &[],
    );
    member_entry.accepted_struct = Some("leaf:string".to_string());
    let member = LockEntry::from_catalog_entry(&member_entry);
    books_lock(vec![resource, store, member], high_water)
}

#[test]
fn source_only_check_proposes_epoch_one_and_writes_nothing() {
    let root = temp_root("provider-source-only");
    books_source(&root);

    let snapshot =
        analyze_project(&root, &config(), &ProjectSources::new(), None, None).expect("analyze");

    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let proposal = snapshot
        .program
        .catalog
        .proposal
        .expect("first-run proposal");
    assert_eq!(proposal.epoch, 1);
    assert_eq!(snapshot.program.catalog.accepted_epoch, None);
    // The checker is read-only: it proposes a baseline but establishes no durable state,
    // so the project directory holds only the source it came in with.
    let entries: Vec<_> = std::fs::read_dir(&*root)
        .expect("read project root")
        .map(|entry| entry.expect("dir entry").file_name())
        .collect();
    assert_eq!(
        entries,
        [std::ffi::OsString::from("src")],
        "a source-only check must not write any durable artifact: {entries:?}"
    );
}

/// The committed id of the proposal entry at `(kind, path)`, or a panic naming the entry the
/// proposal should carry.
fn adopted_id(proposal: &CatalogMetadata, kind: CatalogEntryKind, path: &str) -> String {
    entry_id(&proposal.entries, kind, path)
}

fn entry_id(entries: &[CatalogEntry], kind: CatalogEntryKind, path: &str) -> String {
    entries
        .iter()
        .find(|entry| entry.kind == kind && entry.path == path)
        .unwrap_or_else(|| panic!("catalog carries {kind:?} `{path}`"))
        .stable_id
        .clone()
}

#[test]
fn first_run_with_present_lock_adopts_committed_identity_by_path() {
    // A wiped store with no accepted catalog, but the source tree still carries the committed
    // lock. First-run binding adopts the committed id for every entity by its `(kind, path)` —
    // including the SHAPED store and member — rather than minting fresh ids a shape-only match
    // would risk. The source shape matches every committed entry's fingerprint, so the lock binds
    // cleanly as accepted at its committed high-water, the same epoch a present store at that shape
    // holds; the order-sensitive whole-source digest does not force a spurious advance. This proves
    // the adoption reaches the production pipeline through `analyze_project`, not only the in-module
    // binding test.
    let root = temp_root("provider-lock-adoption");
    books_source(&root);
    let high_water = 12;
    let lock = books_committed_lock(high_water);

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
        .expect("analyze");

    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot.program.catalog.proposal.is_none(),
        "a shape-matching lock binds cleanly as accepted, not a drifted proposal"
    );
    assert_eq!(
        snapshot.program.catalog.accepted_epoch,
        Some(high_water),
        "a clean adoption binds at the committed high-water, never advancing"
    );

    let accepted = &snapshot.program.catalog.accepted_entries;
    assert_eq!(
        entry_id(accepted, CatalogEntryKind::Resource, "books::Book"),
        derived_id("res-book"),
        "the resource adopts the committed lock id"
    );
    assert_eq!(
        entry_id(accepted, CatalogEntryKind::Store, "books::^books"),
        derived_id("store-books"),
        "the SHAPED store adopts its committed lock id by path, not minting fresh"
    );
    assert_eq!(
        entry_id(
            accepted,
            CatalogEntryKind::ResourceMember,
            "books::Book::title"
        ),
        derived_id("member-title"),
        "the SHAPED member adopts its committed lock id by path, not minting fresh"
    );
}

/// Two resources sharing the same shape but distinct paths each adopt their OWN committed id by
/// path: a shape fingerprint cannot disambiguate them, but `(kind, path)` does, so no two entities
/// collide onto one committed identity.
#[test]
fn same_shape_resources_adopt_their_own_committed_ids_by_path() {
    let root = temp_root("provider-lock-same-shape");
    write(
        &root,
        "src/books.mw",
        "module books\nresource Book\n    title: string\nresource Note\n    title: string\n",
    );
    let book = LockEntry::from_catalog_entry(&entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    ));
    let note = LockEntry::from_catalog_entry(&entry(
        CatalogEntryKind::Resource,
        "books::Note",
        "res-note",
        &[],
    ));
    let lock = books_lock(vec![book, note], 5);

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
        .expect("analyze");

    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let proposal = snapshot
        .program
        .catalog
        .proposal
        .expect("first-run proposal");
    let book_id = adopted_id(&proposal, CatalogEntryKind::Resource, "books::Book");
    let note_id = adopted_id(&proposal, CatalogEntryKind::Resource, "books::Note");
    assert_eq!(
        book_id,
        derived_id("res-book"),
        "Book adopts its own committed id"
    );
    assert_eq!(
        note_id,
        derived_id("res-note"),
        "Note adopts its own committed id, not Book's same-shape one"
    );
    assert_ne!(book_id, note_id, "same-shape entities never share an id");
}

/// First-run adoption is deterministic: binding the same source against the same lock twice
/// yields identical ids and epoch, with no OS entropy on the adoption path.
#[test]
fn first_run_lock_adoption_is_deterministic() {
    let lock = books_committed_lock(8);
    let bind_once = || {
        let root = temp_root("provider-lock-determinism");
        books_source(&root);
        let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
            .expect("analyze");
        (
            snapshot.program.catalog.accepted_epoch,
            snapshot.program.catalog.accepted_entries.clone(),
        )
    };

    let (first_epoch, first_entries) = bind_once();
    let (second_epoch, second_entries) = bind_once();
    assert_eq!(first_epoch, second_epoch, "epoch is deterministic");
    assert_eq!(
        first_entries, second_entries,
        "adopted ids and entries are byte-identical across re-binds"
    );
}

#[test]
fn first_run_lock_adoption_refuses_a_tombstoned_committed_id() {
    // A committed entry whose id the lock's own ledger has retired cannot be constructed by the
    // lock codec, so adoption's tombstone-reissue refusal is its independent fail-closed gate.
    // The check-layer rendering is proven directly in the binding pass; here we prove a clean
    // present-lock first run reports no lock corruption, fencing the refusal off from the happy
    // adoption path so a passing test cannot conflate corrupt-lock with absent-lock.
    let root = temp_root("provider-lock-clean");
    books_source(&root);
    let committed_resource = LockEntry::from_catalog_entry(&entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    ));
    let lock = books_lock(vec![committed_resource], 7);

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
        .expect("analyze");

    assert!(
        !snapshot
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_LOCK_CORRUPT),
        "a valid present lock reports no lock corruption: {:#?}",
        snapshot.report.diagnostics
    );
}

/// A fully-shaped accepted snapshot for `books_source`: the store carries its `int` identity-key
/// shape and the member its `leaf:string` structural signature, so a reshaped source is diffed
/// against real accepted shapes rather than backfilled without flagging.
fn books_shaped_accepted() -> CatalogMetadata {
    let resource = entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]);
    let mut store = entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]);
    store.accepted_key_shape = Some("int".to_string());
    let mut member = entry(
        CatalogEntryKind::ResourceMember,
        "books::Book::title",
        "member-title",
        &[],
    );
    member.accepted_struct = Some("leaf:string".to_string());
    CatalogMetadata::new(7, vec![resource, store, member]).expect("shaped accepted builds")
}

/// The signature asymmetry between the two authorities is reconciled: a member reshape that changes
/// its durable signature drifts the bind whether identity comes from a store snapshot — which
/// carries signatures the checker diffs — or a committed lock, which carries only a fingerprint. The
/// lock records no signature to diff, yet the reshape still drifts it, because the clean-adoption
/// decision reconciles the missing signatures against the committed fingerprint. Neither authority
/// may silently bless a reshaped source as unchanged.
#[test]
fn a_member_reshape_drifts_under_both_the_store_and_the_lock_authority() {
    // Source reshapes `title` from the accepted `string` (`leaf:string`) to `int` (`leaf:int`).
    let reshape = |root: &std::path::Path| {
        write(
            root,
            "src/books.mw",
            "module books\nresource Book\n    title: int\nstore ^books(id: int): Book\n",
        );
    };

    // Store authority: the accepted snapshot's `leaf:string` signature diffs against the reshaped
    // `leaf:int`, so the bind advances a proposal.
    let store_root = temp_root("provider-reshape-store");
    reshape(&store_root);
    let store = analyze_project(
        &store_root,
        &config(),
        &ProjectSources::new(),
        Some(&books_shaped_accepted()),
        None,
    )
    .expect("analyze against the store snapshot");
    assert!(
        store.program.catalog.proposal.is_some(),
        "the store authority drifts a reshaped member: {:#?}",
        store.report.diagnostics
    );

    // Lock authority: no store, so the committed lock — carrying only the member's `leaf:string`
    // fingerprint, no signature — must still drift the reshape through fingerprint reconciliation.
    let lock_root = temp_root("provider-reshape-lock");
    reshape(&lock_root);
    let lock = analyze_project(
        &lock_root,
        &config(),
        &ProjectSources::new(),
        None,
        Some(&books_committed_lock(7)),
    )
    .expect("analyze against the committed lock");
    assert!(
        lock.program.catalog.proposal.is_some(),
        "the lock authority drifts a reshaped member despite carrying no signature to diff: {:#?}",
        lock.report.diagnostics
    );
}

#[test]
fn injected_snapshot_binds_identity_exactly_as_the_accepted_catalog_did() {
    let root = temp_root("provider-identity-preserved");
    books_source(&root);
    let accepted = books_accepted();

    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
    .expect("analyze");

    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let program = &snapshot.program;
    assert_eq!(program.catalog.accepted_epoch, Some(7));

    // The accepted ids are carried forward onto the live source facts exactly: the
    // resource binds the accepted resource id, not a freshly minted one.
    let module = program.facts.module_id("books").expect("books module");
    let resource = program.facts.resource_id(module, "Book").expect("Book");
    assert_eq!(
        program.facts.resource(resource).catalog_id.as_deref(),
        Some(derived_id("res-book").as_str()),
        "the injected accepted id binds onto the live resource fact"
    );

    // Source matches the accepted snapshot exactly, so there is no proposal to advance.
    assert!(
        program.catalog.proposal.is_none(),
        "an unchanged program against its accepted snapshot proposes nothing"
    );
    assert_eq!(
        program.catalog.accepted_entries, accepted.entries,
        "the accepted entries are the injected snapshot's, verbatim"
    );
}

#[test]
fn proposal_only_member_binds_activation_default_not_ordinary_facts() {
    // A brand-new member current source adds has no accepted id; its identity lives
    // only in the proposal. An `evolve default` over it binds through the proposal id,
    // while the live resource fact keeps the accepted-only binding (no proposal id leaks
    // onto ordinary facts).
    let root = temp_root("provider-proposal-only-default");
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   title: string\n\
         \x20   required pages: int\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   default Book.pages = 0\n",
    );
    let accepted = books_accepted();

    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
    .expect("analyze");

    let program = &snapshot.program;
    let proposal = program
        .catalog
        .proposal
        .as_ref()
        .expect("a new member advances the proposal");
    let pages_proposal_id = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::pages"
        })
        .expect("proposal carries the new member")
        .stable_id
        .clone();

    // The default binds through the proposal id of the brand-new member.
    let default = program
        .catalog
        .evolve_defaults
        .iter()
        .find(|default| default.catalog_id == pages_proposal_id)
        .expect("default binds the new member's proposal id");
    assert_eq!(default.catalog_id, pages_proposal_id);

    // The accepted-only ids never carry the proposal-only member, so no live fact is
    // bound to it: it has no accepted identity yet.
    assert!(
        !program
            .catalog
            .accepted_entries
            .iter()
            .any(|entry| entry.stable_id == pages_proposal_id),
        "a proposal-only id is not an accepted entry"
    );
}

#[test]
fn first_run_seeds_a_pending_member_rename_against_the_committed_old_name() {
    // A fresh checkout: the committed lock still records `books::Book::title` as the canonical
    // member identity, the store body is gone (no accepted snapshot), and the source carries a
    // pending (unapplied) rename to `Book.subtitle`. The present store resolves this rename
    // against its live accepted catalog and advances a proposal; the seed-from-lock path must do
    // the same, carrying the committed identity forward under the new path with the old path as an
    // alias — not fail `check.evolve_target` because the empty store could not be seeded.
    let root = temp_root("provider-pending-rename");
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   rename Book.title -> Book.subtitle\n",
    );
    let lock = books_committed_lock(12);

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
        .expect("analyze");

    assert!(
        !snapshot.report.has_errors(),
        "a pending rename resolves against the committed old name on a fresh checkout: {:#?}",
        snapshot.report.diagnostics
    );
    let proposal = snapshot
        .program
        .catalog
        .proposal
        .expect("first-run proposal");
    let renamed = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::subtitle"
        })
        .expect("proposal carries the renamed member");
    assert_eq!(
        renamed.stable_id,
        derived_id("member-title"),
        "the renamed member carries the committed identity forward, not a fresh mint"
    );
    assert!(
        renamed
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::title"),
        "the committed old path is recorded as an alias: {renamed:#?}"
    );
    assert!(
        !proposal
            .entries
            .iter()
            .any(|entry| entry.path == "books::Book::title"),
        "no stale entry lingers at the renamed-from path: {:#?}",
        proposal.entries
    );
}

#[test]
fn first_run_seeds_a_pending_member_retire_as_a_reserved_row() {
    // A fresh checkout whose committed lock records `books::Book::title`, with a pending
    // (unapplied) retire of that member. The present store reserves the retired identity; the
    // seed-from-lock path must reconstruct the same reserved row from the committed identity
    // rather than fail `check.evolve_target`.
    let root = temp_root("provider-pending-retire");
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   isbn: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire Book.title\n",
    );
    let lock = books_committed_lock(12);

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
        .expect("analyze");

    assert!(
        !snapshot.report.has_errors(),
        "a pending retire resolves against the committed identity on a fresh checkout: {:#?}",
        snapshot.report.diagnostics
    );
    let proposal = snapshot
        .program
        .catalog
        .proposal
        .expect("first-run proposal");
    let reserved = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::title"
        })
        .expect("proposal reserves the retired member");
    assert_eq!(
        reserved.lifecycle,
        CatalogLifecycle::Reserved,
        "the retired member is reserved, not active: {reserved:#?}"
    );
    assert_eq!(
        reserved.stable_id,
        derived_id("member-title"),
        "the reserved row carries the committed identity"
    );
}

#[test]
fn first_run_still_fails_an_unresolvable_evolve_target_on_a_fresh_checkout() {
    // The fix must not blanket-accept every evolve intent on a fresh checkout: a rename whose
    // old name names nothing the lock records and no source path still fails `check.evolve_target`,
    // exactly as it does against a present store.
    let root = temp_root("provider-unresolvable-evolve");
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   title: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   rename Book.ghost -> Book.title\n",
    );
    let lock = books_committed_lock(12);

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
        .expect("analyze");

    assert!(
        snapshot
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_EVOLVE_TARGET),
        "an evolve target naming nothing in the lock or source still fails closed: {:#?}",
        snapshot.report.diagnostics
    );
}
