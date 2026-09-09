use super::*;

use crate::decl::DeclarationBudget;
use crate::types::{GenericInvariant, Reserved, TypeInstKind, count_metadata_directory_builds};
use marrow_image::{EnumTypeDef, RecordTypeDef};
use marrow_syntax::{Declaration, parse_source};

fn span() -> SourceSpan {
    SourceSpan {
        line: 1,
        column: 1,
        ..SourceSpan::default()
    }
}

fn name(name: &str) -> Expression {
    Expression::Name {
        segments: Box::new([NameSegment::new(name, span())]),
        span: span(),
    }
}

fn generic_enum_registry(draft: &mut DraftTxn<'_>) -> TypeRegistry {
    let mut diagnostics = DiagnosticCollector::new();
    TypeRegistry::build(
        draft,
        &[],
        &[],
        &[],
        &[],
        &[],
        &mut diagnostics,
        DeclarationBudget::default(),
    )
    .expect("the test registry stays within the ledger budget")
}

fn generic_struct_registry(draft: &mut DraftTxn<'_>) -> TypeRegistry {
    let parsed = parse_source(
        r#"struct Box<T> {
    value: T
}
"#,
    );
    assert!(!parsed.has_errors());
    let declaration = parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Struct(declaration) => Some(declaration),
            _ => None,
        })
        .expect("generic struct parses");
    let mut diagnostics = DiagnosticCollector::new();
    let records = TypeRegistry::build(
        draft,
        &[],
        &[],
        &[(
            crate::analysis::FileRef::admitted(0),
            crate::test_file_identity("src/main.mw"),
            declaration,
        )],
        &[],
        &[],
        &mut diagnostics,
        DeclarationBudget::default(),
    );
    assert!(diagnostics.is_empty());
    records.expect("the test registry stays within the ledger budget")
}

#[test]
fn recursive_generic_unification_builds_one_metadata_directory() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let list = records
        .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
        .expect("List<int> mints");
    let map = records
        .instantiate_map(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            GArg::Collection(list),
        )
        .expect("Map<int,List<int>> mints");
    let parameter = || TypeExpr::Name {
        text: "T".to_string(),
        segment_spans: Vec::new(),
        span: span(),
    };
    let annotation = TypeExpr::Apply {
        head: "Map".to_string(),
        head_span: span(),
        args: vec![
            parameter(),
            TypeExpr::Apply {
                head: "List".to_string(),
                head_span: span(),
                args: vec![parameter()],
                span: span(),
            },
        ],
        span: span(),
    };
    let type_params = vec![("T".to_string(), None)];
    let mut subst = vec![None];

    let (result, builds) = count_metadata_directory_builds(|| {
        unify_type_param(
            &records,
            &type_params,
            &annotation,
            LTy::Collection {
                idx: map,
                optional: false,
            },
            &mut subst,
        )
    });

    assert!(matches!(result, Ok(())));
    assert_eq!(subst, vec![Some(GArg::Scalar(ScalarType::Int))]);
    assert_eq!(builds, 1);
}

#[test]
fn generic_unification_prevalidates_inferred_metadata_before_named_mismatch() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let records = generic_enum_registry(&mut draft);
    let (_, orphan) = orphan_enum_and_struct(&mut draft);
    let arg = GArg::Struct(orphan);
    let expected = GenericInvariant::TypeArgumentTargetMissing(arg);
    let draft_before = draft.encode().expect("hostile draft still encodes");
    let type_params = vec![("T".to_string(), None)];
    let sentinel = vec![Some(GArg::Scalar(ScalarType::Bool))];

    assert_eq!(records.validate_type_arguments(&[arg]), Err(expected));
    let (_, builds) = count_metadata_directory_builds(|| {
        for (name, optional) in [
            ("int", false),
            ("MissingType", false),
            ("MissingType", true),
        ] {
            let annotation = TypeExpr::Name {
                text: name.to_string(),
                segment_spans: Vec::new(),
                span: span(),
            };
            let mut subst = sentinel.clone();
            let result = unify_type_param(
                &records,
                &type_params,
                &annotation,
                LTy::Struct {
                    ty: orphan,
                    optional,
                },
                &mut subst,
            );

            assert!(matches!(
                result,
                Err(UnifyError::Invariant(found)) if found == expected
            ));
            assert_eq!(subst, sentinel);
        }
    });
    assert_eq!(
        builds, 1,
        "the hostile preflight builds the shared directory once and reuses it across \
             every inferred-metadata check rather than rebuilding per argument"
    );
    assert_eq!(records.validate_type_arguments(&[arg]), Err(expected));
    let draft_after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

#[test]
fn map_resolution_validates_hostile_key_metadata_before_refusal() {
    let annotation = TypeExpr::Apply {
        head: "Map".to_string(),
        head_span: span(),
        args: vec![
            TypeExpr::Name {
                text: "K".to_string(),
                segment_spans: Vec::new(),
                span: span(),
            },
            TypeExpr::Name {
                text: "int".to_string(),
                segment_spans: Vec::new(),
                span: span(),
            },
        ],
        span: span(),
    };

    for family in ["struct", "enum", "collection"] {
        let mut draft_owner = ImageDraft::new();
        let savepoint = draft_owner.savepoint();
        let mut draft = draft_owner
            .begin_transaction(savepoint)
            .expect("a fresh savepoint admits");
        let mut records = generic_enum_registry(&mut draft);
        let (orphan_enum, orphan_struct) = orphan_enum_and_struct(&mut draft);
        let arg = match family {
            "struct" => GArg::Struct(orphan_struct),
            "enum" => GArg::Enum(orphan_enum),
            "collection" => GArg::Collection(CollTypeId::from_index(0)),
            _ => unreachable!("the hostile family table is closed"),
        };
        let expected = GenericInvariant::TypeArgumentTargetMissing(arg);
        let params = [TypeParamSlot {
            name: "K".to_string(),
            binding: ParamBinding::Concrete(arg),
        }];
        let draft_before = draft.encode().expect("hostile draft still encodes");
        assert_eq!(records.validate_type_arguments(&[arg]), Err(expected));

        let (result, builds) = count_metadata_directory_builds(|| {
            resolve_type(
                &mut records,
                &mut draft,
                &DurableRegistry::empty(DeclarationBudget::default()),
                &annotation,
                TypeEnv { params: &params },
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            )
        });
        assert!(matches!(
            result,
            Err(ResolveError::Invariant(found)) if found == expected
        ));
        assert_eq!(builds, 1, "{family} key uses one metadata proof");
        assert_eq!(records.validate_type_arguments(&[arg]), Err(expected));
        let draft_after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }
}

#[test]
fn lower_map_resolution_rejects_a_missing_nominal_before_value_mint() {
    let annotation = TypeExpr::Apply {
        head: "Map".to_string(),
        head_span: span(),
        args: vec![
            TypeExpr::Name {
                text: "K".to_string(),
                segment_spans: Vec::new(),
                span: span(),
            },
            TypeExpr::Apply {
                head: "List".to_string(),
                head_span: span(),
                args: vec![TypeExpr::Name {
                    text: "int".to_string(),
                    segment_spans: Vec::new(),
                    span: span(),
                }],
                span: span(),
            },
        ],
        span: span(),
    };
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let missing = GArg::Nominal(NominalId(0));
    let params = [TypeParamSlot {
        name: "K".to_string(),
        binding: ParamBinding::Concrete(missing),
    }];
    let expected = GenericInvariant::TypeArgumentTargetMissing(missing);
    let draft_before = draft.encode().expect("empty draft encodes");

    let (resolved, builds) = count_metadata_directory_builds(|| {
        resolve_type(
            &mut records,
            &mut draft,
            &DurableRegistry::empty(DeclarationBudget::default()),
            &annotation,
            TypeEnv { params: &params },
            MintSite {
                file: crate::test_main_file_identity(),
                span: span(),
            },
        )
    });
    assert!(matches!(
        resolved,
        Err(ResolveError::Invariant(found)) if found == expected
    ));
    assert_eq!(
        builds, 0,
        "the nominal owner rejects before List resolution"
    );
    let draft_after = draft.encode().expect("rejected draft encodes");
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
    assert_eq!(
        records
            .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
            .expect("the first post-refusal collection mints"),
        CollTypeId::from_index(0),
        "the refused Map did not mint its List value"
    );
}

#[allow(clippy::too_many_arguments)]
fn lowerer<'a, 'd>(
    draft: &'a mut DraftTxn<'d>,
    records: &'a mut TypeRegistry,
    durable: &'a DurableRegistry,
    functions: &'a FunctionRegistry,
    generics: &'a GenericRegistry<'a>,
    consts: &'a ConstRegistry,
    diagnostics: &'a mut DiagnosticCollector,
    facts: FactSink<'a>,
) -> FnLowerer<'a, 'd> {
    FnLowerer::new(
        draft,
        records,
        durable,
        functions,
        generics,
        consts,
        diagnostics,
        facts,
        crate::test_main_file_identity(),
        "main",
        RetType::Unit,
        BodyKind::Function,
    )
}

#[test]
fn local_slot_limit_rejection_is_atomic_and_reported_once() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    draft.commit();
    let before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let request_span = SourceSpan {
        start_byte: 40,
        end_byte: 41,
        line: 3,
        column: 15,
    };
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );

    for expected in 0..marrow_image::bounds::MAX_LOCALS {
        assert_eq!(
            lowerer.alloc_slot(request_span).map(usize::from),
            Some(expected)
        );
    }
    assert_eq!(
        usize::from(lowerer.slot_count),
        marrow_image::bounds::MAX_LOCALS
    );
    assert!(lowerer.alloc_slot(request_span).is_none());
    assert_eq!(
        usize::from(lowerer.slot_count),
        marrow_image::bounds::MAX_LOCALS,
        "a rejected request does not mutate the admitted count"
    );
    assert!(lowerer.alloc_slot(request_span).is_none());
    assert!(lowerer.terminal_rejection());
    assert_eq!(lowerer.diagnostics.probe_rows().len(), 1);
    assert_eq!(
        lowerer.diagnostics.probe_rows()[0].code(),
        Code::CheckResourceLimit.as_str()
    );
    assert_eq!(lowerer.diagnostics.probe_rows()[0].span(), request_span);
    assert!(matches!(
        lowerer.finish(func, "rejected", Vec::new(), ImageType::Unit),
        Ok(BodyOutcome::Refused)
    ));

    fill_refusal_sentinel(&mut draft, func);
    let after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(after.bytes, before.bytes);
    assert_eq!(after.image_id, before.image_id);
    assert_eq!(diagnostics.probe_rows().len(), 1);
}

#[test]
fn code_byte_limit_rejection_precedes_tape_mutation_and_reports_once() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    draft.commit();
    let before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let request_span = SourceSpan {
        start_byte: 40,
        end_byte: 60,
        line: 3,
        column: 5,
    };
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );

    assert_eq!(Instr::Pop.encoded_len(), 1);
    for _ in 0..marrow_image::bounds::MAX_CODE_BYTES {
        assert_eq!(lowerer.push(Instr::Pop, request_span), Ok(()));
    }
    assert_eq!(lowerer.code_bytes, marrow_image::bounds::MAX_CODE_BYTES);
    assert_eq!(lowerer.code.len(), marrow_image::bounds::MAX_CODE_BYTES);
    assert_eq!(lowerer.spans.len(), lowerer.code.len());
    assert_eq!(lowerer.full_spans.len(), lowerer.code.len());

    assert_eq!(
        lowerer.push(Instr::Pop, request_span),
        Err(LoweringFailure::CodeLimitReached)
    );
    assert_eq!(
        lowerer.push(Instr::Pop, request_span),
        Err(LoweringFailure::CodeLimitReached)
    );
    assert_eq!(lowerer.code_bytes, marrow_image::bounds::MAX_CODE_BYTES);
    assert_eq!(lowerer.code.len(), marrow_image::bounds::MAX_CODE_BYTES);
    assert_eq!(lowerer.spans.len(), lowerer.code.len());
    assert_eq!(lowerer.full_spans.len(), lowerer.code.len());
    assert!(lowerer.terminal_rejection());
    assert_eq!(lowerer.diagnostics.probe_rows().len(), 1);
    assert_eq!(
        lowerer.diagnostics.probe_rows()[0].code(),
        Code::CheckResourceLimit.as_str()
    );
    assert_eq!(lowerer.diagnostics.probe_rows()[0].span(), request_span);
    assert!(matches!(
        lowerer.finish(func, "rejected", Vec::new(), ImageType::Unit),
        Ok(BodyOutcome::Refused)
    ));

    fill_refusal_sentinel(&mut draft, func);
    let after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(after.bytes, before.bytes);
    assert_eq!(after.image_id, before.image_id);
    assert_eq!(diagnostics.probe_rows().len(), 1);
}

fn orphan_enum_and_struct(draft: &mut DraftTxn<'_>) -> (EnumId, TypeId) {
    let enum_name = draft
        .intern_string("OrphanEnum")
        .expect("a within-domain mint");
    let enum_id = draft
        .add_enum_type(EnumTypeDef {
            name: enum_name,
            variants: Vec::new(),
        })
        .expect("a within-domain mint");
    let struct_name = draft
        .intern_string("OrphanStruct")
        .expect("a within-domain mint");
    let struct_id = draft
        .add_record_type(RecordTypeDef {
            name: struct_name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
    (enum_id, struct_id)
}

fn assert_typed_invariant_rejects_consumer(invariant: GenericInvariant) {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    draft.commit();
    let before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );

    assert!(
        lowerer
            .accept_resolution::<()>(
                Err(ResolveError::Invariant(invariant)),
                span(),
                "this generic consumer",
            )
            .is_none()
    );
    assert!(lowerer.terminal_rejection());
    assert!(matches!(
        lowerer.finish(func, "broken", Vec::new(), ImageType::Unit),
        Err(found) if found == invariant
    ));
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(after.bytes, before.bytes);
    assert_eq!(after.image_id, before.image_id);
}

#[test]
fn lower_generic_reports_exact_missing_option_and_result_templates() {
    for reserved in [Reserved::Option, Reserved::Result] {
        assert_typed_invariant_rejects_consumer(GenericInvariant::ReservedTemplateMissing(
            reserved,
        ));
    }
}

#[test]
fn lower_generic_reports_exact_wrong_option_and_result_template_kinds() {
    for template in [0, 1] {
        assert_typed_invariant_rejects_consumer(GenericInvariant::TemplateKindMismatch {
            template,
            expected: TypeInstKind::Enum,
            actual: TypeInstKind::Struct,
        });
    }
}

/// An enum-shaped local whose row is not semantically Ready is a
/// typed internal failure, not an `enum_variants` expectation unwind.
#[test]
fn bare_enum_without_ready_variants_fails_without_unwinding() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let (enum_id, _) = orphan_enum_and_struct(&mut draft);
    draft.commit();
    let draft_before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    lowerer.locals.push(Local {
        name: "value".to_string(),
        ty: LTy::Enum {
            ty: enum_id,
            optional: false,
        },
        mutable: false,
        slot: 0,
    });

    assert!(matches!(
        lowerer.lower_match(&name("value"), &[], span()),
        Ok(Flow::Rejected)
    ));
    assert!(
        lowerer.lower_generic_struct_literal(0, &[], span()) == Err(LoweringFailure::Recoverable),
        "a later template-kind invariant also rejects lowering"
    );
    let result = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit);
    let Err(invariant) = result else {
        panic!("the first generic invariant must reject the real finish path")
    };
    fill_refusal_sentinel(&mut draft, func);
    let draft_after = draft.encode().expect("rejected draft still encodes");

    assert_eq!(
        invariant,
        GenericInvariant::ReadyBodyMissing(TypeInstId::Enum(enum_id))
    );
    assert!(diagnostics.is_empty());
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

/// An enum template routed to the generic-struct constructor is
/// classified by the template owner rather than unwinding at `expect`.
#[test]
fn enum_template_at_struct_constructor_fails_without_unwinding() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let (_, struct_id) = orphan_enum_and_struct(&mut draft);
    draft.commit();
    let draft_before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );

    assert!(
        lowerer.lower_generic_struct_literal(0, &[], span()) == Err(LoweringFailure::Recoverable)
    );
    assert!(
        lowerer
            .resolve_product_field(
                LTy::Struct {
                    ty: struct_id,
                    optional: false,
                },
                "value",
                span(),
                span(),
            )
            .is_none(),
        "a later missing-body invariant also rejects lowering"
    );
    let result = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit);
    let Err(invariant) = result else {
        panic!("the first generic invariant must reject the real finish path")
    };
    fill_refusal_sentinel(&mut draft, func);
    let draft_after = draft.encode().expect("rejected draft still encodes");

    assert_eq!(
        invariant,
        GenericInvariant::TemplateKindMismatch {
            template: 0,
            expected: TypeInstKind::Struct,
            actual: TypeInstKind::Enum,
        }
    );
    assert!(diagnostics.is_empty());
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

/// A bare struct id with no Ready body is a typed internal
/// failure, not a cache-body panic.
#[test]
fn bare_struct_without_ready_body_fails_without_unwinding() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let (_, type_id) = orphan_enum_and_struct(&mut draft);
    draft.commit();
    let draft_before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );

    assert!(
        lowerer
            .resolve_product_field(
                LTy::Struct {
                    ty: type_id,
                    optional: false,
                },
                "value",
                span(),
                span(),
            )
            .is_none()
    );
    assert!(
        lowerer.lower_generic_struct_literal(0, &[], span()) == Err(LoweringFailure::Recoverable),
        "a later template-kind invariant also rejects lowering"
    );
    let result = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit);
    let Err(invariant) = result else {
        panic!("the first generic invariant must reject the real finish path")
    };
    fill_refusal_sentinel(&mut draft, func);
    let draft_after = draft.encode().expect("rejected draft still encodes");

    assert_eq!(
        invariant,
        GenericInvariant::ReadyBodyMissing(TypeInstId::Record(type_id))
    );
    assert!(diagnostics.is_empty());
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

#[test]
fn generic_struct_minted_as_enum_is_an_exact_invariant() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_struct_registry(&mut draft);
    let template = records
        .type_template_by_name("Box")
        .expect("Box template exists");
    let record_id = records
        .mint_type_instance(
            &mut draft,
            template,
            &[GArg::Scalar(ScalarType::Int)],
            MintSite {
                file: crate::test_main_file_identity(),
                span: span(),
            },
        )
        .expect("Box row mints ready");
    let TypeInstId::Record(_) = record_id else {
        panic!("Box mints a record")
    };
    let (enum_id, _) = orphan_enum_and_struct(&mut draft);
    let expected = GenericInvariant::TypeBodyKindMismatch {
        id: TypeInstId::Enum(enum_id),
        body: TypeInstKind::Struct,
    };
    draft.commit();
    let before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    lowerer.reject_unification(
        UnifyError::Invariant(expected),
        span(),
        "this generic struct inference",
    );
    lowerer.locals.push(Local {
        name: "item".to_string(),
        ty: LTy::bare_scalar(ScalarType::Int),
        mutable: false,
        slot: 0,
    });
    let args = [Argument {
        name: Some(NameSegment::new("value", span())),
        value: name("item"),
    }];

    assert!(
        lowerer.lower_generic_struct_literal(template, &args, span())
            == Err(LoweringFailure::Recoverable)
    );
    let Err(invariant) = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit) else {
        panic!("wrong minted ID kind rejects finish")
    };
    assert_eq!(invariant, expected);
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(after.bytes, before.bytes);
    assert_eq!(after.image_id, before.image_id);
}

#[test]
fn generic_enum_minted_as_record_is_an_exact_invariant() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let template = records
        .type_template_by_name("Option")
        .expect("Option template exists");
    let _enum_id = records
        .instantiate_reserved_option(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            MintSite {
                file: crate::test_main_file_identity(),
                span: span(),
            },
        )
        .expect("Option row mints ready");
    let (_, record_id) = orphan_enum_and_struct(&mut draft);
    let expected = GenericInvariant::TypeBodyKindMismatch {
        id: TypeInstId::Record(record_id),
        body: TypeInstKind::Enum,
    };
    draft.commit();
    let before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    lowerer.reject_unification(
        UnifyError::Invariant(expected),
        span(),
        "this generic enum inference",
    );
    lowerer.locals.push(Local {
        name: "item".to_string(),
        ty: LTy::bare_scalar(ScalarType::Int),
        mutable: false,
        slot: 0,
    });
    let args = [Argument {
        name: Some(NameSegment::new("value", span())),
        value: name("item"),
    }];

    assert!(
        lowerer.lower_generic_enum_construct(template, "some", &args, span())
            == Err(LoweringFailure::Recoverable)
    );
    let Err(invariant) = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit) else {
        panic!("wrong minted ID kind rejects finish")
    };
    assert_eq!(invariant, expected);
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(after.bytes, before.bytes);
    assert_eq!(after.image_id, before.image_id);
}

#[test]
fn ready_enum_id_with_struct_body_rejects_lowering_exactly() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let enum_id = records
        .instantiate_reserved_option(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            MintSite {
                file: crate::test_main_file_identity(),
                span: span(),
            },
        )
        .expect("Option row mints ready");
    let expected = GenericInvariant::TypeBodyKindMismatch {
        id: TypeInstId::Enum(enum_id),
        body: TypeInstKind::Struct,
    };
    draft.commit();
    let draft_before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    assert!(
        lowerer
            .accept_resolution::<()>(
                Err(ResolveError::Invariant(expected)),
                span(),
                "this enum match",
            )
            .is_none()
    );
    lowerer.locals.push(Local {
        name: "value".to_string(),
        ty: LTy::Enum {
            ty: enum_id,
            optional: false,
        },
        mutable: false,
        slot: 0,
    });

    assert_eq!(
        lowerer.lower_match(&name("value"), &[], span()),
        Ok(Flow::Rejected)
    );
    let Err(invariant) = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit) else {
        panic!("wrong Ready body rejects finish")
    };
    assert_eq!(invariant, expected);
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let draft_after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

#[test]
fn template_confirmed_generic_enum_missing_ready_variant_is_invariant() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let template = records
        .type_template_by_name("Option")
        .expect("Option template exists");
    let enum_id = records
        .instantiate_reserved_option(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            MintSite {
                file: crate::test_main_file_identity(),
                span: span(),
            },
        )
        .expect("Option row mints ready");
    let expected = GenericInvariant::ReadyEnumVariantMissing {
        id: enum_id,
        template,
        variant: 1,
    };
    draft.commit();
    let draft_before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    assert!(
        lowerer
            .accept_resolution::<()>(
                Err(ResolveError::Invariant(expected)),
                span(),
                "this generic enum construction",
            )
            .is_none()
    );
    lowerer.locals.push(Local {
        name: "item".to_string(),
        ty: LTy::bare_scalar(ScalarType::Int),
        mutable: false,
        slot: 0,
    });
    let args = [Argument {
        name: Some(NameSegment::new("value", span())),
        value: name("item"),
    }];

    assert!(
        lowerer.lower_generic_enum_construct(template, "some", &args, span())
            == Err(LoweringFailure::Recoverable)
    );
    let Err(invariant) = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit) else {
        panic!("missing Ready variant rejects finish")
    };
    assert_eq!(invariant, expected);
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let draft_after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

#[test]
fn interpolation_invariant_stops_before_later_literal_emission() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let template = records
        .type_template_by_name("Option")
        .expect("Option template exists");
    let enum_id = records
        .instantiate_reserved_option(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            MintSite {
                file: crate::test_main_file_identity(),
                span: span(),
            },
        )
        .expect("Option row mints ready");
    let expected = GenericInvariant::ReadyEnumVariantMissing {
        id: enum_id,
        template,
        variant: 1,
    };
    draft.commit();
    let draft_before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    assert!(
        lowerer
            .accept_resolution::<()>(
                Err(ResolveError::Invariant(expected)),
                span(),
                "this interpolation expression",
            )
            .is_none()
    );
    lowerer.locals.push(Local {
        name: "item".to_string(),
        ty: LTy::bare_scalar(ScalarType::Int),
        mutable: false,
        slot: 0,
    });
    let parts = [
        InterpolationPart::Expr(Expression::Call {
            callee: Box::new(Expression::Name {
                segments: Box::new([
                    NameSegment::new("Option", span()),
                    NameSegment::new("some", span()),
                ]),
                span: span(),
            }),
            args: vec![Argument {
                name: Some(NameSegment::new("value", span())),
                value: name("item"),
            }],
            multiline: false,
            span: span(),
        }),
        InterpolationPart::Text {
            text: "later-sentinel".into(),
            span: span(),
        },
    ];

    assert_eq!(
        lowerer.lower_interpolation(&parts, span()),
        Err(LoweringFailure::Recoverable)
    );
    assert!(lowerer.code.is_empty());
    let Err(invariant) = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit) else {
        panic!("interpolation invariant rejects finish")
    };
    assert_eq!(invariant, expected);
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let draft_after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

#[test]
fn reserved_constructor_and_try_stop_before_effects_after_typed_reader_failure() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let option = records
        .instantiate_reserved_option(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            MintSite {
                file: crate::test_main_file_identity(),
                span: span(),
            },
        )
        .expect("Option row mints ready");
    let expected = GenericInvariant::TypeBodyKindMismatch {
        id: TypeInstId::Enum(option),
        body: TypeInstKind::Struct,
    };
    draft.commit();
    let before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    assert!(
        lowerer
            .accept_resolution::<()>(
                Err(ResolveError::Invariant(expected)),
                span(),
                "this reserved type reader",
            )
            .is_none()
    );

    assert!(
        lowerer.lower_ctor_as(
            CtorKind::None,
            &Expression::Name {
                segments: Box::new([NameSegment::new("none", span())]),
                span: span(),
            },
            LTy::Enum {
                ty: option,
                optional: false,
            },
        ) == Err(LoweringFailure::Recoverable)
    );
    assert_eq!(
        lowerer.lower_try(&name("value"), span()),
        Err(LoweringFailure::Recoverable)
    );
    assert!(lowerer.code.is_empty());
    assert!(matches!(
        lowerer.finish(func, "broken", Vec::new(), ImageType::Unit),
        Err(found) if found == expected
    ));
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(after.bytes, before.bytes);
    assert_eq!(after.image_id, before.image_id);
}

#[test]
fn checked_result_invariant_stops_before_handler_and_patch_work() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let expected = GenericInvariant::ReservedTemplateMissing(Reserved::Option);
    draft.intern_int(1).expect("a within-domain mint");
    draft.intern_int(2).expect("a within-domain mint");
    draft.commit();
    let draft_before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    assert!(
        lowerer
            .accept_resolution::<()>(
                Err(ResolveError::Invariant(expected)),
                span(),
                "this checked result annotation",
            )
            .is_none()
    );
    let integer = |text: &str| Expression::Literal {
        kind: LiteralKind::Integer,
        text: text.into(),
        span: span(),
    };
    let operation = Expression::Binary {
        op: BinaryOp::Add,
        operands: Box::new(marrow_syntax::BinaryOperands {
            left: integer("1"),
            right: integer("2"),
        }),
        span: span(),
    };
    let annotation = TypeExpr::Apply {
        head: "Option".to_string(),
        head_span: span(),
        args: vec![TypeExpr::Name {
            text: "int".to_string(),
            segment_spans: Vec::new(),
            span: span(),
        }],
        span: span(),
    };
    let handler = Block {
        statements: Box::new([Statement::Expr {
            value: Expression::Literal {
                kind: LiteralKind::String,
                text: "handler-sentinel".into(),
                span: span(),
            },
            span: span(),
        }]),
        comments: Vec::new(),
        span: span(),
    };

    assert_eq!(
        lowerer.lower_checked(
            &CheckedBind::Const {
                name: "result".to_string(),
                name_span: span(),
                ty: Some(Box::new(annotation)),
            },
            &operation,
            Some(&handler),
            None,
            span(),
        ),
        Ok(Flow::Rejected)
    );
    assert!(lowerer.code.is_empty());
    let Err(invariant) = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit) else {
        panic!("checked-result invariant rejects finish")
    };
    assert_eq!(invariant, expected);
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let draft_after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

#[test]
fn nested_else_if_terminal_invariant_never_falls_through_or_patches() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let expected = GenericInvariant::ReservedTemplateMissing(Reserved::Result);
    draft.commit();
    let before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    assert!(
        lowerer
            .accept_resolution::<()>(
                Err(ResolveError::Invariant(expected)),
                span(),
                "this nested condition",
            )
            .is_none()
    );
    let condition = Expression::Literal {
        kind: LiteralKind::Bool,
        text: "true".into(),
        span: span(),
    };
    let empty = Block {
        statements: Box::new([]),
        comments: Vec::new(),
        span: span(),
    };
    let else_ifs = [ElseIf {
        condition: condition.clone(),
        block: empty.clone(),
    }];

    assert_eq!(
        lowerer.lower_if_const_bindings(&[], Some(&condition), &empty, &else_ifs, Some(&empty),),
        Ok(Flow::Rejected)
    );
    assert_eq!(
        lowerer.lower_cond_chain(&[(&condition, &empty)], Some(&empty)),
        Ok(Flow::Rejected)
    );
    assert!(lowerer.code.is_empty());
    assert!(matches!(
        lowerer.finish(func, "broken", Vec::new(), ImageType::Unit),
        Err(found) if found == expected
    ));
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(after.bytes, before.bytes);
    assert_eq!(after.image_id, before.image_id);
}

#[test]
fn first_invariant_stops_real_block_before_later_owner_mutation() {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let mut records = generic_enum_registry(&mut draft);
    let template = records
        .type_template_by_name("Option")
        .expect("Option template exists");
    let enum_id = records
        .instantiate_reserved_option(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            MintSite {
                file: crate::test_main_file_identity(),
                span: span(),
            },
        )
        .expect("Option row mints ready");
    let expected = GenericInvariant::ReadyBodyMissing(TypeInstId::Enum(enum_id));
    draft.commit();
    let draft_before = refusal_control(&mut draft_owner);
    let mut draft = crate::compile::admitted(&mut draft_owner);
    let func = draft
        .reserve_function()
        .expect("the probe reserves its body slot");
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::discarding(),
    );
    assert!(
        lowerer
            .accept_resolution::<()>(
                Err(ResolveError::Invariant(expected)),
                span(),
                "this enum match",
            )
            .is_none()
    );
    lowerer.locals.push(Local {
        name: "value".to_string(),
        ty: LTy::Enum {
            ty: enum_id,
            optional: false,
        },
        mutable: false,
        slot: 0,
    });
    let block = Block {
        statements: Box::new([
            Statement::Match {
                scrutinee: name("value"),
                arms: Vec::new(),
                span: span(),
            },
            Statement::Const {
                name: "later_generic".to_string(),
                name_span: span(),
                ty: Some(Box::new(TypeExpr::Apply {
                    head: "Option".to_string(),
                    head_span: span(),
                    args: vec![TypeExpr::Name {
                        text: "int".to_string(),
                        segment_spans: Vec::new(),
                        span: span(),
                    }],
                    span: span(),
                })),
                value: Expression::Absent { span: span() },
                span: span(),
            },
            Statement::Expr {
                value: Expression::Literal {
                    kind: LiteralKind::String,
                    text: "later-sentinel".into(),
                    span: span(),
                },
                span: span(),
            },
            Statement::Expr {
                value: name("value"),
                span: span(),
            },
        ]),
        comments: Vec::new(),
        span: span(),
    };

    assert_eq!(lowerer.lower_block(&block), Ok(Flow::Rejected));
    assert!(lowerer.code.is_empty());
    assert_eq!(lowerer.locals.len(), 1);
    assert_eq!(lowerer.locals[0].name, "value");
    assert_eq!(lowerer.slot_count, 0);
    let Err(invariant) = lowerer.finish(func, "broken", Vec::new(), ImageType::Unit) else {
        panic!("first block invariant rejects finish")
    };
    assert_eq!(invariant, expected);
    assert!(diagnostics.is_empty());
    fill_refusal_sentinel(&mut draft, func);
    let draft_after = draft.encode().expect("rejected draft still encodes");
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);

    assert_eq!(
        records.mint_type_instance(
            &mut draft,
            template,
            &[GArg::Scalar(ScalarType::Int)],
            MintSite {
                file: crate::test_main_file_identity(),
                span: span(),
            },
        ),
        Ok(TypeInstId::Enum(enum_id))
    );
    let after_probe = draft.encode().expect("cache probe leaves draft intact");
    assert_eq!(after_probe.bytes, draft_before.bytes);
    assert_eq!(after_probe.image_id, draft_before.image_id);
}
