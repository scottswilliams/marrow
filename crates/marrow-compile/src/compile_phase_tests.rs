//! The compile driver's own unit tests: policy-outcome classification, phase
//! gating, and the registry fixtures the driver's seams are pinned with.

use crate::compile::admitted;

use super::valid_export_path;
use super::{
    AcceptedQueuedTemplateProofs, AcyclicCallGraph, AmbientTransactionClosure, Analyzed, Artifacts,
    BoundedDiagnostics, Built, CompileFailure, CompileStage, CompleteDeclaredFunctionBodies,
    CompleteDeclaredTestBodies, CompleteFunctionRegistry, CompleteLoweredFunctionSet,
    CompleteTypeRegistry, DeclarationExit, Driven, InvariantCause, SemanticOutcome,
    SignaturesComplete, analyze_outcome,
};
use crate::compile::Declaration;
use crate::diag::{DiagnosticCollector, MAX_DIAGNOSTIC_COUNT, SourceDiagnostic};
use crate::lower::FunctionRegistry;
use crate::types::{
    CollectionKind, GenericCacheInvariant, GenericInvariant, Reserved, TemplateProofError,
    TypeInstKind,
};
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
        has_direct_durable_op: false,
        owns_transaction: false,
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

/// The settlement wrappers expose exact operations, never a detachable producer or
/// payload. This pins the privacy shape that makes cross-transaction staging
/// unrepresentable: widening either wrapper requires changing this gate deliberately.
#[test]
fn staged_settlement_exposes_no_detached_owner_or_payload_surface() {
    let body = include_str!("analysis/facts/staging.rs");
    let durable = include_str!("durable/staging.rs");
    for source in [body, durable] {
        for forbidden in [
            "pub(crate) fn parts",
            "pub(super) fn parts",
            "-> &mut",
            "impl std::ops::Deref",
            "impl std::ops::DerefMut",
            "impl FnOnce",
            "impl FnMut",
            "into_inner",
        ] {
            assert!(
                !source.contains(forbidden),
                "a staged settlement wrapper exposes forbidden surface `{forbidden}`"
            );
        }
    }

    for producer in [
        "lower_function",
        "lower_instance",
        "lower_test",
        "prove_template",
    ] {
        assert!(
            body.contains(&format!("fn {producer}<'a>(\n        self,")),
            "transaction-derived `{producer}` output must consume its producer before it returns"
        );
    }
    for forbidden in [
        "pub(crate) fn commit(",
        "pub(crate) fn commit_export(",
        "pub(crate) fn commit_test(",
        "pub(crate) fn erase_proof(",
    ] {
        assert!(
            !body.contains(forbidden),
            "body custody exposes settlement separately from production through `{forbidden}`"
        );
    }
    assert!(durable.contains("pub(super) fn build_one(\n        self,"));
    assert!(!durable.contains("pub(super) fn commit("));
    assert!(!durable.contains("pub(super) fn rollback("));

    let facts = include_str!("analysis/facts.rs");
    assert!(facts.contains("pub(crate) struct FactSink<'a>"));
    assert!(facts.contains("state: FactSinkState<'a>"));
    assert!(!facts.contains("pub(crate) enum FactSink"));
    assert!(!facts.contains("pub(crate) fn sink"));

    let diagnostics = include_str!("diag.rs");
    assert!(!diagnostics.contains("StagedDiagnosticTxn"));

    let lower = include_str!("lower/mod.rs");
    assert!(
        !lower.contains("pub(crate) type LowerResult"),
        "the lowered outcome alias is private to the lowerer that produces it"
    );
}

fn diagnostic(code: &'static str, line: u32) -> SourceDiagnostic {
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

fn private_generic_cause_label(cause: GenericInvariant) -> &'static str {
    match cause {
        GenericInvariant::TemplateProof(cause) => match cause {
            TemplateProofError::UnstableFillState => "unstable template proof",
            TemplateProofError::LimitOwnerNotOpen => "closed limit owner",
        },
        GenericInvariant::CacheState(cause) => match cause {
            GenericCacheInvariant::ActiveBatchMissing => "active batch missing",
            GenericCacheInvariant::ActiveBatchRange => "active batch range",
            GenericCacheInvariant::ActiveRowCardinality => "active row cardinality",
            GenericCacheInvariant::ActiveRowKeyMismatch => "active row key mismatch",
            GenericCacheInvariant::ActiveFillStackNotEmpty => "active stack not empty",
            GenericCacheInvariant::FailureIndexOutOfRange => "failure index range",
            GenericCacheInvariant::DependentIndexOutOfRange => "dependent index range",
            GenericCacheInvariant::StableRowInActiveBatch => "stable active row",
            GenericCacheInvariant::IncompleteRowWithoutRefusal => "incomplete row",
            GenericCacheInvariant::FillingReuseOutsideBatch => "orphan Filling reuse",
            GenericCacheInvariant::SettledRowMissing => "settled row missing",
            GenericCacheInvariant::SettledRowStillFilling => "settled row Filling",
            GenericCacheInvariant::FillStackMismatch => "fill stack mismatch",
            GenericCacheInvariant::MintIndexDrift => "mint index drift",
            GenericCacheInvariant::MintKeyAlreadyPresent => "mint key already present",
        },
        GenericInvariant::ReservedTemplateMissing(reserved) => match reserved {
            Reserved::Option => "Option template missing",
            Reserved::Result => "Result template missing",
        },
        GenericInvariant::TypeTemplateMissing(_) => "type template missing",
        GenericInvariant::TypeArgumentCountMismatch { .. } => "type argument count mismatch",
        GenericInvariant::TemplateKindMismatch {
            expected, actual, ..
        } => match (expected, actual) {
            (TypeInstKind::Struct, TypeInstKind::Struct) => "struct is struct",
            (TypeInstKind::Struct, TypeInstKind::Enum) => "enum where struct expected",
            (TypeInstKind::Enum, TypeInstKind::Struct) => "struct where enum expected",
            (TypeInstKind::Enum, TypeInstKind::Enum) => "enum is enum",
        },
        GenericInvariant::TypeBodyKindMismatch { body, .. } => match body {
            TypeInstKind::Struct => "Ready struct body mismatch",
            TypeInstKind::Enum => "Ready enum body mismatch",
        },
        GenericInvariant::ReadyBodyShapeMismatch(_) => "Ready body shape mismatch",
        GenericInvariant::ReadyBodyMissing(_) => "Ready body missing",
        GenericInvariant::ReadyEnumVariantMissing { .. } => "Ready enum variant missing",
        GenericInvariant::TypeIdentityCollision(_) => "type identity collision",
        GenericInvariant::TypeInstantiationKeyCollision { .. } => {
            "type instantiation key collision"
        }
        GenericInvariant::TypeArgumentOrderViolation { .. } => "type argument order violation",
        GenericInvariant::TypeArgumentTargetMissing(_) => "type argument target missing",
        GenericInvariant::TypeArgumentParameter(_) => "concrete type argument is a parameter",
        GenericInvariant::BuilderDomain(_) => "value shape outside the builder domain",
        GenericInvariant::CollectionIndexMismatch { kind, .. } => match kind {
            CollectionKind::List => "List owner mismatch",
            CollectionKind::Map => "Map owner mismatch",
        },
        GenericInvariant::DeclarationIndexDrift => "declaration index drift",
        GenericInvariant::FunctionIndexDomain => "function index domain",
        GenericInvariant::DurableConstructionRefused => "durable construction refused",
        GenericInvariant::DurableResourceMissing(_) => "durable resource missing",
        GenericInvariant::DurableBranchKeyUnresolved => "durable branch key unresolved",
        GenericInvariant::DurableBranchFieldUnresolved => "durable branch field unresolved",
        GenericInvariant::ScalarResolutionLimit => "scalar resolution limit",
        GenericInvariant::DeclarationCoordinateMissing(_) => "declaration coordinate missing",
    }
}

#[test]
fn private_generic_cause_classification_has_no_wildcard() {
    assert_eq!(
        private_generic_cause_label(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        )),
        "unstable template proof"
    );
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
    .into_built();
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
    let parse_row = diagnostic(Code::CheckType.as_str(), 3);
    for semantic in [
        image_bytes_stop(),
        SemanticOutcome::Diagnostics(
            finished(vec![diagnostic(Code::CheckType.as_str(), 9)]),
            CompileStage::BodyLowering,
        ),
    ] {
        let failure = driven(
            finished(vec![parse_row.clone()]),
            empty_terminal(),
            semantic,
        )
        .into_built()
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
        diagnostic(Code::CheckType.as_str(), 4),
        diagnostic(Code::CheckType.as_str(), 9),
    ];
    let terminal = finished(expected.clone());
    let original_ptr = match &terminal {
        BoundedDiagnostics::Complete { rows, .. } => rows.as_ptr(),
        BoundedDiagnostics::Limited { .. } => panic!("two rows stay complete"),
    };
    let failure = driven(terminal, empty_terminal(), image_bytes_stop())
        .into_built()
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

/// Availability and the value are minted by one owner, and the availability
/// proof is zero-sized.
///
/// `Artifacts.functions` holds the proof token, never the table, so what `encode`
/// consumes cannot be forged outside [`CompleteFunctionRegistry`] — the property
/// the artifact set protects, that a resolved signature table nothing vouches for
/// is unrepresentable at encode, is preserved verbatim while the table itself
/// stays available to every phase that resolves call sites through it.
///
/// Unforgeability is enforced by the token's private field rather than asserted
/// here: `SignaturesComplete(())` has no literal form outside the `signatures`
/// module, so writing one in this file does not compile. Zero size is what this
/// test still owns — the proof must cost the artifact set nothing.
#[test]
fn the_signature_completeness_proof_is_zero_sized() {
    assert_eq!(std::mem::size_of::<SignaturesComplete>(), 0);
}

/// A refused signature standing behind an accepted duplicate of its name still
/// withholds the completeness proof.
///
/// The proof reads the ledger's refused set. A set that answered only for the
/// keys a lookup resolves to a refusal would skip this one, and `Artifacts`
/// would hold a proof minted over a table the compiler refused a declaration
/// in — the amendment's sentence would be false for exactly this source shape.
/// Resolve `functions` through the production registry owners, over a project
/// that declares nothing else.
///
/// Built through the real builders rather than hand-assembled: a completeness
/// proof is only meaningful over a table the production build produced.
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
    )
    .expect("an empty project builds an empty durable registry");
    let mut draft = admitted(&mut draft_owner);
    crate::lower::FunctionRegistry::build(
        &mut records,
        &mut draft,
        &durable,
        functions,
        crate::lower::ModuleLedger::new(crate::decl::DeclarationNamespace::Module, budget.clone()),
        BTreeMap::new(),
        &mut diagnostics,
        budget,
    )
    .expect("the signature ledger stays within its budget")
}

#[test]
fn a_refusal_behind_an_accepted_duplicate_withholds_the_completeness_proof() {
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
    assert!(
        CompleteFunctionRegistry(signatures).complete().is_none(),
        "a refused signature withholds the completeness proof",
    );
}

/// The fence's positive direction. Production cannot reach it — an unavailable
/// artifact always follows a refusal that reported, so the terminal is non-empty and
/// the diagnostic arm wins — which is exactly why the rule needs a direct test:
/// without one, deleting the availability arm outright leaves the whole workspace
/// green. Every artifact is withheld in turn, so each conjunct of the availability
/// destructure is load-bearing, and the all-available base is checked to be checked.
#[test]
fn an_empty_terminal_with_a_withheld_artifact_is_an_invariant() {
    let available = || Artifacts {
        types: Some(CompleteTypeRegistry),
        functions: CompleteFunctionRegistry(signature_registry(&[])).complete(),
        template_proofs: Some(AcceptedQueuedTemplateProofs),
        function_bodies: Some(CompleteDeclaredFunctionBodies),
        test_bodies: Some(CompleteDeclaredTestBodies),
        lowered: Some(CompleteLoweredFunctionSet(Vec::new())),
        call_graph: Some(AcyclicCallGraph {
            order: crate::call_graph::analyze(&[])
                .into_acyclic_order()
                .expect("an empty graph is acyclic"),
        }),
        transactions: Some(AmbientTransactionClosure),
    };
    assert!(
        available().refusal(empty_terminal()).is_none(),
        "an empty terminal with every artifact available is a checked program"
    );

    /// One artifact withheld from an otherwise complete set, by name.
    type Withhold = (&'static str, fn(&mut Artifacts));
    let withhold: [Withhold; 8] = [
        ("types", |a| a.types = None),
        ("functions", |a| a.functions = None),
        ("template_proofs", |a| a.template_proofs = None),
        ("function_bodies", |a| a.function_bodies = None),
        ("test_bodies", |a| a.test_bodies = None),
        ("lowered", |a| a.lowered = None),
        ("call_graph", |a| a.call_graph = None),
        ("transactions", |a| a.transactions = None),
    ];
    for (name, withhold) in withhold {
        let mut artifacts = available();
        withhold(&mut artifacts);
        let refusal = artifacts
            .refusal(empty_terminal())
            .unwrap_or_else(|| panic!("withholding {name} must refuse the program"));
        assert!(
            matches!(
                refusal,
                SemanticOutcome::Invariant(InvariantCause::UnavailableWithoutReport)
            ),
            "withholding {name} with an empty terminal is the unavailable-without-report \
             invariant, not a checked program or a diagnostic outcome"
        );
    }
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
        .into_built()
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
/// is suppressed for the precheck union; a semantic empty terminal is a
/// truthful empty snapshot (where production reports the empty-boundary
/// invariant above); and the union of stages may cross a ceiling no
/// single stage crossed.
#[test]
fn the_analysis_union_follows_the_stage_table() {
    let row = |line| diagnostic(Code::CheckType.as_str(), line);

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

    // A semantic empty terminal with empty prechecks is an empty snapshot.
    let Analyzed::Diagnostics(rows) = analyze_outcome(
        empty_terminal(),
        empty_terminal(),
        SemanticOutcome::Diagnostics(empty_terminal(), CompileStage::BodyLowering),
    ) else {
        panic!("a clean union is a producible snapshot")
    };
    assert!(rows.is_empty());

    // The ordered union: parse, then structural, then semantic rows.
    let Analyzed::Diagnostics(rows) = analyze_outcome(
        finished(vec![row(1)]),
        finished(vec![row(2)]),
        SemanticOutcome::Diagnostics(finished(vec![row(3)]), CompileStage::BodyLowering),
    ) else {
        panic!("a bounded union is a producible snapshot")
    };
    assert_eq!(rows, vec![row(1), row(2), row(3)]);

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
        collector.push(diagnostic(Code::CheckType.as_str(), line + 1));
    }
    let failure = driven(collector.finish(), empty_terminal(), image_bytes_stop())
        .into_built()
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
    // `EncodeDrift` classifies through the same invariant arm; its payload has no
    // constructor outside the emitter, so the wildcard-free match carries it.
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

/// A key table is constructed exactly once per declared tuple, per compile — and
/// the count is charged inside `KeyTable::take` itself, so a reconstruction spelled
/// at any call site still moves it.
///
/// The corpus is the round-1 review's own: two stores over one keyed-branch
/// resource with no identity ledger, so the first store refuses after staging and
/// the second walks the same product. Three declared tuples — one root tuple per
/// store row and one branch tuple in the shared resource projection — mean exactly
/// three constructions; a drift back to per-attempt branch construction, or any
/// consumer minting its own table from retained raw material, adds to the count.
#[test]
fn a_key_table_is_constructed_once_per_declared_tuple() {
    let source = "module main\n\nresource R {\n    required title: string\n\n    \
                  items[itemId: string] {\n        required value: string\n    }\n}\n\n\
                  store ^a[id: int]: R\n\nstore ^b[id: int]: R\n\nfn main() {\n}\n";
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        None,
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture project");
    let (result, counts) =
        crate::types::capture_scaling_counts(|| crate::compile::compile(&project));
    assert!(
        result.is_err(),
        "the unminted corpus is refused; both stores reach the durable build"
    );
    assert_eq!(
        counts.key_table_constructions, 3,
        "two root tuples and one branch tuple mean exactly three key-table \
         constructions, store-attempt independent",
    );
}

// ---- Image capacity: the semantic drive stops once retained bodies cannot fit.

/// The instruction population the driver has retained through settled bodies, when a
/// test is observing. Template proofs are erased with their transaction and never
/// settle, so the sum is exactly what the draft holds at each poll.
fn retained_population() -> &'static std::thread::LocalKey<std::cell::Cell<Option<usize>>> {
    thread_local! {
        static RETAINED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }
    &RETAINED
}

pub(super) fn observe_settled_body(
    draft: &marrow_image::ImageDraft,
    settled: marrow_image::FuncId,
) {
    retained_population().with(|slot| {
        let Some(population) = slot.get() else { return };
        let code = draft
            .function_code(settled)
            .expect("a settled body is retained by the draft");
        slot.set(Some(population + code.len()));
    });
}

/// Run `drive` while counting the instruction population settled bodies retain.
fn observing<T>(drive: impl FnOnce() -> T) -> (T, usize) {
    retained_population().with(|slot| slot.set(Some(0)));
    let result = drive();
    let population = retained_population()
        .with(|slot| slot.take())
        .expect("observation stays enabled across the drive");
    (result, population)
}

/// The encoder's span row width, which the charge mirrors. The largest prefix the
/// charge admits is `MAX_IMAGE_BYTES / (1 + SPAN_ROW_BYTES)` one-byte instructions,
/// and one settled body adds at most `MAX_CODE_BYTES` more, so the population any
/// stop retains is bounded by their sum. This is a retention bound, not a capacity
/// claim: nested operands, other owners, and input size are outside it.
const SPAN_ROW_BYTES: usize = 12;
const RETENTION_BOUND: usize = marrow_image::bounds::MAX_IMAGE_BYTES / (1 + SPAN_ROW_BYTES)
    + marrow_image::bounds::MAX_CODE_BYTES;

/// Every body of the corpus below is `4 + 4 * statements` instructions: `total += 1`
/// lowers to four, and the binding and return add four.
const fn wide_body_instructions(statements: usize) -> usize {
    4 + 4 * statements
}

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
            assert_eq!(limit.limit(), marrow_image::bounds::MAX_IMAGE_BYTES as u64);
        }
        other => panic!("expected the image-bytes limit, got {other:?}"),
    }
}

/// The 16x512 shape fits: every body is retained and the image is the one the base
/// produced.
#[test]
fn an_accepted_shape_retains_every_body_and_keeps_its_image_identity() {
    let input = capacity_project(&[("src/main.mw", wide_module(16, 512))]);
    let (compiled, population) = observing(|| crate::compile(&input));
    let compiled = compiled.expect("sixteen wide bodies fit the image");
    assert_eq!(population, 16 * wide_body_instructions(512));
    assert_eq!(compiled.image.bytes.len(), 477_073);
    assert_eq!(
        compiled.image.image_id.to_hex(),
        "20102c203a94992c2adb1396e052df4d5ce179be65c2070bd6ff9fe74b942869",
    );
}

/// The 32x512 shape cannot fit. Both production entries stop the drive at the first
/// settled body whose charge proves it — the twentieth — and report the image-bytes
/// limit; the remaining twelve bodies are never lowered.
#[test]
fn a_refused_shape_stops_the_drive_within_the_retention_bound() {
    let input = capacity_project(&[("src/main.mw", wide_module(32, 512))]);
    let stop_population = 20 * wide_body_instructions(512);
    assert!(stop_population * (1 + SPAN_ROW_BYTES) > marrow_image::bounds::MAX_IMAGE_BYTES);
    assert!(stop_population <= RETENTION_BOUND);

    let (result, population) = observing(|| crate::compile(&input));
    image_bytes_limit(result);
    assert_eq!(population, stop_population);

    let (result, population) = observing(|| {
        crate::analyze(
            std::sync::Arc::new(capacity_project(&[("src/main.mw", wide_module(32, 512))])),
            crate::InputRevision::new(1),
        )
    });
    match result {
        Err(crate::AnalysisFailure::ResourceLimit {
            limit: crate::AnalysisResourceLimit::Compile(limit),
            ..
        }) => assert_eq!(limit.kind(), super::ResourceLimitKind::ImageBytes),
        Err(_) => panic!("analysis reports the same stop through the compile limit"),
        Ok(_) => panic!("analysis does not mint a snapshot past the stop"),
    }
    assert_eq!(population, stop_population);
}

/// Test bodies settle under the same stop: the production image excludes them and
/// fits, the test image includes them and stops.
#[test]
fn test_bodies_settle_under_the_same_stop() {
    let mut source = wide_module(16, 512);
    for index in 0..16 {
        source.push_str(&format!("test \"t{index}\" {{\n"));
        source.push_str(&wide_body(512).replace("    return total\n", "    assert total == 512\n"));
        source.push_str("}\n\n");
    }
    let input = capacity_project(&[("src/main.mw", source)]);
    let (compiled, population) = observing(|| crate::compile(&input));
    assert!(compiled.is_ok());
    assert_eq!(population, 16 * wide_body_instructions(512));
    let (result, population) = observing(|| crate::compile_with_tests(&input));
    image_bytes_limit(result);
    // `assert total == 512` is three instructions longer than `return total`.
    let test_body_instructions = wide_body_instructions(512) + 3;
    assert_eq!(
        population,
        16 * wide_body_instructions(512) + 4 * test_body_instructions
    );
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
    let (result, population) = observing(|| crate::compile(&input));
    image_bytes_limit(result);
    assert_eq!(population, 20 * wide_body_instructions(512));
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
    let source = format!(
        "{}{}pub fn driver(): int {{\n    return acc(1)\n}}\n",
        wide_module(19, 512),
        wide_template(512),
    );
    let input = capacity_project(&[("src/main.mw", source)]);
    let (result, population) = observing(|| crate::compile(&input));
    image_bytes_limit(result);
    let driver_instructions = 3;
    assert_eq!(
        population,
        20 * wide_body_instructions(512) + driver_instructions
    );
}

/// A template proof is erased with its transaction and never polled: a proof body wide
/// enough to cross the charge still erases to exactly the accepted 16x512 image.
#[test]
fn a_template_proof_is_erased_before_any_poll() {
    let source = format!("{}{}", wide_module(16, 512), wide_template(3_000));
    let input = capacity_project(&[("src/main.mw", source)]);
    let (compiled, population) = observing(|| crate::compile(&input));
    let compiled = compiled.expect("an uninstantiated proof retains nothing");
    assert_eq!(population, 16 * wide_body_instructions(512));
    assert_eq!(
        compiled.image.image_id.to_hex(),
        "20102c203a94992c2adb1396e052df4d5ce179be65c2070bd6ff9fe74b942869",
    );
}

/// The stop precedes a later export-table limit: 257 public bodies wide enough to
/// cross the charge report the image-bytes limit at the hundredth body, while the
/// same declarations kept narrow reach the encoder's export verdict.
#[test]
fn the_stop_precedes_a_later_export_table_limit() {
    let input = capacity_project(&[("src/main.mw", wide_module(257, 100))]);
    let (result, population) = observing(|| crate::compile(&input));
    image_bytes_limit(result);
    assert_eq!(population, 100 * wide_body_instructions(100));

    let input = capacity_project(&[("src/main.mw", wide_module(257, 8))]);
    let (result, population) = observing(|| crate::compile(&input));
    match result {
        Err(CompileFailure::ResourceLimit(limit)) => {
            assert_eq!(limit.kind(), super::ResourceLimitKind::Exports);
        }
        other => panic!("narrow bodies reach the export verdict, got {other:?}"),
    }
    assert_eq!(population, 257 * wide_body_instructions(8));
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
/// the located instantiation limit report first.
#[test]
fn the_stop_precedes_a_later_instantiation_limit() {
    let input = capacity_project(&[("src/main.mw", growing_chain(40))]);
    let (result, population) = observing(|| crate::compile(&input));
    image_bytes_limit(result);
    assert!(population > marrow_image::bounds::MAX_IMAGE_BYTES / (1 + SPAN_ROW_BYTES));
    assert!(population <= RETENTION_BOUND);

    let input = capacity_project(&[("src/main.mw", growing_chain(0))]);
    let (result, population) = observing(|| crate::compile(&input));
    match result {
        Err(CompileFailure::Diagnostics(diagnostics)) => assert_eq!(
            diagnostics.as_slice().len(),
            1,
            "the located instantiation limit reports once"
        ),
        other => panic!("a narrow chain reaches the instantiation limit, got {other:?}"),
    }
    assert!(population <= marrow_image::bounds::MAX_IMAGE_BYTES / (1 + SPAN_ROW_BYTES));
}

fn image_bytes_stop() -> SemanticOutcome {
    SemanticOutcome::ResourceLimit(super::CompileResourceLimit::new(
        super::ResourceLimitKind::ImageBytes,
        marrow_image::bounds::MAX_IMAGE_BYTES as u64,
    ))
}

/// An invariant discovered in executed work dominates the stop and every precheck
/// finding in both projections; the stop itself yields to a precheck finding.
#[test]
fn an_executed_invariant_dominates_the_stop_and_precheck_findings() {
    let row = || diagnostic(Code::CheckType.as_str(), 3);

    let production = driven(
        finished(vec![row()]),
        finished(vec![row()]),
        SemanticOutcome::Invariant(template_proof_cause()),
    )
    .into_built();
    assert!(matches!(production, Err(CompileFailure::Invariant(_))));
    assert!(matches!(
        analyze_outcome(
            finished(vec![row()]),
            finished(vec![row()]),
            SemanticOutcome::Invariant(template_proof_cause()),
        ),
        Analyzed::Invariant(_)
    ));

    let production = driven(empty_terminal(), empty_terminal(), image_bytes_stop()).into_built();
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
        driven(finished(vec![row()]), empty_terminal(), image_bytes_stop()).into_built();
    assert!(matches!(production, Err(CompileFailure::Diagnostics(_))));
    let Analyzed::Diagnostics(rows) =
        analyze_outcome(finished(vec![row()]), empty_terminal(), image_bytes_stop())
    else {
        panic!("a precheck finding is reported over the stop")
    };
    assert_eq!(rows, vec![row()]);
}
