//! The compile driver's own unit tests: policy-outcome classification, phase
//! gating, and the registry fixtures the driver's seams are pinned with.

use crate::compile::admitted;

use super::valid_export_path;
use super::{
    Analyzed, BoundedDiagnostics, Built, CompileFailure, CompileStage, DeclarationExit, Driven,
    InvariantCause, SemanticOutcome, analyze_outcome,
};
use crate::compile::Declaration;
use crate::diag::{DiagnosticCollector, MAX_DIAGNOSTIC_COUNT, SourceDiagnostic};
use crate::lower::FunctionRegistry;
use crate::types::{GenericInvariant, TemplateProofError};
use marrow_codes::Code;
use marrow_syntax::SourceSpan;
use std::collections::BTreeMap;

#[test]
fn borrowed_bodies_require_the_actual_function_and_every_instruction_span() {
    use marrow_image::{FunctionDef, ImageDraft, ImageType, Instr};

    let mut draft = ImageDraft::new();
    let mut txn = admitted(&mut draft);
    let name = txn.intern_string("body").expect("name fits");
    let source = txn.intern_string("src/main.mw").expect("source fits");
    let func = txn
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            code: vec![Instr::Return],
            spans: Vec::new(),
        })
        .expect("append a body without sites");
    txn.commit();
    let mut function = super::LoweredFn {
        func,
        file: crate::test_main_file_identity().clone(),
        name: "body".to_string(),
        span: SourceSpan::default(),
        callees: Vec::new(),
        is_export: false,
        is_test: false,
        unwrapped_mutations: Vec::new(),
        unwrapped_calls: Vec::new(),
        erased_families: Vec::new(),
        presence_obligations: Vec::new(),
        has_direct_durable_op: false,
        code_spans: vec![SourceSpan::default()],
    };

    assert!(matches!(
        function.borrow_body(&ImageDraft::new()),
        Err(InvariantCause::MissingFunctionBody(actual)) if actual == func,
    ));
    for spans in [0, 2] {
        function.code_spans = vec![SourceSpan::default(); spans];
        assert!(matches!(
            function.borrow_body(&draft),
            Err(InvariantCause::InstructionSpanMismatch {
                function: actual,
                instructions: 1,
                spans: actual_spans,
            }) if actual == func && actual_spans == spans,
        ));
    }
    function.code_spans = vec![SourceSpan::default()];
    let body = function
        .borrow_body(&draft)
        .expect("one coordinate per instruction");
    assert!(matches!(body.code, [Instr::Return]));
    assert_eq!(
        body.code.as_ptr(),
        draft.function_code(func).expect("appended body").as_ptr()
    );
}

/// The minting guard rejects every input class whose dotted join would break
/// the ExportId payload's injectivity, even though the current capture path
/// cannot produce them.
#[test]
fn export_path_validation_guards_the_id_payload() {
    // Ordinary declaration paths mint.
    assert!(valid_export_path("main", "run"));
    assert!(valid_export_path("shelf.books", "add"));
    assert!(valid_export_path("a_b", "_x1"));

    // Empty or dotted components would let two distinct declaration paths
    // collide on one payload.
    assert!(!valid_export_path("", "run"));
    assert!(!valid_export_path("a", ""));
    assert!(!valid_export_path("a..b", "run"));
    assert!(!valid_export_path("a.", "run"));
    assert!(!valid_export_path(".a", "run"));
    assert!(!valid_export_path("a", "b.c"));

    // Non-ASCII and non-identifier characters are outside the frozen payload
    // domain.
    assert!(!valid_export_path("caf\u{e9}", "run"));
    assert!(!valid_export_path("a", "r\u{e9}sum\u{e9}"));
    assert!(!valid_export_path("a-b", "run"));
    assert!(!valid_export_path("1a", "run"));
    assert!(!valid_export_path("a", "1run"));
    assert!(!valid_export_path("a b", "run"));
}

fn diagnostic(code: Code, line: u32) -> SourceDiagnostic {
    SourceDiagnostic::at(
        code,
        crate::test_main_file_identity(),
        SourceSpan {
            line,
            column: 7,
            ..SourceSpan::default()
        },
        "retained source diagnostic".to_string(),
    )
}

fn template_proof_cause() -> InvariantCause {
    InvariantCause::Generic(GenericInvariant::TemplateProof(
        TemplateProofError::UnstableFillState,
    ))
}

fn stage_label(stage: CompileStage) -> &'static str {
    match stage {
        CompileStage::TypeInstantiation => "type instantiation",
        CompileStage::TemplateProof => "template proof",
        CompileStage::BodyLowering => "body lowering",
        CompileStage::PostLoweringValidation => "post-lowering validation",
    }
}

/// A driven pass whose stage terminals are exactly `parse`, `structural`,
/// and `semantic`, with no orthogonal analysis facts. The projection under
/// test reads only these.
fn driven(
    parse: BoundedDiagnostics,
    structural: BoundedDiagnostics,
    semantic: SemanticOutcome,
) -> Driven {
    Driven {
        parse,
        structural,
        semantic,
        facts: crate::analysis::BoundedAnalysisFacts::Complete(
            crate::analysis::RetainedFacts::default(),
        ),
        symbol_bounded_files: Vec::new(),
    }
}

fn empty_terminal() -> BoundedDiagnostics {
    DiagnosticCollector::new().finish()
}

fn finished(rows: Vec<SourceDiagnostic>) -> BoundedDiagnostics {
    let mut collector = DiagnosticCollector::new();
    for row in rows {
        collector.push(row);
    }
    collector.finish()
}

/// A semantic invariant reaches the public boundary opaque: no partial
/// image, a private cause, and the fixed rendering.
#[test]
fn a_semantic_invariant_is_opaque_at_the_public_boundary() {
    let outcome: Result<Built, CompileFailure> = driven(
        empty_terminal(),
        empty_terminal(),
        SemanticOutcome::Invariant(template_proof_cause()),
    )
    .production_built();
    let Err(failure) = outcome else {
        panic!("an invariant must not produce a partial image")
    };

    assert_eq!(failure.to_string(), "compiler invariant failure");
    assert!(std::error::Error::source(&failure).is_some());
    let CompileFailure::Invariant(invariant) = failure else {
        panic!("the private invariant must stay an invariant")
    };
    assert!(matches!(
        invariant.0,
        InvariantCause::Generic(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(format!("{invariant:?}"), "CompileInvariant");
    assert_eq!(invariant.to_string(), "compiler invariant failure");
    assert!(std::error::Error::source(&invariant).is_none());
}

/// An earlier stage's complete diagnostics dominate a later semantic
/// resource limit or diagnostic set: the projection reports the first
/// logically non-empty stage and never mixes stages.
#[test]
fn an_earlier_stage_dominates_a_later_semantic_failure() {
    let parse_row = diagnostic(Code::CheckType, 3);
    for semantic in [
        image_bytes_stop(),
        SemanticOutcome::Diagnostics(
            finished(vec![diagnostic(Code::CheckType, 9)]),
            CompileStage::BodyLowering,
        ),
    ] {
        let failure = driven(
            finished(vec![parse_row.clone()]),
            empty_terminal(),
            semantic,
        )
        .production_built()
        .map(|_| ())
        .expect_err("a parse-stage row fails compilation");
        let CompileFailure::Diagnostics(diagnostics) = failure else {
            panic!("the parse stage's own rows are the failure")
        };
        assert_eq!(diagnostics.as_slice(), std::slice::from_ref(&parse_row));
    }
}

/// Diagnostics preserve their collector order and allocation behind a
/// statically nonempty owner. Every borrowed and owned iteration surface
/// observes that order; recovering the vector recovers the collector's
/// allocation without a copy.
#[test]
fn diagnostic_failure_preserves_order_allocation_and_iteration_views() {
    let expected = vec![
        diagnostic(Code::CheckType, 4),
        diagnostic(Code::CheckType, 9),
    ];
    let terminal = finished(expected.clone());
    let original_ptr = match &terminal {
        BoundedDiagnostics::Complete { rows, .. } => rows.as_ptr(),
        BoundedDiagnostics::Limited { .. } => panic!("two rows stay complete"),
    };
    let failure = driven(terminal, empty_terminal(), image_bytes_stop())
        .production_built()
        .map(|_| ())
        .expect_err("a nonempty parse stage fails compilation");
    assert_eq!(
        failure.to_string(),
        "compilation failed with source diagnostics"
    );
    assert!(std::error::Error::source(&failure).is_none());
    let CompileFailure::Diagnostics(diagnostics) = failure else {
        panic!("a nonempty source failure must remain diagnostics")
    };
    assert_eq!(diagnostics.as_slice(), expected.as_slice());
    let as_ref: &[SourceDiagnostic] = diagnostics.as_ref();
    assert_eq!(as_ref, expected.as_slice());
    assert_eq!(diagnostics.iter().cloned().collect::<Vec<_>>(), expected);
    assert_eq!(
        (&diagnostics).into_iter().cloned().collect::<Vec<_>>(),
        expected
    );
    let recovered = diagnostics.into_vec();
    assert_eq!(
        recovered.as_ptr(),
        original_ptr,
        "the collector's allocation is recovered without a copy"
    );
    assert_eq!(recovered, expected);
}

/// A completeness artifact is minted for a declaration set only when every
/// declaration in it took the index reserved for it. A stop on the shared
/// instantiation limit leaves the loop's unvisited suffix unlowered, so it mints
/// nothing — the artifact's documented claim would otherwise be false for every
/// declaration after the stop, leaving the truncated set's honesty resting entirely
/// on the caller's separate limit return.
#[test]
fn only_an_exhausted_declaration_set_is_complete() {
    assert!(DeclarationExit::Exhausted.complete());
    assert!(!DeclarationExit::Refused.complete());
    assert!(!DeclarationExit::StoppedOnInstantiationLimit.complete());
}

/// A refused signature standing behind an accepted duplicate of its name still
/// leaves the signature table incomplete.
///
/// Completeness reads the ledger's refused set. A set that answered only for the
/// keys a lookup resolves to a refusal would skip this one, and the semantic fence
/// would call the pass complete over a table the compiler refused a declaration in.
/// Resolve the signatures through the production registry owners, over a project
/// that declares nothing else.
fn signature_registry(functions: &[crate::lower::DeclaredFn<'_>]) -> FunctionRegistry {
    let budget = crate::decl::DeclarationBudget::default();
    let mut draft_owner = marrow_image::ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let mut diagnostics = DiagnosticCollector::new();
    let mut records = crate::types::TypeRegistry::build(
        &mut draft,
        &[],
        &[],
        &[],
        &[],
        &[],
        &mut diagnostics,
        budget.clone(),
    )
    .expect("the test registry stays within the ledger budget");
    draft.commit();
    let durable = crate::durable::DurableRegistry::build(
        &mut draft_owner,
        &records,
        &[],
        &[],
        None,
        &mut diagnostics,
        budget.clone(),
        &mut Vec::new(),
    )
    .expect("an empty project builds an empty durable registry");
    let mut draft = admitted(&mut draft_owner);
    crate::lower::FunctionRegistry::build(
        &mut records,
        &mut draft,
        &durable,
        functions,
        crate::lower::ModuleScope {
            modules: crate::lower::ModuleLedger::new(
                crate::decl::DeclarationNamespace::Module,
                budget.clone(),
            ),
            imports: BTreeMap::new(),
            budget,
        },
        &mut diagnostics,
        &mut Vec::new(),
    )
    .expect("the signature ledger stays within its budget")
}

#[test]
fn a_refusal_behind_an_accepted_duplicate_leaves_the_signature_table_incomplete() {
    let (identity, _) =
        marrow_project::FileIdentity::validate("src/main.mw").expect("a valid source path");
    let parsed = marrow_syntax::parse_source(
        "module main\n\nfn dup(a: int): int {\n    return a\n}\n\n\
         fn dup(a: Nope): int {\n    return 1\n}\n",
    );
    let functions: Vec<crate::lower::DeclaredFn<'_>> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|decl| match decl {
            Declaration::Function(function) => Some(crate::lower::DeclaredFn {
                file: identity.clone(),
                at: crate::analysis::FileRef::admitted(0),
                module: "main".to_string(),
                decl: function,
            }),
            _ => None,
        })
        .collect();
    assert_eq!(functions.len(), 2, "both declarations are parsed");
    let signatures = signature_registry(&functions);

    assert!(
        !signatures.every_signature_accepted(),
        "the second `dup` was refused for its parameter type",
    );
}

/// A complete-but-empty semantic diagnostics terminal is a private
/// invariant carrying the exact stage that attempted to cross the
/// boundary; a logically empty parse or structural terminal instead
/// passes over. The matcher intentionally has no wildcard, so adding a
/// stage requires updating this contract.
#[test]
fn an_empty_semantic_terminal_is_an_exact_invariant_at_every_stage() {
    for stage in [
        CompileStage::TypeInstantiation,
        CompileStage::TemplateProof,
        CompileStage::BodyLowering,
        CompileStage::PostLoweringValidation,
    ] {
        let empty = driven(
            empty_terminal(),
            empty_terminal(),
            SemanticOutcome::Diagnostics(empty_terminal(), stage),
        )
        .production_built()
        .map(|_| ())
        .expect_err("an empty semantic terminal must not build");
        let CompileFailure::Invariant(invariant) = empty else {
            panic!("an empty diagnostic terminal must become a compiler invariant")
        };
        let InvariantCause::EmptyDiagnostics(actual) = invariant.0 else {
            panic!("the empty boundary keeps its private stage")
        };
        assert_eq!(stage_label(actual), stage_label(stage));
        assert_eq!(actual, stage);
    }
}

/// The analysis union reads the same terminals the production projection
/// does, row by row: a semantic invariant passes through whether or not
/// prechecks reported; with prechecks present a semantic resource limit
/// is suppressed for the precheck union; a semantic empty terminal is the
/// same empty-boundary invariant production reports above, never an empty
/// clean result; and the union of stages may cross a ceiling no single
/// stage crossed.
#[test]
fn the_analysis_union_follows_the_stage_table() {
    let row = |line| diagnostic(Code::CheckType, line);

    // Empty prechecks pass the semantic failure through.
    assert!(matches!(
        analyze_outcome(
            empty_terminal(),
            empty_terminal(),
            SemanticOutcome::Invariant(template_proof_cause()),
        ),
        Analyzed::Invariant(_)
    ));

    // A precheck row does not suppress an executed semantic invariant.
    assert!(matches!(
        analyze_outcome(
            finished(vec![row(3)]),
            empty_terminal(),
            SemanticOutcome::Invariant(template_proof_cause()),
        ),
        Analyzed::Invariant(_)
    ));

    // A semantic empty terminal with empty prechecks is the empty-boundary invariant,
    // with its stage.
    let Analyzed::Invariant(invariant) = analyze_outcome(
        empty_terminal(),
        empty_terminal(),
        SemanticOutcome::Diagnostics(empty_terminal(), CompileStage::BodyLowering),
    ) else {
        panic!("an empty semantic terminal is not a clean union")
    };
    assert!(matches!(
        invariant.0,
        InvariantCause::EmptyDiagnostics(CompileStage::BodyLowering)
    ));

    // The ordered union: parse, then structural, then semantic rows.
    let Analyzed::Diagnostics(rows) = analyze_outcome(
        finished(vec![row(1)]),
        finished(vec![row(2)]),
        SemanticOutcome::Diagnostics(finished(vec![row(3)]), CompileStage::BodyLowering),
    ) else {
        panic!("a bounded union is a producible snapshot")
    };
    assert_eq!(rows.as_slice(), &[row(1), row(2), row(3)]);

    // Analysis alone strengthens an OwnedBytes limit to Count across
    // stages: a byte-limited parse terminal plus enough semantic rows to
    // cross the count ceiling resolves as the count limit.
    let byte_limited = BoundedDiagnostics::Limited {
        count: MAX_DIAGNOSTIC_COUNT - 5,
        owned_bytes: crate::diag::MAX_DIAGNOSTIC_BYTES + 1,
        limit: crate::diag::CompileDiagnosticLimit::OwnedBytes {
            limit: crate::diag::MAX_DIAGNOSTIC_BYTES,
        },
    };
    let semantic_rows: Vec<SourceDiagnostic> = (0..10).map(|line| row(line + 1)).collect();
    let Analyzed::ResourceLimit(limit) = analyze_outcome(
        byte_limited,
        empty_terminal(),
        SemanticOutcome::Diagnostics(finished(semantic_rows), CompileStage::BodyLowering),
    ) else {
        panic!("a limited union is the displacing resource limit")
    };
    assert_eq!(limit.kind(), super::ResourceLimitKind::DiagnosticCount);
}

#[test]
fn public_invariant_is_worker_transferable_without_exposing_its_cause() {
    fn assert_worker_type<T: Send + Sync + 'static>() {}

    assert_worker_type::<super::CompileInvariant>();
}

/// The frozen kind-detail surface: each aggregate bound names itself with a stable
/// identifier the CLI resource-limit record carries verbatim. A drift here is a
/// deliberate change to that published surface.
#[test]
fn resource_limit_kind_detail_is_frozen() {
    use super::ResourceLimitKind::*;
    for (kind, detail) in [
        (Strings, "Strings"),
        (Consts, "Consts"),
        (Types, "Types"),
        (Enums, "Enums"),
        (Collections, "Collections"),
        (Roots, "Roots"),
        (Sites, "Sites"),
        (Functions, "Functions"),
        (Exports, "Exports"),
        (TestEntries, "TestEntries"),
        (ImageBytes, "ImageBytes"),
        (StringBytes, "StringBytes"),
        (DiagnosticCount, "DiagnosticCount"),
        (DiagnosticBytes, "DiagnosticBytes"),
        (ProjectFiles, "ProjectFiles"),
        (ProjectFileBytes, "ProjectFileBytes"),
        (ProjectSourceBytes, "ProjectSourceBytes"),
        (DeclarationLedgerBytes, "DeclarationLedgerBytes"),
    ] {
        assert_eq!(kind.detail(), detail);
    }
}

#[test]
fn a_limited_stage_terminal_is_the_displacing_resource_limit() {
    let mut collector = DiagnosticCollector::new();
    for line in 0..=MAX_DIAGNOSTIC_COUNT as u32 {
        collector.push(diagnostic(Code::CheckType, line + 1));
    }
    let failure = driven(collector.finish(), empty_terminal(), image_bytes_stop())
        .production_built()
        .map(|_| ())
        .expect_err("an over-ceiling stage must not build");
    let CompileFailure::ResourceLimit(limit) = failure else {
        panic!("an overflowing diagnostic collection is discarded for a resource limit")
    };
    assert_eq!(limit.kind(), super::ResourceLimitKind::DiagnosticCount);
    assert_eq!(limit.limit(), MAX_DIAGNOSTIC_COUNT as u64);
}

#[test]
fn public_resource_limit_is_worker_transferable() {
    fn assert_worker_type<T: Send + Sync + 'static>() {}

    assert_worker_type::<super::CompileResourceLimit>();
}

/// The image-build classifier routes an aggregate whole-program bound to the
/// resource-limit arm and a producer-state contradiction to an opaque invariant,
/// never to a source diagnostic with a fabricated location.
#[test]
fn image_build_errors_classify_without_a_fabricated_location() {
    let aggregate = super::image_build_outcome(marrow_image::ImageBuildError::TooManyFunctions);
    let super::ImagePolicyOutcome::ResourceLimit(limit) = aggregate else {
        panic!("an aggregate count is a resource limit")
    };
    assert_eq!(limit.kind(), super::ResourceLimitKind::Functions);

    let prechecked = super::image_build_outcome(marrow_image::ImageBuildError::TooManyLocals);
    assert!(
        matches!(prechecked, super::ImagePolicyOutcome::Invariant(_)),
        "a compiler draft past the source-prechecked local bound is an opaque invariant"
    );
    let code_prechecked = super::image_build_outcome(marrow_image::ImageBuildError::CodeTooLong);
    assert!(
        matches!(code_prechecked, super::ImagePolicyOutcome::Invariant(_)),
        "a compiler draft past the source-prechecked code-byte bound is an opaque invariant"
    );

    let contradiction =
        super::image_build_outcome(marrow_image::ImageBuildError::InvalidReference("x"));
    assert!(
        matches!(contradiction, super::ImagePolicyOutcome::Invariant(_)),
        "a producer-state contradiction is an opaque invariant, not a diagnostic"
    );
}

#[test]
fn a_hostile_image_draft_retains_the_direct_too_many_locals_error() {
    let mut draft_owner = marrow_image::ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let name = draft
        .intern_string("hostile")
        .expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let Ok(local_count) = u16::try_from(marrow_image::bounds::MAX_LOCALS + 1) else {
        panic!("the current image local bound has a representable hostile successor")
    };
    draft
        .add_function(marrow_image::FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: marrow_image::ImageType::Unit,
            local_count,
            code: vec![marrow_image::Instr::Return],
            spans: vec![marrow_image::SpanEntry {
                instr_index: 0,
                line: 1,
                column: 1,
            }],
        })
        .expect("a storeless body names no operation site");

    assert!(matches!(
        draft.encode(),
        Err(marrow_image::ImageBuildError::TooManyLocals)
    ));
}

/// The staged store producer's checked-refusal arm restores the image draft after
/// real staged work.
///
/// A store refused by managed-index admission is refused *after* its product graph
/// was built against the armed image transaction: building `Note`'s graph appends
/// the `sub` branch's entry record type to the draft before the completeness gate
/// runs, so the `StoreBuild::Refused` settlement is reached with a real staged draft
/// mutation behind it, not on the early admission exits that stage nothing. The
/// rollback's observable effect is byte-exact: the encoded image of a build whose
/// last store staged and then refused equals the encoded image of a build that
/// never declared that store, while the refusal's own diagnostic still settles.
/// A regression that committed — or only partially restored — the refused store's
/// staged rows moves the encoded bytes and fails here.
#[test]
fn a_store_refused_after_real_staging_rolls_back_to_the_unstaged_image() {
    const DECLARATIONS: &str = "module main\n\n\
        resource Book {\n    required title: string\n}\n\n\
        resource Note {\n    required text: string\n    tag: string\n\n    \
        sub[k: int] {\n        v: int\n    }\n}\n\n\
        store ^books[id: int]: Book\n";
    // The refused store: its product graph for `Note` is built and staged first, and
    // only then does the index whose name repeats the stored field `tag` refuse it.
    const REFUSED_STORE: &str = "\nstore ^notes[id: int]: Note {\n    index tag[tag] unique\n}\n";
    // Complete for every anchor either build resolves, so index admission is the one
    // refusal in play. A refused index resolves no identity of its own.
    const ANCHORS: &[&str] = &[
        "application .",
        "root books",
        "product Book",
        "key books.id",
        "field Book.title",
        "root notes",
        "product Note",
        "key notes.id",
        "field Note.text",
        "field Note.tag",
        "root Note.sub",
        "key Note.sub.k",
        "field Note.sub.v",
    ];

    let ledger = {
        let mut text = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
        for (seed, anchor) in ANCHORS.iter().enumerate() {
            use std::fmt::Write as _;
            let _ = writeln!(text, "id {anchor} {:032x}", seed as u128 + 1);
        }
        text.push_str("high-water 0\nend\n");
        marrow_project::IdentityLedger::parse(text.as_bytes()).expect("the ledger parses")
    };

    let build = |source: &str| {
        let parsed = marrow_syntax::parse_source(source);
        assert!(!parsed.has_errors(), "the corpus parses");
        let file = crate::test_file_identity("src/main.mw");
        let at = crate::analysis::FileRef::admitted(0);
        let mut resources = Vec::new();
        let mut stores = Vec::new();
        for declaration in &parsed.file.declarations {
            match declaration {
                marrow_syntax::Declaration::Resource(d) => resources.push((at, file.clone(), d)),
                marrow_syntax::Declaration::Store(d) => stores.push((at, file.clone(), d)),
                other => panic!("the corpus declares only resources and stores: {other:?}"),
            }
        }
        let budget = crate::decl::DeclarationBudget::default();
        let mut draft_owner = marrow_image::ImageDraft::new();
        let mut draft = admitted(&mut draft_owner);
        let mut diagnostics = DiagnosticCollector::new();
        let records = crate::types::TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &resources,
            &mut diagnostics,
            budget.clone(),
        )
        .expect("the corpus registry stays within the ledger budget");
        assert!(diagnostics.is_empty(), "the corpus types check clean");
        draft.commit();
        crate::durable::DurableRegistry::build(
            &mut draft_owner,
            &records,
            &resources,
            &stores,
            Some(&ledger),
            &mut diagnostics,
            budget,
            &mut Vec::new(),
        )
        .expect("the durable build settles refusals as diagnostics, not errors");
        let rows: Vec<String> = diagnostics
            .finish()
            .expect_complete()
            .iter()
            .map(|row| row.message().to_string())
            .collect();
        let bytes = draft_owner
            .encode()
            .expect("the corpus is inside every image bound")
            .bytes;
        (rows, bytes)
    };

    let (refused_rows, refused_bytes) = build(&format!("{DECLARATIONS}{REFUSED_STORE}"));
    let (control_rows, control_bytes) = build(DECLARATIONS);

    assert_eq!(
        refused_rows,
        vec![
            "index `tag` collides with an identity key, a stored field, or another index of \
             `notes`"
                .to_string()
        ],
        "the refused store must be refused by index admission after its graph staged",
    );
    assert!(control_rows.is_empty(), "the control corpus builds clean");
    assert_eq!(
        refused_bytes, control_bytes,
        "a store refused after staging must leave the image byte-identical to a build \
         that never declared it",
    );
}

/// A drift between the type registry and the declaration slice is the typed
/// invariant it was on the base line, raised at the directory join — never a
/// user-facing diagnostic.
///
/// The registry drives the join: an admitted resource whose declaration is missing
/// from the received slice cannot produce a row, so the drift is caught at the one
/// place the two inputs meet, before any store is built. Reporting it as
/// `check.type` would charge the user for a compiler inconsistency, which is
/// exactly what the pre-fix build did.
#[test]
fn a_registry_slice_drift_is_a_typed_invariant_not_a_user_error() {
    let source = "module main\n\nresource R {\n    required title: string\n}\n\n\
                  store ^r[id: int]: R\n\nfn main() {\n}\n";
    let parsed = marrow_syntax::parse_source(source);
    assert!(!parsed.has_errors(), "the corpus parses");
    let file = crate::test_file_identity("src/main.mw");
    let at = crate::analysis::FileRef::admitted(0);
    let mut resources = Vec::new();
    let mut stores = Vec::new();
    for declaration in &parsed.file.declarations {
        match declaration {
            marrow_syntax::Declaration::Resource(d) => resources.push((at, file.clone(), d)),
            marrow_syntax::Declaration::Store(d) => stores.push((at, file.clone(), d)),
            _ => {}
        }
    }
    let budget = crate::decl::DeclarationBudget::default();
    let mut draft_owner = marrow_image::ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let mut diagnostics = DiagnosticCollector::new();
    let records = crate::types::TypeRegistry::build(
        &mut draft,
        &[],
        &[],
        &[],
        &[],
        &resources,
        &mut diagnostics,
        budget.clone(),
    )
    .expect("the corpus registry stays within the ledger budget");
    draft.commit();
    // The drift: the registry admitted `R`, but the durable build receives an
    // empty declaration slice.
    let outcome = crate::durable::DurableRegistry::build(
        &mut draft_owner,
        &records,
        &[],
        &stores,
        None,
        &mut diagnostics,
        budget,
        &mut Vec::new(),
    );
    assert!(
        matches!(
            outcome,
            Err(crate::types::BuildError::Invariant(
                GenericInvariant::DurableResourceMissing(_)
            ))
        ),
        "registry/slice drift must abort at the invariant boundary",
    );
    assert!(
        diagnostics.finish().expect_complete().is_empty(),
        "the drift is a compiler fault; no user-facing row may be minted for it",
    );
}

const BRANCH_FIELD_IDS: &str = "marrow ids v0\n\
    machine-written by marrow; do not edit\n\
    id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
    id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
    id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
    id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
    id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
    id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
    id field Book.notes.elapsed 2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d\n\
    id field Book.notes.pinned 2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e\n\
    id root Book.notes.tags 3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a\n\
    id key Book.notes.tags.tagId 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
    id field Book.notes.tags.weight 3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c\n\
    id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
    id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
    id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
    id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
    high-water 0\nend\n";

fn branch_field_project(source: &str) -> marrow_project::ProjectInput {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    marrow_project::capture(
        &manifest,
        vec![marrow_project::CapturedFile::new(
            "src/main.mw".to_string(),
            source.as_bytes().to_vec(),
        )],
        Some(BRANCH_FIELD_IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture the branch-field fixture")
}

/// Source classification belongs to the Product, even when several roots use its
/// branch entry. Both complete compiler journeys establish their shared
/// declaration and encoded image before the actual resolver count is asserted.
#[test]
fn branch_field_annotation_is_resolved_once_per_product() {
    use marrow_image::{DurableMemberViewKind, LedgerIdBytes, Scalar, ValueShapeView};

    const PRODUCT: &str = r#"resource Book {
    required title: string
    notes[noteId: int] {
        required text: string
    }
}
"#;
    const ADD_A: &str = r#"
pub fn addA(id: int, t: string) {
    transaction {
        ^a[id].notes[1] = Book.notes(text: t)
    }
}
"#;
    const ADD_B: &str = r#"
pub fn addB(id: int, t: string) {
    transaction {
        ^b[id].notes[1] = Book.notes(text: t)
    }
}
"#;
    let one_root = format!("{PRODUCT}\nstore ^a[id: int]: Book\n{ADD_A}");
    let two_roots =
        format!("{PRODUCT}\nstore ^a[id: int]: Book\nstore ^b[id: int]: Book\n{ADD_A}{ADD_B}");
    let mut record_counts = [0; 2];
    for (index, source) in [one_root, two_roots].iter().enumerate() {
        let project = branch_field_project(source);
        let driven = super::drive(&project, super::TestMode::Exclude);
        let checked = driven
            .expect("the fixture fits the drive envelope")
            .production()
            .expect("the whole-entry exports compile");
        assert_eq!(
            checked
                .exports
                .iter()
                .map(|entry| entry.item.as_str())
                .collect::<Vec<_>>(),
            ["addA", "addB"][..index + 1],
        );
        let contract = checked.draft.contract_view();
        let roots = contract.roots().collect::<Vec<_>>();
        assert_eq!(roots.len(), index + 1);
        let mut branch_records = Vec::new();
        for (root_index, root) in roots.iter().enumerate() {
            assert_eq!(
                root.product().ledger_id(),
                LedgerIdBytes::from_bytes([13; 16])
            );
            assert_eq!(root.entry_record(), roots[0].entry_record());
            assert_eq!(
                root.placement().ledger_id(),
                LedgerIdBytes::from_bytes([[11, 27][root_index]; 16]),
            );
            let members = root.members().collect::<Vec<_>>();
            assert_eq!(members.len(), 2);
            let DurableMemberViewKind::Field(title) = members[0].kind() else {
                panic!("the first member is the root field");
            };
            assert_eq!(title.id(), LedgerIdBytes::from_bytes([14; 16]));
            let DurableMemberViewKind::Branch(branch) = members[1].kind() else {
                panic!("the second member is the notes branch");
            };
            assert_eq!(branch.placement(), LedgerIdBytes::from_bytes([42; 16]));
            assert_eq!(branch.keys().len(), 1);
            assert_eq!(branch.keys()[0].id, LedgerIdBytes::from_bytes([43; 16]));
            assert_eq!(branch.keys()[0].scalar, Scalar::Int);
            branch_records.push(branch.record());
            let fields = members[1].members().collect::<Vec<_>>();
            assert_eq!(fields.len(), 1);
            let DurableMemberViewKind::Field(text) = fields[0].kind() else {
                panic!("the branch member is the text field");
            };
            assert_eq!(text.id(), LedgerIdBytes::from_bytes([44; 16]));
            assert!(text.required());
            assert_eq!(
                contract.value_shapes().view(text.value()),
                Some(ValueShapeView::Scalar(Scalar::Text)),
            );
        }
        assert!(
            branch_records
                .iter()
                .all(|record| *record == branch_records[0])
        );
        record_counts[index] = checked.draft.record_type_count();
        let built = checked
            .encode()
            .unwrap_or_else(|_| panic!("the checked draft encodes"));
        assert!(!built.image.bytes.is_empty());
    }
    assert_eq!(
        record_counts[0], record_counts[1],
        "a second root over the same product shares its branch records"
    );
}

/// Typed field reads exercise the executable descriptors as well as the admitted
/// graph. An alias keeps Duration distinct from the narrower durable-key set.
#[test]
fn branch_field_capture_preserves_typed_nested_and_sparse_reads() {
    use marrow_image::{DurableMemberViewKind, LedgerIdBytes, Scalar, ValueShapeView};

    let project = branch_field_project(
        r#"alias Elapsed = duration

resource Book {
    required title: string
    notes[noteId: int] {
        required text: string
        elapsed: Elapsed
        required pinned: bool
        tags[tagId: int] {
            required weight: int
        }
    }
}

store ^a[id: int]: Book

pub fn readText(id: int, noteId: int): string {
    place note = ^a[id].notes[noteId]
    if exists(note) {
        return note.text
    }
    return ""
}

pub fn readElapsed(id: int, noteId: int): duration? {
    place note = ^a[id].notes[noteId]
    if exists(note) {
        return note.elapsed
    }
    return absent
}

pub fn readPinned(id: int, noteId: int): bool? {
    return ^a[id].notes[noteId].pinned
}

pub fn readWeight(id: int, noteId: int, tagId: int): int? {
    return ^a[id].notes[noteId].tags[tagId].weight
}
"#,
    );
    let checked = super::drive(&project, super::TestMode::Exclude)
        .expect("the typed-read fixture fits the envelope")
        .production()
        .expect("each branch field read has its declared type");
    assert_eq!(
        checked
            .exports
            .iter()
            .map(|entry| entry.item.as_str())
            .collect::<Vec<_>>(),
        ["readText", "readElapsed", "readPinned", "readWeight"],
    );
    let contract = checked.draft.contract_view();
    let roots = contract.roots().collect::<Vec<_>>();
    assert_eq!(roots.len(), 1);
    let members = roots[0].members().collect::<Vec<_>>();
    assert_eq!(members.len(), 2);
    assert!(matches!(
        members[1].kind(),
        DurableMemberViewKind::Branch(_)
    ));
    let fields = members[1].members().collect::<Vec<_>>();
    assert_eq!(fields.len(), 4);
    for (member, (id, scalar, required)) in fields.iter().zip([
        (44, Scalar::Text, true),
        (45, Scalar::Duration, false),
        (46, Scalar::Bool, true),
    ]) {
        let DurableMemberViewKind::Field(field) = member.kind() else {
            panic!("direct scalar fields precede nested branches");
        };
        assert_eq!(field.id(), LedgerIdBytes::from_bytes([id; 16]));
        assert_eq!(field.required(), required);
        assert_eq!(
            contract.value_shapes().view(field.value()),
            Some(ValueShapeView::Scalar(scalar))
        );
    }
    let DurableMemberViewKind::Branch(tags) = fields[3].kind() else {
        panic!("the final member is the nested tags branch");
    };
    assert_eq!(tags.placement(), LedgerIdBytes::from_bytes([58; 16]));
    let nested = fields[3].members().collect::<Vec<_>>();
    assert_eq!(nested.len(), 1);
    let DurableMemberViewKind::Field(weight) = nested[0].kind() else {
        panic!("the nested member is the weight field");
    };
    assert_eq!(weight.id(), LedgerIdBytes::from_bytes([60; 16]));
    assert!(weight.required());
    assert_eq!(
        contract.value_shapes().view(weight.value()),
        Some(ValueShapeView::Scalar(Scalar::Int))
    );
    let built = checked
        .encode()
        .unwrap_or_else(|_| panic!("typed branch reads encode"));
    assert!(!built.image.bytes.is_empty());
}

// ---- Image capacity: the semantic drive stops once retained bodies cannot fit.

/// One ordinary body, one generic instance shared by production and a test, and one
/// test body: the smallest shape where a check that skipped a settled population, or
/// visited one twice, encodes a different image than `compile_with_tests`.
const SHARED_GENERIC_WITH_TEST: &str = "module main\n\n\
    fn identity<T>(x: T): T {\n    return x\n}\n\n\
    pub fn f(): int {\n    return identity(1)\n}\n\n\
    test \"identity holds\" {\n    assert identity(2) == 2\n}\n";

/// `check` settles the test-inclusive population: the image it encodes is the one
/// `compile_with_tests` encodes, exports and tests included.
#[test]
fn check_settles_the_test_inclusive_population() {
    let input = capacity_project(&[("src/main.mw", SHARED_GENERIC_WITH_TEST.to_string())]);
    let with_tests = crate::compile_with_tests(&input).expect("the fixture compiles with its test");
    let checked = crate::check(&input).expect("the fixture checks clean");

    assert_eq!(checked.image.bytes, with_tests.image.bytes);
    assert_eq!(checked.exports.len(), 1);
    assert_eq!(checked.tests.len(), 1);
}

/// The check projection reads no editor fact: the same checked program projects to the
/// same image whether its facts were retained complete or discarded at a fact ceiling.
/// The analysis projection over the discarded terminal still refuses, as its consumer
/// contract requires.
#[test]
fn check_encodes_the_same_image_over_complete_and_limited_editor_facts() {
    use crate::analysis::{AnalysisFactLimit, BoundedAnalysisFacts, MAX_SNAPSHOT_FACT_COUNT};

    let input = capacity_project(&[("src/main.mw", SHARED_GENERIC_WITH_TEST.to_string())]);
    let complete = super::drive(&input, super::TestMode::Include).expect("admitted");
    assert!(matches!(complete.facts, BoundedAnalysisFacts::Complete(_)));
    let mut limited = super::drive(&input, super::TestMode::Include).expect("admitted");
    limited.facts = BoundedAnalysisFacts::Limited {
        limit: AnalysisFactLimit::Count {
            limit: MAX_SNAPSHOT_FACT_COUNT,
        },
    };

    let complete = complete.check_built().expect("complete facts check");
    let limited = limited.check_built().expect("limited facts still check");
    assert_eq!(complete.image.bytes, limited.image.bytes);
    assert_eq!(complete.exports.len(), 1);
    assert_eq!(limited.tests.len(), 1);
}

use marrow_image::bounds::MAX_IMAGE_BYTES;

fn capacity_project(files: &[(&str, String)]) -> marrow_project::ProjectInput {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| {
            marrow_project::CapturedFile::new(path.to_string(), source.as_bytes().to_vec())
        })
        .collect();
    marrow_project::capture(
        &manifest,
        captured,
        None,
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture project")
}

fn wide_body(statements: usize) -> String {
    let mut body = String::from("    var total = 0\n");
    for _ in 0..statements {
        body.push_str("    total += 1\n");
    }
    body.push_str("    return total\n");
    body
}

/// `functions` public bodies of `statements` accumulating statements each.
fn wide_functions(prefix: &str, functions: usize, statements: usize) -> String {
    let mut source = String::new();
    for index in 0..functions {
        source.push_str(&format!("pub fn {prefix}{index}(): int {{\n"));
        source.push_str(&wide_body(statements));
        source.push_str("}\n\n");
    }
    source
}

fn wide_module(functions: usize, statements: usize) -> String {
    format!(
        "module main\n\n{}",
        wide_functions("f", functions, statements)
    )
}

fn image_bytes_limit(result: Result<impl std::fmt::Debug, CompileFailure>) {
    match result {
        Err(CompileFailure::ResourceLimit(limit)) => {
            assert_eq!(limit.kind(), super::ResourceLimitKind::ImageBytes);
            assert_eq!(limit.limit(), MAX_IMAGE_BYTES as u64);
        }
        other => panic!("expected the image-bytes limit, got {other:?}"),
    }
}

/// The 16x512 shape fits: every body is retained and the image matches its
/// current-generation known answer.
#[test]
fn an_accepted_shape_retains_every_body_and_keeps_its_image_identity() {
    let input = capacity_project(&[("src/main.mw", wide_module(16, 512))]);
    let compiled = crate::compile(&input).expect("sixteen wide bodies fit the image");
    assert_eq!(compiled.image.bytes.len(), 477_073);
    assert_eq!(
        compiled.image.image_id.to_hex(),
        "8f634b4a3bceaaf05a2cee46afb82d22218522eea53269c5eb1025496372bbde",
    );
}

/// The 32x512 shape cannot fit: every production entry stops the drive at the first
/// settled body whose charge proves it and reports the image-bytes limit.
#[test]
fn a_refused_shape_reports_the_image_bytes_limit_from_every_entry() {
    let input = capacity_project(&[("src/main.mw", wide_module(32, 512))]);
    image_bytes_limit(crate::compile(&input));
    image_bytes_limit(crate::check(&input));

    match crate::analyze(
        std::sync::Arc::new(capacity_project(&[("src/main.mw", wide_module(32, 512))])),
        crate::InputRevision::new(1),
    ) {
        Err(crate::AnalysisFailure::ResourceLimit {
            limit: crate::AnalysisResourceLimit::Compile(limit),
            ..
        }) => assert_eq!(limit.kind(), super::ResourceLimitKind::ImageBytes),
        Err(_) => panic!("analysis reports the same stop through the compile limit"),
        Ok(_) => panic!("analysis does not mint a snapshot past the stop"),
    }
}

/// The stop is what keeps a far-over-ceiling program bounded: the drive stops at the
/// first settled body whose charge proves the ceiling rather than lowering the whole
/// program first. Sixteen modules of wide bodies are an order of magnitude past the
/// ceiling, and refuse in the work the twenty settled bodies of the shape just past it
/// cost — a drive that lowered them all would be sixteen times that.
#[test]
fn a_shape_far_past_the_ceiling_refuses_without_lowering_it_whole() {
    let mut files = vec![("src/main.mw".to_string(), wide_module(32, 512))];
    for index in 0..15 {
        files.push((
            format!("src/wide{index}.mw"),
            format!(
                "module wide{index}\n\n{}",
                wide_functions(&format!("g{index}_"), 32, 512)
            ),
        ));
    }
    let files: Vec<(&str, String)> = files
        .iter()
        .map(|(path, source)| (path.as_str(), source.clone()))
        .collect();

    image_bytes_limit(crate::compile(&capacity_project(&files)));
}

/// Test bodies settle under the same stop: the production image excludes them and
/// fits, the test image includes them and stops.
#[test]
fn test_bodies_settle_under_the_same_stop() {
    fn test_body(index: usize) -> String {
        format!(
            "test \"t{index}\" {{\n{}}}\n\n",
            wide_body(512).replace("    return total\n", "    assert total == 512\n")
        )
    }
    let mut source = wide_module(16, 512);
    for index in 0..16 {
        source.push_str(&test_body(index));
    }
    let input = capacity_project(&[("src/main.mw", source)]);

    assert!(crate::compile(&input).is_ok());
    image_bytes_limit(crate::compile_with_tests(&input));
    image_bytes_limit(crate::check(&input));
}

/// A later module's bodies settle under the same stop.
#[test]
fn a_later_module_settles_under_the_same_stop() {
    let input = capacity_project(&[
        ("src/main.mw", wide_module(16, 512)),
        (
            "src/wide.mw",
            format!("module wide\n\n{}", wide_functions("g", 16, 512)),
        ),
    ]);
    image_bytes_limit(crate::compile(&input));
}
/// A generic template whose body is wide enough to cross the charge on its own.
fn wide_template(statements: usize) -> String {
    format!(
        "fn acc<T>(seed: T): int {{\n{}}}\n\n",
        wide_body(statements)
    )
}

/// An inferred instance settles under the same stop, in the drain: nineteen ordinary
/// bodies stay under the charge, and the instance the driver infers crosses it.
#[test]
fn an_inferred_instance_settles_under_the_same_stop() {
    let driver = format!(
        "{}pub fn driver(): int {{\n    return acc(1)\n}}\n",
        wide_template(512)
    );
    let source = format!("{}{driver}", wide_module(19, 512));
    let input = capacity_project(&[("src/main.mw", source)]);
    image_bytes_limit(crate::compile(&input));
}

/// A template proof is erased with its transaction and never polled: a proof body wide
/// enough to cross the charge still erases to exactly the accepted 16x512 image.
#[test]
fn a_template_proof_is_erased_before_any_poll() {
    let source = format!("{}{}", wide_module(16, 512), wide_template(3_000));
    let input = capacity_project(&[("src/main.mw", source)]);
    let compiled = crate::compile(&input).expect("an uninstantiated proof retains nothing");
    assert_eq!(
        compiled.image.image_id.to_hex(),
        "8f634b4a3bceaaf05a2cee46afb82d22218522eea53269c5eb1025496372bbde",
    );
}

/// The stop precedes a later export-table limit: 257 public bodies wide enough to
/// cross the charge report the image-bytes limit at the hundredth body, while the
/// same declarations kept narrow reach the encoder's export verdict.
#[test]
fn the_stop_precedes_a_later_export_table_limit() {
    let input = capacity_project(&[("src/main.mw", wide_module(257, 100))]);
    image_bytes_limit(crate::compile(&input));

    let input = capacity_project(&[("src/main.mw", wide_module(257, 8))]);
    match crate::compile(&input) {
        Err(CompileFailure::ResourceLimit(limit)) => {
            assert_eq!(limit.kind(), super::ResourceLimitKind::Exports);
        }
        other => panic!("narrow bodies reach the export verdict, got {other:?}"),
    }
}
/// A growing generic instance chain: each drained instance queues the next until the
/// shared instantiation limit refuses a reservation.
fn growing_chain(statements: usize) -> String {
    format!(
        "module main\n\nfn grow<T>(x: T): int {{\n    var xs: List<T> = List()\n    xs = append(xs, x)\n{}    return grow(xs)\n}}\n\npub fn driver(): int {{\n    return grow(1)\n}}\n",
        "    xs = append(xs, x)\n".repeat(statements),
    )
}

/// The stop precedes a later instantiation limit: wide instance bodies cross the
/// charge before the chain exhausts the instantiation budget, while narrow ones let
/// the located instantiation limit report first. A body that trips the limit itself is
/// refused by the lowerer's terminal check and never appended, so no single body can
/// both settle and trip the limit; the drain's poll is nevertheless guarded like the
/// declared-body sites so the located row would win if that ever changed.
#[test]
fn the_stop_precedes_a_later_instantiation_limit() {
    let input = capacity_project(&[("src/main.mw", growing_chain(40))]);
    image_bytes_limit(crate::compile(&input));
    let input = capacity_project(&[("src/main.mw", growing_chain(0))]);
    match crate::compile(&input) {
        Err(CompileFailure::Diagnostics(diagnostics)) => assert_eq!(
            diagnostics.as_slice().len(),
            1,
            "the located instantiation limit reports once"
        ),
        other => panic!("a narrow chain reaches the instantiation limit, got {other:?}"),
    }
}
fn image_bytes_stop() -> SemanticOutcome {
    SemanticOutcome::ResourceLimit(super::CompileResourceLimit::new(
        super::ResourceLimitKind::ImageBytes,
        MAX_IMAGE_BYTES as u64,
    ))
}

/// An invariant discovered in executed work dominates the stop and every precheck
/// finding in both projections; the stop itself yields to a precheck finding.
#[test]
fn an_executed_invariant_dominates_the_stop_and_precheck_findings() {
    let row = || diagnostic(Code::CheckType, 3);

    let production = driven(
        finished(vec![row()]),
        finished(vec![row()]),
        SemanticOutcome::Invariant(template_proof_cause()),
    )
    .production_built();
    assert!(matches!(production, Err(CompileFailure::Invariant(_))));
    assert!(matches!(
        analyze_outcome(
            finished(vec![row()]),
            finished(vec![row()]),
            SemanticOutcome::Invariant(template_proof_cause()),
        ),
        Analyzed::Invariant(_)
    ));

    let production =
        driven(empty_terminal(), empty_terminal(), image_bytes_stop()).production_built();
    let Err(CompileFailure::ResourceLimit(limit)) = production else {
        panic!("the stop is the image-bytes resource limit")
    };
    assert_eq!(limit.kind(), super::ResourceLimitKind::ImageBytes);
    let Analyzed::ResourceLimit(limit) =
        analyze_outcome(empty_terminal(), empty_terminal(), image_bytes_stop())
    else {
        panic!("analysis reports the same stop")
    };
    assert_eq!(limit.kind(), super::ResourceLimitKind::ImageBytes);

    let production =
        driven(finished(vec![row()]), empty_terminal(), image_bytes_stop()).production_built();
    assert!(matches!(production, Err(CompileFailure::Diagnostics(_))));
    let Analyzed::Diagnostics(rows) =
        analyze_outcome(finished(vec![row()]), empty_terminal(), image_bytes_stop())
    else {
        panic!("a precheck finding is reported over the stop")
    };
    assert_eq!(rows.as_slice(), &[row()]);
}
