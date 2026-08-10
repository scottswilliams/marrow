//! Declared-entry causality: a declaration the compiler refused keeps its name.
//!
//! A namespace that drops a refused declaration makes every later lookup read as
//! *never declared*, so the use site reports a fabricated absence — "is not in
//! scope" — for a name the reader can see declared, and reports it once per use.
//! Under the declaration ledger a refused key answers `Refused`: the declaring
//! cause is reported once at the declaration, the first use is steered to it, and
//! later uses fail silently.
//!
//! Diagnostics are asserted by code, span, and count. The one prose assertion is
//! negative — that a refused name is never called out of scope — which is the
//! fabrication these fixtures exist to kill.

use std::collections::BTreeMap;

use marrow_compile::{CompileFailure, ResourceLimitKind, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, IdentityAnchor, Manifest, ProjectInput};

fn project(source: &str) -> ProjectInput {
    files(&[("src/main.mw", source.to_string())])
}

fn files(sources: &[(&str, String)]) -> ProjectInput {
    captured(sources, None)
}

fn captured(sources: &[(&str, String)], ids: Option<&[u8]>) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = sources
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    marrow_project::capture(&manifest, files, ids, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// A project whose durable identity ledger is complete, so no store refuses for a
/// missing anchor.
///
/// The identity gap is the one refusal class entitled to the "see the
/// `check.durable_identity` reports" steer, and with no ledger *every* store refuses
/// that way first. A red for any other durable refusal class has to mint past it, so
/// this resolves the typed gaps the compiler itself reports — never a hand-written
/// anchor list, which would encode a second opinion about the anchor set — until the
/// compiler reports none.
fn with_minted_ids(sources: &[(&str, String)]) -> ProjectInput {
    let mut minted: BTreeMap<IdentityAnchor, String> = BTreeMap::new();
    for _ in 0..64 {
        let ids = serialize_ids(&minted);
        let project = captured(sources, Some(ids.as_bytes()));
        let gaps: Vec<IdentityAnchor> = match compile(&project) {
            Ok(_) => Vec::new(),
            Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics
                .into_iter()
                .filter_map(|row| row.identity_gap().map(marrow_compile::IdentityGap::anchor))
                .collect(),
            Err(other) => panic!("expected diagnostics while minting, got {other:?}"),
        };
        let mut fresh = false;
        for anchor in gaps {
            if !minted.contains_key(&anchor) {
                let id = format!("{:032x}", minted.len() + 1);
                minted.insert(anchor, id);
                fresh = true;
            }
        }
        if !fresh {
            return captured(sources, Some(serialize_ids(&minted).as_bytes()));
        }
    }
    panic!("minting a complete identity ledger did not converge");
}

fn serialize_ids(minted: &BTreeMap<IdentityAnchor, String>) -> String {
    let mut text = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
    for (anchor, id) in minted {
        text.push_str(&format!("id {} {} {id}\n", anchor.kind.keyword(), anchor.path));
    }
    text.push_str(&format!("high-water {}\nend\n", minted.len()));
    text
}

fn diagnostics(source: &str) -> Vec<SourceDiagnostic> {
    match compile(&project(source)) {
        Ok(compiled) => panic!("expected a refused declaration, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}

/// Every row, as `(code, line, column)` — the typed shape a red asserts.
fn rows(diagnostics: &[SourceDiagnostic]) -> Vec<(&str, u32, u32)> {
    diagnostics
        .iter()
        .map(|row| {
            let span = row.span();
            (row.code(), span.line, span.column)
        })
        .collect()
}

fn assert_never_out_of_scope(diagnostics: &[SourceDiagnostic], name: &str) {
    for row in diagnostics {
        assert!(
            !row.message().contains(&format!("`{name}` is not in scope")),
            "`{name}` is declared in this source; no row may call it out of scope: {:#?}",
            rows(diagnostics),
        );
    }
}

/// R1 — a constant refused for a type mismatch is reported at its declaration and
/// its use is steered to that report, never called out of scope.
#[test]
fn r1_a_type_refused_constant_is_not_out_of_scope_at_its_use() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit: int = \"x\"\n\n\
         pub fn read(): int {\n\
         \x20   return limit\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "limit");
    assert_eq!(
        rows(&diagnostics),
        vec![("check.type", 3, 1), ("check.type", 6, 12)],
        "the declaration reports the cause and the use is steered to it",
    );
}

/// R2 — a constant refused for a non-literal value behaves the same, and the steer
/// reuses the declaring code (`check.unsupported`), not the use site's own.
#[test]
fn r2_a_value_refused_constant_steers_with_the_declaring_code() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit = 1 + 2\n\n\
         pub fn read(): int {\n\
         \x20   return limit\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "limit");
    assert_eq!(
        rows(&diagnostics),
        vec![("check.unsupported", 3, 15), ("check.unsupported", 6, 12)],
        "the steer carries the declaring cause's code, so a use-site assertion \
         names the declaration's typed identity",
    );
}

/// R24 — the report is once per refused key, not once per use. Two uses of one
/// refused constant produce the declaring row and exactly one steer.
#[test]
fn r24_a_refused_constant_is_reported_once_across_many_uses() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit: int = \"x\"\n\n\
         pub fn read(): int {\n\
         \x20   const a = limit\n\
         \x20   const b = limit\n\
         \x20   const c = limit\n\
         \x20   return a + b + c\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "limit");
    assert_eq!(
        diagnostics.len(),
        2,
        "one declaring row and one steer, whatever the use count: {:#?}",
        rows(&diagnostics),
    );
    assert_eq!(rows(&diagnostics)[0], ("check.type", 3, 1));
}

/// R25 — a refused declaration still occupies its name, in both orders. The
/// duplicate check sees the refused occurrence, so the second declaration is a
/// name conflict whether the refused one came first or second.
#[test]
fn r25_a_refused_constant_occupies_its_name_when_declared_first() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit = 1 + 2\n\
         const limit = 5\n\n\
         pub fn read(): int {\n\
         \x20   return limit\n\
         }\n",
    );

    assert!(
        diagnostics
            .iter()
            .any(|row| row.code() == "check.name_conflict"),
        "a refused declaration occupies its name, so the redeclaration conflicts: {:#?}",
        rows(&diagnostics),
    );
}

/// The sibling direction, which already held: the refused occurrence comes second.
#[test]
fn r25_a_refused_constant_occupies_its_name_when_declared_second() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit = 5\n\
         const limit = 1 + 2\n\n\
         pub fn read(): int {\n\
         \x20   return limit\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![("check.name_conflict", 4, 1)],
        "the accepted first declaration answers the use; only the conflict reports",
    );
}

/// E9 — the retained names are bounded. A project whose refused declarations would
/// retain more than the ledger's declared ceiling stops with the typed resource
/// limit. It never drops a key to stay under budget, which is the one outcome that
/// would put a fabricated absence back at every use of the dropped name.
///
/// Neither the image bounds nor the diagnostic ceiling bounds this retention: a
/// refused declaration never reaches the encoder, and a collector at its ceiling
/// keeps admitting and discarding while the pass runs on.
#[test]
fn e9_crossing_the_ledger_ceiling_is_a_typed_resource_limit() {
    // Each refused constant retains its name plus the summary's fixed footprint, so
    // wide names cross the 1 MiB ceiling in a project well inside the capture
    // limits. `1 + 2` is a non-literal value, refused with `check.unsupported`.
    let wide = "n".repeat(1000);
    let module = |module: &str, from: usize| {
        let mut source = format!("module {module}\n\n");
        for index in from..from + 600 {
            source.push_str(&format!("const {wide}{index} = 1 + 2\n"));
        }
        source
    };
    let project = files(&[
        ("src/main.mw", module("main", 0)),
        ("src/more.mw", module("more", 600)),
    ]);

    match compile(&project) {
        Err(CompileFailure::ResourceLimit(limit)) => {
            assert_eq!(limit.kind(), ResourceLimitKind::DeclarationLedgerBytes);
            assert_eq!(limit.kind().detail(), "DeclarationLedgerBytes");
        }
        other => panic!("expected the ledger's typed ceiling, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Durable roots — I-3, I-9, I-10
//
// A `store` root refused for any reason other than a missing ledger identity is
// dropped from the registry entirely, so every `^root` reference reads as an
// unknown name. The identity class is the one class already retained, and it is
// also the one class entitled to the "see the `check.durable_identity` reports"
// steer — which nine of the ten refusal sites give it wrongly today, naming
// reports that were never made.
// ---------------------------------------------------------------------------

/// Every diagnostic row's message, for the negative assertions below.
fn messages(diagnostics: &[SourceDiagnostic]) -> Vec<&str> {
    diagnostics.iter().map(SourceDiagnostic::message).collect()
}

fn assert_not_steered_to_identity(diagnostics: &[SourceDiagnostic]) {
    for row in diagnostics {
        assert!(
            !row.message().contains("failed identity admission"),
            "this root was refused for a cause other than a missing identity, so no \
             row may send the reader to `check.durable_identity` reports that do not \
             exist: {:#?}",
            messages(diagnostics),
        );
    }
    assert!(
        diagnostics
            .iter()
            .all(|row| row.code() != "check.durable_identity"),
        "the fixture must isolate a non-identity refusal: {:#?}",
        messages(diagnostics),
    );
}

/// R3 — a store root refused because its resource is undeclared is reported at its
/// declaration, and a write through it is steered to that report. Today the root is
/// dropped whole, so the write says `items` is not in scope — of a root declared
/// two lines above.
#[test]
fn r3_a_root_refused_for_its_resource_is_not_out_of_scope_at_a_write() {
    let diagnostics = diagnostics(
        "module main\n\n\
         store ^items[id: int]: Widget\n\n\
         pub fn write() {\n\
         \x20   transaction {\n\
         \x20       ^items[1].name = \"a\"\n\
         \x20   }\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "items");
    assert_not_steered_to_identity(&diagnostics);
    assert_eq!(
        rows(&diagnostics),
        vec![("check.type", 3, 1), ("check.type", 7, 9)],
        "the declaration reports the cause and the write is steered to it",
    );
}

/// R4 — the same root through a `place` binding. The sibling lookup, not only
/// `resolve_root`'s write path, must reuse the declaring cause.
#[test]
fn r4_a_root_refused_for_its_resource_is_not_out_of_scope_at_a_place() {
    let diagnostics = diagnostics(
        "module main\n\n\
         store ^items[id: int]: Widget\n\n\
         pub fn write() {\n\
         \x20   transaction {\n\
         \x20       place p = ^items[1]\n\
         \x20       p.name = \"a\"\n\
         \x20   }\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "items");
    assert_not_steered_to_identity(&diagnostics);
    assert_eq!(
        rows(&diagnostics)[0],
        ("check.type", 3, 1),
        "the declaration still owns the cause: {:#?}",
        rows(&diagnostics),
    );
}

/// R17 — the steer is once per refused root, not once per reference. Ten uses of one
/// refused root produce the declaring row and exactly one steer.
#[test]
fn r17_a_refused_root_is_reported_once_across_many_uses() {
    let mut source = String::from(
        "module main\n\n\
         store ^items[id: int]: Widget\n\n\
         pub fn write() {\n\
         \x20   transaction {\n",
    );
    for index in 1..=10 {
        source.push_str(&format!("        ^items[{index}].name = \"a\"\n"));
    }
    source.push_str("    }\n}\n");
    let diagnostics = diagnostics(&source);

    assert_never_out_of_scope(&diagnostics, "items");
    assert_eq!(
        diagnostics.len(),
        2,
        "one declaring row and one steer, whatever the reference count: {:#?}",
        rows(&diagnostics),
    );
}

/// R18 — a refused root is still offered as a did-you-mean. Dropping the key removes
/// it from the correction corpus too, so a near miss on a refused root gets no
/// suggestion at all.
#[test]
fn r18_a_refused_root_is_offered_as_a_did_you_mean() {
    let diagnostics = diagnostics(
        "module main\n\n\
         store ^items[id: int]: Widget\n\n\
         pub fn write() {\n\
         \x20   transaction {\n\
         \x20       ^itmes[1].name = \"a\"\n\
         \x20   }\n\
         }\n",
    );

    assert!(
        diagnostics
            .iter()
            .any(|row| row.message().contains("`itmes` is not in scope")
                && row.message().contains("items")),
        "a genuinely undeclared root is still an unknown name, corrected against the \
         refused root's retained key: {:#?}",
        messages(&diagnostics),
    );
}

/// R20 · `Bound` — a root refused for crossing a fixed compiler-owned bound keeps
/// its name and reuses its own `check.resource_limit` cause. It must not claim an
/// identity admission failure.
#[test]
fn r20_bound_a_root_refused_for_a_key_tuple_bound_reuses_its_own_cause() {
    let mut source = String::from(
        "module main\n\n\
         resource Widget {\n\
         \x20   required name: string\n\
         }\n\n\
         store ^items[",
    );
    // One column past `marrow_image::bounds::MAX_KEY_COLUMNS`, which the checker
    // rejects before any identity anchor is resolved.
    let columns: Vec<String> = (0..17).map(|index| format!("k{index}: int")).collect();
    source.push_str(&columns.join(", "));
    source.push_str(
        "]: Widget\n\n\
         pub fn write() {\n\
         \x20   transaction {\n\
         \x20       ^items[1].name = \"a\"\n\
         \x20   }\n\
         }\n",
    );
    let diagnostics = diagnostics(&source);

    assert_never_out_of_scope(&diagnostics, "items");
    assert_not_steered_to_identity(&diagnostics);
    assert_eq!(
        diagnostics
            .iter()
            .filter(|row| row.code() == "check.resource_limit")
            .count(),
        2,
        "the declaration reports the bound and the use reuses its code: {:#?}",
        rows(&diagnostics),
    );
}

const CYCLE_SOURCES: [(&str, &str); 1] = [(
    "src/main.mw",
    "module main\n\n\
     struct Node {\n\
     \x20   next: Node\n\
     \x20   x: int\n\
     }\n\n\
     resource Book {\n\
     \x20   required title: string\n\
     \x20   n: Node\n\
     }\n\n\
     store ^books[id: int]: Book\n\n\
     pub fn write() {\n\
     \x20   transaction {\n\
     \x20       ^books[1].title = \"a\"\n\
     \x20   }\n\
     }\n",
)];

/// R20 · `ValueCycle` — the one refusal site that pushes no diagnostic of its own.
/// Its cause is the `check.recursion` report from the value-cycle pass, which runs
/// after lowering, so this class asserts set membership and that the steer carries
/// that cause — never an identity admission claim.
#[test]
fn r20_value_cycle_a_root_refused_for_a_value_cycle_names_the_recursion_cause() {
    let sources: Vec<(&str, String)> = CYCLE_SOURCES
        .iter()
        .map(|(path, source)| (*path, (*source).to_string()))
        .collect();
    let diagnostics = match compile(&with_minted_ids(&sources)) {
        Ok(compiled) => panic!("expected a refused declaration, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            diagnostics.into_iter().collect::<Vec<_>>()
        }
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    };

    assert_never_out_of_scope(&diagnostics, "books");
    for row in &diagnostics {
        assert!(
            !row.message().contains("failed identity admission"),
            "a value cycle is not an identity gap: {:#?}",
            messages(&diagnostics),
        );
    }
    assert!(
        diagnostics
            .iter()
            .any(|row| row.code() == "check.recursion"),
        "the covering pass reports the cycle: {:#?}",
        rows(&diagnostics),
    );
}

/// R19 — a refused resource member must not narrow the identity-gap anchor set. A
/// member dropped from the record is never anchored, so the durable graph reports
/// fewer `check.durable_identity` rows than the same program with a valid member
/// type, and the mint action that consumes those rows mints an incomplete ledger.
#[test]
#[ignore = "sequenced behind resource-member granularity (I-7, design §2.3): the \
            member is dropped by the type registry before the durable resolver \
            walks it, so the anchor is restored only once `RecordInfo::fields` is \
            itself a ledger"]
fn r19_a_refused_member_does_not_narrow_the_identity_gap_set() {
    let anchors = |field: &str| {
        let source = format!(
            "module main\n\n\
             resource Widget {{\n\
             \x20   required name: {field}\n\
             }}\n\n\
             store ^items[id: int]: Widget\n"
        );
        let diagnostics = diagnostics(&source);
        diagnostics
            .iter()
            .filter_map(|row| row.identity_gap().map(|gap| gap.anchor()))
            .collect::<Vec<_>>()
    };

    let valid = anchors("string");
    let refused = anchors("Nope");
    assert!(
        refused.len() >= valid.len(),
        "a refused member narrowed the anchor set from {valid:#?} to {refused:#?}",
    );
}
