//! The admitted-subset AGREEMENT GATE (RV01).
//!
//! The durable round trip is spellable only if the checker and the independent
//! verifier agree on the admitted subset: a program the checker accepts must
//! also verify (checker-accept ⇒ verify-accept) and, when it carries a driving
//! `test`, run without an artifact rejection. Five checker/verifier divergences
//! in this family were found by the 2026-07-18 review of record; this gate turns
//! that family into a standing enumeration so a divergence becomes a failing
//! test rather than a review finding.
//!
//! The gate enumerates a bounded matrix of durable op forms × contexts — no
//! unbounded fuzzing — and pins each composition's whole-pipeline verdict
//! (capture → compile → verify → run). A composition whose intended round trip
//! is not yet whole is recorded as an EXPLICIT ledger entry with its exact
//! current code, so the remaining divergence set is visible rather than skipped.
//! When a fix lands, its row moves to [`Expect::RoundTrips`]; if the fix lands
//! without moving the row, the gate fails on the changed verdict — the ledger
//! cannot silently drift.
//!
//! RV01 closed the three review-of-record defects: D1 (a test body drives a mutating
//! export, each call its own invocation boundary), D2 (a whole-entry read inside a
//! read-only region is admitted), and D3 (a whole-entry write through an identity
//! lookup lowers). DX01 then made a `return` inside an owned region a commit site, so
//! the return-inside-region row is a round trip too. TX02 promoted the last divergence —
//! an empty (no-op) `transaction`, which the verifier refuses — to a check-time
//! diagnostic, so the checker now rejects it before an image is minted and the
//! divergence ledger is empty. Its row is a `CheckerRejects` control. The correct-
//! rollback journey below locks the invocation-boundary isolation law: a faulting export
//! invocation rolls back without disturbing a prior committed one.

use marrow_verify::{TestKind, VerifiedImage};
use marrow_vm::{
    DurableRun, Ephemeral, Value, mint_ephemeral, run_driver_test, run_durable_test, run_export,
};

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

/// A declaration-only agreement sibling for the nominal-field managed-index
/// boundary. It is intentionally separate from [`SCHEMA`] and [`IDS`]: nominal-root
/// operations remain parked, while checker admission must still imply that the
/// independent verifier accepts the declaration graph.
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
    CheckerRejected(&'static str),
    /// The checker accepted, but the independent verifier rejected the image
    /// (`image.*`) with this code and detail — a checker/verifier divergence.
    VerifyRejected {
        code: &'static str,
        detail: &'static str,
    },
    /// The checker accepted and the verifier sealed the image.
    Verified(Box<VerifiedImage>),
}

/// Drive one composition through the production pipeline: capture the schema
/// plus the appended operations, compile *with tests* (so a test-body driver is
/// part of the image), and verify. The verifier reconstructs demand and the
/// transaction/flow laws from the image alone, so this reads the true
/// checker⇒verifier relationship, not a compiler self-report.
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
                .code,
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
    /// A recorded checker/verifier divergence: the checker accepts but the
    /// verifier rejects. The exact current code and detail are pinned so a fix
    /// that changes the verdict forces this row to move to `RoundTrips`.
    ///
    /// The ledger is empty after TX02 promoted the last divergence (the empty
    /// transaction) to a check-time diagnostic; the variant is retained as the
    /// mechanism a future divergence is recorded through, so a regression becomes a
    /// failing row rather than a review finding.
    #[allow(dead_code)]
    KnownDivergent {
        code: &'static str,
        detail: &'static str,
    },
    /// The checker rejects the composition at check time, so it never reaches the
    /// verifier — checker-accept ⇒ verify holds vacuously and the two agree. The exact
    /// `check.*` code is pinned so a change to the verdict forces this row to move.
    /// A former `KnownDivergent` row lands here once its divergence is promoted to a
    /// source-facing diagnostic (TX02 moved the empty-transaction row here).
    CheckerRejects { code: &'static str },
}

struct Row {
    label: &'static str,
    ops: &'static str,
    expect: Expect,
}

/// The bounded composition matrix: durable op forms (whole-entry read/write,
/// field read/write, group read/write, branch read/write, index lookup,
/// identity write, bounded traversal) × contexts (outside a transaction, inside
/// a mutating region, inside a test body directly, via an export call from a
/// test body). Positive controls pin the executable subset; the three review-of-
/// record defects pin the current divergence set.
fn matrix() -> Vec<Row> {
    vec![
        // ---- Positive controls: the admitted executable subset. ----
        Row {
            label: "whole-entry read / outside a transaction",
            ops: "pub fn weReadOut(id: int): string? {\n    if const b = ^books[id] {\n        return b.title\n    }\n    return absent\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "whole-entry read / inside a mutating region (read-modify-write)",
            ops: "pub fn weReadTxn(id: int) {\n    transaction {\n        if const b = ^books[id] {\n            ^books[id].subtitle = b.title\n        }\n    }\n}",
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
            ops: "pub fn fieldWrite(id: int) {\n    transaction {\n        ^books[id].subtitle = \"x\"\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "group-leaf write / inside a mutating region",
            ops: "pub fn groupWrite(id: int) {\n    transaction {\n        ^books[id].details.pages = 3\n    }\n}",
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
            ops: "pub fn identityFieldWrite(isbn: string) {\n    transaction {\n        if const found = ^books.byIsbn[isbn] {\n            ^books[found].subtitle = \"x\"\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "bounded index scan / outside a transaction",
            ops: "pub fn scan(t: string): int {\n    var n = 0\n    for id in ^books.byShelf[t] at most 10 {\n        n += 1\n    } on more {\n        n = -1\n    }\n    return n\n}",
            expect: Expect::RoundTrips { run: false },
        },
        Row {
            label: "field write + read-back / in a test body directly",
            ops: "test \"direct field round trip\" {\n    ^books[1].subtitle = \"x\"\n    assert ^books[1].subtitle ?? \"n\" == \"x\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        Row {
            label: "whole-entry write + read-back / in a test body directly",
            ops: "test \"direct whole-entry round trip\" {\n    ^books[1] = Book(title: \"dune\", isbn: \"i1\")\n    if const b = ^books[1] {\n        assert b.title == \"dune\"\n    } else {\n        assert false\n    }\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A composite-key root place carries several key slots but is still a root, so a
        // field read (`g.score`) and a symmetric field write (`g.score = s`) through it both
        // resolve the root's field — not a branch record. The node kind is recorded at the
        // binding from the canonical resolved durable node, independent of key-operand count.
        Row {
            label: "composite-root place field read + write / read outside, write inside a region",
            ops: "pub fn crPlaceRead(student: string, course: string): int? {\n    place g = ^grades[student, course]\n    return g.score\n}\n\npub fn crPlaceWrite(student: string, course: string, score: int) {\n    transaction {\n        place g = ^grades[student, course]\n        g.score = score\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        // Driven end to end: a seed export writes a composite-root entry, a composite-root
        // place field write updates its `score`, and a composite-root place field read reads
        // the new value back — each export call its own invocation boundary, so the write and
        // the read-back both resolve the root field through their own place binding.
        Row {
            label: "composite-root place field write round trip / driver test",
            ops: "pub fn crSeed(student: string, course: string, score: int) {\n    transaction {\n        ^grades[student, course] = Grade(score: score)\n    }\n}\n\npub fn crWriteVia(student: string, course: string, score: int) {\n    transaction {\n        place g = ^grades[student, course]\n        g.score = score\n    }\n}\n\npub fn crReadVia(student: string, course: string): int? {\n    place g = ^grades[student, course]\n    return g.score\n}\n\ntest \"composite-root place writes then reads a score back\" {\n    crSeed(\"amy\", \"cs\", 90)\n    crWriteVia(\"amy\", \"cs\", 75)\n    assert crReadVia(\"amy\", \"cs\") ?? 0 == 75\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // ---- Resource values at function boundaries. ----
        // A whole-entry read materializes a resource value that is passed to a helper
        // function, reworked on a local copy, returned, and written back inside the
        // owned region — the copy-part-and-save-back journey through ordinary
        // functions. The verifier reconstructs the boundary types from the image, so a
        // sealed image proves the resource value crosses the call by value.
        Row {
            label: "resource value read -> helper param -> return -> whole-entry write / in a region",
            ops: "fn rework(b: Book): Book {\n    var working = b\n    working.subtitle = working.title\n    return working\n}\n\npub fn revise(id: int) {\n    transaction {\n        if const current = ^books[id] {\n            ^books[id] = rework(current)\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        // The same journey run end to end: a driver test seeds an entry through an
        // export, reworks it through a helper-returning export, and reads the reworked
        // field back — each export call its own invocation boundary.
        Row {
            label: "resource value round trip through a helper / driver test",
            ops: "fn withSubtitle(b: Book, s: string): Book {\n    var working = b\n    working.subtitle = s\n    return working\n}\n\npub fn seed(id: int, title: string, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: title, isbn: isbn)\n    }\n}\n\npub fn revise(id: int, s: string) {\n    transaction {\n        if const current = ^books[id] {\n            ^books[id] = withSubtitle(current, s)\n        }\n    }\n}\n\npub fn subtitleOf(id: int): string? {\n    return ^books[id].subtitle\n}\n\ntest \"resource value crosses a helper and writes back\" {\n    seed(4, \"dune\", \"i4\")\n    revise(4, \"revised\")\n    assert subtitleOf(4) ?? \"none\" == \"revised\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // ---- The review-of-record round trip, now whole (RV01 closes D1/D2/D3). ----
        // D2 (closed): a whole-entry read inside a region the export owns is coherent.
        // The owner lattice now runs for any export that owns a transaction, so a
        // read-only region reads inside and returns the captured value after the block.
        Row {
            label: "D2: whole-entry read / inside a read-only region (captured, returned after)",
            ops: "pub fn d2ReadOnlyRegion(id: int): string? {\n    var out: string? = absent\n    transaction {\n        if const b = ^books[id] {\n            out = b.title\n        }\n    }\n    return out\n}",
            expect: Expect::RoundTrips { run: false },
        },
        // D1 (closed): a test body drives a mutating export, then reads back through a
        // reading export — each call its own invocation boundary. The round trip runs.
        Row {
            label: "D1: driver test — mutating export call then read-back export",
            ops: "pub fn d1Add(id: int, title: string) {\n    transaction {\n        ^books[id] = Book(title: title, isbn: \"i\")\n    }\n}\n\npub fn d1Title(id: int): string? {\n    return ^books[id].title\n}\n\ntest \"driver adds through an export and reads it back\" {\n    d1Add(7, \"dune\")\n    assert d1Title(7) ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // D3 (closed): a whole-entry write through an identity-lookup result. A field
        // write through the same lookup already round-trips; the whole-entry form now
        // lowers by spreading the identity into the root's key columns.
        Row {
            label: "D3: identity-keyed whole-entry write / inside a mutating region",
            ops: "pub fn d3IdentityWrite(isbn: string, title: string) {\n    transaction {\n        if const found = ^books.byIsbn[isbn] {\n            ^books[found] = Book(title: title, isbn: isbn)\n        }\n    }\n}",
            expect: Expect::RoundTrips { run: false },
        },
        // ---- DX01: a return inside an owned region commits, then returns. ----
        // The in-region `return b.title` commits the region's staged writes (here a
        // read-only region, so nothing is staged), evaluates the return value
        // pre-commit, then returns it. The lowering places `TxnCommit` before the
        // `Return`; the verifier proves that ordering, so checker and verifier agree
        // and the round trip runs. Driven end to end: a seed export commits an entry,
        // then the in-region return reads its title back.
        Row {
            label: "DX01: return inside an owned region (commits, then returns the read value)",
            ops: "pub fn dxSeed(id: int, title: string) {\n    transaction {\n        ^books[id] = Book(title: title, isbn: \"i\")\n    }\n}\n\npub fn returnInsideRegion(id: int): string? {\n    transaction {\n        if const b = ^books[id] {\n            return b.title\n        }\n    }\n    return absent\n}\n\ntest \"in-region return commits and returns the read value\" {\n    dxSeed(8, \"dune\")\n    assert returnInsideRegion(8) ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // DX01: an all-paths-return region — every path returns from inside the
        // `transaction`, so the region has no fall-through. The checker must accept it
        // (the region diverges, so the function returns on every path) and the verifier
        // must admit it (no unreachable closing commit is emitted). This is the natural
        // `transaction { ...; return x }` shape; it nets the checker-accept ⇒ verify
        // class for a region with no trailing return. Driven end to end.
        Row {
            label: "DX01: all-paths-return region (no fall-through, no closing commit)",
            ops: "pub fn allPaths(id: int, title: string): string? {\n    transaction {\n        ^books[id] = Book(title: title, isbn: \"i\")\n        return ^books[id].title\n    }\n}\n\ntest \"all-paths-return region commits and returns the staged value\" {\n    assert allPaths(11, \"dune\") ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // DX01: a mutating in-region guard-return — the shape the Workshop app teaches.
        // The present path returns `false` at the guard (committing an empty stage); the
        // absent path stages the write and returns `true` at the closing brace. Driven:
        // a first add commits, a re-add is rejected without disturbing the first entry.
        Row {
            label: "DX01: mutating in-region guard-return (commits on both exits)",
            ops: "pub fn addOnce(id: int, title: string): bool {\n    transaction {\n        if exists(^books[id]) {\n            return false\n        }\n        ^books[id] = Book(title: title, isbn: \"i\")\n    }\n    return true\n}\n\npub fn titleOf(id: int): string? {\n    return ^books[id].title\n}\n\ntest \"guard-return adds once and rejects a re-add\" {\n    assert addOnce(9, \"dune\")\n    assert not addOnce(9, \"impostor\")\n    assert titleOf(9) ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A `transaction` block with no durable operation is a no-op region the runtime
        // cannot run (it opens no session). TX02 promoted this law to a check-time
        // diagnostic, so the checker now refuses it before an image is minted — the
        // former checker-accept/verify-reject divergence is closed and the checker and
        // verifier agree (a tampered image is still refused at `image.flow`).
        Row {
            label: "empty transaction — no durable operation (checker-rejected since TX02)",
            ops: "pub fn emptyRegion() {\n    transaction {\n    }\n}",
            expect: Expect::CheckerRejects {
                code: "check.transaction_empty",
            },
        },
        // ---- DX05: `exists` over a unique index — the presence half of the lookup. ----
        // `exists(^books.byIsbn[isbn])` probes the unique index for a matching entry
        // without materializing its identity: the same complete-key lookup the `if const`
        // read uses, yielding a bare bool. Driven end to end — a present and an absent isbn.
        Row {
            label: "DX05: exists over a unique index / driver test",
            ops: "pub fn hasIsbn(isbn: string): bool {\n    return exists(^books.byIsbn[isbn])\n}\n\npub fn addBook(id: int, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: isbn)\n    }\n}\n\ntest \"exists over a unique index sees a present and an absent isbn\" {\n    addBook(30, \"i30\")\n    assert hasIsbn(\"i30\")\n    assert not hasIsbn(\"absent-isbn\")\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // ---- IDK01: entry-identity operands in every key-path-capturing position. ----
        // An identity operand spreads into the addressed root's key columns at the one
        // capture point a read-modify-write, an upsert, or a `place` binding evaluates its
        // key-path into slots — the same `IdentityKeyPath` spread the single-emit forms
        // (field read/write, whole-entry read, delete, exists) already use. These rows drive
        // each formerly-refused capturing position end to end.
        //
        // A `place` bound to an identity operand: the identity is captured into the root's
        // key slots at the binding, so a whole-entry write and a field read through the place
        // key off the one pre-evaluated address (durable-places.md §Named Places).
        Row {
            label: "IDK01: place bound to an identity operand writes then reads back / driver test",
            ops: "pub fn plWrite(id: int, title: string) {\n    transaction {\n        place p = ^books[Id(^books, id)]\n        p = Book(title: title, isbn: \"i\")\n    }\n}\n\npub fn plTitle(id: int): string? {\n    return ^books[id].title\n}\n\ntest \"place over an identity operand round trips\" {\n    plWrite(20, \"dune\")\n    assert plTitle(20) ?? \"none\" == \"dune\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A branch whole-entry write through an identity root-parent: the key-path is
        // [root identity, branch key]. The upsert captures the identity into the root's
        // columns and the branch key into its own slot, then keys exists/replace/create off
        // the same evaluation.
        Row {
            label: "IDK01: branch whole-entry write through an identity key / driver test",
            ops: "pub fn brWrite(id: int, n: string, text: string) {\n    transaction {\n        ^books[Id(^books, id)].notes[n] = Book.notes(text: text)\n    }\n}\n\npub fn brText(id: int, n: string): string? {\n    return ^books[id].notes[n].text\n}\n\ntest \"branch write through an identity key round trips\" {\n    brWrite(21, \"n1\", \"hello\")\n    assert brText(21, \"n1\") ?? \"none\" == \"hello\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A group-leaf write through an identity key (formerly deferred): the whole-group
        // read-modify-write captures the identity into the root's key columns, reads the
        // group, rewrites the leaf, and writes the group back off the same slots.
        Row {
            label: "IDK01: group-leaf write through an identity key / driver test",
            ops: "pub fn glWrite(id: int, pages: int) {\n    transaction {\n        ^books[Id(^books, id)] = Book(title: \"t\", isbn: \"i\")\n        ^books[Id(^books, id)].details.pages = pages\n    }\n}\n\npub fn glPages(id: int): int? {\n    return ^books[id].details.pages\n}\n\ntest \"group-leaf write through an identity key round trips\" {\n    glWrite(22, 7)\n    assert glPages(22) ?? 0 == 7\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A group-leaf delete through an identity key: the same read-modify-write, clearing
        // the leaf. The sibling of the write above, from the same capturing helper.
        Row {
            label: "IDK01: group-leaf delete through an identity key / driver test",
            ops: "pub fn gdSet(id: int, pages: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n        ^books[id].details.pages = pages\n    }\n}\n\npub fn gdClear(id: int) {\n    transaction {\n        delete ^books[Id(^books, id)].details.pages\n    }\n}\n\npub fn gdPages(id: int): int? {\n    return ^books[id].details.pages\n}\n\ntest \"group-leaf delete through an identity key round trips\" {\n    gdSet(23, 7)\n    gdClear(23)\n    assert gdPages(23) ?? 0 == 0\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A composite-root `place` bound to a single identity operand: the identity spreads
        // into the composite root's several key columns at the binding (the PL01 provenance —
        // a place carries several key slots yet is still a root), so a field write and read
        // through the place resolve the root's field off the pre-evaluated address.
        Row {
            label: "IDK01: composite-root place bound to a single identity operand / driver test",
            ops: "pub fn crIdSeed(s: string, c: string, score: int) {\n    transaction {\n        ^grades[s, c] = Grade(score: score)\n    }\n}\n\npub fn crIdPlaceWrite(s: string, c: string, score: int) {\n    transaction {\n        place g = ^grades[Id(^grades, s, c)]\n        g.score = score\n    }\n}\n\npub fn crIdRead(s: string, c: string): int? {\n    return ^grades[s, c].score\n}\n\ntest \"composite-root place over a single identity operand round trips\" {\n    crIdSeed(\"amy\", \"cs\", 90)\n    crIdPlaceWrite(\"amy\", \"cs\", 75)\n    assert crIdRead(\"amy\", \"cs\") ?? 0 == 75\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // ---- DX02: a named place or per-iteration pin as a bounded-traversal base. ----
        // A place already addresses an entry; `for k in <place>.branch` traverses the branch
        // family beneath it, feeding the place's captured key slots as the traversal's
        // ancestor key-path. Each row drives the branch traversal through a place/pin base
        // end to end.
        //
        // A root place is a branch traversal base: `place b = ^books[id]; for noteId in
        // b.notes` counts the entries under the fixed parent the place binds.
        Row {
            label: "DX02: root place branch traversal / driver test",
            ops: "pub fn aAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn aAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn aCountViaPlace(id: int): int {\n    var c = 0\n    place b = ^books[id]\n    for noteId in b.notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"root place is a branch traversal base\" {\n    aAddBook(50)\n    aAddNote(50, \"a\")\n    aAddNote(50, \"b\")\n    assert aCountViaPlace(50) == 2\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A two-binding place base: the pin's key-path is the place's captured root slot
        // followed by each frozen note key, so `delete note` erases through the pin. This
        // drives the ancestor-slot capture over a `PlaceKey::Bound` column.
        Row {
            label: "DX02: two-binding place base deletes through the pin / driver test",
            ops: "pub fn bAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn bAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn bClearViaPlace(id: int): int {\n    var c = 0\n    transaction {\n        place b = ^books[id]\n        for noteId, note in b.notes at most 100 {\n            c += 1\n            delete note\n        } on more {\n            c = -1\n        }\n    }\n    return c\n}\n\npub fn bCountViaPlace(id: int): int {\n    var c = 0\n    place b = ^books[id]\n    for noteId in b.notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"two-binding place base deletes through the pin\" {\n    bAddBook(60)\n    bAddNote(60, \"a\")\n    bAddNote(60, \"b\")\n    assert bClearViaPlace(60) == 2\n    assert bCountViaPlace(60) == 0\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A per-iteration pin is an inner traversal base: the outer pin `book` addresses each
        // frozen entry, and `for noteId in book.notes` traverses the branch beneath it.
        Row {
            label: "DX02: per-iteration pin as an inner traversal base / driver test",
            ops: "pub fn cAddBook(id: int, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: isbn)\n    }\n}\n\npub fn cAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn cCountViaPin(): int {\n    var c = 0\n    for id, book in ^books at most 100 {\n        for noteId in book.notes at most 100 {\n            c += 1\n        } on more {\n            c = -1\n        }\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"a per-iteration pin is an inner traversal base\" {\n    cAddBook(70, \"i70\")\n    cAddNote(70, \"a\")\n    cAddBook(71, \"i71\")\n    cAddNote(71, \"b\")\n    assert cCountViaPin() == 2\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // ---- DX06: a named place composes as a base for branch-entry and group-leaf ops. ----
        // A bound place already addresses an entry; extending it with `.branch[bk]` or
        // `.group.leaf` composes the same operation an inline `^root(k).branch(bk)` /
        // `^root(k).group.leaf` does, keying off the place's pre-evaluated slots. Each row
        // drives a formerly-refused composition end to end, pinning checker-accept ⇒ verify.
        //
        // A root place composes a whole branch-entry write and a branch-field read.
        Row {
            label: "DX06: root place composes a branch-entry write + branch-field read / driver test",
            ops: "pub fn dAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn dAddNoteVia(id: int, n: string, t: string) {\n    transaction {\n        place b = ^books[id]\n        b.notes[n] = Book.notes(text: t)\n    }\n}\n\npub fn dNoteVia(id: int, n: string): string? {\n    place b = ^books[id]\n    return b.notes[n].text\n}\n\ntest \"root place composes a branch write then reads it back\" {\n    dAddBook(100)\n    dAddNoteVia(100, \"a\", \"hello\")\n    assert dNoteVia(100, \"a\") ?? \"none\" == \"hello\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // A root place composes a group-leaf write and read (whole-group read-modify-write).
        Row {
            label: "DX06: root place composes a group-leaf write + read / driver test",
            ops: "pub fn eAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn eSetPagesVia(id: int, p: int) {\n    transaction {\n        place b = ^books[id]\n        b.details.pages = p\n    }\n}\n\npub fn ePagesVia(id: int): int? {\n    place b = ^books[id]\n    return b.details.pages\n}\n\ntest \"root place composes a group-leaf write then reads it back\" {\n    eAddBook(101)\n    eSetPagesVia(101, 7)\n    assert ePagesVia(101) ?? 0 == 7\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // `exists(place.branch)` is the family-populated probe, not a missing-field error.
        Row {
            label: "DX06: exists over a branch family named through a place / driver test",
            ops: "pub fn fAddBook(id: int, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: isbn)\n    }\n}\n\npub fn fAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn fHasNotesVia(id: int): bool {\n    place b = ^books[id]\n    return exists(b.notes)\n}\n\ntest \"exists over a branch family named through a place\" {\n    fAddBook(102, \"i102\")\n    fAddBook(103, \"i103\")\n    fAddNote(102, \"a\")\n    assert fHasNotesVia(102)\n    assert not fHasNotesVia(103)\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // ---- IDTRAV01: an entry-identity parent as a bounded-traversal / family-probe base. ----
        // A traversal or family probe whose fixed parent is addressed through an entry
        // identity (`^root[Id(…)].branch`, or an identity-keyed place base) feeds an identity
        // column as the ancestor key-path. The verifier's ancestor pop re-proves that column's
        // root and scalar exactly as every other key-path pop does, so each round trip is whole.
        //
        // An identity-keyed place base: `place b = ^books[Id(^books, id)]; for noteId in
        // b.notes` traverses the branch beneath the entry the identity addresses.
        Row {
            label: "IDTRAV01: identity-keyed place base branch traversal / driver test",
            ops: "pub fn iAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn iAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn iCountViaIdPlace(id: int): int {\n    var c = 0\n    place b = ^books[Id(^books, id)]\n    for noteId in b.notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"identity-keyed place base is a branch traversal base\" {\n    iAddBook(80)\n    iAddNote(80, \"a\")\n    iAddNote(80, \"b\")\n    assert iCountViaIdPlace(80) == 2\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // The inline sibling: `for noteId in ^books[Id(^books, id)].notes` supplies the one
        // identity operand as the branch traversal's ancestor key-path.
        Row {
            label: "IDTRAV01: inline identity-parent branch traversal / driver test",
            ops: "pub fn jAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn jAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn jCountViaInlineId(id: int): int {\n    var c = 0\n    for noteId in ^books[Id(^books, id)].notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"inline identity parent is a branch traversal base\" {\n    jAddBook(81)\n    jAddNote(81, \"a\")\n    jAddNote(81, \"b\")\n    assert jCountViaInlineId(81) == 2\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // The two-binding inline form: the per-iteration pin `note` reuses the identity
        // ancestor slots plus the frozen key to delete each note through the pin.
        Row {
            label: "IDTRAV01: inline identity-parent two-binding delete through the pin / driver test",
            ops: "pub fn kAddBook(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn kAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn kClearViaInlineId(id: int): int {\n    var c = 0\n    transaction {\n        for noteId, note in ^books[Id(^books, id)].notes at most 100 {\n            c += 1\n            delete note\n        } on more {\n            c = -1\n        }\n    }\n    return c\n}\n\npub fn kCountViaInlineId(id: int): int {\n    var c = 0\n    for noteId in ^books[Id(^books, id)].notes at most 100 {\n        c += 1\n    } on more {\n        c = -1\n    }\n    return c\n}\n\ntest \"inline identity parent two-binding deletes through the pin\" {\n    kAddBook(82)\n    kAddNote(82, \"a\")\n    kAddNote(82, \"b\")\n    assert kClearViaInlineId(82) == 2\n    assert kCountViaInlineId(82) == 0\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // The family-probe sibling: `exists(^books[Id(^books, id)].notes)` emits only the
        // identity ancestor key-path before `DurFamilyExists`, so its ancestor pop re-proves
        // the same identity column the traversal pop does.
        Row {
            label: "IDTRAV01: family-populated probe under an identity parent / driver test",
            ops: "pub fn mAddBook(id: int, isbn: string) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: isbn)\n    }\n}\n\npub fn mAddNote(id: int, n: string) {\n    transaction {\n        ^books[id].notes[n] = Book.notes(text: \"x\")\n    }\n}\n\npub fn mHasNotes(id: int): bool {\n    return exists(^books[Id(^books, id)].notes)\n}\n\ntest \"family probe under an identity parent sees present and empty\" {\n    mAddBook(83, \"i83\")\n    mAddBook(84, \"i84\")\n    mAddNote(83, \"a\")\n    assert mHasNotes(83)\n    assert not mHasNotes(84)\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // The strict present-entry sibling: a presence-dominated sparse field set through an
        // identity-keyed named place reads its key-path from the place's pre-evaluated slots,
        // which carry the identity column. The set-sparse-present slot-type check re-proves
        // that identity column exactly as the stack key-path pop does, so the round trip runs.
        Row {
            label: "IDTRAV01: strict present sparse set through an identity-keyed place / driver test",
            ops: "pub fn spSeed(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\npub fn spSetVia(id: int, s: string): bool {\n    transaction {\n        place b = ^books[Id(^books, id)]\n        if exists(b) {\n            b.subtitle = s\n            return true\n        }\n    }\n    return false\n}\n\npub fn spSubtitle(id: int): string? {\n    return ^books[id].subtitle\n}\n\ntest \"strict present sparse set through an identity place round trips\" {\n    spSeed(90)\n    assert spSetVia(90, \"x\")\n    assert spSubtitle(90) ?? \"none\" == \"x\"\n}",
            expect: Expect::RoundTrips { run: true },
        },
        // ---- ENUMDUP01: two durable fields of one enum type. ----
        // `glucose` and `lactate` are both `Option<int>`, so they share one enum durable
        // identity (its sum and member ids appear once per referencing field). The
        // checker emits the shared identity and the verifier reads the reuse as one
        // per-declaration claim rather than a duplicate ledger id. Driven end to end: one
        // write sets both fields, and a reader unwraps each `some` payload back.
        Row {
            label: "ENUMDUP01: two Option<int> fields of one enum type round trip / driver test",
            ops: "pub fn setReadings(id: int, g: int, l: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\", glucose: some(g), lactate: some(l))\n    }\n}\n\npub fn glucoseVal(id: int): int {\n    if const cell = ^books[id].glucose {\n        match cell {\n            some(v) => return v\n            none => return -1\n        }\n    }\n    return -2\n}\n\npub fn lactateVal(id: int): int {\n    if const cell = ^books[id].lactate {\n        match cell {\n            some(v) => return v\n            none => return -1\n        }\n    }\n    return -2\n}\n\ntest \"two fields of one enum type round trip\" {\n    setReadings(1, 95, 12)\n    assert glucoseVal(1) == 95\n    assert lactateVal(1) == 12\n}",
            expect: Expect::RoundTrips { run: true },
        },
    ]
}

/// Run every `test` entry in a verified image through the ephemeral kernel and
/// require each to run without an artifact rejection, a mint failure, or a
/// runtime fault. This is the run-side half of the agreement invariant: a
/// verified round trip must also execute.
fn run_all_tests(label: &str, image: &VerifiedImage) {
    assert!(
        !image.test_entries().is_empty(),
        "{label}: a run-row must carry at least one test entry",
    );
    for entry in image.test_entries() {
        // Dispatch each test through the runtime its kind names: a driver test runs
        // each export call as its own invocation boundary; a direct-durable test runs
        // against one harness session. A storeless test never reaches a run row.
        let run = match entry.kind() {
            TestKind::Driver => run_driver_test(image, entry),
            TestKind::DirectDurable | TestKind::Storeless => run_durable_test(image, entry),
        };
        match run {
            DurableRun::Ran(Ok(_)) => {}
            DurableRun::Ran(Err(fault)) => {
                panic!(
                    "{label}: test `{}` faulted at run: {}",
                    entry.name(),
                    fault.code()
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
                    "{label}: test `{}` failed to mint its attachment: {code}",
                    entry.name()
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
                 ({code}: {detail}). A round trip regressed into a divergence.",
                row.label
            ),
            (Expect::RoundTrips { .. }, Stage::CheckerRejected(code)) => panic!(
                "`{}` was expected to round-trip but the checker refused it ({code}).",
                row.label
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
                "LEDGER STALE — `{}` now verifies; the {code} divergence (\"{detail}\") is fixed. \
                 Move this row to Expect::RoundTrips so the gate enforces it.",
                row.label
            ),
            (Expect::KnownDivergent { .. }, Stage::CheckerRejected(code)) => panic!(
                "`{}` was a checker-accept/verify-reject divergence but the checker now refuses it \
                 ({code}); re-classify the row to Expect::CheckerRejects.",
                row.label
            ),
            (Expect::CheckerRejects { code }, Stage::CheckerRejected(got_code)) => {
                assert_eq!(*code, got_code, "{}: check-time code drifted", row.label);
                checker_rejected += 1;
            }
            (Expect::CheckerRejects { code }, Stage::Verified(_)) => panic!(
                "LEDGER STALE — `{}` now verifies; the checker no longer refuses it ({code}). \
                 A promoted diagnostic regressed — restore the check or move the row.",
                row.label
            ),
            (
                Expect::CheckerRejects { code },
                Stage::VerifyRejected {
                    code: got_code,
                    detail,
                },
            ) => panic!(
                "`{}` was expected to be refused at check time ({code}) but the checker accepted it \
                 and the verifier rejected it ({got_code}: {detail}) — the check-time promotion \
                 regressed into a divergence.",
                row.label
            ),
        }
    }

    // The divergence set is closed. RV01 closed D1/D2/D3, DX01 turned the return-inside-
    // region row into a round trip, IDK01 lowered every identity-operand capturing position,
    // and TX02 promoted the last divergence — the empty (no-op) transaction — to a check-time
    // diagnostic, so the checker-accept/verify-reject ledger is now empty. A new divergence
    // added without a ledger row fails an individual row above; these counts fail if the
    // closed empty-transaction row silently changes verdict.
    assert_eq!(
        known_divergent, 0,
        "the divergence ledger is empty after TX02"
    );
    assert_eq!(
        checker_rejected, 1,
        "expected exactly the TX02-promoted empty-transaction check-time rejection",
    );
}

#[test]
fn nominal_field_index_admission_implies_independent_verification() {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let project = marrow_project::capture(
        &manifest,
        vec![marrow_project::CapturedFile::new(
            "src/main.mw".to_string(),
            NOMINAL_INDEX_SCHEMA.as_bytes().to_vec(),
        )],
        Some(NOMINAL_INDEX_IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project)
        .expect("the checker admits the nominal-field index declaration");
    let image = marrow_verify::verify(&compiled.image.bytes)
        .expect("the independent verifier admits the checker-accepted declaration");
    assert_eq!(image.indexes().len(), 1, "the declaration seals one index");
}

/// The correct-rollback journey, locking the invocation-boundary isolation law: three
/// exports run in sequence against one persistent attachment, exactly as a terminal
/// drives them. A mutating export commits; a second export that mutates then faults
/// rolls its own staged write back without disturbing the first commit; a reading
/// export then observes the committed-only state. This is the isolation a driver test
/// relies on — each call is its own boundary — proven at the invocation level.
#[test]
fn a_faulting_export_invocation_rolls_back_without_disturbing_a_prior_commit() {
    let Stage::Verified(image) = pipeline(
        "pub fn shelve(id: int, title: string, isbn: string) {\n    \
             transaction {\n        ^books[id] = Book(title: title, isbn: isbn)\n    }\n}\n\n\
         pub fn badUpdate(id: int, divisor: int) {\n    transaction {\n        \
             ^books[id].title = \"changed\"\n        \
             ^books[id].details.pages = 100 / divisor\n    }\n}\n\n\
         pub fn titleOf(id: int): string? {\n    return ^books[id].title\n}",
    ) else {
        panic!("the rollback-journey program must verify");
    };

    let Ephemeral::Ready(mut attachment) = mint_ephemeral(&image) else {
        panic!("the books root must mint an executable attachment");
    };

    // Commit an entry.
    assert!(matches!(
        run_export(
            &image,
            &mut attachment,
            export_by_name(&image, "shelve"),
            vec![
                Value::Int(1),
                Value::Text("first".into()),
                Value::Text("i1".into())
            ],
        ),
        DurableRun::Ran(Ok(_)),
    ));

    // A mutating invocation that faults before its commit (a divide-by-zero) rolls its
    // staged title write back.
    match run_export(
        &image,
        &mut attachment,
        export_by_name(&image, "badUpdate"),
        vec![Value::Int(1), Value::Int(0)],
    ) {
        DurableRun::Ran(Err(fault)) => assert_eq!(
            fault.code(),
            "run.divide_by_zero",
            "the fault reached the caller"
        ),
        other => panic!("badUpdate must fault, not {:?}", DurableRunDebug(&other)),
    }

    // The prior commit survives, unchanged by the rolled-back invocation.
    match run_export(
        &image,
        &mut attachment,
        export_by_name(&image, "titleOf"),
        vec![Value::Int(1)],
    ) {
        DurableRun::Ran(Ok(Some(Value::Optional(Some(title))))) => {
            assert_eq!(
                *title,
                Value::Text("first".into()),
                "the rolled-back write left no trace"
            )
        }
        other => panic!(
            "titleOf must read the committed-only value, got {:?}",
            DurableRunDebug(&other)
        ),
    }
}

/// The verified export whose function is named `name`. The image carries no export
/// name, so the function directory resolves the name to its function index.
fn export_by_name<'a>(image: &'a VerifiedImage, name: &str) -> &'a marrow_verify::SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .unwrap_or_else(|| panic!("export `{name}` is present"))
}

impl std::fmt::Debug for DurableRunDebug<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            DurableRun::Ran(Ok(value)) => write!(f, "Ran(Ok({value:?}))"),
            DurableRun::Ran(Err(fault)) => write!(f, "Ran(Err({}))", fault.code()),
            DurableRun::Parked => write!(f, "Parked"),
            DurableRun::Failed(code) => write!(f, "Failed({code})"),
        }
    }
}

struct DurableRunDebug<'a>(&'a DurableRun);
