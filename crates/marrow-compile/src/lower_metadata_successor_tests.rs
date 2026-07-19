use super::*;

use marrow_syntax::{Declaration, parse_source};

fn function(source: &str) -> FunctionDecl {
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
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
    let registry = TypeRegistry::default();
    let mut donor = ImageDraft::new();
    let _ = registry.instantiate_list(&mut donor, GArg::Scalar(ScalarType::Int));
    registry
}

fn draft_fingerprint(draft: &ImageDraft) -> (Vec<u8>, marrow_image::ImageId) {
    let encoded = draft.encode().expect("test draft encodes");
    (encoded.bytes, encoded.image_id)
}

fn expected_ints(values: &[i64]) -> (Vec<u8>, marrow_image::ImageId) {
    let mut draft = ImageDraft::new();
    for value in values {
        draft.intern_int(*value);
    }
    draft_fingerprint(&draft)
}

#[allow(clippy::too_many_arguments)]
fn lowerer<'a>(
    draft: &'a mut ImageDraft,
    records: &'a TypeRegistry,
    durable: &'a DurableRegistry,
    functions: &'a FunctionRegistry,
    generics: &'a GenericRegistry<'a>,
    consts: &'a ConstRegistry,
    diagnostics: &'a mut Vec<SourceDiagnostic>,
) -> FnLowerer<'a> {
    FnLowerer::new(
        draft,
        records,
        durable,
        functions,
        generics,
        consts,
        diagnostics,
        "src/main.mw",
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
    let records = cache_ahead_registry();
    let durable = DurableRegistry::default();
    let functions = FunctionRegistry::default();
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::default();
    let mut diagnostics = Vec::new();
    let mut draft = ImageDraft::new();
    let mut lowerer = lowerer(
        &mut draft,
        &records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
    );

    let result = lowerer.lower_interpolation(parts, *span);
    let code = lowerer.code.clone();
    let local_count = lowerer.locals.len();
    let _ = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert!(result.is_none());
    assert!(diagnostics.is_empty());
    assert_eq!(local_count, 0);
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
    let records = cache_ahead_registry();
    let durable = DurableRegistry::default();
    let functions = FunctionRegistry::default();
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::default();
    let mut diagnostics = Vec::new();
    let mut draft = ImageDraft::new();
    let mut lowerer = lowerer(
        &mut draft,
        &records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
    );

    let flow = lowerer.lower_statement(statement);
    let code = lowerer.code.clone();
    let local_count = lowerer.locals.len();
    let slot_count = lowerer.slot_count;
    let _ = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert_eq!(flow, Flow::Rejected);
    assert!(diagnostics.is_empty());
    assert_eq!(local_count, 0);
    assert_eq!(slot_count, 2);
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
    let records = cache_ahead_registry();
    let durable = DurableRegistry::default();
    let functions = FunctionRegistry::default();
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::default();
    let mut diagnostics = Vec::new();
    let mut draft = ImageDraft::new();
    let mut lowerer = lowerer(
        &mut draft,
        &records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
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
    let _ = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert_eq!(flow, Flow::Rejected);
    assert!(diagnostics.is_empty());
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
    let records = cache_ahead_registry();
    let durable = DurableRegistry::default();
    let functions = FunctionRegistry::default();
    let generics = GenericRegistry::default();
    let consts = ConstRegistry::default();
    let mut diagnostics = Vec::new();
    let mut draft = ImageDraft::new();
    let mut lowerer = lowerer(
        &mut draft,
        &records,
        &durable,
        &functions,
        &generics,
        &consts,
        &mut diagnostics,
    );

    let flow = lowerer.lower_block(&function.body);
    let code = lowerer.code.clone();
    let _ = lowerer.finish("probe", Vec::new(), ImageType::Unit);

    assert_eq!(flow, Flow::Rejected);
    assert!(diagnostics.is_empty());
    assert!(
        !code
            .iter()
            .any(|instruction| matches!(instruction, Instr::ListNew(_)))
    );
    assert_eq!(draft_fingerprint(&draft), expected_ints(&[1]));
}
