//! The admitted-subset agreement gate: a program the checker accepts must also
//! verify, and, when it carries a driving `test`, run without an artifact rejection.
//! The gate pins each composition's whole-pipeline verdict (capture -> compile ->
//! verify -> run) over a bounded matrix of durable op forms by context. A composition
//! whose round trip is not yet whole carries its exact current code, so a fix that
//! lands without moving its row to [`Expect::RoundTrips`] fails on the changed verdict.

use marrow_codes::Code;
use marrow_verify::VerifiedImage;
use marrow_vm::{DurableRun, Value, fresh_test, prepare, run_test};

use crate::common::{CallOutcome, Project};

/// The shared durable graph every composition is written against: a flat keyed
/// root with a required and a sparse field, a root-level group, a keyed branch,
/// a unique index (identity lookup), a nonunique index (bounded scan), and a
/// composite-key root (two key operands). The identity ledger below pins one id
/// per anchor, so the schema is identity complete on its own and each
/// composition only appends operations.
const SCHEMA: &str = r#"resource Book {
    required title: string
    required isbn: string
    subtitle: string
    glucose: Option<int>
    lactate: Option<int>

    details {
        pages: int
    }

    notes[noteId: string] {
        required text: string
    }
}

store ^books[id: int]: Book {
    index byIsbn[isbn] unique
    index byShelf[title, id]
}

resource Grade {
    required score: int
}

store ^grades[student: string, course: string]: Grade
"#;

/// The committed identity ledger for [`SCHEMA`]. Machine-minted from OS entropy
/// once; embedded verbatim so the gate needs no ledger side effect.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 31985fe4a848fb49176f9debb5948854\n\
     id product Book 37476822645b6802b40160c53d1a7fb6\n\
     id field Book.details.pages 7557aec5eed45271842bd2d8f03c065e\n\
     id field Book.glucose a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1\n\
     id field Book.isbn dc43cd86f5de791211612a599f1a1b01\n\
     id field Book.lactate a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2\n\
     id field Book.notes.text c3cde175f2329c20c8c8ce0d39405712\n\
     id field Book.subtitle 26ba2d1538308102805dfa7e5007a493\n\
     id field Book.title ea95ccce4ce370579210f6697baf7316\n\
     id sum Option[int] a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3\n\
     id member Option[int].none a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4\n\
     id member Option[int].some a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5\n\
     id root Book.notes adc70cb07526b070b5a5a23f078c0784\n\
     id root books 980b01438681e85db8137bb42f2960c5\n\
     id key Book.notes.noteId 6b07eb3f8fb174293b3f8a5b67ffc27b\n\
     id key books.id 7e84c2e0e11e07094481ec3a522dadce\n\
     id group Book.details d69902579081537e5b526739d66131be\n\
     id index books.byIsbn 711c5dcd42019503ab5bbf3470f989c4\n\
     id index books.byShelf f3f35e9ded68649a50bd977094452cc3\n\
     id product Grade b3022a809b506926824b11de41d07565\n\
     id field Grade.score 0353d95c37594c0b2cbeb477b3adc10d\n\
     id root grades de72c544f1a56b2e4341fc8c6e59361e\n\
     id key grades.student 2116e6ec78f09131260cf018042e542e\n\
     id key grades.course 1cc4fe005d385c2fa8137c54e910b89f\n\
     high-water 0\n\
     end\n";

/// A nominal-bearing indexed binding must fail before publishing an image,
/// including when the program declares no durable operations.
const NOMINAL_INDEX_SCHEMA: &str = r#"type Rank: int in 0..=100

resource Book {
    required title: string
    rank: Rank
}

store ^books[id: int]: Book {
    index byRank[rank, id]
}
"#;

const NOMINAL_INDEX_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.rank 10101010101010101010101010101010\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index books.byRank 70707070707070707070707070707070\n\
     high-water 0\n\
     end\n";

/// The pipeline verdict for one composition, at the stage it first stops.
enum Stage {
    /// The checker rejected the source with this typed code.
    CheckerRejected(Code),
    /// The checker accepted, but the independent verifier rejected the image
    /// (`image.*`) with this code and detail — a checker/verifier divergence.
    VerifyRejected { code: Code, detail: &'static str },
    /// The checker accepted and the verifier sealed the image.
    Verified(Box<VerifiedImage>),
}

/// Drive one composition through the production pipeline: capture the schema plus
/// the appended operations, compile *with tests* (so a test-body driver is part of
/// the image), and verify. The verifier reconstructs demand and the transaction/flow
/// laws from the image alone, so this reads the true checker⇒verifier relationship,
/// not a compiler self-report. The shared `Project` harness exposes neither
/// `compile_with_tests` nor a typed verifier rejection, so this captures directly.
fn pipeline(ops: &str) -> Stage {
    let source = format!("{SCHEMA}\n{ops}");
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
    match marrow_compile::compile_with_tests(&project) {
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => Stage::CheckerRejected(
            diagnostics
                .as_slice()
                .first()
                .expect("a rejection carries at least one diagnostic")
                .code(),
        ),
        Err(
            marrow_compile::CompileFailure::Invariant(_)
            | marrow_compile::CompileFailure::ResourceLimit(_),
        ) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
        Ok(compiled) => match marrow_verify::verify(&compiled.image.bytes) {
            Err(rejection) => Stage::VerifyRejected {
                code: rejection.code(),
                detail: rejection.detail(),
            },
            Ok(image) => Stage::Verified(Box::new(image)),
        },
    }
}

/// The pinned verdict for a matrix row.
enum Expect {
    /// The intended round trip is whole: the checker accepts and the verifier
    /// seals. `run` additionally drives every `test` in the image through the
    /// ephemeral kernel and requires each to run without an artifact rejection
    /// or a runtime fault — the run-side half of "checker-accept ⇒ verify+run".
    RoundTrips { run: bool },
    /// A recorded checker/verifier divergence: the checker accepts but the verifier
    /// rejects. The exact current code and detail are pinned so a fix that changes the
    /// verdict forces this row to move to `RoundTrips`. The ledger is empty; the
    /// variant is the mechanism a divergence is recorded through, so a regression
    /// becomes a failing row.
    #[allow(dead_code)]
    KnownDivergent { code: Code, detail: &'static str },
    /// The checker rejects the composition at check time, so it never reaches the
    /// verifier — checker-accept ⇒ verify holds vacuously and the two agree. The exact
    /// `check.*` code is pinned so a change to the verdict forces this row to move.
    CheckerRejects { code: Code },
}

struct Row {
    label: &'static str,
    ops: &'static str,
    expect: Expect,
}

/// The bounded composition matrix: durable op forms (whole-entry read/write,
/// field read/write, group read/write, branch read/write, index lookup,
/// identity write, bounded traversal) × contexts (outside a transaction, inside
/// a mutating region, through test seed and observer calls, via an export call from a
/// test body).
fn matrix() -> Vec<Row> {
    let mut rows = admitted_subset_rows();
    rows.extend(resource_value_rows());
    rows.extend(owned_region_rows());
    rows.extend(entry_identity_rows());
    rows.extend(place_base_rows());
    rows.extend(place_composition_rows());
    rows.extend(identity_parent_rows());
    rows.extend(shared_enum_rows());
    rows
}

/// The positive controls: every durable op form in every admitted context.
fn admitted_subset_rows() -> Vec<Row> {
    vec![
        Row {
            label: "whole-entry read / outside a transaction",
            ops: "pub fn weReadOut(id: int): string? {\n    if const b = ^books[id] {\n        return b.title\n    }\n    return absent\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "whole-entry read / inside a mutating region (read-modify-write)",
            ops: "pub fn weReadTxn(id: int) {\n    transaction {\n        place m = ^books[id]\n        if const b = m {\n            m.subtitle = b.title\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "whole-entry write / inside a mutating region",
            ops: "pub fn weWrite(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "field read / outside a transaction",
            ops: "pub fn fieldRead(id: int): string? {\n    return ^books[id].title\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "field write / inside a mutating region",
            ops: "pub fn fieldWrite(id: int) {\n    transaction {\n        place m = ^books[id]\n        if exists(m) {\n            m.subtitle = \"x\"\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "group-leaf write / inside a mutating region",
            ops: "pub fn groupWrite(id: int) {\n    transaction {\n        place m = ^books[id]\n        if exists(m) {\n            m.details.pages = 3\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "group read / outside a transaction",
            ops: "pub fn groupRead(id: int): int? {\n    return ^books[id].details.pages\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "branch write / inside a mutating region",
            ops: "pub fn branchWrite(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"t\")\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "index lookup + identity read / outside a transaction",
            ops: "pub fn lookupRead(isbn: string): string? {\n    if const found = ^books.byIsbn[isbn] {\n        return ^books[found].title\n    }\n    return absent\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "identity field write / inside a mutating region",
            ops: "pub fn identityFieldWrite(isbn: string) {\n    transaction {\n        if const found = ^books.byIsbn[isbn] {\n            place m = ^books[found]\n            if exists(m) {\n                m.subtitle = \"x\"\n            }\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "bounded index scan / outside a transaction",
            ops: "pub fn scan(t: string): int {\n    var n = 0\n    for id in ^books.byShelf[t] at most 10 {\n        n += 1\n    } on more {\n        n = -1\n    }\n    return n\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "field write + read-back / through seed and private observer calls",
            ops: "pub fn seedSubtitle() {\n    transaction {\n        place m = ^books[1]\n        m = Book(title: \"t\", isbn: \"i\")\n        m.subtitle = \"x\"\n    }\n}\n\nfn subtitle(): string? {\n    return ^books[1].subtitle\n}\n\ntest \"direct field round trip\" {\n    seedSubtitle()\n    assert subtitle() ?? \"n\" == \"x\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        Row {
            label: "whole-entry write + read-back / through seed and private observer calls",
            ops: "pub fn seedBook() {\n    transaction {\n        ^books[1] = Book(title: \"dune\", isbn: \"i1\")\n    }\n}\n\nfn book(): Book? {\n    return ^books[1]\n}\n\ntest \"direct whole-entry round trip\" {\n    seedBook()\n    if const b = book() {\n        assert b.title == \"dune\"\n    } else {\n        assert false\n    }\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A composite-key root place carries several key slots but is still a root, so
        // field reads and writes through it resolve the root's field, not a branch record:
        // the node kind is recorded at the binding from the canonical resolved durable
        // node, independent of key-operand count.
        Row {
            label: "composite-root place field read + write / read outside, write inside a region",
            ops: "pub fn crPlaceRead(student: string, course: string): int? {\n    place g = ^grades[student, course]\n    return g.score\n}\n\npub fn crPlaceWrite(student: string, course: string, score: int) {\n    transaction {\n        place g = ^grades[student, course]\n        if exists(g) {\n            g.score = score\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        // Each export call is its own invocation boundary, so the write and the read-back
        // resolve the root field through their own place binding.
        Row {
            label: "composite-root place field write round trip / driver test",
            ops: "pub fn crSeed(student: string, course: string, score: int) {\n    transaction {\n        ^grades[student, course] = Grade(score: score)\n    }\n}\n\npub fn crWriteVia(student: string, course: string, score: int) {\n    transaction {\n        place g = ^grades[student, course]\n        if exists(g) {\n            g.score = score\n        }\n    }\n}\n\npub fn crReadVia(student: string, course: string): int? {\n    place g = ^grades[student, course]\n    return g.score\n}\n\ntest \"composite-root place writes then reads a score back\" {\n    crSeed(\"amy\", \"cs\", 90)\n    crWriteVia(\"amy\", \"cs\", 75)\n    assert crReadVia(\"amy\", \"cs\") ?? 0 == 75\n}",
            expect: Expect::RoundTrips { run: true },
        },
    ]
}

/// Resource values crossing function boundaries.
fn resource_value_rows() -> Vec<Row> {
    vec![
        // The verifier reconstructs boundary types from the image, so a sealed image
        // proves the resource value crosses the call by value.
        Row {
            label: "resource value read -> helper param -> return -> whole-entry write / in a region",
            ops: "fn rework(b: Book): Book {\n    var working = b\n    working.subtitle = working.title\n    return working\n}\n\npub fn revise(id: int) {\n    transaction {\n        if const current = ^books[id] {\n            ^books[id] = rework(current)\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "resource value round trip through a helper / driver test",
            ops: "fn withSubtitle(b: Book, s: string): Book {\n    var working = b\n    working.subtitle = s\n    return working\n}\n\npub fn seed(id: int, title: string, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: title, isbn: isbn)\n    }\n}\n\npub fn revise(id: int, s: string) {\n    transaction {\n        if const current = ^books[id] {\n            ^books[id] = withSubtitle(current, s)\n        }\n    }\n}\n\npub fn subtitleOf(id: int): string? {\n    return ^books[id].subtitle\n}\n\ntest \"resource value crosses a helper and writes back\" {\n    seed(4, \"dune\", \"i4\")\n    revise(4, \"revised\")\n    assert subtitleOf(4) ?? \"none\" == \"revised\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // The owner lattice runs for any export that owns a transaction, so a read-only
        // region reads inside and returns the captured value after the block.
        Row {
            label: "whole-entry read / inside a read-only region (captured, returned after)",
            ops: "pub fn d2ReadOnlyRegion(id: int): string? {\n    var out: string? = absent\n    transaction {\n        if const b = ^books[id] {\n            out = b.title\n        }\n    }\n    return out\n}",
            expect: Expect::RoundTrips { run: false },
        },
        // Each call from a test body is its own invocation boundary.
        Row {
            label: "driver test — mutating export call then read-back export",
            ops: "pub fn d1Add(id: int, title: string) {\n    transaction {\n        ^books[id] = Book(title: title, isbn: \"i\")\n    }\n}\n\npub fn d1Title(id: int): string? {\n    return ^books[id].title\n}\n\ntest \"driver adds through an export and reads it back\" {\n    d1Add(7, \"dune\")\n    assert d1Title(7) ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A whole-entry write through an identity-lookup result lowers by spreading the
        // identity into the root's key columns.
        Row {
            label: "identity-keyed whole-entry write / inside a mutating region",
            ops: "pub fn d3IdentityWrite(isbn: string, title: string) {\n    transaction {\n        if const found = ^books.byIsbn[isbn] {\n            ^books[found] = Book(title: title, isbn: isbn)\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
    ]
}

/// A return inside an owned transaction region commits, then returns.
fn owned_region_rows() -> Vec<Row> {
    vec![
        // The return value is evaluated pre-commit and the lowering places `TxnCommit`
        // before the `Return`; the verifier proves that ordering.
        Row {
            label: "return inside an owned region (commits, then returns the read value)",
            ops: "pub fn dxSeed(id: int, title: string) {\n    transaction {\n        ^books[id] = Book(title: title, isbn: \"i\")\n    }\n}\n\npub fn returnInsideRegion(id: int): string? {\n    transaction {\n        if const b = ^books[id] {\n            return b.title\n        }\n    }\n    return absent\n}\n\ntest \"in-region return commits and returns the read value\" {\n    dxSeed(8, \"dune\")\n    assert returnInsideRegion(8) ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // Every path returns from inside the `transaction`, so the region has no
        // fall-through: the checker accepts it because the region diverges, and the
        // verifier admits it because no unreachable closing commit is emitted.
        Row {
            label: "all-paths-return region (no fall-through, no closing commit)",
            ops: "pub fn allPaths(id: int, title: string): string? {\n    transaction {\n        ^books[id] = Book(title: title, isbn: \"i\")\n        return ^books[id].title\n    }\n}\n\ntest \"all-paths-return region commits and returns the staged value\" {\n    assert allPaths(11, \"dune\") ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A mutating in-region guard-return: the guard exit commits an empty stage, the
        // fall-through exit commits the staged write. Both exits commit.
        Row {
            label: "mutating in-region guard-return (commits on both exits)",
            ops: "pub fn addOnce(id: int, title: string): bool {\n    transaction {\n        if exists(^books[id]) {\n            return false\n        }\n        ^books[id] = Book(title: title, isbn: \"i\")\n    }\n    return true\n}\n\npub fn titleOf(id: int): string? {\n    return ^books[id].title\n}\n\ntest \"guard-return adds once and rejects a re-add\" {\n    assert addOnce(9, \"dune\")\n    assert not addOnce(9, \"impostor\")\n    assert titleOf(9) ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A `transaction` block with no durable operation is a no-op region the runtime
        // cannot run (it opens no session). The checker refuses it before an image is
        // minted, so checker and verifier agree (a tampered image is still refused at
        // `image.flow`).
        Row {
            label: "empty transaction — no durable operation (checker-rejected)",
            ops: "pub fn emptyRegion() {\n    transaction {\n    }\n}",
            expect: Expect::CheckerRejects {
                code: Code::CheckTransactionEmpty,
            },
        },
        // A field write updates an entry and never creates one, so the inline form with
        // no presence proof is refused at check time and never reaches the verifier.
        Row {
            label: "inline field write without a presence proof (checker-rejected)",
            ops: "pub fn inlineFieldWrite(id: int) {\n    transaction {\n        ^books[id].subtitle = \"x\"\n    }\n}",
            expect: Expect::CheckerRejects {
                code: Code::CheckRequiresPresence,
            },
        },
        // A require failure commits its owner's active region before returning.
        Row {
            label: "require inside an owned region",
            ops: "pub fn addPositive(id: int): Result<bool, string> {\n    transaction {\n        require id > 0 else \"id must be positive\"\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n        return ok(true)\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        // The admitted shape: the guard lives in a helper joining the export's region, so
        // the failure exit is ordinary control flow into the export's committing
        // in-region `return`.
        Row {
            label: "require in a helper joining the region / driver test",
            ops: "fn addChecked(id: int, title: string): Result<bool, string> {\n    require id > 0 else \"id must be positive\"\n    require not exists(^books[id]) else \"already shelved\"\n    ^books[id] = Book(title: title, isbn: \"i\")\n    return ok(true)\n}\n\npub fn shelve(id: int, title: string): Result<bool, string> {\n    transaction {\n        return addChecked(id, title)\n    }\n}\n\npub fn shelvedTitle(id: int): string? {\n    return ^books[id].title\n}\n\ntest \"require guards admit the valid add and reject the invalid ones\" {\n    match shelve(200, \"dune\") {\n        ok(v) => {}\n        err(e) => {\n            assert false\n        }\n    }\n    match shelve(0, \"zero\") {\n        ok(v) => {\n            assert false\n        }\n        err(e) => {\n            assert e == \"id must be positive\"\n        }\n    }\n    match shelve(200, \"impostor\") {\n        ok(v) => {\n            assert false\n        }\n        err(e) => {\n            assert e == \"already shelved\"\n        }\n    }\n    assert shelvedTitle(200) ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // `exists` over a unique index probes for a matching entry without materializing
        // its identity: the same complete-key lookup the `if const` read uses, yielding a
        // bare bool.
        Row {
            label: "exists over a unique index / driver test",
            ops: "pub fn hasIsbn(isbn: string): bool {\n    return exists(^books.byIsbn[isbn])\n}\n\npub fn addBook(id: int, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: isbn)\n    }\n}\n\ntest \"exists over a unique index sees a present and an absent isbn\" {\n    addBook(30, \"i30\")\n    assert hasIsbn(\"i30\")\n    assert not hasIsbn(\"absent-isbn\")\n}",
            expect: Expect::RoundTrips { run: true },
        },
    ]
}

/// Entry-identity operands in every key-path-capturing position.
fn entry_identity_rows() -> Vec<Row> {
    vec![
        // An identity operand spreads into the addressed root's key columns at the one
        // capture point a read-modify-write, an upsert, or a `place` binding evaluates its
        // key-path into slots — the same `IdentityKeyPath` spread the single-emit forms
        // (field read/write, whole-entry read, delete, exists) use.
        Row {
            label: "place bound to an identity operand writes then reads back / driver test",
            ops: "pub fn plWrite(id: int, title: string) {\n    transaction {\n        place p = ^books[Id(^books, id)]\n        p = Book(title: title, isbn: \"i\")\n    }\n}\n\npub fn plTitle(id: int): string? {\n    return ^books[id].title\n}\n\ntest \"place over an identity operand round trips\" {\n    plWrite(20, \"dune\")\n    assert plTitle(20) ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A branch whole-entry write through an identity root-parent: the key-path is
        // [root identity, branch key], and exists/replace/create all key off that one
        // evaluation.
        Row {
            label: "branch whole-entry write through an identity key / driver test",
            ops: "pub fn brWrite(id: int, n: string, text: string) {\n    transaction {\n        ^books[Id(^books, id)].notes[n] = Book.notes(text: text)\n    }\n}\n\npub fn brText(id: int, n: string): string? {\n    return ^books[id].notes[n].text\n}\n\ntest \"branch write through an identity key round trips\" {\n    brWrite(21, \"n1\", \"hello\")\n    assert brText(21, \"n1\") ?? \"none\" == \"hello\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A group-leaf write through an identity key is a whole-group read-modify-write:
        // read, rewrite the leaf, write back, all off the same captured key slots.
        Row {
            label: "group-leaf write through an identity key / driver test",
            ops: "pub fn glWrite(id: int, pages: int) {\n    transaction {\n        place m = ^books[Id(^books, id)]\n        m = Book(title: \"t\", isbn: \"i\")\n        m.details.pages = pages\n    }\n}\n\npub fn glPages(id: int): int? {\n    return ^books[id].details.pages\n}\n\ntest \"group-leaf write through an identity key round trips\" {\n    glWrite(22, 7)\n    assert glPages(22) ?? 0 == 7\n}",
            expect: Expect::RoundTrips { run: true },
        },
        Row {
            label: "group-leaf delete through an identity key / driver test",
            ops: "pub fn gdSet(id: int, pages: int) {\n    transaction {\n        place m = ^books[id]\n        m = Book(title: \"t\", isbn: \"i\")\n        m.details.pages = pages\n    }\n}\n\npub fn gdClear(id: int) {\n    transaction {\n        delete ^books[Id(^books, id)].details.pages\n    }\n}\n\npub fn gdPages(id: int): int? {\n    return ^books[id].details.pages\n}\n\ntest \"group-leaf delete through an identity key round trips\" {\n    gdSet(23, 7)\n    gdClear(23)\n    assert gdPages(23) ?? 0 == 0\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // One identity operand spreads into a composite root's several key columns at the
        // binding, so field reads and writes through the place resolve the root's field
        // off the pre-evaluated address.
        Row {
            label: "composite-root place bound to a single identity operand / driver test",
            ops: "pub fn crIdSeed(s: string, c: string, score: int) {\n    transaction {\n        ^grades[s, c] = Grade(score: score)\n    }\n}\n\npub fn crIdPlaceWrite(s: string, c: string, score: int) {\n    transaction {\n        place g = ^grades[Id(^grades, s, c)]\n        if exists(g) {\n            g.score = score\n        }\n    }\n}\n\npub fn crIdRead(s: string, c: string): int? {\n    return ^grades[s, c].score\n}\n\ntest \"composite-root place over a single identity operand round trips\" {\n    crIdSeed(\"amy\", \"cs\", 90)\n    crIdPlaceWrite(\"amy\", \"cs\", 75)\n    assert crIdRead(\"amy\", \"cs\") ?? 0 == 75\n}",
            expect: Expect::RoundTrips { run: true },
        },
    ]
}

/// A named place or per-iteration pin as a bounded-traversal base.
fn place_base_rows() -> Vec<Row> {
    vec![
        // A place already addresses an entry; `for k in <place>.branch` traverses the branch
        // family beneath it, feeding the place's captured key slots as the traversal's
        // ancestor key-path.
        Row {
            label: "root place branch traversal / driver test",
            ops: "pub fn aAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn aAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn aCountViaPlace(id: int): int {\n    var c = 0\n    place b = ^books[id]\n    for noteId in b.notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"root place is a branch traversal base\" {\n    aAddBook(50)\n    aAddNote(50, \"a\")\n    aAddNote(50, \"b\")\n    assert aCountViaPlace(50) == 2\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A two-binding place base: the pin's key-path is the place's captured root slot
        // followed by each frozen branch key, exercising ancestor-slot capture over a
        // `PlaceKey::Bound` column.
        Row {
            label: "two-binding place base deletes through the pin / driver test",
            ops: "pub fn bAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn bAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn bClearViaPlace(id: int): int {\n    var c = 0\n    transaction {\n        place b = ^books[id]\n        for noteId, note in b.notes at most 100 {\n            c += 1\n            delete note\n        } on more {\n            c = -1\n        }\n    }\n    return c\n}\n\npub fn bCountViaPlace(id: int): int {\n    var c = 0\n    place b = ^books[id]\n    for noteId in b.notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"two-binding place base deletes through the pin\" {\n    bAddBook(60)\n    bAddNote(60, \"a\")\n    bAddNote(60, \"b\")\n    assert bClearViaPlace(60) == 2\n    assert bCountViaPlace(60) == 0\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A per-iteration pin is an inner traversal base: it addresses each frozen entry,
        // and the inner `for` traverses the branch beneath it.
        Row {
            label: "per-iteration pin as an inner traversal base / driver test",
            ops: "pub fn cAddBook(id: int, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: isbn)\n    }\n}\n\npub fn cAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn cCountViaPin(): int {\n    var c = 0\n    for id, book in ^books at most 100 {\n        for noteId in book.notes at most 100 {\n            c += 1\n        } on more {\n            c = -1\n        }\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"a per-iteration pin is an inner traversal base\" {\n    cAddBook(70, \"i70\")\n    cAddNote(70, \"a\")\n    cAddBook(71, \"i71\")\n    cAddNote(71, \"b\")\n    assert cCountViaPin() == 2\n}",
            expect: Expect::RoundTrips { run: true },
        },
    ]
}

/// A named place composing as a base for branch-entry and group-leaf ops.
fn place_composition_rows() -> Vec<Row> {
    vec![
        // Extending a bound place with `.branch[bk]` or `.group.leaf` composes the same
        // operation the inline `^root(k).branch(bk)` / `^root(k).group.leaf` form does,
        // keying off the place's pre-evaluated slots.
        Row {
            label: "root place composes a branch-entry write + branch-field read / driver test",
            ops: "pub fn dAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn dAddNoteVia(id: int, n: string, t: string) {\n    transaction {\n        place b = ^books[id]\n        b.notes[n] = Book.notes(text: t)\n    }\n}\n\npub fn dNoteVia(id: int, n: string): string? {\n    place b = ^books[id]\n    return b.notes[n].text\n}\n\ntest \"root place composes a branch write then reads it back\" {\n    dAddBook(100)\n    dAddNoteVia(100, \"a\", \"hello\")\n    assert dNoteVia(100, \"a\") ?? \"none\" == \"hello\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        Row {
            label: "root place composes a group-leaf write + read / driver test",
            ops: "pub fn eAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn eSetPagesVia(id: int, p: int) {\n    transaction {\n        place b = ^books[id]\n        if exists(b) {\n            b.details.pages = p\n        }\n    }\n}\n\npub fn ePagesVia(id: int): int? {\n    place b = ^books[id]\n    return b.details.pages\n}\n\ntest \"root place composes a group-leaf write then reads it back\" {\n    eAddBook(101)\n    eSetPagesVia(101, 7)\n    assert ePagesVia(101) ?? 0 == 7\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // `exists(place.branch)` is the family-populated probe, not a missing-field error.
        Row {
            label: "exists over a branch family named through a place / driver test",
            ops: "pub fn fAddBook(id: int, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: isbn)\n    }\n}\n\npub fn fAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn fHasNotesVia(id: int): bool {\n    place b = ^books[id]\n    return exists(b.notes)\n}\n\ntest \"exists over a branch family named through a place\" {\n    fAddBook(102, \"i102\")\n    fAddBook(103, \"i103\")\n    fAddNote(102, \"a\")\n    assert fHasNotesVia(102)\n    assert not fHasNotesVia(103)\n}",
            expect: Expect::RoundTrips { run: true },
        },
    ]
}

/// An entry-identity parent as a bounded-traversal or family-probe base.
fn identity_parent_rows() -> Vec<Row> {
    vec![
        // A traversal or family probe whose fixed parent is addressed through an entry
        // identity feeds an identity column as the ancestor key-path. The verifier's
        // ancestor pop re-proves that column's root and scalar exactly as every other
        // key-path pop does.
        Row {
            label: "identity-keyed place base branch traversal / driver test",
            ops: "pub fn iAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn iAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn iCountViaIdPlace(id: int): int {\n    var c = 0\n    place b = ^books[Id(^books, id)]\n    for noteId in b.notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"identity-keyed place base is a branch traversal base\" {\n    iAddBook(80)\n    iAddNote(80, \"a\")\n    iAddNote(80, \"b\")\n    assert iCountViaIdPlace(80) == 2\n}",
            expect: Expect::RoundTrips { run: true },
        },
        Row {
            label: "inline identity-parent branch traversal / driver test",
            ops: "pub fn jAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn jAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn jCountViaInlineId(id: int): int {\n    var c = 0\n    for noteId in ^books[Id(^books, id)].notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"inline identity parent is a branch traversal base\" {\n    jAddBook(81)\n    jAddNote(81, \"a\")\n    jAddNote(81, \"b\")\n    assert jCountViaInlineId(81) == 2\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // The two-binding inline form: the per-iteration pin reuses the identity ancestor
        // slots plus the frozen key to delete through the pin.
        Row {
            label: "inline identity-parent two-binding delete through the pin / driver test",
            ops: "pub fn kAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn kAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn kClearViaInlineId(id: int): int {\n    var c = 0\n    transaction {\n        for noteId, note in ^books[Id(^books, id)].notes at most 100 {\n            c += 1\n            delete note\n        } on more {\n            c = -1\n        }\n    }\n    return c\n}\n\npub fn kCountViaInlineId(id: int): int {\n    var c = 0\n    for noteId in ^books[Id(^books, id)].notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"inline identity parent two-binding deletes through the pin\" {\n    kAddBook(82)\n    kAddNote(82, \"a\")\n    kAddNote(82, \"b\")\n    assert kClearViaInlineId(82) == 2\n    assert kCountViaInlineId(82) == 0\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // The family probe emits only the identity ancestor key-path before
        // `DurFamilyExists`, so its ancestor pop re-proves the same identity column the
        // traversal pop does.
        Row {
            label: "family-populated probe under an identity parent / driver test",
            ops: "pub fn mAddBook(id: int, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: isbn)\n    }\n}\n\npub fn mAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn mHasNotes(id: int): bool {\n    return exists(^books[Id(^books, id)].notes)\n}\n\ntest \"family probe under an identity parent sees present and empty\" {\n    mAddBook(83, \"i83\")\n    mAddBook(84, \"i84\")\n    mAddNote(83, \"a\")\n    assert mHasNotes(83)\n    assert not mHasNotes(84)\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A presence-dominated sparse field set through an identity-keyed place reads its
        // key-path from the place's pre-evaluated slots, which carry the identity column;
        // the set-sparse-present slot-type check re-proves it as the stack key-path pop does.
        Row {
            label: "strict present sparse set through an identity-keyed place / driver test",
            ops: "pub fn spSeed(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn spSetVia(id: int, s: string): bool {\n    transaction {\n        place b = ^books[Id(^books, id)]\n        if exists(b) {\n            b.subtitle = s\n            return true\n        }\n    }\n    return false\n}\n\npub fn spSubtitle(id: int): string? {\n    return ^books[id].subtitle\n}\n\ntest \"strict present sparse set through an identity place round trips\" {\n    spSeed(90)\n    assert spSetVia(90, \"x\")\n    assert spSubtitle(90) ?? \"none\" == \"x\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
    ]
}

/// Two durable fields that share one enum type, and so one durable identity.
fn shared_enum_rows() -> Vec<Row> {
    vec![
        // `glucose` and `lactate` are both `Option<int>`, so they share one enum durable
        // identity; the verifier reads the reuse as one per-declaration claim rather than
        // a duplicate ledger id.
        Row {
            label: "two Option<int> fields of one enum type round trip / driver test",
            ops: "pub fn setReadings(id: int, g: int, l: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\", glucose: some(g), lactate: some(l))\n    }\n}\n\npub fn glucoseVal(id: int): int {\n    if const cell = ^books[id].glucose {\n        match cell {\n            some(v) => return v\n            none => return -1\n        }\n    }\n    return -2\n}\n\npub fn lactateVal(id: int): int {\n    if const cell = ^books[id].lactate {\n        match cell {\n            some(v) => return v\n            none => return -1\n        }\n    }\n    return -2\n}\n\ntest \"two fields of one enum type round trip\" {\n    setReadings(1, 95, 12)\n    assert glucoseVal(1) == 95\n    assert lactateVal(1) == 12\n}",
            expect: Expect::RoundTrips { run: true },
        },
    ]
}

/// Run every `test` entry in a verified image through the ephemeral kernel and require
/// each to run without an artifact rejection, a mint failure, or a runtime fault — the
/// run-side half of the agreement invariant.
fn run_all_tests(label: &str, image: &VerifiedImage) {
    assert!(
        !image.test_entries().is_empty(),
        "{label}: a run-row must carry at least one test entry",
    );
    let prepared = prepare(image.clone());
    for (index, entry) in image.test_entries().iter().enumerate() {
        assert!(
            !image
                .function(entry.func())
                .expect("test function belongs to image")
                .demand()
                .is_empty(),
            "{label}: a run-row test entry is durable",
        );
        let test = fresh_test(&prepared, index).expect("the entry index is in the image");
        match run_test(test) {
            DurableRun::Ran(Ok(_)) => {}
            DurableRun::Ran(Err(fault)) => {
                panic!(
                    "{label}: test `{}` faulted at run: {}",
                    entry.name(),
                    fault.code().as_str()
                )
            }
            DurableRun::Parked => {
                panic!(
                    "{label}: test `{}` parked — the round trip is not executable",
                    entry.name()
                )
            }
            DurableRun::Failed(code) => {
                panic!(
                    "{label}: test `{}` failed to mint its attachment: {}",
                    entry.name(),
                    code.as_str()
                )
            }
        }
    }
}

/// The standing agreement gate. Each row's whole-pipeline verdict must match its
/// pinned expectation exactly; the known-divergent ledger is additionally
/// size-pinned so the divergence set cannot grow unremarked.
#[test]
fn checker_acceptance_implies_verification_over_the_composition_matrix() {
    let mut known_divergent = 0usize;
    let mut checker_rejected = 0usize;

    for row in matrix() {
        let stage = pipeline(row.ops);
        match (&row.expect, stage) {
            (Expect::RoundTrips { run }, Stage::Verified(image)) => {
                if *run {
                    run_all_tests(row.label, &image);
                }
            }
            (Expect::RoundTrips { .. }, Stage::VerifyRejected { code, detail }) => panic!(
                "AGREEMENT BROKEN — `{}` is checker-accepted but the verifier rejected it \
                 ({}: {detail}). A round trip regressed into a divergence.",
                row.label,
                code.as_str()
            ),
            (Expect::RoundTrips { .. }, Stage::CheckerRejected(code)) => panic!(
                "`{}` was expected to round-trip but the checker refused it ({}).",
                row.label,
                code.as_str()
            ),
            (
                Expect::KnownDivergent { code, detail },
                Stage::VerifyRejected {
                    code: got_code,
                    detail: got_detail,
                },
            ) => {
                assert_eq!(*code, got_code, "{}: divergence code drifted", row.label);
                assert_eq!(
                    *detail, got_detail,
                    "{}: divergence detail drifted",
                    row.label
                );
                known_divergent += 1;
            }
            (Expect::KnownDivergent { code, detail }, Stage::Verified(_)) => panic!(
                "LEDGER STALE — `{}` now verifies; the {} divergence (\"{detail}\") is fixed. \
                 Move this row to Expect::RoundTrips so the gate enforces it.",
                row.label,
                code.as_str()
            ),
            (Expect::KnownDivergent { .. }, Stage::CheckerRejected(code)) => panic!(
                "`{}` was a checker-accept/verify-reject divergence but the checker now refuses it \
                 ({}); re-classify the row to Expect::CheckerRejects.",
                row.label,
                code.as_str()
            ),
            (Expect::CheckerRejects { code }, Stage::CheckerRejected(got_code)) => {
                assert_eq!(*code, got_code, "{}: check-time code drifted", row.label);
                checker_rejected += 1;
            }
            (Expect::CheckerRejects { code }, Stage::Verified(_)) => panic!(
                "LEDGER STALE — `{}` now verifies; the checker no longer refuses it ({}). \
                 A promoted diagnostic regressed — restore the check or move the row.",
                row.label,
                code.as_str()
            ),
            (
                Expect::CheckerRejects { code },
                Stage::VerifyRejected {
                    code: got_code,
                    detail,
                },
            ) => panic!(
                "`{}` was expected to be refused at check time ({}) but the checker accepted it \
                 and the verifier rejected it ({}: {detail}) — the check-time promotion \
                 regressed into a divergence.",
                row.label,
                code.as_str(),
                got_code.as_str()
            ),
        }
    }

    // A new divergence added without a ledger row fails an individual row above; these
    // counts fail if a checker-rejected row silently changes verdict.
    assert_eq!(known_divergent, 0, "the divergence ledger is empty");
    assert_eq!(
        checker_rejected, 2,
        "expected exactly the empty-transaction and unproven-field-write \
         check-time rejections",
    );
}

#[test]
fn nominal_field_index_binding_is_refused_before_image_publication() {
    let Err(diagnostics) = Project::single(NOMINAL_INDEX_SCHEMA)
        .ids(NOMINAL_INDEX_IDS)
        .try_image()
    else {
        panic!("a nominal-bearing indexed binding must report a source diagnostic");
    };
    assert_eq!(diagnostics.len(), 1, "{:?}", diagnostics.all());
    let diagnostic = diagnostics.only("check.unsupported");
    assert_eq!(diagnostic.code(), marrow_codes::Code::CheckUnsupported);
    assert_eq!(diagnostic.file().as_str(), "src/main.mw");
    assert_eq!((diagnostic.line(), diagnostic.column()), (8, 1));
}

/// The invocation-boundary isolation law: three exports run in sequence against one
/// persistent attachment, as a terminal drives them. A faulting export rolls its own
/// staged write back without disturbing an earlier commit — each call is its own
/// boundary.
#[test]
fn a_faulting_export_invocation_rolls_back_without_disturbing_a_prior_commit() {
    let ops = "pub fn shelve(id: int, title: string, isbn: string) {\n    \
             transaction {\n        ^books[id] = Book(title: title, isbn: isbn)\n    }\n}\n\n\
         pub fn badUpdate(id: int, divisor: int) {\n    transaction {\n        \
             place m = ^books[id]\n        if exists(m) {\n            \
             m.title = \"changed\"\n            \
             m.details.pages = 100 / divisor\n        }\n    }\n}\n\n\
         pub fn titleOf(id: int): string? {\n    return ^books[id].title\n}";
    let mut session = Project::single(&format!("{SCHEMA}\n{ops}"))
        .ids(IDS)
        .session();

    assert!(matches!(
        session.try_call(
            "shelve",
            vec![
                Value::Int(1),
                Value::Text("first".into()),
                Value::Text("i1".into())
            ],
        ),
        CallOutcome::Value(_),
    ));

    assert_eq!(
        session
            .try_call("badUpdate", vec![Value::Int(1), Value::Int(0)])
            .fault_code(),
        Some(Code::RunDivideByZero),
        "the fault reached the caller"
    );

    assert_eq!(
        session.call("titleOf", vec![Value::Int(1)]),
        Some(Value::Optional(Some(Box::new(Value::Text("first".into()))))),
        "the rolled-back write left no trace"
    );
}
