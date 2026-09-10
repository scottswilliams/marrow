use crate::{FunctionIndex, RetShape, SealedInstr, TestKind, VerifiedImage, VerifyPhase};
use marrow_image::{
    ConstId, DeclarationMemberDef, DeclarationMemberShape, DemandAtom, ExportDemand, ExportId,
    FieldDef, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, OP_CALL,
    OP_RETURN, OperationClass, PlannedSiteRef, RecordTypeDef, RootOccurrenceDef, Scalar,
    SemanticPath, SemanticTarget, SpanEntry,
};
use std::cell::Cell;
use std::panic::{catch_unwind, resume_unwind};

use crate::verify::{admitted_plan, image_forgery, site_seam};

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    projection_instructions: usize,
    closure_edges: usize,
}

thread_local! {
    static COUNTS: Cell<Option<Counts>> = const { Cell::new(None) };
}

pub(in crate::verify) fn record_projection_instruction() {
    COUNTS.with(|cell| {
        if let Some(mut counts) = cell.get() {
            counts.projection_instructions += 1;
            cell.set(Some(counts));
        }
    });
}

pub(in crate::verify) fn record_closure_edge() {
    COUNTS.with(|cell| {
        if let Some(mut counts) = cell.get() {
            counts.closure_edges += 1;
            cell.set(Some(counts));
        }
    });
}

fn observe(bytes: &[u8]) -> (VerifiedImage, Counts) {
    COUNTS.with(|cell| {
        assert!(cell.get().is_none(), "observations do not nest");
        cell.set(Some(Counts::default()));
    });
    let result = catch_unwind(|| crate::verify(bytes));
    let counts = COUNTS.with(|cell| cell.take().expect("observation is active"));
    match result {
        Ok(result) => (result.expect("acyclic presence diamond verifies"), counts),
        Err(panic) => resume_unwind(panic),
    }
}

fn image(
    bodies: impl FnOnce(ConstId, &PlannedSiteRef) -> Vec<Vec<Instr>>,
    export: Option<usize>,
    test: Option<usize>,
) -> Vec<u8> {
    image_with_roots(
        &[("counters", 4, 5)],
        |key, entries| bodies(key, &entries[0]),
        export,
        test,
    )
}

fn image_with_roots(
    roots: &[(&str, u8, u8)],
    bodies: impl FnOnce(ConstId, &[PlannedSiteRef]) -> Vec<Vec<Instr>>,
    export: Option<usize>,
    test: Option<usize>,
) -> Vec<u8> {
    let mut owner = ImageDraft::new();
    let savepoint = owner.savepoint();
    let mut draft = owner.begin_transaction(savepoint).expect("fresh savepoint");
    draft.set_application_identity(LedgerIdBytes::from_bytes([1; 16]));
    let product = LedgerIdBytes::from_bytes([2; 16]);
    let name = draft.intern_string("Counter").expect("record name");
    let field_name = draft.intern_string("value").expect("field name");
    let record = draft
        .add_record_type(RecordTypeDef {
            name,
            fields: vec![FieldDef {
                name: field_name,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        })
        .expect("required integer field");
    let value = draft.value_scalar(Scalar::Int).expect("integer shape");
    draft
        .declare_product(
            &admitted_plan::admitted_plan(),
            product,
            record,
            vec![DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Field {
                    id: LedgerIdBytes::from_bytes([3; 16]),
                    required: true,
                    value,
                },
            }],
        )
        .expect("one-field product");
    assert!(!roots.is_empty() && roots.len() <= 2);
    let mut entries = Vec::new();
    for &(name, placement, key_id) in roots {
        let name = draft.intern_string(name).expect("root name");
        let root = draft
            .add_root_occurrence(
                &admitted_plan::admitted_plan(),
                product,
                RootOccurrenceDef {
                    name,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: LedgerIdBytes::from_bytes([key_id; 16]),
                    }],
                    placement: LedgerIdBytes::from_bytes([placement; 16]),
                    indexes: Vec::new().into(),
                },
            )
            .expect("integer-keyed root");
        entries.push(site_seam::site(
            &mut draft,
            root.occurrence(),
            root.placement_path(),
            SemanticTarget::WholePayload,
        ));
    }
    let key = draft.intern_int(0).expect("zero key");
    let bodies = bodies(key, &entries);
    assert!(!bodies.is_empty() && bodies.len() <= 5);
    assert!(bodies.iter().map(Vec::len).sum::<usize>() <= 32);
    let source = draft.intern_string("calls.mw").expect("source name");
    for (index, code) in bodies.into_iter().enumerate() {
        let name = draft
            .intern_string(["f0", "f1", "f2", "f3", "f4"][index])
            .expect("function name");
        let function = draft
            .add_function(FunctionDef {
                name,
                source,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                spans: vec![SpanEntry {
                    instr_index: 0,
                    line: u32::try_from(index + 1).expect("small function index"),
                    column: 1,
                }],
                code,
            })
            .expect("function operands are live");
        assert_eq!(usize::from(function.index()), index);
        if export == Some(index) {
            draft.add_export(ExportId::of_local("", "entry"), function);
        }
        if test == Some(index) {
            draft.add_test_entry(name, function);
        }
    }
    let bytes = draft.encode().expect("small coherent image").bytes;
    assert!(bytes.len() <= 8 * 1024);
    bytes
}

fn presence_read(key: ConstId, entry: &PlannedSiteRef) -> Vec<Instr> {
    vec![
        Instr::ConstLoad(key),
        Instr::DurExists(entry.clone()),
        Instr::Pop,
        Instr::Return,
    ]
}

fn presence_demand() -> ExportDemand {
    ExportDemand::from_atoms([DemandAtom::new(
        SemanticPath::root(
            LedgerIdBytes::from_bytes([1; 16]),
            LedgerIdBytes::from_bytes([4; 16]),
        ),
        OperationClass::Presence,
    )])
}

#[test]
fn call_projection_and_closure_visit_each_occurrence_once() {
    // Complete both numbering variants' semantic assertions before the work bound.
    let observations = [[0u16, 1, 2, 3, 4], [4, 3, 2, 1, 0]].map(|roles| {
        let [root, left, right, leaf, isolated] = roles;
        let bytes = image(
            |key, entry| {
                let mut bodies = vec![vec![Instr::Return]; 5];
                bodies[usize::from(root)] = vec![
                    Instr::Call(left),
                    Instr::Call(left),
                    Instr::Call(right),
                    Instr::Return,
                ];
                bodies[usize::from(left)] = vec![Instr::Call(leaf), Instr::Return];
                bodies[usize::from(right)] = vec![Instr::Call(leaf), Instr::Return];
                bodies[usize::from(leaf)] = presence_read(key, entry);
                bodies
            },
            Some(usize::from(root)),
            None,
        );
        let (verified, counts) = observe(&bytes);
        assert_eq!(verified.functions().len(), 5);
        assert_eq!(verified.exports().len(), 1);
        assert!(verified.test_entries().is_empty());
        assert_eq!(
            verified
                .functions()
                .iter()
                .map(|function| function.instrs().len())
                .sum::<usize>(),
            13,
        );
        let expected = presence_demand();
        for index in [root, left, right, leaf] {
            let function = verified
                .function(FunctionIndex::new(index))
                .expect("connected function exists");
            assert_eq!(function.demand(), expected.as_view());
            assert!(!function.body().is_mutating());
            assert!(function.body().params().is_empty());
            assert_eq!(function.body().ret(), RetShape::Unit);
        }
        for index in [left, right] {
            assert!(matches!(
                verified.functions()[usize::from(index)].instrs(),
                [SealedInstr::Call(target), SealedInstr::Return] if *target == leaf
            ));
        }
        assert!(matches!(
            verified.functions()[usize::from(leaf)].instrs(),
            [
                SealedInstr::ConstLoad(0),
                SealedInstr::DurExists(0),
                SealedInstr::Pop,
                SealedInstr::Return
            ]
        ));
        let empty = verified
            .function(FunctionIndex::new(isolated))
            .expect("isolated function exists");
        assert!(empty.demand().is_empty());
        assert!(!empty.body().is_mutating());
        assert!(matches!(empty.body().instrs(), [SealedInstr::Return]));
        let export = &verified.exports()[0];
        assert_eq!(export.function(), FunctionIndex::new(root));
        assert_eq!(export.id(), ExportId::of_local("", "entry"));
        assert_eq!(
            verified
                .function(export.function())
                .expect("verified export function")
                .demand(),
            expected.as_view()
        );
        assert_eq!(export.demand_id(), expected.demand_set_id());
        assert_eq!(export.reachable_sites(), &[0]);
        assert!(!export.is_mutating());
        let root_code = verified.functions()[usize::from(root)].instrs();
        assert!(matches!(
            root_code,
            [SealedInstr::Call(a), SealedInstr::Call(b), SealedInstr::Call(c), SealedInstr::Return]
                if *a == left && *b == left && *c == right
        ));
        eprintln!("call graph roles={roles:?}: {counts:?}");
        counts
    });
    let actual = observations.into_iter().fold((0, 0), |total, counts| {
        (
            total.0 + counts.projection_instructions,
            total.1 + counts.closure_edges,
        )
    });
    assert_eq!(
        actual,
        (26, 10),
        "each sealed instruction is projected once and each call occurrence expands once",
    );
}

#[test]
fn function_and_selected_demands_preserve_driver_semantics() {
    let bytes = image(
        |key, entry| {
            vec![
                vec![
                    Instr::Call(1),
                    Instr::Call(1),
                    Instr::Call(2),
                    Instr::Return,
                ],
                vec![Instr::Call(3), Instr::Return],
                vec![Instr::Call(3), Instr::Return],
                presence_read(key, entry),
                vec![Instr::Call(0), Instr::Return],
            ]
        },
        Some(0),
        Some(4),
    );
    let verified = crate::verify(&bytes).expect("driver demand verifies");
    assert_eq!(verified.functions().len(), 5);
    assert_eq!(verified.exports().len(), 1);
    assert_eq!(verified.test_entries().len(), 1);
    assert_eq!(
        verified
            .functions()
            .iter()
            .map(|function| function.instrs().len())
            .sum::<usize>(),
        14,
    );
    let expected = presence_demand();
    let expected_id = expected.demand_set_id();
    for index in 0..5 {
        let function = verified
            .function(FunctionIndex::new(index))
            .expect("every function ordinal remains materialized");
        assert_eq!(function.demand(), expected.as_view());
        assert_eq!(function.demand().demand_set_id(), expected_id);
        assert!(function.body().params().is_empty());
        assert_eq!(function.body().ret(), RetShape::Unit);
        assert!(!function.body().is_mutating());
    }
    let export = &verified.exports()[0];
    assert_eq!(export.function(), FunctionIndex::new(0));
    assert_eq!(export.id(), ExportId::of_local("", "entry"));
    assert_eq!(
        verified
            .function(export.function())
            .expect("verified export function")
            .demand(),
        expected.as_view()
    );
    assert_eq!(export.demand_id(), expected_id);
    assert_eq!(export.reachable_sites(), &[0]);
    assert!(!export.is_mutating());
    let test = &verified.test_entries()[0];
    assert_eq!(test.func(), FunctionIndex::new(4));
    assert_eq!(test.name(), "f4");
    assert_eq!(test.kind(), TestKind::Driver);
    assert_eq!(
        verified
            .function(test.func())
            .expect("verified test function")
            .demand(),
        expected.as_view()
    );
    assert_eq!(
        verified
            .function(test.func())
            .expect("verified test function")
            .demand()
            .demand_set_id(),
        expected_id
    );
    assert!(matches!(
        verified.functions()[4].instrs(),
        [SealedInstr::Call(0), SealedInstr::Return]
    ));
}

#[test]
fn unequal_function_demands_borrow_the_same_atoms() {
    let bytes = image_with_roots(
        &[("counters", 4, 5), ("others", 6, 7)],
        |key, entries| {
            vec![
                vec![Instr::Call(1), Instr::Call(3), Instr::Return],
                vec![Instr::Call(2), Instr::Return],
                presence_read(key, &entries[0]),
                presence_read(key, &entries[1]),
                vec![Instr::Return],
            ]
        },
        Some(1),
        Some(0),
    );
    let verified = crate::verify(&bytes).expect("overlapping presence demands verify");
    assert_eq!(verified.functions().len(), 5);
    assert_eq!(verified.exports().len(), 1);
    assert_eq!(verified.test_entries().len(), 1);
    assert_eq!(verified.sites().len(), 2);
    assert_eq!(
        verified
            .functions()
            .iter()
            .map(|function| function.instrs().len())
            .sum::<usize>(),
        14,
    );
    let a = presence_demand();
    let b = ExportDemand::from_atoms([DemandAtom::new(
        SemanticPath::root(
            LedgerIdBytes::from_bytes([1; 16]),
            LedgerIdBytes::from_bytes([6; 16]),
        ),
        OperationClass::Presence,
    )]);
    let both = ExportDemand::union([&a, &b]);
    let empty = ExportDemand::from_atoms([]);
    for (index, expected) in [0u16, 1, 2, 3, 4]
        .into_iter()
        .zip([&both, &a, &a, &b, &empty])
    {
        let function = verified
            .function(FunctionIndex::new(index))
            .expect("every function ordinal remains available");
        assert_eq!(function.demand(), expected.as_view());
        assert_eq!(function.demand().demand_set_id(), expected.demand_set_id());
        assert_eq!(
            function.demand().atom_set_payload(),
            expected.atom_set_payload()
        );
        assert_eq!(function.demand().reads(), !expected.is_empty());
        assert!(!function.demand().writes());
        assert!(function.body().params().is_empty());
        assert_eq!(function.body().ret(), RetShape::Unit);
        assert!(!function.body().is_mutating());
    }
    assert!(matches!(
        verified.functions()[0].instrs(),
        [
            SealedInstr::Call(1),
            SealedInstr::Call(3),
            SealedInstr::Return
        ]
    ));
    assert!(matches!(
        verified.functions()[1].instrs(),
        [SealedInstr::Call(2), SealedInstr::Return]
    ));
    for (function, site) in [(2, 0), (3, 1)] {
        assert!(matches!(
            verified.functions()[function].instrs(),
            [
                SealedInstr::ConstLoad(0),
                SealedInstr::DurExists(actual),
                SealedInstr::Pop,
                SealedInstr::Return
            ] if *actual == site
        ));
    }
    assert!(matches!(
        verified.functions()[4].instrs(),
        [SealedInstr::Return]
    ));
    let export = &verified.exports()[0];
    assert_eq!(export.function(), FunctionIndex::new(1));
    assert_eq!(export.id(), ExportId::of_local("", "entry"));
    assert_eq!(
        verified
            .function(export.function())
            .expect("verified export function")
            .demand(),
        a.as_view()
    );
    assert_eq!(export.demand_id(), a.demand_set_id());
    assert_eq!(export.reachable_sites(), &[0]);
    assert!(!export.is_mutating());
    let test = &verified.test_entries()[0];
    assert_eq!(test.func(), FunctionIndex::new(0));
    assert_eq!(test.name(), "f0");
    assert_eq!(test.kind(), TestKind::Driver);
    assert_eq!(
        verified
            .function(test.func())
            .expect("verified test function")
            .demand(),
        both.as_view()
    );
    assert_eq!(
        verified
            .function(test.func())
            .expect("verified test function")
            .demand()
            .demand_set_id(),
        both.demand_set_id()
    );
    assert_eq!(verified.demand_union(), a);
    assert_eq!(verified.test_demand_union(), both);
    assert_eq!(
        verified.demand_incidence(),
        vec![crate::NodeIncidence {
            path: a.atoms()[0].path().clone(),
            touched_by: vec![crate::AtomIncidence {
                export: export.id(),
                class: OperationClass::Presence,
            }],
        }],
    );

    // These references come from the verified image, never the expected sets.
    // Sharing complete equal sets is insufficient: `both` also contains atom b.
    fn atom<'image>(
        image: &'image VerifiedImage,
        function: u16,
        expected: &DemandAtom,
    ) -> &'image DemandAtom {
        image
            .function(FunctionIndex::new(function))
            .expect("verified function")
            .demand()
            .atoms()
            .find(|atom| *atom == expected)
            .expect("the semantic assertions established this atom")
    }
    let leaf_a = atom(&verified, 2, &a.atoms()[0]);
    let leaf_b = atom(&verified, 3, &b.atoms()[0]);
    assert!(std::ptr::eq(leaf_a, atom(&verified, 1, &a.atoms()[0])));
    assert!(std::ptr::eq(leaf_a, atom(&verified, 0, &a.atoms()[0])));
    assert!(std::ptr::eq(leaf_b, atom(&verified, 0, &b.atoms()[0])));
}

fn assert_refusal(bytes: &[u8], phase: VerifyPhase, detail: &str) {
    let refusal = crate::verify(bytes).expect_err("the artifact violates this phase");
    assert_eq!(refusal.phase(), phase);
    assert_eq!(refusal.code(), phase.code());
    assert_eq!(refusal.detail(), detail);
}

#[test]
fn self_and_disconnected_cycles_are_rejected() {
    for bodies in [
        vec![vec![Instr::Call(0), Instr::Return]],
        vec![
            vec![Instr::Return],
            vec![Instr::Call(2), Instr::Return],
            vec![Instr::Call(1), Instr::Return],
        ],
    ] {
        let bytes = image(|_, _| bodies, Some(0), None);
        assert_refusal(
            &bytes,
            VerifyPhase::Closure,
            "the call graph contains a cycle",
        );
    }
}

#[test]
fn an_unrelated_function_error_precedes_cycle_rejection() {
    let bytes = image(
        |_, _| {
            vec![
                vec![Instr::Call(0), Instr::Return],
                vec![Instr::Pop, Instr::Return],
            ]
        },
        Some(0),
        None,
    );
    let refusal = crate::verify(&bytes).expect_err("unrelated stack underflow");
    assert_eq!(refusal.phase(), VerifyPhase::Function);
    assert_eq!(refusal.code(), VerifyPhase::Function.code());
}

#[test]
fn cycle_rejection_precedes_transaction_flow() {
    for (first, phase, detail) in [
        (
            vec![Instr::Return],
            VerifyPhase::Flow,
            "a transaction marker sits outside its owning export",
        ),
        (
            vec![Instr::Call(0), Instr::Return],
            VerifyPhase::Closure,
            "the call graph contains a cycle",
        ),
    ] {
        let bytes = image(
            |_, _| {
                vec![
                    first,
                    vec![Instr::TxnBegin, Instr::TxnCommit, Instr::Return],
                ]
            },
            Some(0),
            None,
        );
        assert_refusal(&bytes, phase, detail);
    }
}

#[test]
fn a_transitive_presence_read_must_precede_commit() {
    for root in [
        vec![
            Instr::TxnBegin,
            Instr::Call(1),
            Instr::TxnCommit,
            Instr::Return,
        ],
        vec![
            Instr::TxnBegin,
            Instr::TxnCommit,
            Instr::Call(1),
            Instr::Return,
        ],
    ] {
        let after_commit = matches!(root[1], Instr::TxnCommit);
        let bytes = image(
            |key, entry| {
                vec![
                    root,
                    vec![Instr::Call(2), Instr::Return],
                    presence_read(key, entry),
                ]
            },
            Some(0),
            None,
        );
        if after_commit {
            assert_refusal(
                &bytes,
                VerifyPhase::Flow,
                "a durable operation follows the transaction's commit",
            );
        } else {
            let verified = crate::verify(&bytes).expect("read through a helper in the region");
            for index in 0..3 {
                let function = verified
                    .function(FunctionIndex::new(index))
                    .expect("helper chain function");
                assert_eq!(function.demand(), presence_demand().as_view());
                assert!(!function.body().is_mutating());
            }
            assert_eq!(verified.exports()[0].reachable_sites(), &[0]);
        }
    }
}

fn replace_terminal_call(bytes: &mut [u8], from: u16, to: u16) {
    let [from_hi, from_lo] = from.to_be_bytes();
    let [to_hi, to_lo] = to.to_be_bytes();
    let needle = [OP_CALL, from_hi, from_lo, OP_RETURN];
    assert_eq!(
        bytes
            .windows(needle.len())
            .filter(|window| *window == needle.as_slice())
            .count(),
        1,
    );
    image_forgery::forge(bytes, &needle, 0, &[OP_CALL, to_hi, to_lo, OP_RETURN]);
}

#[test]
fn a_call_into_a_test_entry_reaches_the_verifier_after_cycle_checking() {
    let mut bytes = image(
        |_, _| {
            vec![
                vec![Instr::Call(1), Instr::Return],
                vec![Instr::Return],
                vec![Instr::Return],
                vec![Instr::Call(4), Instr::Return],
                vec![Instr::Return],
            ]
        },
        Some(0),
        Some(2),
    );
    let verified = crate::verify(&bytes).expect("ordinary calls beside a test entry");
    assert_eq!(verified.test_entries().len(), 1);
    assert_eq!(verified.test_entries()[0].kind(), TestKind::Storeless);
    replace_terminal_call(&mut bytes, 1, 2);
    assert_refusal(
        &bytes,
        VerifyPhase::TestEntry,
        "a test entry may not be called",
    );
    replace_terminal_call(&mut bytes, 4, 3);
    assert_refusal(
        &bytes,
        VerifyPhase::Closure,
        "the call graph contains a cycle",
    );
}

#[test]
fn a_mixed_test_driver_is_rejected_by_the_verifier() {
    for direct in [false, true] {
        let mut bytes = image(
            |key, entry| {
                let test = if direct {
                    vec![
                        Instr::ConstLoad(key),
                        Instr::DurExists(entry.clone()),
                        Instr::Pop,
                        Instr::Call(1),
                        Instr::Return,
                    ]
                } else {
                    vec![Instr::Call(0), Instr::Return]
                };
                vec![
                    vec![
                        Instr::TxnBegin,
                        Instr::ConstLoad(key),
                        Instr::DurExists(entry.clone()),
                        Instr::Pop,
                        Instr::TxnCommit,
                        Instr::Return,
                    ],
                    vec![Instr::Return],
                    test,
                ]
            },
            Some(0),
            Some(2),
        );
        let verified = crate::verify(&bytes).expect("separate durable test and owner driver");
        assert_eq!(
            verified
                .function(verified.exports()[0].function())
                .expect("verified export function")
                .demand(),
            presence_demand().as_view()
        );
        let test = &verified.test_entries()[0];
        assert_eq!(
            verified
                .function(test.func())
                .expect("verified test function")
                .demand(),
            presence_demand().as_view()
        );
        assert_eq!(
            test.kind(),
            if direct {
                TestKind::DirectDurable
            } else {
                TestKind::Driver
            },
        );
        if direct {
            replace_terminal_call(&mut bytes, 1, 0);
            assert_refusal(
                &bytes,
                VerifyPhase::TestEntry,
                "a test body performs a direct durable operation and also drives a \
                 transaction-owning export",
            );
        }
    }
}
