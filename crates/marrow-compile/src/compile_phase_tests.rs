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
/// invariant or resource limit: the projection reports the first
/// logically non-empty stage and never mixes stages.
#[test]
fn an_earlier_stage_dominates_a_later_semantic_failure() {
    let parse_row = diagnostic(Code::CheckType.as_str(), 3);
    let failure = driven(
        finished(vec![parse_row.clone()]),
        empty_terminal(),
        SemanticOutcome::Invariant(template_proof_cause()),
    )
    .into_built()
    .map(|_| ())
    .expect_err("a parse-stage row fails compilation");
    let CompileFailure::Diagnostics(diagnostics) = failure else {
        panic!("the parse stage's own rows are the failure")
    };
    assert_eq!(diagnostics.as_slice(), std::slice::from_ref(&parse_row));
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
    let failure = driven(
        terminal,
        empty_terminal(),
        SemanticOutcome::Invariant(template_proof_cause()),
    )
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
/// does, row by row: with empty prechecks a semantic invariant or
/// resource limit passes through; with prechecks present it is
/// suppressed for the precheck union; a semantic empty terminal is a
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

    // A precheck row suppresses the semantic invariant.
    let Analyzed::Diagnostics(rows) = analyze_outcome(
        finished(vec![row(3)]),
        empty_terminal(),
        SemanticOutcome::Invariant(template_proof_cause()),
    ) else {
        panic!("prechecks suppress the semantic failure")
    };
    assert_eq!(rows, vec![row(3)]);

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
    let failure = driven(
        collector.finish(),
        empty_terminal(),
        SemanticOutcome::Invariant(template_proof_cause()),
    )
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
