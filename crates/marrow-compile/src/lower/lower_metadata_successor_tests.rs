use super::*;

use crate::compile::admitted;

use crate::decl::DeclarationBudget;
use crate::types::{CollectionKind, GenericInvariant, TypeInstKind};
use marrow_syntax::{Declaration, parse_source};

fn function(source: &str) -> FunctionDecl {
    let parsed = parse_source(source);
    assert!(
        !parsed.has_errors(),
        "fixture must parse cleanly: {:?}",
        parsed.diagnostics
    );
    parsed
        .file
        .declarations
        .into_iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.name == "probe" => Some(function),
            _ => None,
        })
        .expect("probe function exists")
}

fn cache_ahead_registry() -> TypeRegistry {
    let mut registry = TypeRegistry::empty(DeclarationBudget::default());
    let mut donor_owner = ImageDraft::new();
    let mut donor = admitted(&mut donor_owner);
    let _ = registry.instantiate_list(&mut donor, GArg::Scalar(ScalarType::Int));
    registry
}

fn draft_fingerprint(draft: &ImageDraft) -> (Vec<u8>, marrow_image::ImageId) {
    let encoded = draft.encode().expect("test draft encodes");
    (encoded.bytes, encoded.image_id)
}

fn expected_ints(values: &[i64]) -> (Vec<u8>, marrow_image::ImageId) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    for value in values {
        draft.intern_int(*value).expect("a within-domain mint");
    }
    draft_fingerprint(&draft)
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
fn collection_mismatch_in_interpolation_stops_before_later_part() {
    let function = function(
        "fn probe() {\n    const rendered = $\"{isEmpty(List(1))}AFTER_INTERPOLATION\"\n}\n",
    );
    let Statement::Const {
        value: Expression::Interpolation { parts, span },
        ..
    } = &function.body.statements[0]
    else {
        panic!("fixture contains an interpolation")
    };
    let mut records = cache_ahead_registry();
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::Discarding,
    );

    let result = lowerer.lower_interpolation(parts, *span);
    let code = lowerer.code.clone();
    let local_count = lowerer.locals.len();
    let outcome = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert!(result.is_none());
    assert!(diagnostics.is_empty());
    assert_eq!(local_count, 0);
    assert!(matches!(
        outcome,
        Err(GenericInvariant::CollectionIndexMismatch {
            kind: CollectionKind::List,
            cache_index: 1,
            draft_index: 0,
        })
    ));
    assert!(!code.iter().any(|instruction| matches!(
        instruction,
        Instr::ListNew(_) | Instr::ListLen | Instr::TextConcat
    )));
    assert_eq!(draft_fingerprint(&draft), expected_ints(&[1]));
}

#[test]
fn collection_mismatch_in_checked_annotation_stops_before_handler() {
    let function = function(
        "fn probe() {\n    const value: List<int> = checked 1 + 2\n        on out_of_range {\n            unreachable(\"AFTER_CHECKED_HANDLER\")\n        }\n}\n",
    );
    let statement = &function.body.statements[0];
    let mut records = cache_ahead_registry();
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::Discarding,
    );

    let flow = lowerer.lower_statement(statement);
    let code = lowerer.code.clone();
    let local_count = lowerer.locals.len();
    let slot_count = lowerer.slot_count;
    let outcome = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert_eq!(flow, Flow::Rejected);
    assert!(diagnostics.is_empty());
    assert_eq!(local_count, 0);
    assert_eq!(slot_count, 2);
    assert!(matches!(
        outcome,
        Err(GenericInvariant::CollectionIndexMismatch {
            kind: CollectionKind::List,
            cache_index: 1,
            draft_index: 0,
        })
    ));
    assert!(matches!(code.last(), Some(Instr::IntAddChecked(0))));
    assert_eq!(
        code.iter()
            .filter(|instruction| matches!(instruction, Instr::IntAddChecked(0)))
            .count(),
        1
    );
    assert!(
        !code
            .iter()
            .any(|instruction| matches!(instruction, Instr::Jump(_) | Instr::Unreachable(_)))
    );
    assert_eq!(draft_fingerprint(&draft), expected_ints(&[1, 2]));
}

#[test]
fn collection_mismatch_in_if_const_else_if_condition_is_terminal() {
    let function = function(
        "fn probe() {\n    if const present = maybe {\n    } else if isEmpty(List(1)) {\n        const after = trim(\"AFTER_COND\")\n    } else {\n    }\n}\n",
    );
    let statement = &function.body.statements[0];
    let mut records = cache_ahead_registry();
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::Discarding,
    );
    lowerer.locals.push(Local {
        name: "maybe".to_string(),
        ty: LTy::Scalar {
            scalar: ScalarType::Int,
            optional: true,
        },
        mutable: false,
        slot: 0,
    });
    lowerer.slot_count = 1;

    let flow = lowerer.lower_statement(statement);
    let code = lowerer.code.clone();
    let outcome = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert_eq!(flow, Flow::Rejected);
    assert!(diagnostics.is_empty());
    assert!(matches!(
        outcome,
        Err(GenericInvariant::CollectionIndexMismatch {
            kind: CollectionKind::List,
            cache_index: 1,
            draft_index: 0,
        })
    ));
    assert!(
        code.iter()
            .any(|instruction| matches!(instruction, Instr::Jump(0)))
    );
    assert!(
        !code
            .iter()
            .any(|instruction| matches!(instruction, Instr::JumpIfFalse(_)))
    );
    assert!(
        !code
            .iter()
            .any(|instruction| matches!(instruction, Instr::TextTrim))
    );
    assert!(!code.iter().any(|instruction| matches!(
        instruction,
        Instr::ListNew(_) | Instr::ListAppend | Instr::ListLen
    )));
    assert_eq!(draft_fingerprint(&draft), expected_ints(&[1]));
}

#[test]
fn collection_mismatch_in_first_block_statement_stops_later_mint_and_finish() {
    let function = function(
        "fn probe() {\n    const first = List(1)\n    const later = List(\"AFTER_BLOCK_MINT\")\n}\n",
    );
    let mut records = cache_ahead_registry();
    let durable = DurableRegistry::empty(DeclarationBudget::default());
    let functions = FunctionRegistry::empty(DeclarationBudget::default());
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::empty(DeclarationBudget::default());
    let mut diagnostics = DiagnosticCollector::new();
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let mut lowerer = lowerer(
        &mut draft,
        &mut records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
        FactSink::Discarding,
    );

    let flow = lowerer.lower_block(&function.body);
    let code = lowerer.code.clone();
    let outcome = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert_eq!(flow, Flow::Rejected);
    assert!(diagnostics.is_empty());
    assert!(matches!(
        outcome,
        Err(GenericInvariant::CollectionIndexMismatch {
            kind: CollectionKind::List,
            cache_index: 1,
            draft_index: 0,
        })
    ));
    assert!(
        !code
            .iter()
            .any(|instruction| matches!(instruction, Instr::ListNew(_)))
    );
    assert_eq!(draft_fingerprint(&draft), expected_ints(&[1]));
}

fn built_registry_with_generic_struct() -> (TypeRegistry, DraftTxn<'static>, usize) {
    let parsed = parse_source("struct Box<T> {\n    value: T\n}\n");
    assert!(!parsed.has_errors());
    let declaration = parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Struct(item) => Some(item),
            _ => None,
        })
        .expect("generic struct declaration exists");
    let structs = [(
        FileRef::admitted(0),
        crate::test_file_identity("src/main.mw"),
        declaration,
    )];
    let draft_owner: &'static mut ImageDraft = Box::leak(Box::new(ImageDraft::new()));
    let mut draft = draft_owner
        .begin_transaction(draft_owner.savepoint())
        .expect("a fresh savepoint admits");
    let mut diagnostics = DiagnosticCollector::new();
    let registry = TypeRegistry::build(
        &mut draft,
        &[],
        &[],
        &structs,
        &[],
        &[],
        &mut diagnostics,
        DeclarationBudget::default(),
    )
    .expect("the test registry stays within the ledger budget");
    assert!(diagnostics.is_empty());
    let template = registry
        .type_template_by_name("Box")
        .expect("Box template is registered");
    (registry, draft, template)
}

fn built_reserved_registry() -> (TypeRegistry, DraftTxn<'static>) {
    let draft_owner: &'static mut ImageDraft = Box::leak(Box::new(ImageDraft::new()));
    let mut draft = draft_owner
        .begin_transaction(draft_owner.savepoint())
        .expect("a fresh savepoint admits");
    let mut diagnostics = DiagnosticCollector::new();
    let registry = TypeRegistry::build(
        &mut draft,
        &[],
        &[],
        &[],
        &[],
        &[],
        &mut diagnostics,
        DeclarationBudget::default(),
    )
    .expect("the test registry stays within the ledger budget");
    assert!(diagnostics.is_empty());
    (registry, draft)
}

#[test]
fn generic_struct_constructor_transfers_the_registry_witness_error() {
    let (mut records, mut draft) = built_reserved_registry();
    let template = records
        .type_template_by_name("Option")
        .expect("reserved Option template exists");
    let draft_before = draft_fingerprint(&draft);
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
        FactSink::Discarding,
    );

    assert!(
        lowerer
            .lower_generic_struct_literal(template, &[], SourceSpan::default())
            .is_none()
    );
    let code = lowerer.code.clone();
    let outcome = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert!(matches!(
        outcome,
        Err(GenericInvariant::TemplateKindMismatch {
            template: selected,
            expected: TypeInstKind::Struct,
            actual: TypeInstKind::Enum,
        }) if selected == template
    ));
    assert!(diagnostics.is_empty());
    assert!(code.is_empty());
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

#[test]
fn generic_enum_constructor_transfers_the_registry_witness_error() {
    let (mut records, mut draft, template) = built_registry_with_generic_struct();
    let draft_before = draft_fingerprint(&draft);
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
        FactSink::Discarding,
    );

    assert!(
        lowerer
            .lower_generic_enum_construct(template, "item", &[], SourceSpan::default())
            .is_none()
    );
    let code = lowerer.code.clone();
    let outcome = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert!(matches!(
        outcome,
        Err(GenericInvariant::TemplateKindMismatch {
            template: selected,
            expected: TypeInstKind::Enum,
            actual: TypeInstKind::Struct,
        }) if selected == template
    ));
    assert!(diagnostics.is_empty());
    assert!(code.is_empty());
    assert_eq!(draft_fingerprint(&draft), draft_before);
}
