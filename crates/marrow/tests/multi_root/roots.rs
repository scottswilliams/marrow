//! A project may declare more than one `store` root, and the kernel executes
//! over all of them. Each root is a distinct durable graph node with its own complete
//! ledger identity, its own slot in the image DURABLE table, its own kernel
//! `StoreSchema`, and its own name-keyed physical cell family. Two roots over two
//! resources (`^assets` + `^tallies`) compile, seal, verify, and *execute* together:
//! each is addressed by its own name in ordinary function bodies, a per-root read or
//! write dispatches to that root's schema, and a single `transaction` region may write
//! both roots and commit — or roll back — as one atomic unit.
//!
//! Entry identity stays root-local: `Id(^assets, id)` addresses `^assets` and only
//! `^assets`. Using it against `^assets` executes; naming it against `^tallies` is a
//! precise `check.type` rejection, never a silent confusion of two distinct durable
//! addresses.

use marrow_codes::Code;
use marrow_verify::{SealedSite, SealedSiteTarget, VerifiedImage};
use marrow_vm::{DurableRun, Value, fresh_test, prepare, run_test};

use crate::common::{CallOutcome, Diagnostics, Project, Session};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Asset 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Asset.name 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root assets 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key assets.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id product Tally 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id field Tally.count 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id root tallies 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key tallies.key 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

/// Two roots over two resources. Reads and writes address each root by its own name;
/// `putBoth` writes both roots in one transaction, and `putBothOrFail` proves an
/// atomic cross-root rollback. `viaId` proves a root-local entry identity round-trips
/// against its own root.
const SOURCE: &str = r#"resource Asset {
    required name: string
}

resource Tally {
    required count: int
}

store ^assets[id: int]: Asset
store ^tallies[key: string]: Tally

pub fn putAsset(id: int, n: string) {
    transaction {
        ^assets[id] = Asset(name: n)
    }
}

pub fn putTally(key: string, c: int) {
    transaction {
        ^tallies[key] = Tally(count: c)
    }
}

pub fn assetName(id: int): string? {
    return ^assets[id].name
}

pub fn tallyCount(key: string): int? {
    return ^tallies[key].count
}

pub fn viaId(id: int): string? {
    const a = Id(^assets, id)
    return ^assets[a].name
}

pub fn putBoth(id: int, key: string, n: string, c: int) {
    transaction {
        ^assets[id] = Asset(name: n)
        ^tallies[key] = Tally(count: c)
    }
}

pub fn putBothOrFail(id: int, key: string, n: string, c: int, boom: bool) {
    transaction {
        ^assets[id] = Asset(name: n)
        ^tallies[key] = Tally(count: c)
        if boom {
            unreachable("the invariant broke after staging both roots")
        }
    }
}
"#;

/// Capture, compile, and verify one durable `src/main.mw` through the production path.
fn verify(source: &str, ids: &str) -> VerifiedImage {
    Project::single(source).ids(ids).image()
}

/// An ephemeral session over `source` against the ledger `ids`.
fn open(source: &str, ids: &str) -> Session {
    Project::single(source).ids(ids).session()
}

/// The diagnostics a source the checker must reject reports.
fn errors(source: &str, ids: &str) -> Diagnostics {
    let Err(diagnostics) = Project::single(source).ids(ids).try_image() else {
        panic!("the checker must reject this program");
    };
    diagnostics
}

/// Call `name`, requiring a source-mapped runtime fault, and return its registered code.
fn fault_code(session: &mut Session, name: &str, args: Vec<Value>) -> Code {
    match session.try_call(name, args) {
        CallOutcome::Fault { code, .. } => code,
        other => panic!("{name} did not fault: {other:?}"),
    }
}

fn some_text(text: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(text.into())))))
}

fn some_int(value: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(value)))))
}

/// Two roots over two resources compile and verify into one image carrying both roots
/// in declaration order.
#[test]
fn two_roots_compile_seal_and_verify() {
    let image = verify(SOURCE, IDS);
    assert_eq!(
        image.roots().len(),
        2,
        "both declared roots enter the image's DURABLE table"
    );
    assert_eq!(image.roots()[0].name(), "assets");
    assert_eq!(image.roots()[1].name(), "tallies");
}

/// The kernel executes over a two-root image: a write to each root dispatches to that
/// root's own schema and name-keyed cell family, and a later read of each root returns
/// exactly that root's committed value. The two roots do not alias — a value written to
/// `^assets` is not observable through `^tallies` and vice versa.
#[test]
fn each_root_reads_and_writes_independently() {
    let mut session = open(SOURCE, IDS);

    // Both roots start empty.
    assert_eq!(
        session.call("assetName", vec![Value::Int(1)]),
        Some(Value::Optional(None))
    );
    assert_eq!(
        session.call("tallyCount", vec![Value::Text("x".into())]),
        Some(Value::Optional(None))
    );

    // Write each root under its own key type; read each back from its own schema.
    session.call(
        "putAsset",
        vec![Value::Int(1), Value::Text("widget".into())],
    );
    session.call("putTally", vec![Value::Text("x".into()), Value::Int(5)]);
    assert_eq!(
        session.call("assetName", vec![Value::Int(1)]),
        some_text("widget"),
    );
    assert_eq!(
        session.call("tallyCount", vec![Value::Text("x".into())]),
        some_int(5),
    );
}

/// A root-local entry identity round-trips against its own root: `Id(^assets, id)` used
/// against `^assets` reads the committed entry, exercising the declaration-ordered
/// `RootId` at runtime rather than only at check time.
#[test]
fn a_root_local_identity_reads_its_own_root() {
    let mut session = open(SOURCE, IDS);
    session.call("putAsset", vec![Value::Int(7), Value::Text("gear".into())]);
    assert_eq!(
        session.call("viaId", vec![Value::Int(7)]),
        some_text("gear"),
        "an identity minted over ^assets reads ^assets",
    );
}

/// One `transaction` region writes both roots and commits them as one atomic unit: a
/// later read observes both writes. The witness rides one engine transaction spanning
/// the disjoint name-keyed cell families of both roots.
#[test]
fn a_cross_root_transaction_commits_both_roots() {
    let mut session = open(SOURCE, IDS);

    session.call(
        "putBoth",
        vec![
            Value::Int(2),
            Value::Text("k".into()),
            Value::Text("bolt".into()),
            Value::Int(9),
        ],
    );
    assert_eq!(
        session.call("assetName", vec![Value::Int(2)]),
        some_text("bolt"),
        "the cross-root transaction committed the ^assets write",
    );
    assert_eq!(
        session.call("tallyCount", vec![Value::Text("k".into())]),
        some_int(9),
        "the cross-root transaction committed the ^tallies write",
    );
}

/// A `transaction` region that stages writes to both roots and then faults before
/// committing rolls *both* roots back together: neither the `^assets` write nor the
/// `^tallies` write survives. Atomicity is cross-root, not per-root.
#[test]
fn a_cross_root_transaction_rolls_both_roots_back() {
    let mut session = open(SOURCE, IDS);

    // Seed distinct committed values on both roots.
    session.call(
        "putBoth",
        vec![
            Value::Int(3),
            Value::Text("r".into()),
            Value::Text("old".into()),
            Value::Int(1),
        ],
    );

    // Stage a replacement of both roots, then fault before the commit.
    let code = fault_code(
        &mut session,
        "putBothOrFail",
        vec![
            Value::Int(3),
            Value::Text("r".into()),
            Value::Text("new".into()),
            Value::Int(2),
            Value::Bool(true),
        ],
    );
    assert_eq!(code, Code::RunUnreachable);

    // Both roots retain their pre-transaction committed values: the rollback was atomic
    // across both roots, not partial.
    assert_eq!(
        session.call("assetName", vec![Value::Int(3)]),
        some_text("old"),
        "the faulted transaction rolled the ^assets write back",
    );
    assert_eq!(
        session.call("tallyCount", vec![Value::Text("r".into())]),
        some_int(1),
        "the faulted transaction rolled the ^tallies write back",
    );
}

/// Two `store` declarations that share a root name are a precise `check.type` rejection:
/// each root's name keys a distinct physical cell family, so a repeated name has no
/// unambiguous address. The verifier rejects the same collision independently for a
/// forged image (see marrow-verify's multi_root_hostile).
#[test]
fn two_stores_sharing_a_root_name_are_rejected_at_check() {
    let source = r#"resource Asset {
    required name: string
}

store ^assets[id: int]: Asset
store ^assets[key: int]: Asset
"#;
    let diagnostics = errors(source, IDS);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == marrow_codes::Code::CheckType
                && d.message().contains("more than once")),
        "expected a duplicate-root-name check.type rejection, got {:?}",
        diagnostics.all()
    );
}

/// A driver `test` drives both roots through export calls — each call its own invocation
/// boundary — exactly as a terminal drives an application: a mutating export writes both
/// roots and commits, and later reading exports observe each root's committed value.
#[test]
fn a_two_root_driver_test_drives_both_roots_through_exports() {
    let source = format!(
        "{SOURCE}\ntest \"cross-root driver round trip\" {{\n    \
             putBoth(5, \"d\", \"beam\", 4)\n    \
             assert (assetName(5) ?? \"none\") == \"beam\"\n    \
             assert (tallyCount(\"d\") ?? 0) == 4\n}}\n"
    );
    // A driving `test` body lives in the image only under `compile_with_tests`, which
    // the shared harness does not expose, so this row captures and compiles directly.
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile_with_tests(&project).unwrap_or_else(|diagnostics| {
        panic!("a two-root driver test must compile: {diagnostics:#?}")
    });
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");

    let index = image
        .test_entries()
        .iter()
        .position(|entry| entry.name() == "cross-root driver round trip")
        .expect("the driver test entry is sealed");
    let demand = image
        .function(image.test_entries()[index].func())
        .expect("test function belongs to image")
        .demand();
    assert!(demand.reads() && demand.writes());
    let test = fresh_test(&prepare(image.clone()), index).expect("the entry index is sealed");
    match run_test(test) {
        DurableRun::Ran(Ok(_)) => {}
        DurableRun::Ran(Err(fault)) => {
            panic!(
                "the two-root driver test faulted: {}",
                fault.code().as_str()
            )
        }
        DurableRun::Parked => panic!("the two-root driver test parked"),
        DurableRun::Failed(code) => panic!("the two-root driver test failed: {}", code.as_str()),
    }
}

/// Each root's entry identity `Id(^root)` carries that root's own RootId, so an identity
/// minted over one root cannot address another: it is a precise `check.type` rejection,
/// not a silently accepted confusion of two distinct durable addresses.
#[test]
fn a_cross_root_identity_cannot_address_another_root() {
    let source = r#"resource Asset {
    required name: string
}

resource Tally {
    required count: int
}

store ^assets[id: int]: Asset
store ^tallies[key: string]: Tally

pub fn confuse(id: int): int? {
    const a = Id(^assets, id)
    return ^tallies[a].count
}
"#;
    let diagnostics = errors(source, IDS);
    assert!(
        diagnostics.has_code("check.type"),
        "expected a check.type rejection, got {:?}",
        diagnostics.all()
    );
}

// --- Shared Product declarations: two roots over one resource ------------------

/// The identity ledger for the shared-Product projects below. Its shape is the
/// declaration/occurrence split the ledger already implements: **one** `product` row and
/// **one** row per Product-scoped member (`Book.title`, the `Book.notes` branch placement,
/// its key column, and `Book.notes.text`), against **two** occurrence-scoped `root`/`key`
/// row pairs for `^a` and `^b`.
const SHARED_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

/// The ledger for the branchless shared-Product project: one `product R` row, one
/// `field R.v` row, and one occurrence pair per root.
const SHARED_FLAT_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product R 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field R.v 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

/// The ledger for two keyless, never-operated roots over one resource.
const SHARED_KEYLESS_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product R 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field R.v 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root r0 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id root r1 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     high-water 0\n\
     end\n";

/// Two keyed roots over one branchless resource. A Product is a declaration and a root
/// is an occurrence of it, so declaring the same resource at two placements is an
/// ordinary program: it compiles and independently verifies.
#[test]
fn two_keyed_roots_may_share_one_product() {
    let source = r#"resource R {
    required v: int
}

store ^a[id: int]: R
store ^b[id: int]: R

pub fn setA(id: int, v: int) {
    transaction {
        ^a[id] = R(v: v)
    }
}

pub fn setB(id: int, v: int) {
    transaction {
        ^b[id] = R(v: v)
    }
}
"#;
    let image = verify(source, SHARED_FLAT_IDS);
    assert_eq!(image.roots().len(), 2, "each occurrence keeps its own row");
    assert_eq!(image.roots()[0].name(), "a");
    assert_eq!(image.roots()[1].name(), "b");
    assert_eq!(
        image.roots()[0].record(),
        image.roots()[1].record(),
        "one Product declaration has one entry record however many roots occur over it"
    );
}

/// Two keyless, never-operated roots over one resource, in a project whose only export
/// is storeless: a repeated Product declaration alone is not a refusal, independent of
/// keys, sites, branches, and operations.
#[test]
fn two_keyless_never_operated_roots_may_share_one_product() {
    let source = r#"resource R {
    required v: int
}

store ^r0: R
store ^r1: R

pub fn plain(n: int): int {
    return n + 1
}
"#;
    let image = verify(source, SHARED_KEYLESS_IDS);
    assert_eq!(image.roots().len(), 2);
    assert_eq!(image.roots()[0].name(), "r0");
    assert_eq!(image.roots()[1].name(), "r1");
}

/// A nested keyed branch is a Product declaration fact, so its materialized entry record
/// is minted **once** for the Product — not once per root that occurs over it. Both
/// occurrences bind the same branch entry record type.
#[test]
fn a_shared_product_mints_one_branch_entry_record() {
    let source = r#"resource Book {
    required title: string
    notes[noteId: int] {
        required text: string
    }
}

store ^a[id: int]: Book
store ^b[id: int]: Book

pub fn addA(id: int, t: string) {
    transaction {
        ^a[id].notes[1] = Book.notes(text: t)
    }
}

pub fn addB(id: int, t: string) {
    transaction {
        ^b[id].notes[1] = Book.notes(text: t)
    }
}
"#;
    let one_root = r#"resource Book {
    required title: string
    notes[noteId: int] {
        required text: string
    }
}

store ^a[id: int]: Book

pub fn addA(id: int, t: string) {
    transaction {
        ^a[id].notes[1] = Book.notes(text: t)
    }
}
"#;
    let control = verify(one_root, SHARED_IDS);
    let image = verify(source, SHARED_IDS);
    assert_eq!(
        image.record_types().len(),
        control.record_types().len(),
        "a second occurrence of one Product mints no second branch entry record type"
    );
    let a_branch = image.roots()[0].branches();
    let b_branch = image.roots()[1].branches();
    assert_eq!(a_branch.len(), 1);
    assert_eq!(b_branch.len(), 1);
    assert_eq!(
        a_branch[0].record(),
        b_branch[0].record(),
        "both occurrences bind the declaration's one branch entry record"
    );
}

/// The identity ledger for the fitting byte-order corpus below.
const BYTE_ORDER_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Alpha 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Alpha.a 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root Alpha.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Alpha.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Alpha.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id product Beta 3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d\n\
     id field Beta.b 3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e\n\
     id root Beta.marks 4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a\n\
     id key Beta.marks.markId 4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b\n\
     id field Beta.marks.tag 4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c\n\
     id root x 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key x.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root y 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key y.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

/// Two distinct Products with nested branches whose **resource declaration order** is the
/// opposite of their **store occurrence order**: `Alpha` is declared first but `^y: Beta`
/// is the first store.
///
/// A Product's branch entry record is materialized once, at that Product's first root in
/// canonical store-traversal order. Keying it on resource declaration order instead would
/// move the mint whenever the two orders differ, and every TypeId ordinal after it with
/// it. This corpus fixes the whole image byte-exactly, so a later owner that moves the
/// mint point fails here rather than silently re-numbering an accepted image. The pinned
/// digest is the encoding of this source under the whole-entry branch write.
#[test]
fn the_fitting_byte_order_corpus_is_byte_exact() {
    let source = r#"resource Alpha {
    required a: int
    notes[noteId: int] {
        required text: string
    }
}

resource Beta {
    required b: string
    marks[markId: int] {
        required tag: string
    }
}

store ^y[id: int]: Beta
store ^x[id: int]: Alpha

pub fn putY(id: int, t: string) {
    transaction {
        ^y[id].marks[1] = Beta.marks(tag: t)
    }
}

pub fn putX(id: int, t: string) {
    transaction {
        ^x[id].notes[1] = Alpha.notes(text: t)
    }
}
"#;
    let image = verify(source, BYTE_ORDER_IDS);
    assert_eq!(
        image.image_id().to_hex(),
        "75ebfcef56422bf6a3def1012712444bb84353ddde30eff161c02f7f646a5ea1",
        "the whole image, and so every table in it, is byte-exact"
    );
    // `^y` is the first store, so `Beta.marks` mints its entry record before
    // `Alpha.notes` even though `Alpha` is the first resource declared.
    let beta_branch = image.roots()[0].branches()[0].record();
    let alpha_branch = image.roots()[1].branches()[0].record();
    assert!(
        beta_branch < alpha_branch,
        "the first store's Product mints its branch entry record first \
         ({beta_branch:?} then {alpha_branch:?})"
    );
}

/// Two occurrences of one Product execute independently: a write through `^a` is not
/// observable through `^b`. One declaration is one shape, not one reference.
#[test]
fn two_roots_over_one_product_execute_independently() {
    let source = r#"resource R {
    required v: int
}

store ^a[id: int]: R
store ^b[id: int]: R

pub fn setA(id: int, v: int) {
    transaction {
        ^a[id] = R(v: v)
    }
}

pub fn setB(id: int, v: int) {
    transaction {
        ^b[id] = R(v: v)
    }
}

pub fn readA(id: int): int? {
    return ^a[id].v
}

pub fn readB(id: int): int? {
    return ^b[id].v
}
"#;
    let mut session = open(source, SHARED_FLAT_IDS);
    session.call("setA", vec![Value::Int(1), Value::Int(7)]);
    assert_eq!(session.call("readA", vec![Value::Int(1)]), some_int(7));
    assert_eq!(
        session.call("readB", vec![Value::Int(1)]),
        Some(Value::Optional(None)),
        "a write through one occurrence is not observable through the other"
    );
    session.call("setB", vec![Value::Int(1), Value::Int(9)]);
    assert_eq!(session.call("readB", vec![Value::Int(1)]), some_int(9));
    assert_eq!(
        session.call("readA", vec![Value::Int(1)]),
        some_int(7),
        "the other occurrence keeps its own value"
    );
}

/// The nested-branch half of the same law: a branch entry written through one occurrence
/// is read back through that occurrence and is absent through the other.
#[test]
fn two_roots_over_one_product_execute_their_own_branches() {
    let source = r#"resource Book {
    required title: string
    notes[noteId: int] {
        required text: string
    }
}

store ^a[id: int]: Book
store ^b[id: int]: Book

pub fn addA(id: int, t: string) {
    transaction {
        ^a[id].notes[1] = Book.notes(text: t)
    }
}

pub fn addB(id: int, t: string) {
    transaction {
        ^b[id].notes[1] = Book.notes(text: t)
    }
}

pub fn readA(id: int): string? {
    return ^a[id].notes[1].text
}

pub fn readB(id: int): string? {
    return ^b[id].notes[1].text
}
"#;
    let mut session = open(source, SHARED_IDS);
    session.call("addA", vec![Value::Int(1), Value::Text("a".into())]);
    assert_eq!(session.call("readA", vec![Value::Int(1)]), some_text("a"));
    assert_eq!(
        session.call("readB", vec![Value::Int(1)]),
        Some(Value::Optional(None)),
        "a branch entry written through one occurrence is not observable through the other"
    );
    session.call("addB", vec![Value::Int(1), Value::Text("b".into())]);
    assert_eq!(session.call("readB", vec![Value::Int(1)]), some_text("b"));
    assert_eq!(session.call("readA", vec![Value::Int(1)]), some_text("a"));
}

/// A whole branch-entry write through the second occurrence of a shared Product lands in
/// that occurrence's own branch, not in the first occurrence's.
#[test]
fn a_whole_branch_entry_write_selects_its_own_occurrence() {
    let source = r#"resource Book {
    required title: string
    notes[noteId: int] {
        required text: string
    }
}

store ^a[id: int]: Book
store ^b[id: int]: Book

pub fn putA(id: int, t: string) {
    transaction {
        ^a[id].notes[1] = Book.notes(text: t)
    }
}

pub fn putB(id: int, t: string) {
    transaction {
        ^b[id].notes[1] = Book.notes(text: t)
    }
}

pub fn readA(id: int): string? {
    return ^a[id].notes[1].text
}

pub fn readB(id: int): string? {
    return ^b[id].notes[1].text
}
"#;
    let mut session = open(source, SHARED_IDS);
    // The branch entry record is a declaration fact shared by both occurrences, so a
    // whole-entry write must be routed by the occurrence its reference names, never by the
    // record type it constructs.
    session.call("putB", vec![Value::Int(1), Value::Text("b".into())]);
    assert_eq!(session.call("readB", vec![Value::Int(1)]), some_text("b"));
    assert_eq!(
        session.call("readA", vec![Value::Int(1)]),
        Some(Value::Optional(None)),
        "the write landed in the occurrence its reference named, not the declaration's first"
    );
    session.call("putA", vec![Value::Int(1), Value::Text("a".into())]);
    assert_eq!(session.call("readA", vec![Value::Int(1)]), some_text("a"));
    assert_eq!(session.call("readB", vec![Value::Int(1)]), some_text("b"));
}

/// The ledger for the nested-branch shared-Product project: one Product row set for
/// `Book`, its `notes` branch and that branch's own `tags` branch, against two
/// occurrence-scoped `root`/`key` pairs.
const SHARED_NESTED_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root Book.notes.tags 3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a\n\
     id key Book.notes.tags.tagId 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id field Book.notes.tags.weight 3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

/// A `Book` with a `notes` branch that itself holds a `tags` branch, projected by two
/// roots. Every export addresses `^b` through a `reference` bound to `^b`'s branch entry, so
/// the branch's own materialized record type — a Product declaration fact both
/// occurrences share — is never enough to decide which root an operation lands on. The
/// text setters create the entry whole when it is absent and write the field through
/// the proven reference when it is present, so a repeated call takes the field-write path.
const SHARED_NESTED_SOURCE: &str = r#"resource Book {
    required title: string
    notes[noteId: int] {
        required text: string
        tags[tagId: int] {
            required weight: int
        }
    }
}

store ^a[id: int]: Book
store ^b[id: int]: Book

pub fn referenceSetTextB(id: int, n: int, t: string) {
    transaction {
        ref p = ^b[id].notes[n] else {
            ^b[id].notes[n] = Book.notes(text: t)
            return
        }
        p.text = t
    }
}

pub fn referenceSetTextA(id: int, n: int, t: string) {
    transaction {
        ref p = ^a[id].notes[n] else {
            ^a[id].notes[n] = Book.notes(text: t)
            return
        }
        p.text = t
    }
}

pub fn referenceAddTagB(id: int, n: int, g: int, w: int) {
    transaction {
        ref p = ^b[id].notes[n] else {
            ^b[id].notes[n].tags[g] = Book.notes.tags(weight: w)
            return
        }
        p.tags[g] = Book.notes.tags(weight: w)
    }
}

pub fn textA(id: int, n: int): string? {
    return ^a[id].notes[n].text
}

pub fn textB(id: int, n: int): string? {
    return ^b[id].notes[n].text
}

pub fn tagWeightA(id: int, n: int, g: int): int? {
    return ^a[id].notes[n].tags[g].weight
}

pub fn tagWeightB(id: int, n: int, g: int): int? {
    return ^b[id].notes[n].tags[g].weight
}
"#;

/// A `reference` bound to a branch entry of the **second** occurrence of a shared Product
/// addresses that occurrence. A branch's materialized entry record is a declaration fact
/// — one record for the Product however many roots project it — so recovering the
/// addressed branch from that record type answers with whichever occurrence was declared
/// first. A field write through the reference must land in the root the reference named.
#[test]
fn a_reference_bound_branch_addresses_its_own_occurrence() {
    let mut session = open(SHARED_NESTED_SOURCE, SHARED_NESTED_IDS);
    // The first call creates `^b`'s note whole; the second is the field write through
    // the proven reference, and its value is the one read back.
    session.call(
        "referenceSetTextB",
        vec![Value::Int(1), Value::Int(2), Value::Text("seed".into())],
    );
    session.call(
        "referenceSetTextB",
        vec![Value::Int(1), Value::Int(2), Value::Text("b".into())],
    );
    assert_eq!(
        session.call("textB", vec![Value::Int(1), Value::Int(2)]),
        some_text("b"),
        "the write landed in the occurrence the reference named"
    );
    assert_eq!(
        session.call("textA", vec![Value::Int(1), Value::Int(2)]),
        Some(Value::Optional(None)),
        "and not in the declaration's first occurrence"
    );
    session.call(
        "referenceSetTextA",
        vec![Value::Int(1), Value::Int(2), Value::Text("a".into())],
    );
    assert_eq!(
        session.call("textA", vec![Value::Int(1), Value::Int(2)]),
        some_text("a")
    );
    assert_eq!(
        session.call("textB", vec![Value::Int(1), Value::Int(2)]),
        some_text("b"),
        "neither occurrence's branch entry aliases the other's"
    );
}

/// The same law one level deeper: a nested branch reached *through* a reference bound to the
/// second occurrence's branch entry addresses that occurrence's nested branch.
#[test]
fn a_nested_branch_through_a_reference_addresses_its_own_occurrence() {
    let mut session = open(SHARED_NESTED_SOURCE, SHARED_NESTED_IDS);
    session.call(
        "referenceAddTagB",
        vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(11)],
    );
    assert_eq!(
        session.call(
            "tagWeightB",
            vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        ),
        some_int(11),
        "the nested-branch write landed in the occurrence the reference named"
    );
    assert_eq!(
        session.call(
            "tagWeightA",
            vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        ),
        Some(Value::Optional(None)),
        "and not in the declaration's first occurrence"
    );
    assert_eq!(
        session.call("textB", vec![Value::Int(1), Value::Int(2)]),
        Some(Value::Optional(None)),
        "creating the child through a direct path leaves its parent absent",
    );
    session.call(
        "referenceSetTextB",
        vec![Value::Int(1), Value::Int(2), Value::Text("parent".into())],
    );
    session.call(
        "referenceAddTagB",
        vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(12)],
    );
    assert_eq!(
        session.call(
            "tagWeightB",
            vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        ),
        some_int(12),
        "the checked-reference branch updates the same nested entry",
    );
    assert_eq!(
        session.call(
            "tagWeightA",
            vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        ),
        Some(Value::Optional(None)),
        "the checked-reference update still leaves the other occurrence absent",
    );
}

/// Cross-root site distinctness: one Product field declaration, touched through two
/// occurrences, receives one operation site **per occurrence**, and repeated touches
/// within an occurrence reuse that occurrence's site.
///
/// A site names the durable node an instruction addresses. Two roots over one Product
/// share every member declaration identity, so a dedup key that saw only the addressed
/// declaration would hand the second occurrence the first occurrence's site — and with
/// it, the first occurrence's data. The key is the addressed node's whole semantic path,
/// which is occurrence-qualified, so the two sites are distinct while a repeat inside one
/// occurrence is not.
#[test]
fn one_product_field_touched_through_two_roots_gets_one_site_per_occurrence() {
    // `setA`/`setB` each touch `R.v` twice, and `readA`/`readB` touch it once more, so a
    // per-occurrence site that failed to dedup would show four rows, not two.
    let source = r#"resource R {
    required v: int
}

store ^a[id: int]: R
store ^b[id: int]: R

pub fn setA(id: int, v: int) {
    transaction {
        ref m = ^a[id] else { return }
        m.v = v
        m.v = v
    }
}

pub fn setB(id: int, v: int) {
    transaction {
        ref m = ^b[id] else { return }
        m.v = v
        m.v = v
    }
}

pub fn readA(id: int): int? {
    return ^a[id].v
}

pub fn readB(id: int): int? {
    return ^b[id].v
}
"#;
    // The same program with the two occurrences' first touches in the opposite order.
    let reversed = r#"resource R {
    required v: int
}

store ^a[id: int]: R
store ^b[id: int]: R

pub fn setB(id: int, v: int) {
    transaction {
        ref m = ^b[id] else { return }
        m.v = v
        m.v = v
    }
}

pub fn setA(id: int, v: int) {
    transaction {
        ref m = ^a[id] else { return }
        m.v = v
        m.v = v
    }
}

pub fn readB(id: int): int? {
    return ^b[id].v
}

pub fn readA(id: int): int? {
    return ^a[id].v
}
"#;
    for program in [source, reversed] {
        let image = verify(program, SHARED_FLAT_IDS);
        let leaves: Vec<(usize, u16)> = image
            .sites()
            .iter()
            .enumerate()
            .filter_map(|(site_id, site)| match site {
                SealedSite::Flat {
                    root,
                    target: SealedSiteTarget::FieldLeaf(0),
                    ..
                } => Some((site_id, root.index() as u16)),
                _ => None,
            })
            .collect();
        let mut roots: Vec<u16> = leaves.iter().map(|(_, root)| *root).collect();
        roots.sort_unstable();
        assert_eq!(
            roots,
            vec![0, 1],
            "one field-leaf site per occurrence, and exactly one however often it is \
             touched: {leaves:?}"
        );
        assert_ne!(
            leaves[0].0, leaves[1].0,
            "the two occurrences of one Product field hold distinct site ids: {leaves:?}",
        );
    }

    // A single-root control fixes the per-occurrence count the two-root image doubles:
    // the second occurrence adds sites, it does not divide the first occurrence's.
    let one_root = r#"resource R {
    required v: int
}

store ^a[id: int]: R

pub fn setA(id: int, v: int) {
    transaction {
        ref m = ^a[id] else { return }
        m.v = v
        m.v = v
    }
}

pub fn readA(id: int): int? {
    return ^a[id].v
}
"#;
    let control = verify(one_root, SHARED_FLAT_IDS);
    let control_leaves = control
        .sites()
        .iter()
        .filter(|site| {
            matches!(
                site,
                SealedSite::Flat {
                    target: SealedSiteTarget::FieldLeaf(_),
                    ..
                }
            )
        })
        .count();
    assert_eq!(control_leaves, 1, "one occurrence, one field-leaf site");
    // A single root over an unshared Product is outside the repeated-Product domain, so
    // its whole image — site table included — is pinned byte-exactly: the two-root image
    // above adds rows; it may not renumber this one. The digest pins the current definite
    // field operand and strict field-write encoding along with the site numbering.
    assert_eq!(
        control.image_id().to_hex(),
        "6af37a2cb358bdb3439dcbebe942ac1015b5eeb911ec6da870de202a729bd6e7",
        "the fitting single-root image is byte-exact outside the repeated-Product domain",
    );
}

/// The ledger for the multiplicity corpus: one Product `Book` carrying a static `group`
/// and a keyed `notes` branch, occurring at `^a` and `^b`.
const MULTIPLICITY_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id group Book.tally 3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a\n\
     id field Book.tally.n 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

/// The multiplicity corpus source: `Book` carries a never-operated `tally` group and a
/// `notes` branch operated only through `^a`. `^b` is written at its top-level field only.
const MULTIPLICITY_SOURCE: &str = r#"resource Book {
    required title: string
    tally {
        required n: int
    }
    notes[noteId: int] {
        required text: string
    }
}

store ^a[id: int]: Book
store ^b[id: int]: Book

pub fn addA(id: int, t: string) {
    transaction {
        ^a[id].notes[1] = Book.notes(text: t)
    }
}

pub fn setB(id: int, t: string) {
    transaction {
        ref m = ^b[id] else { return }
        m.title = t
    }
}
"#;

/// A Product with more than one occurrence pre-seeds only each occurrence's root
/// whole-payload and root-scoped index sites; its member group and branch sites are minted
/// on the first instruction that addresses them.
///
/// `^b` never addresses `Book.tally` or `Book.notes`, so it carries no site for either.
/// Seeding eagerly instead would cost `occurrences x declared nodes` sites whether or
/// not a program ever named them.
#[test]
fn a_repeated_product_mints_its_member_sites_on_demand() {
    let image = verify(MULTIPLICITY_SOURCE, MULTIPLICITY_IDS);
    let for_root = |root: u16| -> Vec<String> {
        image
            .sites()
            .iter()
            .filter_map(|site| match site {
                SealedSite::Flat {
                    root: at, target, ..
                } if at.index() == u32::from(root) => Some(format!("{target:?}")),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        for_root(1),
        vec!["WholePayload".to_string(), "FieldLeaf(0)".to_string()],
        "the second occurrence carries only its root site and the one field it writes"
    );
}

/// The ledger for the single-occurrence control: the same `Book` declaration at one root.
const MULTIPLICITY_UNIQUE_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id group Book.tally 3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a\n\
     id field Book.tally.n 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// A Product with exactly one occurrence pre-seeds its whole member graph, whether or not
/// a program names it.
///
/// The eager set and its order: the root whole-payload site, then each group entry and
/// nested branch entry in declaration pre-order. `^a` never writes `tally`, and its group
/// site is there regardless.
#[test]
fn a_single_occurrence_product_pre_seeds_its_whole_member_graph() {
    let source = r#"resource Book {
    required title: string
    tally {
        required n: int
    }
    notes[noteId: int] {
        required text: string
    }
}

store ^a[id: int]: Book

pub fn setA(id: int, t: string) {
    transaction {
        ref m = ^a[id] else { return }
        m.title = t
    }
}
"#;
    let image = verify(source, MULTIPLICITY_UNIQUE_IDS);
    let targets: Vec<String> = image
        .sites()
        .iter()
        .map(|site| match site {
            SealedSite::Flat { target, .. } => format!("{target:?}"),
            SealedSite::Parked { .. } => "parked".to_string(),
        })
        .collect();
    assert_eq!(
        targets,
        vec![
            "WholePayload".to_string(),
            "GroupEntry(0)".to_string(),
            "BranchEntry([0])".to_string(),
            "FieldLeaf(0)".to_string(),
        ],
        "the single-occurrence eager set and its pre-order are unchanged"
    );
}

/// The occurrence census is taken over the store declarations that reach admission, which
/// over-counts the accepted roots by exactly the stores that fail admission. That
/// difference cannot reach an image: a store that fails admission reports, and a reported
/// compilation produces no image at all.
///
/// Here `^b` has no ledger rows, so `Book` is censused as repeated while only `^a` is
/// accepted, and the compilation is a diagnostic rather than a mis-numbered image.
#[test]
fn a_product_whose_second_store_is_refused_produces_no_image() {
    let refused = errors(MULTIPLICITY_SOURCE, MULTIPLICITY_UNIQUE_IDS);
    assert!(
        refused.has_code("check.durable_identity"),
        "the second store reports its identity gap: {:?}",
        refused.all()
    );
}

/// The ledger for the sealed-entry-facts project: a grouped `Book` root with a two-level
/// branch, and a second `Shelf` root carrying a unique index.
const ENTRY_FACTS_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.pages 21212121212121212121212121212121\n\
     id field Book.details.language 22222222222222222222222222222222\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root Book.notes.tags 3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a\n\
     id key Book.notes.tags.tagId 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id field Book.notes.tags.weight 3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id product Shelf 4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d\n\
     id field Shelf.label 4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e\n\
     id field Shelf.code 4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f\n\
     id root shelves 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key shelves.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     id index shelves.byCode 70707070707070707070707070707070\n\
     high-water 0\n\
     end\n";

const ENTRY_FACTS_SOURCE: &str = r#"resource Book {
    required title: string

    details {
        pages: int
        language: string
    }

    notes[noteId: int] {
        required text: string
        tags[tagId: int] {
            required weight: int
        }
    }
}

resource Shelf {
    required label: string
    code: string
}

store ^books[id: int]: Book
store ^shelves[id: int]: Shelf {
    index byCode[code] unique
}

pub fn book(id: int): Book? {
    return ^books[id]
}

pub fn pages(id: int): int {
    if const d = ^books[id].details {
        return d.pages ?? 0
    }
    return 0
}

pub fn tagWeight(id: int, n: int, g: int): int {
    if const t = ^books[id].notes[n].tags[g] {
        return t.weight
    }
    return 0
}

pub fn weight(id: int, n: int, g: int): int? {
    return ^books[id].notes[n].tags[g].weight
}

pub fn shelfByCode(code: string): bool {
    return exists(^shelves.byCode[code])
}
"#;

/// Every executable site seals the record of the whole entry it materializes and that
/// record's trailing group-slot count, which the VM reads instead of walking the roots.
#[test]
fn a_flat_site_seals_its_entry_record_and_group_count() {
    let image = verify(ENTRY_FACTS_SOURCE, ENTRY_FACTS_IDS);
    let field_names = |entry| -> Vec<String> {
        image
            .record_type(entry)
            .fields()
            .iter()
            .map(|field| field.name().to_string())
            .collect()
    };
    let facts: Vec<(String, Vec<String>, usize)> = image
        .sites()
        .iter()
        .filter_map(|site| match site {
            SealedSite::Flat {
                target,
                entry,
                groups,
                ..
            } => Some((format!("{target:?}"), field_names(*entry), *groups)),
            SealedSite::Parked { .. } => None,
        })
        .collect();
    let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect();
    let expect = |target: &str, fields: &[&str], groups: usize| {
        let fact = (target.to_string(), names(fields), groups);
        assert!(facts.contains(&fact), "{fact:?} not among {facts:?}");
    };
    expect("WholePayload", &["title", "details"], 1);
    expect("GroupEntry(0)", &["pages", "language"], 0);
    expect("BranchEntry([0, 0])", &["weight"], 0);
    expect("BranchField { branch: [0, 0], field: 0 }", &["weight"], 0);
    expect("IndexLookup(0)", &["label", "code"], 0);
}
