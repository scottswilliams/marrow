//! Declared-entry causality: a declaration the compiler refused keeps its name.
//!
//! A namespace that drops a refused declaration makes every later lookup read as
//! *never declared*, so the use site reports a fabricated absence — "is not in
//! scope" — for a name the reader can see declared, and reports it once per use.
//! Under the declaration ledger a refused key answers `Refused`: the declaring
//! cause is reported once at the declaration, the first use is steered to it, and
//! later uses fail silently.
//!
//! Diagnostics are asserted by code, span, and count, and a steer additionally by
//! the typed `refused_declaration()` facts it carries — the ledger holding the
//! refusal, the declaring code, and where that report sits. The typed facts are the
//! only discriminator available: a steer and a row about a name that was never
//! declared sit at the same span under the same code, because the steer reuses the
//! *declaring* code rather than minting one of its own.
//!
//! The cascade guard is negative and equally typed: no row carries an `Unresolved`
//! payload naming a declared name, the fabrication these fixtures exist to rule out.

use marrow_codes::Code;
use marrow_compile::{
    CompileFailure, DeclarationNamespace, InputRevision, NameFamily, RefusalReport,
    RefusedDeclaration, SourceDiagnostic, Steer, analyze, compile,
};
use marrow_compile::{ResourceLimitKind, SourceStage};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};
use std::collections::BTreeSet;

#[path = "common/ids.rs"]
mod ids;
#[path = "declaration_causality/name_conflicts.rs"]
mod name_conflicts;
#[path = "common/project.rs"]
mod project_capture;
#[path = "declaration_causality/refused_names.rs"]
mod refused_names;

fn project(source: &str) -> ProjectInput {
    files(&[("src/main.mw", source.to_string())])
}

fn files(sources: &[(&str, String)]) -> ProjectInput {
    captured(sources, None)
}

fn captured(sources: &[(&str, String)], ids: Option<&[u8]>) -> ProjectInput {
    let borrowed: Vec<(&str, &str)> = sources
        .iter()
        .map(|(path, source)| (*path, source.as_str()))
        .collect();
    project_capture::project_with_ids(&borrowed, ids)
}

/// A project whose durable identity ledger is complete, so no store refuses for a
/// missing anchor.
///
/// The identity gap is the one refusal class entitled to the "see the
/// `check.durable_identity` reports" steer, and with no ledger *every* store refuses
/// that way first, so any other durable refusal class has to mint past it.
fn with_minted_ids(sources: &[(&str, String)]) -> ProjectInput {
    ids::minted(|ledger| captured(sources, ledger))
}

fn diagnostics(source: &str) -> Vec<SourceDiagnostic> {
    diagnostics_of(&project(source))
}

fn diagnostics_of(project: &ProjectInput) -> Vec<SourceDiagnostic> {
    match compile(project) {
        Ok(compiled) => panic!("expected a refused declaration, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}

/// Every row the resilient analysis snapshot reports, across all three stages.
///
/// The staged production projection returns the first non-empty stage, so a project
/// whose dependency did not parse never projects its semantic terminal. The snapshot
/// is the production path that does observe it, and it is what an editor renders.
fn analyzed(sources: &[(&str, String)]) -> Vec<SourceDiagnostic> {
    snapshot_diagnostics(files(sources))
}

fn snapshot_diagnostics(input: ProjectInput) -> Vec<SourceDiagnostic> {
    let Ok(snapshot) = analyze(std::sync::Arc::new(input), InputRevision::new(1)) else {
        panic!("a resilient analysis snapshot is produced");
    };
    snapshot.diagnostics().to_vec()
}

/// Every row, as `(file, code, line, column)` — the shape these fixtures assert.
///
/// The file is part of the shape, not context: a multi-file fixture that asserted
/// only `(code, line, column)` would pass with every row attributed to the wrong
/// module, which is the class of defect a project-wide namespace makes possible.
fn rows(diagnostics: &[SourceDiagnostic]) -> Vec<(&str, Code, u32, u32)> {
    diagnostics
        .iter()
        .map(|row| {
            let span = row.span();
            (row.file().as_str(), row.code(), span.line, span.column)
        })
        .collect()
}

/// The last row is a typed steer to a refused declaration, carrying exactly these
/// facts.
///
/// A steer and a row about a name that was never declared are
/// `(code, line, column)`-identical — the steer reuses the *declaring* code rather
/// than minting one of its own — so the discriminator has to be the typed payload.
/// Asserting on prose instead would test the sentence, not the contract.
fn assert_steers_to(
    diagnostics: &[SourceDiagnostic],
    namespace: DeclarationNamespace,
    declaring_code: Code,
    report: RefusalReport,
) {
    let last = diagnostics
        .last()
        .expect("a steered use reports at least one row");
    let steer = last.refused_declaration().unwrap_or_else(|| {
        panic!(
            "the use is steered to a declaration this project refused, so its row \
             carries the typed facts: {:#?}",
            rows(diagnostics),
        )
    });
    assert_eq!(
        (steer.namespace, steer.declaring_code, steer.report),
        (Some(namespace), declaring_code, report),
        "the steer names the ledger holding the refusal, the code of the report the \
         reader must act on, and where that report sits: {:#?}",
        rows(diagnostics),
    );
}

/// No row denies that `name` was declared.
///
/// The discriminator is the typed payload, not the sentence: a steer to the refused
/// declaration and a row about a name that never existed are `(code, line, column)`-identical,
/// and only an `Unresolved` payload says the compiler found no declaration at all.
fn assert_never_out_of_scope(diagnostics: &[SourceDiagnostic], name: &str) {
    for row in diagnostics {
        assert!(
            row.unresolved()
                .is_none_or(|unresolved| unresolved.name != name),
            "`{name}` is declared in this source; no row may call it out of scope: {:#?}",
            rows(diagnostics),
        );
    }
}

/// a constant refused for a type mismatch is reported at its declaration and
/// its use is steered to that report, never called out of scope.
#[test]
fn a_type_refused_constant_is_not_out_of_scope_at_its_use() {
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
        vec![
            ("src/main.mw", Code::CheckType, 3, 1),
            ("src/main.mw", Code::CheckType, 6, 12)
        ],
        "the declaration reports the cause and the use is steered to it",
    );
}

/// a constant refused for a non-literal value behaves the same, and the steer
/// reuses the declaring code (`check.unsupported`), not the use site's own.
#[test]
fn a_value_refused_constant_steers_with_the_declaring_code() {
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
        vec![
            ("src/main.mw", Code::CheckUnsupported, 3, 15),
            ("src/main.mw", Code::CheckUnsupported, 6, 12)
        ],
        "the steer carries the declaring cause's code, so a use-site assertion \
         names the declaration's typed identity",
    );
}

/// the report is once per refused key, not once per use. Two uses of one
/// refused constant produce the declaring row and exactly one steer.
#[test]
fn a_refused_constant_is_reported_once_across_many_uses() {
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
    assert_eq!(
        rows(&diagnostics)[0],
        ("src/main.mw", Code::CheckType, 3, 1)
    );
}

/// a refused declaration still occupies its name, in both orders. The
/// duplicate check sees the refused occurrence, so the second declaration is a
/// name conflict whether the refused one came first or second.
#[test]
fn a_refused_constant_occupies_its_name_when_declared_first() {
    let diagnostics = diagnostics(
        "module main\n\n\
         const limit = 1 + 2\n\
         const limit = 5\n\n\
         pub fn read(): int {\n\
         \x20   return limit\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckUnsupported, 3, 15),
            ("src/main.mw", Code::CheckNameConflict, 4, 1),
            ("src/main.mw", Code::CheckUnsupported, 7, 12),
        ],
        "a refused declaration occupies its name, so the redeclaration conflicts: {:#?}",
        rows(&diagnostics),
    );
}

/// The sibling direction: the refused occurrence comes second.
#[test]
fn a_refused_constant_occupies_its_name_when_declared_second() {
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
        vec![("src/main.mw", Code::CheckNameConflict, 4, 1)],
        "the accepted first declaration answers the use; only the conflict reports",
    );
}

/// the retained names are bounded. A project whose refused declarations would
/// retain more than the ledger's declared ceiling stops with the typed resource
/// limit. It never drops a key to stay under budget, which is the one outcome that
/// would put a fabricated absence back at every use of the dropped name.
///
/// Neither the image bounds nor the diagnostic ceiling bounds this retention: a
/// refused declaration never reaches the encoder, and a collector at its ceiling
/// keeps admitting and discarding while the pass runs on.
#[test]
fn crossing_the_ledger_ceiling_is_a_typed_resource_limit() {
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

/// The ceiling is one budget for the whole pass, not one per namespace: six production
/// ledgers each holding the declared ceiling would retain six times the stated bound.
/// The retention this term bounds is what the pass holds while the diagnostic collector
/// is live.
#[test]
fn the_ledger_ceiling_is_one_budget_across_namespaces() {
    // Neither half crosses the ceiling alone: each retains about 600 names of a
    // thousand bytes against a 1 MiB bound. Together they do.
    let wide = "n".repeat(1000);
    let mut constants = String::from("module main\n\n");
    let mut aliases = String::from("module more\n\n");
    for index in 0..600 {
        constants.push_str(&format!("const {wide}{index} = 1 + 2\n"));
        aliases.push_str(&format!("alias N{wide}{index} = Nope\n"));
    }
    let project = files(&[("src/main.mw", constants), ("src/more.mw", aliases)]);

    match compile(&project) {
        Err(CompileFailure::ResourceLimit(limit)) => {
            assert_eq!(limit.kind(), ResourceLimitKind::DeclarationLedgerBytes);
        }
        other => panic!("expected the ledger's typed ceiling, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Durable roots
//
// A `store` root refused for any reason other than a missing ledger identity is
// dropped from the registry entirely, so every `^root` reference reads as an
// unknown name. The identity class is the one class retained, and the one class
// entitled to the "see the `check.durable_identity` reports" steer: any other
// refusal site naming that steer points at reports that were never made.
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
            .all(|row| row.code() != Code::CheckDurableIdentity),
        "the fixture must isolate a non-identity refusal: {:#?}",
        messages(diagnostics),
    );
}

/// a store root refused because its resource is undeclared is reported at its
/// declaration, and a write through it is steered to that report. Dropping the root
/// whole would make the write say `items` is not in scope, of a root declared two
/// lines above.
#[test]
fn a_root_refused_for_its_resource_is_not_out_of_scope_at_a_write() {
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
        vec![
            ("src/main.mw", Code::CheckType, 3, 1),
            ("src/main.mw", Code::CheckType, 7, 9)
        ],
        "the declaration reports the cause and the write is steered to it",
    );
}

/// the same root through a `place` binding. The sibling lookup, not only
/// `resolve_root`'s write path, must reuse the declaring cause.
#[test]
fn a_root_refused_for_its_resource_is_not_out_of_scope_at_a_place() {
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
        ("src/main.mw", Code::CheckType, 3, 1),
        "the declaration still owns the cause: {:#?}",
        rows(&diagnostics),
    );
}

/// the steer is once per refused root, not once per reference. Ten uses of one
/// refused root produce the declaring row and exactly one steer.
#[test]
fn a_refused_root_is_reported_once_across_many_uses() {
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

/// a refused root is still offered as a did-you-mean. Dropping the key removes
/// it from the correction corpus too, so a near miss on a refused root gets no
/// suggestion at all.
#[test]
fn a_refused_root_is_offered_as_a_did_you_mean() {
    let diagnostics = diagnostics(
        "module main\n\n\
         store ^items[id: int]: Widget\n\n\
         pub fn write() {\n\
         \x20   transaction {\n\
         \x20       ^itmes[1].name = \"a\"\n\
         \x20   }\n\
         }\n",
    );

    let steers: Vec<&Steer> = diagnostics
        .iter()
        .filter_map(SourceDiagnostic::steer)
        .collect();
    assert_eq!(
        steers,
        [&Steer::DidYouMean {
            family: NameFamily::Root,
            candidate: "items".to_string(),
        }],
        "a genuinely undeclared root is still an unknown name, corrected against the \
         refused root's retained key: {:#?}",
        messages(&diagnostics),
    );
}

/// a resource-keyed durable lookup answers the store's refusal, not an absence. The
/// branch constructor `Resource.branch(…)` resolves through the store backing
/// `Resource`; a scan of the executable roots answers `None` for a refused store, so
/// without the steer the call falls through to the method-shaped-call report and blames
/// the language for the store's own reported defect.
#[test]
fn a_branch_constructor_of_a_refused_store_names_the_stores_cause() {
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

    assert_steers_to(
        &diagnostics,
        DeclarationNamespace::DurableRoot,
        Code::CheckDurableIdentity,
        RefusalReport::AtDeclaration,
    );
    // The identity class is the one refusal whose cause is a report *family*
    // rather than a single row, so its steer names that family — the same row
    // every other reference to a store refused for a missing identity receives.
    assert_eq!(
        rows(&diagnostics).last().copied(),
        Some(("src/main.mw", Code::CheckType, 14, 15)),
        "the constructor is steered to the store's own cause: {:#?}",
        messages(&diagnostics),
    );
    assert!(
        messages(&diagnostics)
            .last()
            .is_some_and(|message| message.contains("failed identity admission")),
        "{:#?}",
        messages(&diagnostics),
    );
}

/// the same for the record-keyed steer. A materialized resource value names a
/// member that is neither a field nor, as far as this compilation knows, a branch:
/// the store that would have built the branch tree was refused, so reporting the
/// record as having no such field states as fact something the compiler cannot know.
#[test]
fn a_branch_named_on_a_refused_stores_resource_names_the_stores_cause() {
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
        Some(("src/main.mw", Code::CheckType, 15, 15)),
        "{:#?}",
        messages(&diagnostics),
    );
    assert!(
        messages(&diagnostics)
            .last()
            .is_some_and(|message| message.contains("failed identity admission")),
        "{:#?}",
        messages(&diagnostics),
    );
}

/// `Bound` — a root refused for crossing a fixed compiler-owned bound keeps
/// its name and reuses its own `check.resource_limit` cause. It must not claim an
/// identity admission failure.
#[test]
fn a_root_refused_for_a_key_tuple_bound_reuses_its_own_cause() {
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
            .filter(|row| row.code() == Code::CheckResourceLimit)
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

/// `ValueCycle` — the one refusal site that pushes no diagnostic of its own. Its cause
/// is the `check.recursion` report from the value-cycle pass, which runs after lowering,
/// so this asserts set membership and that the steer carries that cause, never an
/// identity admission claim.
#[test]
fn a_root_refused_for_a_value_cycle_names_the_recursion_cause() {
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
    assert_not_steered_to_identity(&diagnostics);
    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckRecursion, 17, 9),
            ("src/main.mw", Code::CheckRecursion, 3, 8),
        ],
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
// Resource members
//
// The one namespace that drops a *member* and keeps the declaration, so the record
// survives with a silently narrowed field set and every lookup of the dropped
// member makes a false statement about the source.
// ---------------------------------------------------------------------------

/// The over-suppression guard: a field that really is not declared still says so.
/// Member granularity must distinguish a member the compiler refused from one the
/// source never wrote; suppressing both would trade a false absence for a missing
/// report.
#[test]
fn a_genuinely_absent_field_is_still_reported_as_absent() {
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

/// a resource member the compiler refused, then named. The member is dropped from the
/// record while the record survives, so without the steer the constructor reports that
/// the resource has no such field — four lines after the compiler diagnosed it.
#[test]
fn a_refused_resource_member_is_not_absent_at_its_use() {
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
        vec![
            ("src/main.mw", Code::CheckUnsupported, 5, 10),
            ("src/main.mw", Code::CheckUnsupported, 9, 38)
        ],
    );
}

/// a refused resource member must not narrow the identity-gap anchor set. A
/// member dropped from the record is never anchored, so the durable graph reports
/// fewer `check.durable_identity` rows than the same program with a valid member
/// type, and the mint action that consumes those rows mints an incomplete ledger.
#[test]
fn a_refused_member_does_not_narrow_the_identity_gap_set() {
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
// Named types
//
// Every named-type lookup funnels into one untyped bucket, so a use of a type this
// project declared and the compiler refused is reported as a *language* gap: "not
// yet supported on the beta line". The declaration is dropped from its table (so
// nothing resolves against a broken type) and the ledger keeps its name (so the use
// is steered to the cause instead).
// ---------------------------------------------------------------------------

/// A struct refused for a bad field. The construction resolves through the struct
/// table, not through annotation resolution, so without the steer it reports `Point`
/// out of scope — of a struct declared six lines above and already diagnosed.
#[test]
fn a_refused_struct_is_not_out_of_scope_at_its_construction() {
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
        vec![
            ("src/main.mw", Code::CheckUnsupported, 4, 8),
            ("src/main.mw", Code::CheckUnsupported, 9, 15)
        ],
        "the field reports the cause and the construction is steered to it",
    );
}

/// an enum refused for a bad payload. A qualified `Enum::member` is a third resolution
/// path, which without the steer reports the *spelling* as unsupported rather than the
/// enum this project declared and the compiler refused.
#[test]
fn a_refused_enum_steers_its_qualified_use_to_the_payload_report() {
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
        vec![
            ("src/main.mw", Code::CheckUnsupported, 4, 15),
            ("src/main.mw", Code::CheckUnsupported, 9, 15)
        ],
    );
}

/// One refused type declaration, and the annotation that must be steered to its cause.
struct AnnotationSteerCase {
    /// The refusal under test.
    refusal: &'static str,
    source: &'static str,
    /// The code the declaration was refused under, which the steer reuses so one code
    /// leads to one fix.
    declaring_code: Code,
    /// Every reported row as `(line, column)`; each carries `declaring_code`.
    rows: &'static [(u32, u32)],
}

/// A type annotation naming a declaration the compiler refused reports that
/// declaration's own code, never a subset-gap phrase about the language.
const ANNOTATION_STEERS: &[AnnotationSteerCase] = &[
    AnnotationSteerCase {
        refusal: "an alias over an unknown target",
        source: "module main\n\n\
                 alias Count = Nope\n\n\
                 pub fn make(c: Count): int {\n\
                 \x20   return 1\n\
                 }\n",
        declaring_code: Code::CheckType,
        rows: &[(3, 1), (5, 16)],
    },
    AnnotationSteerCase {
        refusal: "a cyclic alias chain",
        source: "module main\n\n\
                 alias A = B\n\
                 alias B = A\n\n\
                 pub fn make(c: A): int {\n\
                 \x20   return 1\n\
                 }\n",
        declaring_code: Code::CheckRecursion,
        rows: &[(3, 7), (4, 7), (6, 16)],
    },
    AnnotationSteerCase {
        refusal: "a nominal type whose interval admits no values",
        source: "module main\n\n\
                 type Age: int in 10..=0\n\n\
                 pub fn make(a: Age): int {\n\
                 \x20   return 1\n\
                 }\n",
        declaring_code: Code::CheckType,
        rows: &[(3, 18), (5, 16)],
    },
];

#[test]
fn a_refused_type_declaration_steers_its_annotation_to_its_own_report() {
    for case in ANNOTATION_STEERS {
        let diagnostics = diagnostics(case.source);

        assert_steers_to(
            &diagnostics,
            DeclarationNamespace::NamedType,
            case.declaring_code,
            RefusalReport::AtDeclaration,
        );
        let expected: Vec<(&str, Code, u32, u32)> = case
            .rows
            .iter()
            .map(|(line, column)| ("src/main.mw", case.declaring_code, *line, *column))
            .collect();
        assert_eq!(
            rows(&diagnostics),
            expected,
            "{}: the declaration reports the cause and the annotation is steered to it: \
             {:#?}",
            case.refusal,
            messages(&diagnostics),
        );
    }
}

/// the cascade split. A refused declaration in one parameter must not absorb
/// a genuinely missing name in the next: each parameter is rejected at its own span
/// with its own cause. Base: two identical subset-gap rows, so the real absence and
/// the project's own refusal read the same.
#[test]
fn a_refused_type_does_not_absorb_a_genuine_absence_beside_it() {
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
            ("src/main.mw", Code::CheckType, 3, 1),
            ("src/main.mw", Code::CheckType, 5, 16),
            ("src/main.mw", Code::CheckUnsupported, 5, 26),
        ],
        "`a` is steered to the alias's cause; `b` names a type nothing declared and \
         keeps the subset-gap report",
    );
}

/// the merge never widens. A refused alias used inside a generic application
/// must not be described as the cause of anything but itself: no row may claim the
/// genuinely absent `AlsoMissing` was refused.
#[test]
fn a_refused_type_is_never_named_as_another_names_cause() {
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
        vec![
            ("src/main.mw", Code::CheckType, 8, 1),
            ("src/main.mw", Code::CheckType, 10, 16)
        ],
    );
}

/// a generic template's defect is reported at its declaration.
///
/// A template's member types are resolved per instantiation, so a member naming an
/// undeclared type must still report its cause at the declaration: reporting only at
/// a *construction* site blames the construction and reports the declaring cause
/// zero times.
#[test]
fn a_refused_template_is_reported_at_its_declaration() {
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
        vec![
            ("src/main.mw", Code::CheckType, 5, 13),
            ("src/main.mw", Code::CheckType, 9, 15)
        ],
        "the member reports the cause and the construction is steered to it",
    );
}

/// The same template with no use at all. Reporting only at a construction site would
/// let a declaration nothing constructs compile clean at exit zero.
#[test]
fn a_refused_template_is_reported_even_with_no_use() {
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

    assert_eq!(
        rows(&diagnostics),
        vec![("src/main.mw", Code::CheckType, 5, 13)]
    );
}

/// the same template named in an annotation. The generic application resolves its head
/// through the template table, which without the steer reports a subset gap for a
/// template this project declared.
#[test]
fn a_refused_template_steers_its_annotation_to_the_member_report() {
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

    assert_steers_to(
        &diagnostics,
        DeclarationNamespace::NamedType,
        Code::CheckType,
        RefusalReport::AtDeclaration,
    );
    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckType, 5, 13),
            ("src/main.mw", Code::CheckType, 8, 16)
        ],
    );
}

/// named types — a refused nominal still occupies its name, so a redeclaration
/// conflicts. The duplicate check reads the ledger, which retains the refused
/// occurrence, rather than the accepted-only table.
#[test]
fn a_refused_nominal_occupies_its_name() {
    let diagnostics = diagnostics(
        "module main\n\n\
         type Age: int in 10..=0\n\
         type Age: int in 0..=150\n\n\
         pub fn make(a: Age): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckType, 3, 18),
            ("src/main.mw", Code::CheckNameConflict, 4, 6),
            ("src/main.mw", Code::CheckType, 6, 16),
        ],
        "the refused first declaration still holds the name: {:#?}",
        rows(&diagnostics),
    );
}

/// roots — a refused store root still occupies its placement name, so a second
/// store of that name is the repeat it always was. The duplicate check reads the
/// ledger, which retains the refused occurrence, rather than a list only admitted
/// roots reach.
#[test]
fn a_refused_root_occupies_its_placement_name() {
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

/// members — a resource member occupies its name against a repeat. Two members of one
/// name have no unambiguous slot in the record, and only a ledger that retains every
/// occurrence can see the repeat at all: a plain vector admits both and lets the first
/// silently win every later lookup.
#[test]
fn a_resource_member_occupies_its_name() {
    let diagnostics = diagnostics(
        "module main\n\n\
         resource Widget {\n\
         \x20   required name: string\n\
         \x20   required name: int\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   const w = Widget(name: \"a\")\n\
         \x20   return 1\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![("src/main.mw", Code::CheckNameConflict, 5, 14)],
        "the second member of one name is a repeat at its name, not a second slot: {:#?}",
        messages(&diagnostics),
    );
}

// ---------------------------------------------------------------------------
// Function signatures
//
// A signature is refused whole. A parameter type the compiler cannot resolve
// pushes no parameter, so admitting the short list would report the call as
// carrying the wrong number of arguments, and the image index must not advance
// past a signature that was never built.
// ---------------------------------------------------------------------------

/// A function whose parameter type was refused is refused whole. With a short list
/// admitted, a call carrying the written number of arguments is reported as an arity
/// mismatch — a fabrication derived from the compiler's own truncation.
#[test]
fn a_refused_parameter_type_never_truncates_its_signature() {
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
        vec![
            ("src/main.mw", Code::CheckUnsupported, 3, 14),
            ("src/main.mw", Code::CheckUnsupported, 8, 12)
        ],
        "the parameter reports the cause and the call is steered to it",
    );
}

/// A signature refused behind an accepted duplicate of its name reports its cause
/// once. The repeat is the duplicate check's own report; the refused annotation is
/// the compiler's reason for refusing that occurrence, and the body-lowering guard
/// has to consult the occurrence it is lowering rather than the name — a name-keyed
/// guard reads the accepted duplicate, lowers the refused body, and re-resolves the
/// annotation that was already reported.
#[test]
fn a_signature_refused_behind_an_accepted_duplicate_reports_its_cause_once() {
    let diagnostics = diagnostics(
        "module main\n\n\
         fn dup(a: int): int {\n\
         \x20   return a\n\
         }\n\n\
         fn dup(a: Nope): int {\n\
         \x20   return 1\n\
         }\n\n\
         pub fn caller(): int {\n\
         \x20   return dup(1)\n\
         }\n",
    );

    assert_eq!(
        diagnostics
            .iter()
            .filter(|row| row.code() == Code::CheckUnsupported)
            .count(),
        1,
        "the refused parameter type is the declaration's own cause, reported at it \
         once: {:#?}",
        messages(&diagnostics),
    );
}

/// The return-type sibling of the same guard: the refused occurrence's return
/// annotation is resolved once, by the signature build, not again by its body.
#[test]
fn a_return_type_refused_behind_an_accepted_duplicate_reports_its_cause_once() {
    let diagnostics = diagnostics(
        "module main\n\n\
         fn dup(): int {\n\
         \x20   return 1\n\
         }\n\n\
         fn dup(): Nope {\n\
         \x20   return 1\n\
         }\n\n\
         pub fn caller(): int {\n\
         \x20   return dup()\n\
         }\n",
    );

    assert_eq!(
        diagnostics
            .iter()
            .filter(|row| row.code() == Code::CheckUnsupported)
            .count(),
        1,
        "the refused return type is the declaration's own cause, reported at it \
         once: {:#?}",
        messages(&diagnostics),
    );
}

// ---------------------------------------------------------------------------
// Modules
//
// A module the project contains but did not admit — a header that disagrees
// with its path, a file that did not parse, a file that is not UTF-8 — is
// dropped from the module set, so a `use` of it reports that the project has no
// such module and a qualified call reports the callee out of scope. Both are
// false statements about a file the reader can see.
// ---------------------------------------------------------------------------

/// a module whose header disagrees with its path is refused, not absent: the import
/// names the refusal, and the qualified call is steered to the header report rather
/// than told its callee is out of scope.
#[test]
fn a_module_refused_for_its_header_is_not_absent_at_its_import() {
    let diagnostics = diagnostics_of(&files(&[
        (
            "src/main.mw",
            "module main\n\n\
             use helper\n\n\
             pub fn run(): int {\n\
             \x20   return helper::twice(2)\n\
             }\n"
            .to_string(),
        ),
        (
            "src/helper.mw",
            "module wrong\n\n\
             pub fn twice(n: int): int {\n\
             \x20   return n\n\
             }\n"
            .to_string(),
        ),
    ]));

    assert_never_out_of_scope(&diagnostics, "helper::twice");
    assert_no_absent_module(&diagnostics, "helper");
    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/helper.mw", Code::CheckModulePath, 1, 1),
            ("src/main.mw", Code::CheckImport, 3, 1),
            ("src/main.mw", Code::CheckModulePath, 6, 12),
        ],
        "the header reports the cause, the import names it, and the call is \
         steered to it: {:#?}",
        messages(&diagnostics),
    );
}

/// the same for a module that did not parse. Its cause is the syntax report
/// an earlier stage already made, so the import and the call name that report
/// rather than denying the module or its callee exists.
///
/// Asserted through the resilient analysis snapshot, which is the production path
/// that observes these rows: the staged production projection returns the parse
/// stage's failure and never reaches the semantic terminal, so the fabrications at
/// stake here are the ones an editor is shown.
#[test]
fn a_module_refused_for_a_parse_error_is_not_absent_at_its_import() {
    let diagnostics = analyzed(&[
        (
            "src/main.mw",
            "module main\n\n\
             use helper\n\n\
             pub fn run(): int {\n\
             \x20   return helper::twice(2)\n\
             }\n"
            .to_string(),
        ),
        (
            "src/helper.mw",
            "module helper\n\n\
             pub fn twice(n: int): int {\n\
             \x20   return n +\n\
             }\n"
            .to_string(),
        ),
    ]);

    assert_never_out_of_scope(&diagnostics, "helper::twice");
    assert_no_absent_module(&diagnostics, "helper");
    let observed = rows(&diagnostics);
    assert_eq!(
        (observed[0].0, observed[0].1),
        ("src/helper.mw", Code::ParseSyntax),
        "the parse stage reports the cause first, in the module it refused: \
         {observed:?}",
    );
    assert_eq!(
        observed[1..],
        [
            ("src/main.mw", Code::CheckImport, 3, 1),
            ("src/main.mw", Code::ParseSyntax, 6, 12)
        ],
        "the import names the parse report and the call is steered to it: {:#?}",
        messages(&diagnostics),
    );
}

/// The same for a file that is not UTF-8: it never entered parsing, and the stage
/// that refused it is the decode, so that is the report the steer names.
#[test]
fn a_module_refused_for_invalid_utf8_is_not_absent_at_its_import() {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let main = "module main\n\n\
                use helper\n\n\
                pub fn run(): int {\n\
                \x20   return helper::twice(2)\n\
                }\n";
    let captured = vec![
        CapturedFile::new("src/helper.mw".to_string(), vec![0xff, 0xfe, 0x00]),
        CapturedFile::new("src/main.mw".to_string(), main.as_bytes().to_vec()),
    ];
    let input = marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project");
    let diagnostics = snapshot_diagnostics(input);

    assert_never_out_of_scope(&diagnostics, "helper::twice");
    assert_no_absent_module(&diagnostics, "helper");
    assert_eq!(
        rows(&diagnostics)[1..],
        [
            ("src/main.mw", Code::CheckImport, 3, 1),
            ("src/main.mw", Code::CheckUnsupported, 6, 12)
        ],
        "the import names the decode report and the call is steered to it: {:#?}",
        messages(&diagnostics),
    );
}

/// A `use` of a module the project genuinely does not contain still says so: the
/// causal arm must not swallow a real absence.
#[test]
fn a_genuinely_absent_module_is_still_reported_as_absent() {
    let diagnostics = diagnostics_of(&files(&[(
        "src/main.mw",
        "module main\n\n\
         use helper\n\n\
         pub fn run(): int {\n\
         \x20   return 1\n\
         }\n"
        .to_string(),
    )]));

    assert_eq!(
        rows(&diagnostics),
        vec![("src/main.mw", Code::CheckImport, 3, 1)]
    );
    assert!(
        diagnostics[0]
            .message()
            .contains("no module `helper` in this project"),
        "the project contains no such file, so the absence is the truth: {:#?}",
        messages(&diagnostics),
    );
}

/// A module the project contains is never described as one it does not contain.
fn assert_no_absent_module(diagnostics: &[SourceDiagnostic], name: &str) {
    for row in diagnostics {
        assert!(
            !row.message()
                .contains(&format!("no module `{name}` in this project")),
            "`{name}` is a file of this project; no row may deny it: {:#?}",
            messages(diagnostics),
        );
    }
}

/// A refused signature stops its own declaration, not the whole project: an
/// unrelated body still lowers and reports its own error. Withholding the whole
/// registry instead would let one bad annotation hide every other diagnostic.
#[test]
fn a_refused_signature_does_not_silence_an_unrelated_body() {
    let diagnostics = diagnostics(
        "module main\n\n\
         fn helper(a: Nope): int {\n\
         \x20   return 1\n\
         }\n\n\
         pub fn other(): int {\n\
         \x20   return missingFn()\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckUnsupported, 3, 14),
            ("src/main.mw", Code::CheckType, 8, 12)
        ],
        "the refused signature reports its own cause and the unrelated body still \
         reports its own: {:#?}",
        messages(&diagnostics),
    );
}

/// Reusing a cause never lets a body through: a `Binding::Refused` lookup fails its
/// body exactly as a `Binding::Absent` one does. Otherwise an unavailable artifact
/// becomes available and a fabricated image reaches `encode`, so the asserted outcome
/// is the diagnostic refusal — never a compiled program, never the empty terminal.
#[test]
fn a_reused_cause_never_admits_the_body_that_reused_it() {
    let source = "module main\n\n\
                  fn helper(a: Nope): int {\n\
                  \x20   return 1\n\
                  }\n\n\
                  pub fn other(): int {\n\
                  \x20   return helper(1)\n\
                  }\n";
    match compile(&project(source)) {
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            let diagnostics: Vec<SourceDiagnostic> = diagnostics.into_iter().collect();
            assert_eq!(
                rows(&diagnostics),
                vec![
                    ("src/main.mw", Code::CheckUnsupported, 3, 14),
                    ("src/main.mw", Code::CheckUnsupported, 8, 12)
                ],
                "the call reuses the signature's cause and still refuses: {:#?}",
                messages(&diagnostics),
            );
        }
        other => panic!("a reused cause must still refuse the program, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Function parameters
//
// A parameter whose type is refused pushes no local and leaves no record of the
// name, so every use in the body reports a fabricated absence — once per use.
// ---------------------------------------------------------------------------

/// The non-generic path: with every body lowering, a refused parameter must not
/// make its own name unknown.
#[test]
fn a_refused_parameter_is_not_out_of_scope_in_its_own_body() {
    let diagnostics = diagnostics(
        "module main\n\n\
         fn helper(p: Nope, t: int): int {\n\
         \x20   const a = p\n\
         \x20   const b = p\n\
         \x20   return t\n\
         }\n\n\
         pub fn other(): int {\n\
         \x20   return 2\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "p");
    assert_eq!(
        rows(&diagnostics),
        vec![("src/main.mw", Code::CheckUnsupported, 3, 14)],
        "the parameter type reports the cause and its uses reuse it: {:#?}",
        messages(&diagnostics),
    );
}

/// the generic path, which bypasses the signature registry and so can amplify per use.
/// Two uses of the refused parameter add no further row.
#[test]
fn a_refused_generic_parameter_is_reported_once_not_once_per_use() {
    let diagnostics = diagnostics(
        "module main\n\n\
         fn helper<T>(p: Nope, t: T): int {\n\
         \x20   const a = p\n\
         \x20   const b = p\n\
         \x20   return 1\n\
         }\n\n\
         pub fn other(): int {\n\
         \x20   return helper(1, 2)\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "p");
    assert_eq!(
        rows(&diagnostics)
            .iter()
            .filter(|(_, _, line, _)| *line == 4 || *line == 5)
            .count(),
        0,
        "the two uses of the refused parameter add no row of their own: {:#?}",
        messages(&diagnostics),
    );
    assert_eq!(
        rows(&diagnostics)[0],
        ("src/main.mw", Code::CheckUnsupported, 3, 17),
        "the parameter type reports the cause once, at its own span: {:#?}",
        messages(&diagnostics),
    );
}

/// The sibling of the leaf probe, inside an unkeyed group: a nested group repeats a
/// name the group already declares, so it is a name conflict rather than a second
/// entry under one interned name. The nested group is refused either way — the
/// beta line does not admit one — but the repeat is what the reader has to fix.
#[test]
fn a_nested_group_repeating_a_member_name_is_a_name_conflict() {
    let diagnostics = diagnostics(
        "module main\n\n\
         resource Widget {\n\
         \x20   required name: string\n\
         \x20   notes {\n\
         \x20       required body: string\n\
         \x20       body {\n\
         \x20           required text: string\n\
         \x20       }\n\
         \x20   }\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   const w = Widget(name: \"a\")\n\
         \x20   return 1\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckNameConflict, 7, 9),
            ("src/main.mw", Code::CheckType, 14, 15),
        ],
        "the nested group repeats `body`, which the group already declares: {:#?}",
        messages(&diagnostics),
    );
}

// ---------------------------------------------------------------------------
// Local bindings
//
// A local binding whose initializer failed is recorded, so a later use reuses
// that cause. The annotation branch beside it returned without recording, so a
// binding refused for its annotation was unknown at every use — once per use.
// ---------------------------------------------------------------------------

/// A binding refused for its type annotation is not out of
/// scope at its uses, and the two uses add no row of their own.
#[test]
fn a_binding_refused_for_its_annotation_is_not_out_of_scope_at_its_uses() {
    let diagnostics = diagnostics(
        "module main\n\n\
         pub fn read(): int {\n\
         \x20   const x: Nope = 1\n\
         \x20   const a = x\n\
         \x20   const b = x\n\
         \x20   return 1\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "x");
    assert_eq!(
        rows(&diagnostics),
        vec![("src/main.mw", Code::CheckUnsupported, 4, 14)],
        "the annotation reports the cause and its uses reuse it: {:#?}",
        messages(&diagnostics),
    );
}

// ---------------------------------------------------------------------------
// Type-registry fill ordering
//
// Pass one reserves every value type's image index; pass two fills each body and
// records the verdict. A name resolved by an *earlier* fill pass binds the
// reservation, because the verdict the later pass reaches does not exist yet. When
// that later pass refuses the declaration, the earlier reference must still address
// a refused declaration — never a dropped one. A dropped target leaves the reference
// dangling, and a dangling reference raises a `GenericInvariant`, which outranks
// diagnostics: the reader would see a spanless `cli.compiler_invariant` instead of
// the `check.unsupported` row reported at the declaration.
//
// Both fill-ordering directions that can bind a refused leaf are covered:
// `fill_records` before the struct and enum `fill_rows` passes, and the struct pass
// filling one struct before a later sibling it names.
// ---------------------------------------------------------------------------

/// One member position naming a value type a later declaration pass refuses, and the
/// rows the reader is owed.
struct FillOrderCase {
    /// The naming position under test.
    naming: &'static str,
    source: &'static str,
    /// Every reported row as `(line, column)`; each carries `check.unsupported` at
    /// `src/main.mw`.
    rows: &'static [(u32, u32)],
}

/// Both fill-ordering directions that can bind a refused leaf: records filled before
/// structs and enums, and one struct filled before a later sibling it names. In every
/// direction the refused declaration's own row survives to the reader rather than being
/// replaced by a compiler invariant.
const FILL_ORDER_CASES: &[FillOrderCase] = &[
    FillOrderCase {
        naming: "a resource field naming a struct the later struct fill refuses",
        source: r#"module main

struct Bad {
    p: int?
}

resource R {
    required b: Bad
}

pub fn make(): int {
    return 1
}
"#,
        rows: &[(4, 8)],
    },
    FillOrderCase {
        naming: "a resource field naming an enum refused for an optional payload leaf",
        source: r#"module main

enum E {
    none
    some(a: int?)
}

resource R {
    required e: E
}

pub fn make(): int {
    return 1
}
"#,
        rows: &[(5, 13)],
    },
    FillOrderCase {
        naming: "the same enum with no resource field, which must report the same row",
        source: r#"module main

enum E {
    none
    some(a: int?)
}

pub fn make(): int {
    return 1
}
"#,
        rows: &[(5, 13)],
    },
    FillOrderCase {
        naming: "a struct field naming a later struct the same pass refuses",
        source: r#"module main

struct A {
    b: B
}

struct B {
    p: int?
}

pub fn make(): int {
    return 1
}
"#,
        rows: &[(8, 8)],
    },
    FillOrderCase {
        naming: "a struct field naming an enum the later enum fill refuses",
        source: r#"module main

struct A {
    e: E
}

enum E {
    none
    some(a: int?)
}

pub fn make(): int {
    return 1
}
"#,
        rows: &[(9, 13)],
    },
    FillOrderCase {
        naming: "a resource field aliasing a refused struct, which reaches the same \
                 reservation through alias expansion",
        source: r#"module main

alias Al = B

struct B {
    p: int?
}

resource R {
    required x: Al
}

pub fn make(): int {
    return 1
}
"#,
        rows: &[(6, 8), (3, 1)],
    },
];

#[test]
fn a_member_naming_a_refused_value_type_reports_that_types_own_cause() {
    for case in FILL_ORDER_CASES {
        let diagnostics = diagnostics(case.source);
        let expected: Vec<(&str, Code, u32, u32)> = case
            .rows
            .iter()
            .map(|(line, column)| ("src/main.mw", Code::CheckUnsupported, *line, *column))
            .collect();
        assert_eq!(
            rows(&diagnostics),
            expected,
            "{}: the refused declaration reports its own cause and the naming position \
             is steered to it: {:#?}",
            case.naming,
            messages(&diagnostics),
        );
    }
}

/// A struct on a containment cycle whose partner is refused: the cycle partner's
/// refusal drops it from the accepted set while the cycle member still names it.
/// The reader gets the refusal, never an invariant.
#[test]
fn a_cycle_partner_refused_for_its_own_cause_does_not_dangle() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct A {\n\
         \x20   b: B\n\
         }\n\n\
         struct B {\n\
         \x20   a: A\n\
         \x20   p: int?\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![("src/main.mw", Code::CheckUnsupported, 9, 8)],
        "the refused cycle partner reports its own cause: {:#?}",
        messages(&diagnostics),
    );
}

/// A refused struct still occupies its name for ordinary name resolution: a use
/// of it is steered to its cause, never resolved to the reserved-but-refused body.
#[test]
fn a_refused_struct_named_after_the_fill_is_still_steered() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct B {\n\
         \x20   p: int?\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   const b = B(p: 1)\n\
         \x20   return 1\n\
         }\n",
    );

    assert_never_out_of_scope(&diagnostics, "B");
    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckUnsupported, 4, 8),
            ("src/main.mw", Code::CheckUnsupported, 8, 15)
        ],
        "the refused struct keeps its name and steers its construction: {:#?}",
        messages(&diagnostics),
    );
}

// ---------------------------------------------------------------------------
// Static named-type projections — the use positions
//
// Annotation resolution, signature building, and body lowering read three static
// projections beside the two registry name lookups, and the verdict filter has to
// reach all five. An unfiltered projection answers a refused struct or enum with its
// reserved-but-unfilled row — a *live empty type* — and every use position then
// reasons against that empty shape and fabricates a statement about the source, or
// accepts it silently.
// ---------------------------------------------------------------------------

/// The refused value types these fixtures are written against: `Point` is refused
/// for an optional struct field, `Color` for a nominal enum payload. Both keep a
/// reserved row, and neither may answer a name.
const REFUSED_STRUCT: &str = "struct Point {\n\
                              \x20   x: int\n\
                              \x20   p: int?\n\
                              }\n";

/// One use position that names [`REFUSED_STRUCT`]'s `Point`, and where the steer lands.
struct StructUseCase {
    /// The use position under test.
    position: &'static str,
    /// The source written after `REFUSED_STRUCT`; its first line is source line 8.
    body: &'static str,
    /// Line and column of the steered row at the use site.
    steered_at: (u32, u32),
}

/// The three resolution entries into the same projection: a signature parameter, a
/// signature return, and a local annotation. Each must be steered to the declaration's
/// own cause rather than reasoning against the reserved-but-unfilled row.
const REFUSED_STRUCT_USES: &[StructUseCase] = &[
    StructUseCase {
        position: "a signature parameter type",
        body: "fn take(q: Point): int {\n\
               \x20   return 1\n\
               }\n\n\
               pub fn make(): int {\n\
               \x20   return 2\n\
               }\n",
        steered_at: (8, 12),
    },
    StructUseCase {
        position: "a signature return type",
        body: "pub fn make(): Point {\n\
               \x20   return 1\n\
               }\n",
        steered_at: (8, 16),
    },
    StructUseCase {
        position: "a local binding's annotation",
        body: "pub fn make(): int {\n\
               \x20   const q: Point = 1\n\
               \x20   return 2\n\
               }\n",
        steered_at: (9, 14),
    },
];

#[test]
fn a_refused_struct_named_in_any_use_position_steers_to_its_cause() {
    for case in REFUSED_STRUCT_USES {
        let diagnostics = diagnostics(&format!(
            "module main\n\n\
             {REFUSED_STRUCT}\n\
             {}",
            case.body,
        ));

        assert_steers_to(
            &diagnostics,
            DeclarationNamespace::NamedType,
            Code::CheckUnsupported,
            RefusalReport::AtDeclaration,
        );
        assert_eq!(
            rows(&diagnostics),
            vec![
                ("src/main.mw", Code::CheckUnsupported, 5, 8),
                (
                    "src/main.mw",
                    Code::CheckUnsupported,
                    case.steered_at.0,
                    case.steered_at.1,
                ),
            ],
            "{}: the struct field reports the cause and the use position is steered to \
             it, never described in terms of the unfilled row: {:#?}",
            case.position,
            messages(&diagnostics),
        );
    }
}

/// a field read through a parameter of the refused struct. The reserved row carries no
/// fields, so an unfiltered projection makes the read report that `Point` has no field
/// `x` — of a field declared four lines above and never diagnosed.
#[test]
fn a_field_read_on_a_refused_struct_is_not_a_missing_field() {
    let diagnostics = diagnostics(&format!(
        "module main\n\n\
         {REFUSED_STRUCT}\n\
         fn take(q: Point): int {{\n\
         \x20   return q.x\n\
         }}\n\n\
         pub fn make(): int {{\n\
         \x20   return 2\n\
         }}\n"
    ));

    for row in &diagnostics {
        assert!(
            !row.message().contains("field `x`"),
            "`x` is declared on `Point`; no row may say the struct has no such \
             field: {:#?}",
            messages(&diagnostics),
        );
    }
    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckUnsupported, 5, 8),
            ("src/main.mw", Code::CheckUnsupported, 8, 12),
        ],
        "the parameter is steered and its body reuses that cause: {:#?}",
        messages(&diagnostics),
    );
}

/// the enum sibling: a `match` over a parameter of a refused enum. The reserved row
/// carries no members, so an unfiltered projection reports every arm as naming a member
/// the enum does not have and the function as not returning on all paths — two
/// fabrications from one unfilled row.
#[test]
fn a_match_on_a_refused_enum_is_not_a_set_of_unknown_members() {
    let diagnostics = diagnostics(
        "module main\n\n\
         enum Color {\n\
         \x20   red\n\
         \x20   blue(a: int?)\n\
         }\n\n\
         fn pick(c: Color): int {\n\
         \x20   match c {\n\
         \x20       red => {\n\
         \x20           return 1\n\
         \x20       }\n\
         \x20       blue => {\n\
         \x20           return 2\n\
         \x20       }\n\
         \x20   }\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   return 2\n\
         }\n",
    );

    for row in &diagnostics {
        assert!(
            !row.message().contains("`red`") && !row.message().contains("`blue`"),
            "`red` and `blue` are declared members of `Color`; no arm may be \
             reported as naming an unknown one: {:#?}",
            messages(&diagnostics),
        );
    }
    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckUnsupported, 5, 13),
            ("src/main.mw", Code::CheckUnsupported, 8, 12),
        ],
        "the payload reports the cause and the parameter is steered to it: {:#?}",
        messages(&diagnostics),
    );
}

// ---------------------------------------------------------------------------
// Member position — the residual subset-gap phrase
//
// `unresolved_member_row` is documented as the one place a member-position
// resolution failure becomes a report, so a refused sibling can never be described
// as an unsupported language form. A member whose declared type names a refused
// sibling must be steered to that sibling's cause: a subset-gap report there would
// blame the *form* for a type the reader can see declared and already diagnosed.
// ---------------------------------------------------------------------------

/// A struct field whose type names a refused sibling is steered to that sibling's
/// cause, never given a phrase blaming the language for a struct this project
/// declared and the compiler refused four lines above.
#[test]
fn a_struct_field_naming_a_refused_sibling_steers_to_its_cause() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct Bad {\n\
         \x20   p: int?\n\
         }\n\n\
         struct Holder {\n\
         \x20   b: Bad\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   return 1\n\
         }\n",
    );

    for row in &diagnostics {
        assert!(
            !row.message().contains("is not yet supported")
                || row.message().contains("optional struct field"),
            "`Bad` is declared and was refused at its own field; the member position \
             must name that cause, not a language gap: {:#?}",
            messages(&diagnostics),
        );
    }
    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckUnsupported, 4, 8),
            ("src/main.mw", Code::CheckUnsupported, 8, 8),
        ],
        "the sibling reports the cause and the field is steered to it: {:#?}",
        messages(&diagnostics),
    );
}

/// the collection-element shape of the same position. The element type is
/// resolved one level inside a generic application, so it reaches the member row by
/// a second route and must arrive at the same steer.
#[test]
fn a_collection_element_naming_a_refused_sibling_steers_to_its_cause() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct Bad {\n\
         \x20   p: int?\n\
         }\n\n\
         struct Holder {\n\
         \x20   xs: List<Bad>\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_steers_to(
        &diagnostics,
        DeclarationNamespace::NamedType,
        Code::CheckUnsupported,
        RefusalReport::AtDeclaration,
    );
    assert_eq!(
        rows(&diagnostics),
        vec![
            ("src/main.mw", Code::CheckUnsupported, 4, 8),
            ("src/main.mw", Code::CheckUnsupported, 8, 9),
        ],
        "the element position is steered to the refused struct's cause: {:#?}",
        messages(&diagnostics),
    );
}

/// The over-suppression partner for both shapes: a member type that is genuinely
/// outside the admitted set keeps the subset-gap phrase. The causal arm must not
/// swallow a real language gap — `List` of a struct is not a durable member type,
/// and that is a true statement about the beta line rather than about any
/// declaration.
#[test]
fn a_genuinely_unadmitted_member_type_keeps_the_subset_gap_phrase() {
    let diagnostics = diagnostics(
        "module main\n\n\
         struct Good {\n\
         \x20   p: int\n\
         }\n\n\
         resource R {\n\
         \x20   required xs: List<Good>\n\
         }\n\n\
         pub fn make(): int {\n\
         \x20   return 1\n\
         }\n",
    );

    assert_eq!(
        rows(&diagnostics),
        vec![("src/main.mw", Code::CheckUnsupported, 8, 18)],
        "nothing here was refused; the report is about the admitted subset: {:#?}",
        messages(&diagnostics),
    );
    assert!(
        diagnostics[0].refused_declaration().is_none(),
        "a subset gap names no declaration: {:#?}",
        messages(&diagnostics),
    );
}

// ---------------------------------------------------------------------------
// Payload enum construction — the last asymmetric member of the steer family
//
// `Enum::member` steers when the enum was refused; `Enum::member(payload…)` is
// dispatched by a different arm. That arm consults the accepted-only enum table, so
// without the steer it falls through to the qualified-call report — `Color::blue` is
// not in scope, of an enum declared six lines above.
// ---------------------------------------------------------------------------

/// A payload construction on a refused enum names the enum's cause, exactly as its
/// bare-member sibling does.
#[test]
fn a_payload_construction_on_a_refused_enum_steers_to_its_cause() {
    let source = |construct: &str| {
        format!(
            "module main\n\n\
             enum Color {{\n\
             \x20   red\n\
             \x20   blue(a: int?)\n\
             }}\n\n\
             pub fn make(): int {{\n\
             \x20   const c = {construct}\n\
             \x20   return 2\n\
             }}\n"
        )
    };

    let payload = diagnostics(&source("Color::blue(a: 1)"));
    assert_never_out_of_scope(&payload, "Color::blue");
    assert_steers_to(
        &payload,
        DeclarationNamespace::NamedType,
        Code::CheckUnsupported,
        RefusalReport::AtDeclaration,
    );

    // Both spellings of a use of the same refused enum report the same rows.
    let bare = diagnostics(&source("Color::red"));
    assert_eq!(
        rows(&payload),
        rows(&bare),
        "a payload construction and a bare member are two spellings of one refused \
         enum's use; neither may report differently: {:#?}",
        messages(&payload),
    );
}

// ---------------------------------------------------------------------------
// The steer facts, audited field by field
//
// Every steer is built by one of two renderers: `declaration_refused`, which reuses
// the declaring row's own code, and `identity_admission_failed`, which reports under
// `check.type` and names the `check.durable_identity` report *family*. Each carries
// the same three typed fields, and each fixture below pins all three for one refusal
// class — the namespace it was declared into, the code the reader must act on, and
// where that report sits.
//
// The rendered sentence is not the contract. What is asserted is the relation between
// a row's own code and the code it steers to: they agree for every class that reuses
// its declaring row, and differ for exactly the identity class, whose cause is a family
// rather than a single row.
// ---------------------------------------------------------------------------

/// The steer facts of the last row, with the row's own code beside them.
fn steer_facts(
    diagnostics: &[SourceDiagnostic],
) -> (Code, Option<DeclarationNamespace>, Code, RefusalReport) {
    let last = diagnostics
        .last()
        .expect("a steered use reports at least one row");
    let steer = last.refused_declaration().unwrap_or_else(|| {
        panic!(
            "the last row is a steer to a refused declaration: {:#?}",
            rows(diagnostics),
        )
    });
    (
        last.code(),
        steer.namespace,
        steer.declaring_code,
        steer.report,
    )
}

/// One refusal class a single module expresses, and the steer facts its use owes.
struct SteerClass {
    /// The refusal under test.
    refusal: &'static str,
    source: &'static str,
    /// The ledger holding the refusal.
    namespace: DeclarationNamespace,
    /// The code of the report the reader must act on, which the steer reuses.
    declaring_code: Code,
}

/// The refusal classes a single module expresses. Each is reported at its own
/// declaration, so its steer names that row rather than an earlier stage or a family.
const SINGLE_MODULE_STEER_CLASSES: &[SteerClass] = &[
    SteerClass {
        refusal: "a constant refused for a type mismatch",
        source: "module main\n\n\
                 const limit: int = \"x\"\n\n\
                 pub fn read(): int {\n\
                 \x20   return limit\n\
                 }\n",
        namespace: DeclarationNamespace::Constant,
        declaring_code: Code::CheckType,
    },
    SteerClass {
        refusal: "a constant refused for a non-literal value",
        source: "module main\n\n\
                 const limit = 1 + 2\n\n\
                 pub fn read(): int {\n\
                 \x20   return limit\n\
                 }\n",
        namespace: DeclarationNamespace::Constant,
        declaring_code: Code::CheckUnsupported,
    },
    SteerClass {
        refusal: "a store root refused for its resource",
        source: "module main\n\n\
                 store ^items[id: int]: Widget\n\n\
                 pub fn write() {\n\
                 \x20   transaction {\n\
                 \x20       ^items[1].name = \"a\"\n\
                 \x20   }\n\
                 }\n",
        namespace: DeclarationNamespace::DurableRoot,
        declaring_code: Code::CheckType,
    },
    SteerClass {
        refusal: "a function signature refused for a parameter type",
        source: "module main\n\n\
                 fn helper(a: Nope, b: int): int {\n\
                 \x20   return b\n\
                 }\n\n\
                 pub fn other(): int {\n\
                 \x20   return helper(1, 2)\n\
                 }\n",
        namespace: DeclarationNamespace::Function,
        declaring_code: Code::CheckUnsupported,
    },
    SteerClass {
        refusal: "an alias over an unknown target",
        source: "module main\n\n\
                 alias Count = Nope\n\n\
                 pub fn make(c: Count): int {\n\
                 \x20   return 1\n\
                 }\n",
        namespace: DeclarationNamespace::NamedType,
        declaring_code: Code::CheckType,
    },
    SteerClass {
        refusal: "a cyclic alias chain",
        source: "module main\n\n\
                 alias A = B\n\
                 alias B = A\n\n\
                 pub fn make(c: A): int {\n\
                 \x20   return 1\n\
                 }\n",
        namespace: DeclarationNamespace::NamedType,
        declaring_code: Code::CheckRecursion,
    },
    SteerClass {
        refusal: "a resource member refused for its type",
        source: "module main\n\n\
                 resource Widget {\n\
                 \x20   required name: string\n\
                 \x20   bad: Nope\n\
                 }\n\n\
                 pub fn make(): int {\n\
                 \x20   const w = Widget(name: \"a\", bad: 1)\n\
                 \x20   return 1\n\
                 }\n",
        namespace: DeclarationNamespace::ResourceMember,
        declaring_code: Code::CheckUnsupported,
    },
];

/// The facts one audited steer contributes to the coverage derivation.
type AuditedSteer = (DeclarationNamespace, Code, RefusalReport);

/// Assert one steer's three typed fields and, where the class reuses its declaring row,
/// that the row's own code is that same code — so one code leads to one fix.
fn audit_steer(
    audited: &mut Vec<AuditedSteer>,
    label: &str,
    diagnostics: &[SourceDiagnostic],
    namespace: DeclarationNamespace,
    declaring_code: Code,
    report: RefusalReport,
) {
    let (row_code, observed_namespace, observed_code, observed_report) = steer_facts(diagnostics);
    assert_eq!(
        (observed_namespace, observed_code, observed_report),
        (Some(namespace), declaring_code, report),
        "{label}: the steer names the ledger holding the refusal, the code of the \
         report the reader must act on, and where that report sits: {:#?}",
        rows(diagnostics),
    );
    assert_eq!(
        row_code,
        declaring_code,
        "{label}: a steer that reuses its declaring row reports under that row's \
         own code, so one code leads to one fix: {:#?}",
        rows(diagnostics),
    );
    audited.push((namespace, declaring_code, report));
}

/// The classes whose cause is a report family or a covering pass rather than the
/// declaring row itself, which the shared audit cannot express.
fn audit_family_and_covering_pass(audited: &mut Vec<AuditedSteer>) {
    // The identity class: the one steer whose row code is *not* its declaring code,
    // because its cause is a report family rather than a single row.
    let identity = diagnostics(
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
    assert_eq!(
        steer_facts(&identity),
        (
            Code::CheckType,
            Some(DeclarationNamespace::DurableRoot),
            Code::CheckDurableIdentity,
            RefusalReport::AtDeclaration,
        ),
        "the identity steer reports under its own code and names the report family \
         the reader must act on: {:#?}",
        rows(&identity),
    );
    audited.push((
        DeclarationNamespace::DurableRoot,
        Code::CheckDurableIdentity,
        RefusalReport::AtDeclaration,
    ));

    // The covering-pass report kind, whose steer claims no location: the cause sits
    // on the cyclic value type, not at the declaration that names it.
    let sources: Vec<(&str, String)> = CYCLE_SOURCES
        .iter()
        .map(|(path, source)| (*path, (*source).to_string()))
        .collect();
    let covered = match compile(&with_minted_ids(&sources)) {
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            diagnostics.into_iter().collect::<Vec<_>>()
        }
        other => panic!("expected a refused declaration, got {other:?}"),
    };
    let covered_steer = covered[0]
        .refused_declaration()
        .expect("the reference to the cyclic root is a steer");
    assert_eq!(
        (
            covered[0].code(),
            covered_steer.namespace,
            covered_steer.declaring_code,
            covered_steer.report,
        ),
        (
            Code::CheckRecursion,
            Some(DeclarationNamespace::DurableRoot),
            Code::CheckRecursion,
            RefusalReport::ByCoveringPass,
        ),
        "{:#?}",
        rows(&covered),
    );
    audited.push((
        DeclarationNamespace::DurableRoot,
        Code::CheckRecursion,
        RefusalReport::ByCoveringPass,
    ));
}

/// Coverage, derived rather than asserted in prose: every namespace a declaration can be
/// refused into, and every kind of report a steer can name, is exercised. A seventh
/// namespace or a fourth report kind fails here until a fixture pins its facts too.
fn assert_every_class_is_audited(audited: &[AuditedSteer]) {
    let namespaces: BTreeSet<String> = audited
        .iter()
        .map(|(namespace, _, _)| format!("{namespace:?}"))
        .collect();
    assert_eq!(
        namespaces,
        [
            "Constant",
            "DurableRoot",
            "Function",
            "Module",
            "NamedType",
            "ResourceMember",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<String>>(),
        "every namespace a declaration is refused into is audited",
    );
    let reports: BTreeSet<String> = audited
        .iter()
        .map(|(_, _, report)| format!("{report:?}"))
        .collect();
    assert_eq!(
        reports,
        [
            "AtDeclaration",
            "ByCoveringPass",
            "ByEarlierStage(Decode)",
            "ByEarlierStage(Parse)",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<String>>(),
        "every kind of report a steer can name is audited",
    );
}

/// The multi-module project every module-class fixture varies: `main` imports `helper`
/// and calls into it, so the import and the qualified call both have a steer to place.
fn importing_main() -> (&'static str, String) {
    (
        "src/main.mw",
        "module main\n\n\
         use helper\n\n\
         pub fn run(): int {\n\
         \x20   return helper::twice(2)\n\
         }\n"
        .to_string(),
    )
}

#[test]
fn every_refusal_class_carries_its_own_typed_steer_facts() {
    let mut audited: Vec<AuditedSteer> = Vec::new();

    for case in SINGLE_MODULE_STEER_CLASSES {
        audit_steer(
            &mut audited,
            case.refusal,
            &diagnostics(case.source),
            case.namespace,
            case.declaring_code,
            RefusalReport::AtDeclaration,
        );
    }

    audit_steer(
        &mut audited,
        "a struct refused for a field type",
        &diagnostics(&format!(
            "module main\n\n\
             {REFUSED_STRUCT}\n\
             fn take(q: Point): int {{\n\
             \x20   return 1\n\
             }}\n\n\
             pub fn make(): int {{\n\
             \x20   return 2\n\
             }}\n"
        )),
        DeclarationNamespace::NamedType,
        Code::CheckUnsupported,
        RefusalReport::AtDeclaration,
    );
    audit_steer(
        &mut audited,
        "a module refused for its header",
        &diagnostics_of(&files(&[
            importing_main(),
            (
                "src/helper.mw",
                "module wrong\n\n\
                 pub fn twice(n: int): int {\n\
                 \x20   return n\n\
                 }\n"
                .to_string(),
            ),
        ])),
        DeclarationNamespace::Module,
        Code::CheckModulePath,
        RefusalReport::AtDeclaration,
    );
    audit_steer(
        &mut audited,
        "a module the parse stage refused",
        &analyzed(&[
            importing_main(),
            (
                "src/helper.mw",
                "module helper\n\n\
                 pub fn twice(n: int): int {\n\
                 \x20   return n +\n\
                 }\n"
                .to_string(),
            ),
        ]),
        DeclarationNamespace::Module,
        Code::ParseSyntax,
        RefusalReport::ByEarlierStage(SourceStage::Parse),
    );
    audit_steer(
        &mut audited,
        "a module the decode stage refused",
        &snapshot_diagnostics(project_capture::project_bytes(&[
            ("src/helper.mw", vec![0xff, 0xfe, 0x00]),
            ("src/main.mw", importing_main().1.into_bytes()),
        ])),
        DeclarationNamespace::Module,
        Code::CheckUnsupported,
        RefusalReport::ByEarlierStage(SourceStage::Decode),
    );

    audit_family_and_covering_pass(&mut audited);
    assert_every_class_is_audited(&audited);
}

#[test]
fn refused_aliases_keep_available_causes_in_scalar_and_concrete_siblings() {
    let consumers = [
        (
            "const value: Bad = 1\npub fn driver(): int { return value }",
            None,
        ),
        (
            "type Value: Bad in 0..=10\npub fn driver(): int { return 0 }",
            Some(13),
        ),
        (
            "enum Value { item(value: Bad) }\npub fn driver(): int { return 0 }",
            Some(26),
        ),
        (
            "struct Value { value: Bad }\npub fn driver(): int { return 0 }",
            Some(23),
        ),
        (
            "resource R { required value: Bad }\npub fn driver(): int { return 0 }",
            Some(30),
        ),
        (
            "resource R { required value: int }\nstore ^root[k: Bad]: R\npub fn driver(): int { return 0 }",
            None,
        ),
        (
            "resource R { entries[k: Bad] { required value: int } }\nstore ^root[k: int]: R\npub fn driver(): int { return 0 }",
            None,
        ),
        (
            "resource R { values { required value: Bad } }\nstore ^root[k: int]: R\npub fn driver(): int { return 0 }",
            None,
        ),
        (
            "resource R { entries[k: int] { required value: Bad } }\nstore ^root[k: int]: R\npub fn driver(): int { return 0 }",
            None,
        ),
    ];
    for (target, code) in [
        ("Missing", Code::CheckType),
        ("Bad", Code::CheckRecursion),
        ("List<int>", Code::CheckUnsupported),
    ] {
        for (consumer, before_validation_column) in consumers {
            let source = format!("module main\nalias Bad = {target}\n{consumer}\n");
            let project = with_minted_ids(&[("src/main.mw", source.clone())]);
            let diagnostics = diagnostics_of(&project);
            let use_start = source.find(consumer).expect("consumer is present");
            let annotation_start =
                use_start + consumer.find("Bad").expect("consumer writes its alias");
            let before_annotation = &source[..annotation_start];
            let annotation_span = marrow_syntax::SourceSpan {
                start_byte: annotation_start,
                end_byte: annotation_start + "Bad".len(),
                line: before_annotation
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count() as u32
                    + 1,
                column: before_annotation
                    .rsplit('\n')
                    .next()
                    .expect("source prefix")
                    .len() as u32
                    + 1,
            };
            if target == "Missing"
                && let Some(column) = before_validation_column
            {
                assert_eq!(
                    rows(&diagnostics),
                    [
                        ("src/main.mw", Code::CheckUnsupported, 3, column),
                        ("src/main.mw", Code::CheckType, 2, 1),
                    ],
                    "{consumer}: {diagnostics:#?}"
                );
                assert!(
                    diagnostics
                        .iter()
                        .all(|row| row.refused_declaration().is_none())
                );
                assert_eq!(diagnostics[0].span(), annotation_span);
                assert_eq!(
                    diagnostics[1].span(),
                    marrow_syntax::SourceSpan {
                        start_byte: 12,
                        end_byte: 31,
                        line: 2,
                        column: 1,
                    }
                );
                continue;
            }
            let expected_cause = RefusedDeclaration {
                namespace: Some(DeclarationNamespace::NamedType),
                declaring_code: code,
                report: RefusalReport::AtDeclaration,
            };
            let causal = diagnostics.iter().find(|row| {
                row.file().as_str() == "src/main.mw"
                    && row.code() == code
                    && row.span() == annotation_span
                    && row.refused_declaration() == Some(&expected_cause)
            });
            assert!(causal.is_some(), "{target}, {consumer}: {diagnostics:#?}");
        }
    }
}

#[test]
fn a_refused_alias_in_a_group_field_keeps_its_declaring_file() {
    let schema =
        "module schema\nalias Bad = Missing\nresource R { values { required value: Bad } }\n";
    let main = "module main\nstore ^root[id: int]: R\npub fn driver(): int { return 0 }\n";
    let project = with_minted_ids(&[
        ("src/schema.mw", schema.to_string()),
        ("src/main.mw", main.to_string()),
    ]);
    let diagnostics = diagnostics_of(&project);
    let start = schema.rfind("Bad").expect("field annotation");
    let expected = RefusedDeclaration {
        namespace: Some(DeclarationNamespace::NamedType),
        declaring_code: Code::CheckType,
        report: RefusalReport::AtDeclaration,
    };
    assert!(
        diagnostics.iter().any(|row| {
            row.file().as_str() == "src/schema.mw"
                && row.code() == Code::CheckType
                && row.span()
                    == marrow_syntax::SourceSpan {
                        start_byte: start,
                        end_byte: start + 3,
                        line: 3,
                        column: 39,
                    }
                && row.refused_declaration() == Some(&expected)
        }),
        "{diagnostics:#?}"
    );
}

fn written_span(source: &str, spelling: &str) -> marrow_syntax::SourceSpan {
    let start = source.rfind(spelling).expect("written diagnostic owner");
    let prefix = &source[..start];
    marrow_syntax::SourceSpan {
        start_byte: start,
        end_byte: start + spelling.len(),
        line: prefix.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1,
        column: prefix.rsplit('\n').next().expect("line prefix").len() as u32 + 1,
    }
}

#[test]
fn refused_alias_member_and_key_uses_keep_their_own_files() {
    for (target, code) in [
        ("Missing", Code::CheckType),
        ("Bad", Code::CheckRecursion),
        ("List<int>", Code::CheckUnsupported),
    ] {
        for (member, root_key, early) in [
            ("required value: Bad", "int", true),
            ("values { required value: Bad }", "int", false),
            ("entries[k: int] { required value: Bad }", "int", false),
            ("entries[k: Bad] { required value: int }", "int", false),
            ("required value: int", "Bad", false),
        ] {
            let aliases = format!("module aliases\nalias Bad = {target}\n");
            let schema = format!("module schema\n\n\nresource R {{ {member} }}\n");
            let main = format!(
                "module main\nstore ^root[id: {root_key}]: R\npub fn driver(): int {{ return 0 }}\n"
            );
            let project = with_minted_ids(&[
                ("src/aliases.mw", aliases),
                ("src/schema.mw", schema.clone()),
                ("src/main.mw", main.clone()),
            ]);
            let diagnostics = diagnostics_of(&project);
            let (file, source) = if root_key == "Bad" {
                ("src/main.mw", &main)
            } else {
                ("src/schema.mw", &schema)
            };
            let (code, cause) = if early && target == "Missing" {
                (Code::CheckUnsupported, None)
            } else {
                (
                    code,
                    Some(RefusedDeclaration {
                        namespace: Some(DeclarationNamespace::NamedType),
                        declaring_code: code,
                        report: RefusalReport::AtDeclaration,
                    }),
                )
            };
            let span = written_span(source, "Bad");
            let matching: Vec<_> = diagnostics
                .iter()
                .filter(|row| row.code() == code && row.span() == span)
                .collect();
            assert!(!matching.is_empty(), "{target}, {member}: {diagnostics:#?}");
            for row in matching {
                assert_eq!(row.file().as_str(), file, "{target}, {member}");
                assert_eq!(row.refused_declaration(), cause.as_ref());
            }
        }
    }
}

#[test]
fn ordinary_group_and_branch_field_refusals_keep_the_resource_file() {
    for field in ["required value: Maybe", "required value[k: int]: int"] {
        for placement in ["values", "entries[k: int]"] {
            let schema =
                format!("module schema\n\n\nresource R {{\n{placement} {{\n{field}\n}}\n}}\n");
            let project = with_minted_ids(&[
                (
                    "src/aliases.mw",
                    "module aliases\nalias Maybe = int?\n".to_string(),
                ),
                ("src/schema.mw", schema.clone()),
                (
                    "src/main.mw",
                    "module main\nstore ^root[id: int]: R\npub fn driver(): int { return 0 }\n"
                        .to_string(),
                ),
            ]);
            let diagnostics = diagnostics_of(&project);
            let span = written_span(&schema, field);
            let row = diagnostics
                .iter()
                .find(|row| row.code() == Code::CheckUnsupported && row.span() == span)
                .unwrap_or_else(|| panic!("{field}, {placement}: {diagnostics:#?}"));
            assert_eq!(row.file().as_str(), "src/schema.mw");
            assert_eq!(row.refused_declaration(), None);
        }
    }
}

#[test]
fn member_shape_bounds_keep_the_resource_file() {
    let keys = (0..=marrow_image::bounds::MAX_KEY_COLUMNS)
        .map(|i| format!("k{i}: int"))
        .collect::<Vec<_>>()
        .join(", ");
    let branch_head = format!("entries[{keys}] {{");
    let branch = format!("{branch_head}\nrequired value: int\n}}");
    let field = "required value: int";
    let depth = marrow_image::bounds::MAX_DURABLE_DEPTH;
    let nested = format!(
        "{}{field}\n{}",
        "nested {\n".repeat(depth),
        "}\n".repeat(depth)
    );
    for (member, owner) in [(&branch, branch_head.as_str()), (&nested, field)] {
        let schema = format!("module schema\n\n\nresource R {{\n{member}\n}}\n");
        let project = with_minted_ids(&[
            ("src/schema.mw", schema.clone()),
            (
                "src/main.mw",
                "module main\nstore ^root[id: int]: R\npub fn driver(): int { return 0 }\n"
                    .to_string(),
            ),
        ]);
        let diagnostics = diagnostics_of(&project);
        let row = diagnostics
            .iter()
            .find(|row| row.code() == Code::CheckResourceLimit)
            .unwrap_or_else(|| panic!("{diagnostics:#?}"));
        assert_eq!(row.span(), written_span(&schema, owner));
        assert_eq!(row.file().as_str(), "src/schema.mw");
        assert_eq!(row.refused_declaration(), None);
    }
}
