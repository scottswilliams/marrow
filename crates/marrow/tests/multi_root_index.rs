//! MR01: managed indexes are per-root. Two roots (`^assets` + `^tallies`) each declare a
//! unique index (`*BySku`) and a nonunique index (`*ByShelf`), and every index cell family
//! is keyed by its owning root's name. These tests drive the whole production path —
//! capture -> compile -> verify -> attach -> VM — and prove, with deliberately shared field
//! values across the two roots, that the index cells do not alias:
//!
//! - a unique lookup on one root yields only that root's entry, even when both roots hold
//!   the same `sku`;
//! - a bounded scan on one root counts only that root's entries, even when both roots hold
//!   the same `shelf`;
//! - the unique-index collision fault is enforced independently per root, including on the
//!   second-declared root (`^tallies`, RootId 1), and a rejected write leaves the prior
//!   committed state intact.

use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{
    DurableRun, EphemeralOutcome, MemoryAttachment, Value, mint_ephemeral, prepare, run_export,
};

// Each root carries one unique index (`*BySku`) and one nonunique index (`*ByShelf`); the
// index anchors live at `<root>.<index name>`. Every durable declaration has a distinct
// ledger id.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Asset 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Asset.name 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Asset.sku 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id field Asset.shelf 2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e\n\
     id root assets 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key assets.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index assets.aBySku 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id index assets.aByShelf 4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b\n\
     id product Tally 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id field Tally.label 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id field Tally.sku 1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f\n\
     id field Tally.shelf 3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e\n\
     id root tallies 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key tallies.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     id index tallies.tBySku 5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b\n\
     id index tallies.tByShelf 6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Asset {
    required name: string
    required sku: string
    required shelf: string
}

resource Tally {
    required label: string
    required sku: string
    required shelf: string
}

store ^assets[id: int]: Asset {
    index aBySku[sku] unique
    index aByShelf[shelf, id]
}

store ^tallies[id: int]: Tally {
    index tBySku[sku] unique
    index tByShelf[shelf, id]
}

pub fn putAsset(id: int, name: string, sku: string, shelf: string) {
    transaction {
        ^assets[id] = Asset(name: name, sku: sku, shelf: shelf)
    }
}

pub fn putTally(id: int, label: string, sku: string, shelf: string) {
    transaction {
        ^tallies[id] = Tally(label: label, sku: sku, shelf: shelf)
    }
}

pub fn assetNameBySku(sku: string): string? {
    if const found = ^assets.aBySku[sku] {
        return ^assets[found].name
    }
    return absent
}

pub fn tallyLabelBySku(sku: string): string? {
    if const found = ^tallies.tBySku[sku] {
        return ^tallies[found].label
    }
    return absent
}

pub fn assetsOnShelf(shelf: string): int {
    var count = 0
    for aid in ^assets.aByShelf[shelf] at most 100 {
        if const a = ^assets[aid] {
            count += 1
        }
    } on more {
        count = -1
    }
    return count
}

pub fn talliesOnShelf(shelf: string): int {
    var count = 0
    for tid in ^tallies.tByShelf[shelf] at most 100 {
        if const t = ^tallies[tid] {
            count += 1
        }
    } on more {
        count = -1
    }
    return count
}

pub fn tallyLabel(id: int): string? {
    return ^tallies[id].label
}

pub fn eqAssets(sku: string, id: int): bool? {
    if const found = ^assets.aBySku[sku] {
        return found == Id(^assets, id)
    }
    return absent
}

pub fn eqTallies(sku: string, id: int): bool? {
    if const found = ^tallies.tBySku[sku] {
        return found == Id(^tallies, id)
    }
    return absent
}
"#;

fn compile_verify() -> VerifiedImage {
    verify_with(SOURCE, IDS)
}

fn verify_with(source: &str, ids: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

fn export<'a>(image: &'a VerifiedImage, name: &str) -> &'a SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .expect("export present")
}

struct DebugRun<'a>(&'a DurableRun);
impl std::fmt::Debug for DebugRun<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            DurableRun::Ran(Ok(_)) => write!(f, "Ran(Ok(value))"),
            DurableRun::Ran(Err(fault)) => write!(f, "Ran(Err({}))", fault.code()),
            DurableRun::Parked => write!(f, "Parked"),
            DurableRun::Failed(code) => write!(f, "Failed({code})"),
        }
    }
}

fn run(
    image: &VerifiedImage,
    attachment: &mut MemoryAttachment,
    name: &str,
    args: Vec<Value>,
) -> Option<Value> {
    match run_export(attachment, export(image, name).id(), args)
        .expect("the export is in the image")
    {
        DurableRun::Ran(Ok(value)) => value,
        other => panic!("{name} did not run cleanly: {:?}", DebugRun(&other)),
    }
}

fn run_faulting(
    image: &VerifiedImage,
    attachment: &mut MemoryAttachment,
    name: &str,
    args: Vec<Value>,
) -> String {
    match run_export(attachment, export(image, name).id(), args)
        .expect("the export is in the image")
    {
        DurableRun::Ran(Err(fault)) => fault.code().to_string(),
        other => panic!("{name} did not fault: {:?}", DebugRun(&other)),
    }
}

fn attach(image: &VerifiedImage) -> MemoryAttachment {
    match mint_ephemeral(prepare(image.clone())) {
        EphemeralOutcome::Ready(attachment) => attachment,
        EphemeralOutcome::Parked(_) => {
            panic!("a two-root indexed image must be executable, not parked")
        }
        EphemeralOutcome::Failed { cause, .. } => panic!("minting the attachment failed: {cause}"),
    }
}

fn text(v: &str) -> Value {
    Value::Text(v.into())
}

fn some_text(v: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(v.into())))))
}

fn int(v: i64) -> Option<Value> {
    Some(Value::Int(v))
}

/// Seed both roots with entries that deliberately SHARE `sku` and `shelf` values across the
/// two roots, so any index-cell aliasing between roots would surface as a wrong lookup or
/// count. `assets` and `tallies` each get two entries on shelf "A" with skus "s1"/"s2".
fn seed(image: &VerifiedImage, store: &mut MemoryAttachment) {
    run(
        image,
        store,
        "putAsset",
        vec![Value::Int(1), text("asset-one"), text("s1"), text("A")],
    );
    run(
        image,
        store,
        "putAsset",
        vec![Value::Int(2), text("asset-two"), text("s2"), text("A")],
    );
    run(
        image,
        store,
        "putTally",
        vec![Value::Int(1), text("tally-one"), text("s1"), text("A")],
    );
    run(
        image,
        store,
        "putTally",
        vec![Value::Int(2), text("tally-two"), text("s2"), text("A")],
    );
}

/// A unique index is per-root: a `*BySku` lookup on one root yields only that root's entry,
/// even though both roots hold the same `sku` values. The index cells are keyed by the
/// owning root's name, so they never alias across roots.
#[test]
fn a_unique_index_lookup_resolves_within_its_own_root() {
    let image = compile_verify();
    let mut store = attach(&image);
    seed(&image, &mut store);

    // Both roots hold sku "s1", but each root's unique index resolves to its own entry.
    assert_eq!(
        run(&image, &mut store, "assetNameBySku", vec![text("s1")]),
        some_text("asset-one"),
        "the asset unique index resolves within ^assets",
    );
    assert_eq!(
        run(&image, &mut store, "tallyLabelBySku", vec![text("s1")]),
        some_text("tally-one"),
        "the tally unique index resolves within ^tallies, not through ^assets",
    );
    assert_eq!(
        run(&image, &mut store, "assetNameBySku", vec![text("s2")]),
        some_text("asset-two"),
    );
    assert_eq!(
        run(&image, &mut store, "tallyLabelBySku", vec![text("s2")]),
        some_text("tally-two"),
    );
}

/// A nonunique index is per-root: a `*ByShelf` bounded scan on one root counts only that
/// root's entries, even though both roots place every entry on the same shelf "A". A scan
/// that saw the other root's cells would over-count.
#[test]
fn a_nonunique_index_scan_counts_only_its_own_root() {
    let image = compile_verify();
    let mut store = attach(&image);
    seed(&image, &mut store);

    assert_eq!(
        run(&image, &mut store, "assetsOnShelf", vec![text("A")]),
        int(2),
        "shelf A holds exactly the two assets, not the tallies sharing that shelf",
    );
    assert_eq!(
        run(&image, &mut store, "talliesOnShelf", vec![text("A")]),
        int(2),
        "shelf A holds exactly the two tallies, not the assets sharing that shelf",
    );
    // A shelf only the tallies use is empty for assets and vice versa (no cross-root bleed).
    run(
        &image,
        &mut store,
        "putTally",
        vec![Value::Int(3), text("tally-three"), text("s3"), text("B")],
    );
    assert_eq!(
        run(&image, &mut store, "talliesOnShelf", vec![text("B")]),
        int(1),
    );
    assert_eq!(
        run(&image, &mut store, "assetsOnShelf", vec![text("B")]),
        int(0),
        "shelf B holds a tally but no asset — the asset scan does not see the tally cell",
    );
}

/// The unique-index collision fault is enforced independently on the second-declared root
/// (`^tallies`, RootId 1): a second tally reusing a committed `sku` faults
/// `run.unique_index` and rolls back, leaving the prior committed state — the first tally
/// and both assets — intact. A same-`sku` write across roots does NOT collide, since each
/// root's unique cells are disjoint.
#[test]
fn per_root_unique_enforcement_on_the_second_root_leaves_committed_state_intact() {
    let image = compile_verify();
    let mut store = attach(&image);
    seed(&image, &mut store);

    // A tally reusing tally 1's sku "s1" collides within ^tallies (RootId 1) and faults.
    let code = run_faulting(
        &image,
        &mut store,
        "putTally",
        vec![Value::Int(9), text("dup"), text("s1"), text("A")],
    );
    assert_eq!(code, "run.unique_index");

    // The collided write rolled back: tally 9 was never committed, tally 1 stands, and the
    // unique lookup still resolves to the original.
    assert_eq!(
        run(&image, &mut store, "tallyLabel", vec![Value::Int(9)]),
        Some(Value::Optional(None)),
        "the faulted unique-collision write left no entry behind",
    );
    assert_eq!(
        run(&image, &mut store, "tallyLabelBySku", vec![text("s1")]),
        some_text("tally-one"),
        "the pre-collision unique cell is intact after the rollback",
    );
    // The other root is untouched: an asset sharing sku "s1" was never part of the tallies
    // collision, and both roots' scans still count correctly.
    assert_eq!(
        run(&image, &mut store, "assetNameBySku", vec![text("s1")]),
        some_text("asset-one"),
    );
    assert_eq!(
        run(&image, &mut store, "talliesOnShelf", vec![text("A")]),
        int(2),
    );
    assert_eq!(
        run(&image, &mut store, "assetsOnShelf", vec![text("A")]),
        int(2),
    );
}

/// A write to one root's unique index never collides with the *other* root's identical
/// `sku`: seeding both roots with the same skus (see [`seed`]) commits cleanly, which is
/// only possible if the two roots' unique-index cell families are disjoint.
#[test]
fn a_shared_sku_across_roots_is_not_a_unique_collision() {
    let image = compile_verify();
    let mut store = attach(&image);
    // seed writes asset(s1) then tally(s1); if the two roots shared a unique cell family the
    // second write would fault. It commits, proving per-root isolation of the unique cells.
    seed(&image, &mut store);
    assert_eq!(
        run(&image, &mut store, "assetNameBySku", vec![text("s1")]),
        some_text("asset-one"),
    );
    assert_eq!(
        run(&image, &mut store, "tallyLabelBySku", vec![text("s1")]),
        some_text("tally-one"),
    );
}

/// The identity a unique-index lookup yields addresses the root it was read from. The
/// lookup on the second-declared root (`^tallies`, RootId 1) must compare equal to the
/// identity constructed for that same entry, exactly as it does on the first root.
/// `Value` equality compares the root tag, so a lookup that reported root 0 for every
/// root would answer a visibly wrong boolean here.
#[test]
fn an_index_lookup_identity_carries_the_root_it_was_read_from() {
    let image = compile_verify();
    let mut store = attach(&image);
    seed(&image, &mut store);

    assert_eq!(
        run(
            &image,
            &mut store,
            "eqAssets",
            vec![text("s1"), Value::Int(1)]
        ),
        Some(Value::Optional(Some(Box::new(Value::Bool(true))))),
        "the ^assets lookup identity equals the identity constructed for the same entry",
    );
    assert_eq!(
        run(
            &image,
            &mut store,
            "eqTallies",
            vec![text("s1"), Value::Int(1)]
        ),
        Some(Value::Optional(Some(Box::new(Value::Bool(true))))),
        "the ^tallies lookup identity carries root 1, not root 0",
    );
}

// --- One shared Product, two roots, different managed-index shapes ------------------

/// One `Item` Product declaration with two occurrences carrying deliberately different
/// index shapes: `^a` indexes `sku` uniquely, while `^b` indexes `sku` uniquely under its
/// own index identity *and* adds a nonunique `shelf` index `^a` does not declare.
const SHARED_INDEX_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Item 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Item.name 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Item.sku 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id field Item.shelf 2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index a.aBySku 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     id index b.bBySku 5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b\n\
     id index b.bByShelf 6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b\n\
     high-water 0\n\
     end\n";

const SHARED_INDEX_SOURCE: &str = r#"resource Item {
    required name: string
    required sku: string
    required shelf: string
}

store ^a[id: int]: Item {
    index aBySku[sku] unique
}

store ^b[id: int]: Item {
    index bBySku[sku] unique
    index bByShelf[shelf, id]
}

pub fn putA(id: int, name: string, sku: string, shelf: string) {
    transaction {
        ^a[id] = Item(name: name, sku: sku, shelf: shelf)
    }
}

pub fn putB(id: int, name: string, sku: string, shelf: string) {
    transaction {
        ^b[id] = Item(name: name, sku: sku, shelf: shelf)
    }
}

pub fn nameA(id: int): string? {
    return ^a[id].name
}

pub fn nameB(id: int): string? {
    return ^b[id].name
}
"#;

fn rep(byte: u8) -> marrow_image::LedgerIdBytes {
    marrow_image::LedgerIdBytes::from_bytes([byte; 16])
}

/// A managed index belongs to a root **occurrence**, but a Product field declaration
/// belongs to the Product: `Item.sku` is one declaration that both `^a` and `^b` project.
/// Asking what a write to that field maintains therefore has no answer until an
/// occurrence is named, and the answer must be that occurrence's indexes alone. Unioning
/// them would tell the maintenance path that a write through `^b` must keep `^a`'s index
/// coherent.
#[test]
fn index_maintenance_is_scoped_to_the_occurrence_that_was_written() {
    let image = verify_with(SHARED_INDEX_SOURCE, SHARED_INDEX_IDS);
    let a = image.root_occurrence(0).expect("^a");
    let b = image.root_occurrence(1).expect("^b");
    assert_eq!(a.root().name(), "a");
    assert_eq!(b.root().name(), "b");

    // `Item.sku` (0x0f) is one Product field declaration indexed by both occurrences.
    assert_eq!(
        a.field_maintenance(rep(0x0f)),
        vec![rep(0x3b)],
        "a write through ^a maintains ^a's index and no other"
    );
    assert_eq!(
        b.field_maintenance(rep(0x0f)),
        vec![rep(0x5b)],
        "a write through ^b maintains ^b's index and no other"
    );

    // `Item.shelf` (0x2e) is indexed only by ^b, so a write through ^a maintains nothing.
    assert!(a.field_maintenance(rep(0x2e)).is_empty());
    assert_eq!(b.field_maintenance(rep(0x2e)), vec![rep(0x6b)]);

    // The whole-entry and collision layouts are occurrence-scoped for the same reason.
    assert_eq!(a.entry_maintenance(), vec![rep(0x3b)]);
    assert_eq!(b.entry_maintenance(), vec![rep(0x5b), rep(0x6b)]);
    assert_eq!(a.unique_collision_outcomes(), vec![rep(0x3b)]);
    assert_eq!(b.unique_collision_outcomes(), vec![rep(0x5b)]);
}

/// The runtime consequence of the same law: the two occurrences' unique `sku` indexes do
/// not alias, so the same `sku` may be held by an entry of each root, and a collision on
/// one occurrence leaves the other's committed state intact.
#[test]
fn a_unique_index_collides_only_within_its_own_occurrence() {
    let image = verify_with(SHARED_INDEX_SOURCE, SHARED_INDEX_IDS);
    let mut store = attach(&image);
    run(
        &image,
        &mut store,
        "putA",
        vec![Value::Int(1), text("in-a"), text("s1"), text("left")],
    );
    // The same `sku` through the other occurrence is not a collision: the index cell
    // families are per-root.
    run(
        &image,
        &mut store,
        "putB",
        vec![Value::Int(1), text("in-b"), text("s1"), text("left")],
    );
    assert_eq!(
        run(&image, &mut store, "nameA", vec![Value::Int(1)]),
        some_text("in-a")
    );
    assert_eq!(
        run(&image, &mut store, "nameB", vec![Value::Int(1)]),
        some_text("in-b")
    );

    // A second entry of ^b claiming ^b's own indexed `sku` does collide, and leaves ^a
    // untouched.
    let code = run_faulting(
        &image,
        &mut store,
        "putB",
        vec![Value::Int(2), text("clash"), text("s1"), text("right")],
    );
    assert_eq!(code, "run.unique_index");
    assert_eq!(
        run(&image, &mut store, "nameA", vec![Value::Int(1)]),
        some_text("in-a")
    );
    assert_eq!(
        run(&image, &mut store, "nameB", vec![Value::Int(1)]),
        some_text("in-b")
    );
}
