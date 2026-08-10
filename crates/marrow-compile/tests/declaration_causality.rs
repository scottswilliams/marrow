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
        text.push_str(&format!(
            "id {} {} {id}\n",
            anchor.kind.keyword(),
            anchor.path
        ));
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

/// No row describes a refused declaration of this project's own as a gap in the
/// language. The subset-gap phrase is reserved for a form the beta line genuinely
/// does not admit; using it for a declared-and-refused name is the fabrication the
/// named-type reds exist to kill.
fn assert_no_subset_gap(diagnostics: &[SourceDiagnostic]) {
    for row in diagnostics {
        assert!(
            !row.message()
                .contains("is not yet supported on the beta line"),
            "this project declared the type; the report must name the declaration's \
             own cause, not a language gap: {:#?}",
            diagnostics
                .iter()
                .map(SourceDiagnostic::message)
                .collect::<Vec<_>>(),
        );
    }
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

/// A4 — a resource-keyed durable lookup answers the store's refusal, not an
/// absence. The branch constructor `Resource.branch(…)` resolves through the store
/// backing `Resource`; scanning the executable roots for it answered `None` for a
/// refused store, so the call fell through to the method-shaped-call report and
/// blamed the language for the store's own reported defect.
#[test]
fn a4_a_branch_constructor_of_a_refused_store_names_the_stores_cause() {
    let diagnostics = diagnostics(
        "module main\n\n\
         resource Widget {\n\
         \x20   required name: string\n\n\
         \x20   notes[nid: int] {\n\
         \x20       required body: string\n\
         \x20   }\n\
         }\n\n\
         store ^items[id: int]: Widget\n\n\
         pub fn make(): int {\n\
         \x20   const n = Widget.notes(body: \"x\")\n\
         \x20   return 1\n\
         }\n",
    );

    assert_no_subset_gap(&diagnostics);
    assert_eq!(
        rows(&diagnostics).last().copied(),
        Some(("check.durable_identity", 14, 15)),
        "the constructor is steered to the store's own cause: {:#?}",
        messages(&diagnostics),
    );
}

/// A4 — the same for the record-keyed steer. A materialized resource value names a
/// member that is neither a field nor, as far as this compilation knows, a branch:
/// the store that would have built the branch tree was refused, so reporting the
/// record as having no such field states as fact something the compiler cannot know.
#[test]
fn a4_a_branch_named_on_a_refused_stores_resource_names_the_stores_cause() {
    let diagnostics = diagnostics(
        "module main\n\n\
         resource Widget {\n\
         \x20   required name: string\n\n\
         \x20   notes[nid: int] {\n\
         \x20       required body: string\n\
         \x20   }\n\
         }\n\n\
         store ^items[id: int]: Widget\n\n\
         pub fn make(): int {\n\
         \x20   const w = Widget(name: \"a\")\n\
         \x20   const b = w.notes\n\
         \x20   return 1\n\
         }\n",
    );

    assert!(
        !messages(&diagnostics)
            .iter()
            .any(|message| message.contains("has no field `notes`")),
        "`notes` is declared eight lines above; no row may say the record has no \
         such member: {:#?}",
        messages(&diagnostics),
    );
    assert_eq!(
        rows(&diagnostics).last().copied(),
        Some(("check.durable_identity", 15, 15)),
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
    // The covering report sits on the cyclic value type, not on `^books`. The steer
    // names the code it must be corrected against and claims no location, because
    // there is no `check.recursion` row at this store's declaration to send anyone to.
    for row in &diagnostics {
        assert!(
            !row.message().contains("at the declaration of `books`"),
            "a covered cause is reported elsewhere; the steer may not place it at \
             this declaration: {:#?}",
            messages(&diagnostics),
        );
    }
}

// ---------------------------------------------------------------------------
// Resource members — I-7
//
// The one namespace that drops a *member* and keeps the declaration, so the record
// survives with a silently narrowed field set and every lookup of the dropped
// member makes a false statement about the source.
// ---------------------------------------------------------------------------

/// R21 — the over-suppression guard, and R15's mutation-kill partner. A field that
/// really is not declared still says so. Member granularity must distinguish a
/// member the compiler refused from one the source never wrote; suppressing both
/// would trade a false absence for a missing report.
#[test]
fn r21_a_genuinely_absent_field_is_still_reported_as_absent() {
    let diagnostics = diagnostics(
        "module main\n\n\
         resource Widget {\n\
         \x20   required name: string\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   const w = Widget(name: \"a\", nope: 1)\n\
         \x20   return 1\n\
         }\n",
    );

    assert!(
        diagnostics
            .iter()
            .any(|row| row.message().contains("has no field `nope`")),
        "`nope` is declared nowhere; the record must still say so: {:#?}",
        messages(&diagnostics),
    );
}

/// R15 — a resource member the compiler refused, then named. The member is dropped
/// from the record and the record survives, so the constructor reports that the
/// resource has no such field — four lines after the compiler diagnosed that field's
/// type.
#[test]
fn r15_a_refused_resource_member_is_not_absent_at_its_use() {
    let diagnostics = diagnostics(
        "module main\n\n\
         resource Widget {\n\
         \x20   required name: string\n\
         \x20   bad: Nope\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   const w = Widget(name: \"a\", bad: 1)\n\
         \x20   return 1\n\
         }\n",
    );

    assert!(
        !messages(&diagnostics)
            .iter()
            .any(|message| message.contains("has no field `bad`")),
        "`bad` is declared four lines above and was refused there; no row may say \
         the resource has no such field: {:#?}",
        messages(&diagnostics),
    );
    assert_eq!(
        rows(&diagnostics),
        vec![("check.unsupported", 5, 10), ("check.unsupported", 9, 38)],
    );
}

/// R19 — a refused resource member must not narrow the identity-gap anchor set. A
/// member dropped from the record is never anchored, so the durable graph reports
/// fewer `check.durable_identity` rows than the same program with a valid member
/// type, and the mint action that consumes those rows mints an incomplete ledger.
#[test]
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

// ---------------------------------------------------------------------------
// Named types — I-2
//
// Every named-type lookup funnels into one untyped bucket, so a use of a type this
// project declared and the compiler refused is reported as a *language* gap: "not
// yet supported on the beta line". The declaration is dropped from its table (so
// nothing resolves against a broken type) and the ledger keeps its name (so the use
// is steered to the cause instead).
// ---------------------------------------------------------------------------

/// R5 — a struct refused for a bad field. The construction resolves through the
/// struct table, not through annotation resolution, so it reached its own
/// not-in-scope report: `Point` is not in scope, of a struct declared six lines
/// above whose field the compiler had just diagnosed.
#[test]
fn r5_a_refused_struct_is_not_out_of_scope_at_its_construction() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct Point {\n\
         \x20   x: Nope\n\
         \x20   y: int\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   const p = Point(x: 1, y: 2)\n\
         \x20   return p.y\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "Point");
    assert_eq!(
        rows(&diagnostics),
        vec![("check.unsupported", 4, 8), ("check.unsupported", 9, 15)],
        "the field reports the cause and the construction is steered to it",
    );
}

/// R10 — an enum refused for a bad payload. A qualified `Enum::member` is a third
/// resolution path again, and it reported the *spelling* as unsupported rather than
/// the enum this project declared and the compiler refused.
#[test]
fn r10_a_refused_enum_steers_its_qualified_use_to_the_payload_report() {
    let diagnostics = diagnostics(
        "module main\n\n\
         enum Shape {\n\
         \x20   Circle(r: Nope)\n\
         \x20   Square\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   const s = Shape::Square\n\
         \x20   return 1\n\
         }\n",
    );

    assert!(
        !messages(&diagnostics)
            .iter()
            .any(|message| message.contains("a qualified name is not yet supported")),
        "the enum is declared; the qualified use must name its cause: {:#?}",
        messages(&diagnostics),
    );
    assert_eq!(
        rows(&diagnostics),
        vec![("check.unsupported", 4, 15), ("check.unsupported", 9, 15)],
    );
}

/// R6 — an alias over an unknown target. Today the annotation blames the language;
/// the alias's own `check.type` report two lines above is never connected to it.
#[test]
fn r6_a_refused_alias_steers_its_annotation_to_the_alias_report() {
    let diagnostics = diagnostics(
        "module main\n\n\
         alias Count = Nope\n\n\
         pub fn make(c: Count): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_no_subset_gap(&diagnostics);
    assert_eq!(
        rows(&diagnostics),
        vec![("check.type", 3, 1), ("check.type", 5, 16)],
        "the alias reports the cause and the annotation is steered to it",
    );
}

/// R7 — a cyclic alias chain. The steer carries `check.recursion`, the code of the
/// report the reader must act on, not a subset-gap phrase.
#[test]
fn r7_a_cyclic_alias_steers_its_annotation_to_the_recursion_report() {
    let diagnostics = diagnostics(
        "module main\n\n\
         alias A = B\n\
         alias B = A\n\n\
         pub fn make(c: A): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_no_subset_gap(&diagnostics);
    assert_eq!(
        rows(&diagnostics),
        vec![
            ("check.recursion", 3, 7),
            ("check.recursion", 4, 7),
            ("check.recursion", 6, 16),
        ],
    );
}

/// R11 — a nominal type whose interval admits no values. The declaration is refused
/// for a `check.type` and the annotation reuses that code.
#[test]
fn r11_a_refused_nominal_steers_its_annotation_to_the_interval_report() {
    let diagnostics = diagnostics(
        "module main\n\n\
         type Age: int in 10..=0\n\n\
         pub fn make(a: Age): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_no_subset_gap(&diagnostics);
    assert_eq!(
        rows(&diagnostics),
        vec![("check.type", 3, 18), ("check.type", 5, 16)],
    );
}

/// R26 — the cascade split. A refused declaration in one parameter must not absorb
/// a genuinely missing name in the next: each parameter is rejected at its own span
/// with its own cause. Base: two identical subset-gap rows, so the real absence and
/// the project's own refusal read the same.
#[test]
fn r26_a_refused_type_does_not_absorb_a_genuine_absence_beside_it() {
    let diagnostics = diagnostics(
        "module main\n\n\
         alias Count = Nope\n\n\
         pub fn make(a: Count, b: AlsoMissing): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![
            ("check.type", 3, 1),
            ("check.type", 5, 16),
            ("check.unsupported", 5, 26),
        ],
        "`a` is steered to the alias's cause; `b` names a type nothing declared and \
         keeps the subset-gap report",
    );
}

/// R27 — the merge never widens. A refused alias used inside a generic application
/// must not be described as the cause of anything but itself: no row may claim the
/// genuinely absent `AlsoMissing` was refused.
#[test]
fn r27_a_refused_type_is_never_named_as_another_names_cause() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct Pair<A, B> {\n\
         \x20   first: A\n\
         \x20   second: B\n\
         }\n\n\
         alias Count = Nope\n\n\
         pub fn make(p: Pair<Count, AlsoMissing>): int {\n\
         \x20   return 1\n\
         }\n",
    );

    for row in &diagnostics {
        assert!(
            !row.message().contains("`AlsoMissing` was declared"),
            "`AlsoMissing` is not declared anywhere; no steer may claim it was: {:#?}",
            messages(&diagnostics),
        );
    }
    assert_eq!(
        rows(&diagnostics),
        vec![("check.type", 8, 1), ("check.type", 10, 16)],
    );
}

/// R13 — a generic template's defect is reported at its declaration.
///
/// A template's member types are resolved per instantiation, so a member naming an
/// undeclared type used to be reported only at a *construction* site, blaming the
/// construction. The declaring cause was reported zero times — the same invariant
/// broken downward rather than upward.
#[test]
fn r13_a_refused_template_is_reported_at_its_declaration() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct Pair<A, B> {\n\
         \x20   first: A\n\
         \x20   second: Nope\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   const p = Pair(first: 1, second: 2)\n\
         \x20   return 1\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![("check.type", 5, 13), ("check.type", 9, 15)],
        "the member reports the cause and the construction is steered to it",
    );
}

/// The same template with no use at all. A declaration nothing constructs used to
/// compile clean at exit zero, because the only report was at a construction site.
#[test]
fn r13_a_refused_template_is_reported_even_with_no_use() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct Pair<A, B> {\n\
         \x20   first: A\n\
         \x20   second: Nope\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_eq!(rows(&diagnostics), vec![("check.type", 5, 13)]);
}

/// R14 — the same template named in an annotation. The generic application resolves
/// its head through the template table, so it reported a subset gap for a template
/// this project declared.
#[test]
fn r14_a_refused_template_steers_its_annotation_to_the_member_report() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct Pair<A, B> {\n\
         \x20   first: A\n\
         \x20   second: Nope\n\
         }\n\n\
         pub fn make(p: Pair<int, int>): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_no_subset_gap(&diagnostics);
    assert_eq!(
        rows(&diagnostics),
        vec![("check.type", 5, 13), ("check.type", 8, 16)],
    );
}

/// R25 · named types — a refused nominal still occupies its name, so a redeclaration
/// conflicts. The duplicate check reads the ledger, which retains the refused
/// occurrence, rather than the accepted-only table it used to scan.
#[test]
fn r25_a_refused_nominal_occupies_its_name() {
    let diagnostics = diagnostics(
        "module main\n\n\
         type Age: int in 10..=0\n\
         type Age: int in 0..=150\n\n\
         pub fn make(a: Age): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert!(
        diagnostics
            .iter()
            .any(|row| row.code() == "check.name_conflict"),
        "the refused first declaration still holds the name: {:#?}",
        rows(&diagnostics),
    );
}

/// R25 · roots — a refused store root still occupies its placement name, so a second
/// store of that name is the repeat it always was. The duplicate check reads the
/// ledger, which retains the refused occurrence, rather than a list only admitted
/// roots reach.
#[test]
fn r25_a_refused_root_occupies_its_placement_name() {
    let diagnostics = diagnostics(
        "module main\n\n\
         resource Widget {\n\
         \x20   required name: string\n\
         }\n\n\
         store ^items[id: int]: Missing\n\
         store ^items[id: int]: Widget\n",
    );

    assert!(
        diagnostics.iter().any(|row| row
            .message()
            .contains("`^items` is declared more than once")),
        "the refused first declaration still holds the name: {:#?}",
        messages(&diagnostics),
    );
}

// ---------------------------------------------------------------------------
// Function signatures — I-5
//
// A refused parameter type pushes no parameter, and the signature is pushed
// anyway with a short parameter list while the image index advances past it.
// Today `FunctionRegistryOutcome::Refused` withholds the whole registry, so
// nothing observes either corruption; deleting that suppression (design §3) is
// what makes both reachable, which is why the fix and the deletion are one row.
// ---------------------------------------------------------------------------

/// The scheduler-ordered red for the truncated-signature arity corruption. A
/// function whose parameter type was refused must be refused whole: no signature
/// with a short parameter list may enter the table, and the image index must not
/// advance past a signature that was never built.
///
/// With a short list admitted, a call carrying the written number of arguments is
/// reported as an arity mismatch — a fabricated statement about the call, derived
/// from the compiler's own truncation of the declaration.
#[test]
#[ignore = "sequenced behind the function ledger and the `CompleteFunctionRegistry` \
            amendment (design §3): `FunctionRegistryOutcome::Refused` withholds the \
            registry today, so no call resolves against the truncated signature and \
            the corruption is unobservable through the production pipeline"]
fn i5_a_refused_parameter_type_never_truncates_its_signature() {
    let diagnostics = diagnostics(
        "module main\n\n\
         fn helper(a: Nope, b: int): int {\n\
         \x20   return b\n\
         }\n\n\
         pub fn other(): int {\n\
         \x20   return helper(1, 2)\n\
         }\n",
    );

    for row in &diagnostics {
        assert!(
            !row.message().contains("argument"),
            "`helper` is written with two parameters; no row may describe the call \
             as carrying the wrong number of them: {:#?}",
            messages(&diagnostics),
        );
    }
    assert_eq!(
        rows(&diagnostics),
        vec![("check.unsupported", 3, 14), ("check.unsupported", 8, 12)],
        "the parameter reports the cause and the call is steered to it",
    );
}
