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
//! Session-1 state (D1 semantic + this skeleton): the D1 test-body invocation
//! boundary and the D2 read-only-region admission are ledgered known-divergent
//! (both `image.flow`), and the D3 identity-keyed whole-entry write is ledgered
//! checker-refused (`check.unsupported`). A later session flips each as its
//! kernel/compiler fix lands.

use marrow_verify::VerifiedImage;
use marrow_vm::{DurableRun, run_durable_test};

/// The shared durable graph every composition is written against: a flat keyed
/// root with a required and a sparse field, a root-level group, a keyed branch,
/// a unique index (identity lookup), and a nonunique index (bounded scan). The
/// identity ledger below pins one id per anchor, so the schema is identity
/// complete on its own and each composition only appends operations.
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
        Err(diagnostics) => Stage::CheckerRejected(
            diagnostics
                .first()
                .expect("a rejection carries at least one diagnostic")
                .code,
        ),
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
        // ---- The review-of-record divergence set (RV01 closes it). ----
        // D2: a whole-entry read inside a region the export owns is coherent, but
        // the owner lattice rejects a region whose closure performs no mutation —
        // the same read outside a transaction verifies. Checker accepts.
        Row {
            label: "D2: whole-entry read / inside a read-only region",
            ops: "pub fn d2ReadOnlyRegion(id: int): string? {\n    transaction {\n        if const b = ^books[id] {\n            return b.title\n        }\n    }\n    return absent\n}",
            expect: Expect::KnownDivergent {
                code: "image.flow",
                detail: "a transaction marker sits outside its owning export",
            },
        },
        // D1: a test body drives a mutating export. The adopted semantic is that
        // each export call from a test body is its own invocation boundary (a
        // terminal-style driver); the owner-not-called law rejects it today.
        // Checker accepts.
        Row {
            label: "D1: export call owning a transaction / from a test body",
            ops: "pub fn d1Add(id: int) {\n    transaction {\n        ^books[id] = Book(title: \"t\", isbn: \"i\")\n    }\n}\n\ntest \"driver adds through an export\" {\n    d1Add(7)\n    assert true\n}",
            expect: Expect::KnownDivergent {
                code: "image.flow",
                detail: "a transaction owner may not be called",
            },
        },
        // D3: a whole-entry write through an identity-lookup result. A field write
        // through the same lookup already round-trips (positive control above);
        // the whole-entry form is refused at the checker.
        Row {
            label: "D3: identity-keyed whole-entry write / inside a mutating region",
            ops: "pub fn d3IdentityWrite(isbn: string) {\n    transaction {\n        if const found = ^books.byIsbn[isbn] {\n            ^books[found] = Book(title: \"t\", isbn: isbn)\n        }\n    }\n}",
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
        match run_durable_test(image, entry) {
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

    // The divergence set is explicit and bounded: exactly D1 and D2 are
    // checker-accept/verify-reject, and exactly D3 is checker-refused. A new
    // divergence added without a ledger row fails an individual row above; these
    // counts fail if a ledger row is silently removed.
    assert_eq!(
        known_divergent, 2,
        "expected exactly the D1 and D2 known-divergent rows",
    );
    assert_eq!(
        checker_refused, 1,
        "expected exactly the D3 checker-refused row",
    );
}
