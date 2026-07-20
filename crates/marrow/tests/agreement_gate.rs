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
//! the return-inside-region row is a round trip too. Their rows are positive controls
//! now. One divergence remains ledgered — an empty (no-op) `transaction` — which the
//! verifier refuses but the checker still accepts; it belongs to the by-design checker
//! false-negative family TX02 promotes to check-time diagnostics. The correct-rollback
//! journey below locks the invocation-boundary isolation law: a faulting export
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
     id field Book.isbn dc43cd86f5de791211612a599f1a1b01\n\
     id field Book.notes.text c3cde175f2329c20c8c8ce0d39405712\n\
     id field Book.subtitle 26ba2d1538308102805dfa7e5007a493\n\
     id field Book.title ea95ccce4ce370579210f6697baf7316\n\
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
    KnownDivergent {
        code: &'static str,
        detail: &'static str,
    },
    /// A composition the checker refuses outright (a not-yet-lowered form). The
    /// checker-accept ⇒ verify implication holds vacuously; the row is ledgered
    /// so promoting the form flips it to `RoundTrips`.
    CheckerRefused { code: &'static str },
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
        // ---- PL01: a place over a composite-key root resolves fields by the root node. ----
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
        // cannot run (it opens no session), refused by the verifier; the checker still
        // accepts it — the same by-design false-negative family TX02 promotes to check
        // time. Ledgered so the remaining divergence stays visible.
        Row {
            label: "empty transaction — no durable operation (TX02 promotes to check time)",
            ops: "pub fn emptyRegion() {\n    transaction {\n    }\n}",
            expect: Expect::KnownDivergent {
                code: "image.flow",
                detail: "a transaction performs no durable operation",
            },
        },
        // ---- A genuinely-deferred form, refused in agreement by both owners. ----
        // A group-leaf write through an identity-lookup result is not yet lowered
        // (distinct from D3's whole-entry write, which is). The checker refuses it, so
        // the checker-accept ⇒ verify implication holds vacuously; ledgered so its
        // eventual promotion flips this row to a round trip.
        Row {
            label: "group-leaf write through an identity key (deferred)",
            ops: "pub fn identityGroupWrite(isbn: string) {\n    transaction {\n        if const found = ^books.byIsbn[isbn] {\n            ^books[found].details.pages = 3\n        }\n    }\n}",
            expect: Expect::CheckerRefused {
                code: "check.unsupported",
            },
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
/// pinned expectation exactly; the known-divergent and checker-refused ledgers
/// are additionally size-pinned so the divergence set cannot grow unremarked.
#[test]
fn checker_acceptance_implies_verification_over_the_composition_matrix() {
    let mut known_divergent = 0usize;
    let mut checker_refused = 0usize;

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
                 ({code}); re-classify the row.",
                row.label
            ),
            (Expect::CheckerRefused { code }, Stage::CheckerRejected(got)) => {
                assert_eq!(*code, got, "{}: refusal code drifted", row.label);
                checker_refused += 1;
            }
            (Expect::CheckerRefused { code }, Stage::Verified(_)) => panic!(
                "LEDGER STALE — `{}` now verifies; the {code} refusal is lowered. Move this row to \
                 Expect::RoundTrips so the gate enforces it.",
                row.label
            ),
            (Expect::CheckerRefused { .. }, Stage::VerifyRejected { code, detail }) => panic!(
                "`{}` was checker-refused but now reaches the verifier and is rejected there \
                 ({code}: {detail}); re-classify the row.",
                row.label
            ),
        }
    }

    // The divergence set is explicit and bounded. RV01 closed D1/D2/D3, and DX01 turned
    // the return-inside-region row into a round trip, so the only remaining
    // checker-accept/verify-reject row is the empty (no-op) transaction TX02 owns;
    // nothing is checker-refused beyond the deferred group-leaf case. A new divergence
    // added without a ledger row fails an individual row above; these counts fail if a
    // ledger row is silently removed or an unledgered one appears.
    assert_eq!(
        known_divergent, 1,
        "expected exactly the TX02-owned empty-transaction divergence",
    );
    assert_eq!(
        checker_refused, 1,
        "expected exactly the deferred group-leaf-through-identity refusal",
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
