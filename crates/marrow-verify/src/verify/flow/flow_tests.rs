use super::check_stack_depth;
use super::stack::{OperandStack, StackId};
use super::work::{FlowWork, take_flow_work};
use crate::vtype::VType;
use crate::{RejectionKind, VerifyPhase};
use marrow_image::bounds::{MAX_CODE_BYTES, MAX_STACK_DEPTH};
use marrow_image::{
    ConstId, ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry,
};

fn boolean_tape(width: u16, padding: usize, terminal: Instr) -> Vec<u8> {
    let mut owner = ImageDraft::new();
    let mut draft = owner.begin_transaction();
    let name = draft.intern_string("inspect").expect("function name");
    let source = draft.intern_string("flow.mw").expect("source name");
    let value = draft.intern_bool(true).expect("boolean constant");
    let mut code = vec![Instr::ConstLoad(value); usize::from(width)];
    code.extend(std::iter::repeat_n(Instr::BoolNot, padding));
    code.extend(std::iter::repeat_n(Instr::Pop, usize::from(width) - 1));
    code.push(terminal);
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Bool),
            local_count: width,
            spans: vec![SpanEntry {
                instr_index: 0,
                line: 7,
                column: 3,
            }],
            code,
        })
        .expect("small boolean tape");
    draft.add_export(ExportId::of_local("", "inspect"), function);
    draft.encode().expect("boolean image").bytes
}

/// Straight-line padding after a boolean tape changes only the instruction count:
/// the sealed locals, stack bound and spans are those of the unpadded tape.
#[test]
fn straight_line_padding_changes_only_the_instruction_count() {
    for width in [8u16, 32] {
        for padding in [8, 32] {
            let bytes = boolean_tape(width, padding, Instr::Return);
            let verified = crate::verify(&bytes).expect("boolean tape verifies");
            assert_eq!(verified.functions().len(), 1);
            let function = &verified.functions()[0];
            assert_eq!(function.local_count(), width);
            assert_eq!(function.max_stack(), usize::from(width));
            assert_eq!(function.instrs().len(), 2 * usize::from(width) + padding);
            assert_eq!(function.span_at(0), Some((7, 3)));
            assert_eq!(function.span_at(function.instrs().len() - 1), Some((7, 3)));
        }
    }
}

#[test]
fn a_boolean_tape_that_falls_off_the_end_rejects() {
    let bytes = boolean_tape(32, 32, Instr::BoolNot);
    let refusal = crate::verify(&bytes).expect_err("missing terminal return");
    assert_eq!(refusal.phase(), VerifyPhase::Function);
    assert_eq!(refusal.code(), marrow_codes::Code::ImageFunction);
    assert_eq!(refusal.kind(), &RejectionKind::FallsOffEnd);
}

/// The constants every flow fixture may load.
struct Consts {
    int: ConstId,
    boolean: ConstId,
    text: ConstId,
}

/// One encoded single-function image and the tape facts the work bounds are stated in.
struct Fixture {
    bytes: Vec<u8>,
    /// Instructions on the tape.
    instrs: usize,
    /// Entry zero plus every distinct jump target: an upper bound on retained boundaries.
    boundaries: usize,
}

fn flow_image(
    ret: ImageType,
    local_count: u16,
    code: impl FnOnce(&Consts) -> Vec<Instr>,
) -> Fixture {
    let mut owner = ImageDraft::new();
    let mut draft = owner.begin_transaction();
    let name = draft.intern_string("inspect").expect("function name");
    let source = draft.intern_string("flow.mw").expect("source name");
    let consts = Consts {
        int: draft.intern_int(1).expect("integer constant"),
        boolean: draft.intern_bool(true).expect("boolean constant"),
        text: draft.intern_text("unreachable").expect("text constant"),
    };
    let code = code(&consts);
    assert!(
        code_bytes(&code) <= MAX_CODE_BYTES,
        "fixture exceeds the code limit"
    );
    let mut targets: Vec<u32> = code
        .iter()
        .filter_map(|i| i.jump_target().copied())
        .collect();
    targets.push(0);
    targets.sort_unstable();
    targets.dedup();
    let instrs = code.len();
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret,
            local_count,
            spans: vec![SpanEntry {
                instr_index: 0,
                line: 1,
                column: 1,
            }],
            code,
        })
        .expect("flow fixture function");
    draft.add_export(ExportId::of_local("", "inspect"), function);
    Fixture {
        bytes: draft.encode().expect("flow fixture image").bytes,
        instrs,
        boundaries: targets.len(),
    }
}

fn unit_image(local_count: u16, code: impl FnOnce(ConstId, ConstId) -> Vec<Instr>) -> Vec<u8> {
    flow_image(ImageType::Unit, local_count, |c| code(c.int, c.boolean)).bytes
}

fn code_bytes(code: &[Instr]) -> usize {
    code.iter().map(Instr::encoded_len).sum()
}

/// Each fork lifts the stack bound by exactly one and adds three instructions, so
/// the sealed function is the same shape whatever the fork count.
#[test]
fn distinct_forks_seal_one_extra_stack_slot_and_three_instructions() {
    for (width, forks, instructions) in [(8u16, 1usize, 20), (32, 4, 77)] {
        let bytes = unit_image(width, |_, boolean| {
            let mut code = vec![Instr::ConstLoad(boolean); usize::from(width)];
            for _ in 0..forks {
                let target = u32::try_from(code.len() + 3).expect("small fork tape");
                code.extend([
                    Instr::ConstLoad(boolean),
                    Instr::JumpIfFalse(target),
                    Instr::BoolNot,
                ]);
            }
            code.extend(std::iter::repeat_n(Instr::Pop, usize::from(width)));
            code.push(Instr::Return);
            code
        });
        let verified = crate::verify(&bytes).expect("forked tape verifies");
        assert_eq!(verified.functions().len(), 1);
        let function = &verified.functions()[0];
        assert_eq!(function.local_count(), width);
        assert_eq!(function.max_stack(), usize::from(width) + 1);
        assert_eq!(function.instrs().len(), instructions);
        assert_eq!(function.span_at(0), Some((1, 1)));
        assert_eq!(function.span_at(instructions - 1), Some((1, 1)));
    }
}

fn function_refusal(bytes: &[u8], kind: RejectionKind) {
    let refusal = crate::verify(bytes).expect_err("invalid flow");
    assert_eq!(refusal.phase(), VerifyPhase::Function);
    assert_eq!(refusal.code(), marrow_codes::Code::ImageFunction);
    assert_eq!(refusal.kind(), &kind);
}

#[test]
fn a_diamond_requires_identical_operand_stacks() {
    for compatible in [true, false] {
        let bytes = unit_image(0, |int, boolean| {
            vec![
                Instr::ConstLoad(boolean),
                Instr::JumpIfFalse(4),
                Instr::ConstLoad(int),
                Instr::Jump(5),
                Instr::ConstLoad(if compatible { int } else { boolean }),
                Instr::Pop,
                Instr::Return,
            ]
        });
        if compatible {
            crate::verify(&bytes).expect("equal-stack diamond");
        } else {
            function_refusal(&bytes, RejectionKind::StackMerge);
        }
    }
}

#[test]
fn a_late_diamond_arm_rechecks_definite_initialization() {
    for initialized in [true, false] {
        let bytes = unit_image(1, |int, boolean| {
            vec![
                Instr::ConstLoad(boolean),
                Instr::JumpIfFalse(5),
                Instr::ConstLoad(int),
                Instr::LocalSet(0),
                Instr::Jump(7),
                Instr::ConstLoad(int),
                if initialized {
                    Instr::LocalSet(0)
                } else {
                    Instr::Pop
                },
                Instr::LocalGet(0),
                Instr::Pop,
                Instr::Return,
            ]
        });
        if initialized {
            crate::verify(&bytes).expect("both arms initialize the local");
        } else {
            function_refusal(&bytes, RejectionKind::LocalUninit);
        }
    }
}

#[test]
fn a_late_backedge_rechecks_definite_initialization() {
    for initialized in [true, false] {
        let bytes = unit_image(1, |int, boolean| {
            vec![
                Instr::ConstLoad(boolean),
                Instr::JumpIfFalse(11),
                Instr::ConstLoad(int),
                Instr::LocalSet(0),
                Instr::ConstLoad(boolean),
                Instr::Pop,
                Instr::LocalGet(0),
                Instr::Pop,
                Instr::ConstLoad(boolean),
                Instr::JumpIfFalse(14),
                Instr::Jump(4),
                Instr::ConstLoad(int),
                if initialized {
                    Instr::LocalSet(0)
                } else {
                    Instr::Pop
                },
                Instr::Jump(10),
                Instr::Return,
            ]
        });
        if initialized {
            crate::verify(&bytes).expect("backedge preserves initialization");
        } else {
            function_refusal(&bytes, RejectionKind::LocalUninit);
        }
    }
}

#[test]
fn an_entry_zero_backedge_meets_the_initial_stack() {
    for compatible in [true, false] {
        let bytes = unit_image(0, |int, boolean| {
            let mut code = vec![Instr::ConstLoad(boolean), Instr::JumpIfFalse(0)];
            if !compatible {
                code.push(Instr::ConstLoad(int));
            }
            code.push(Instr::Jump(0));
            code[1] = Instr::JumpIfFalse(code.len() as u32);
            code.push(Instr::Return);
            code
        });
        if compatible {
            crate::verify(&bytes).expect("empty-stack backedge to entry zero");
        } else {
            function_refusal(&bytes, RejectionKind::StackMerge);
        }
    }
}

#[test]
fn coincident_optional_and_checked_edges_still_merge_both_stacks() {
    for optional in [true, false] {
        for coincident in [false, true] {
            let bytes = unit_image(0, |int, _| {
                let mut code = vec![Instr::ConstLoad(int)];
                if optional {
                    code.push(Instr::SomeWrap);
                }
                let target = code.len() as u32 + if coincident { 1 } else { 2 };
                code.push(if optional {
                    Instr::BranchPresent(target)
                } else {
                    Instr::IntNegChecked(target)
                });
                code.extend([Instr::Pop, Instr::Return]);
                code
            });
            if coincident {
                function_refusal(&bytes, RejectionKind::StackMerge);
            } else {
                assert_eq!(
                    crate::verify(&bytes).expect("distinct edges").functions()[0].max_stack(),
                    1
                );
            }
        }
    }
}

#[test]
fn optional_and_checked_success_edges_preserve_the_stack_depth_limit() {
    for optional in [true, false] {
        let bytes = unit_image(0, |int, _| {
            let mut code = vec![Instr::ConstLoad(int); 256];
            if optional {
                code.push(Instr::SomeWrap);
            }
            let target = code.len() as u32 + 2;
            code.push(if optional {
                Instr::BranchPresent(target)
            } else {
                Instr::IntNegChecked(target)
            });
            code.extend(std::iter::repeat_n(Instr::Pop, 256));
            code.push(Instr::Return);
            code
        });
        let image = crate::verify(&bytes).expect("both edges clean up their own stack");
        assert_eq!(image.functions()[0].max_stack(), 256);
    }
}

/// Verify `bytes` and return the flow work that one call performed.
fn verify_counted(
    bytes: &[u8],
) -> (
    Result<crate::VerifiedImage, crate::VerifyRejection>,
    FlowWork,
) {
    take_flow_work();
    let verdict = crate::verify(bytes);
    (verdict, take_flow_work())
}

/// A boundary is queued on first reach and then once per local slot that weakens, so a
/// function runs at most `(local_count + 1) x boundaries` regions.
fn assert_run_bound(work: &FlowWork, fixture: &Fixture, local_count: u16) {
    let bound = (usize::from(local_count) + 1) * fixture.boundaries;
    assert!(
        work.runs <= bound,
        "{} region runs exceed {bound}",
        work.runs
    );
}

/// Append a chain of `links` retained boundaries, each a bare `Jump`, rounded down to
/// whole triples. Each triple at `x` runs `x -> x+2 -> x+1 -> x+3`, so no link falls
/// through and every target is a retained boundary. The chain exits to the
/// instruction appended after it.
fn push_jump_chain(code: &mut Vec<Instr>, links: usize) {
    for _ in 0..links / 3 {
        let x = u32::try_from(code.len()).expect("small tape");
        code.extend([Instr::Jump(x + 2), Instr::Jump(x + 3), Instr::Jump(x + 1)]);
    }
}

/// Append a weakening ladder over locals `0..steps`: one region initializes every slot
/// in turn and, before each write, branches to a pad that jumps to the head `h`; it then
/// jumps to `h` itself with every slot set. Running the pads in turn weakens `h` one
/// slot at a time, so every region reachable from `h` re-runs `steps` more times. `h` is
/// the instruction appended after the ladder.
fn push_weakening_ladder(code: &mut Vec<Instr>, steps: u16, consts: &Consts) {
    let mut branches = Vec::with_capacity(usize::from(steps));
    for slot in 0..steps {
        code.push(Instr::ConstLoad(consts.boolean));
        branches.push(code.len());
        code.push(Instr::JumpIfFalse(0));
        code.extend([Instr::ConstLoad(consts.int), Instr::LocalSet(slot)]);
    }
    let pads = code.len() + 1;
    let head = u32::try_from(pads + usize::from(steps)).expect("small tape");
    code.push(Instr::Jump(head));
    for (step, at) in branches.into_iter().enumerate() {
        code[at] = Instr::JumpIfFalse(u32::try_from(pads + step).expect("small tape"));
        code.push(Instr::Jump(head));
    }
}

fn ladder_bytes(steps: u16) -> usize {
    usize::from(steps) * (3 + 5 + 3 + 3 + 5) + 5
}

/// Initialize local 0 to the optional of `value`: parameters are never optional, so an
/// optional operand comes from a local, as the lowering of `var o = src.f` leaves it.
fn optional_local(value: ConstId) -> Vec<Instr> {
    vec![Instr::ConstLoad(value), Instr::SomeWrap, Instr::LocalSet(0)]
}

const OPTIONAL_LOCAL_BYTES: usize = 3 + 1 + 3;
const JUMP_BYTES: usize = 5;
const MARKER_BYTES: usize = 3;
const LADDER_STEPS: u16 = 64;

#[test]
fn a_retained_jump_chain_under_a_deep_stack_costs_cells_linear_in_code() {
    let depth = MAX_STACK_DEPTH.min(MAX_CODE_BYTES / 2 / 3);
    let fixture = flow_image(ImageType::Unit, 1, |c| {
        let mut code = vec![Instr::ConstLoad(c.int); depth];
        let links = (MAX_CODE_BYTES - code_bytes(&code) - MARKER_BYTES) / JUMP_BYTES;
        push_jump_chain(&mut code, links);
        code.push(Instr::Unreachable(c.text));
        code
    });
    let (verdict, work) = verify_counted(&fixture.bytes);
    verdict.expect("a deep retained chain verifies");
    assert!(
        work.stack_cells <= fixture.instrs,
        "{} stack cells for {} instructions",
        work.stack_cells,
        fixture.instrs
    );
}

#[test]
fn a_weakening_ladder_over_a_retained_chain_costs_cells_linear_in_code() {
    let chain = 1024;
    let fixed = ladder_bytes(LADDER_STEPS) + chain * JUMP_BYTES + MARKER_BYTES;
    let depth = (MAX_STACK_DEPTH - 1).min((MAX_CODE_BYTES - fixed) / 3);
    let fixture = flow_image(ImageType::Unit, LADDER_STEPS, |c| {
        let mut code = vec![Instr::ConstLoad(c.int); depth];
        push_weakening_ladder(&mut code, LADDER_STEPS, c);
        push_jump_chain(&mut code, chain);
        code.push(Instr::Unreachable(c.text));
        code
    });
    let (verdict, work) = verify_counted(&fixture.bytes);
    verdict.expect("a weakening ladder verifies");
    assert!(
        work.stack_cells <= fixture.instrs,
        "{} stack cells for {} instructions",
        work.stack_cells,
        fixture.instrs
    );
    assert_run_bound(&work, &fixture, LADDER_STEPS);
    assert_eq!(work.queued_while_pending, 0);
}

/// The exact `left ?? right` lowering over an `int?` local, repeated with every result
/// left on the stack: each operand leaves two retained boundaries at the in-flight depth.
#[test]
fn coalescing_operands_cost_cells_linear_in_their_count() {
    let per_operand = 3 + 5 + 5 + 3 + 1;
    let operands = MAX_STACK_DEPTH.min((MAX_CODE_BYTES - OPTIONAL_LOCAL_BYTES - 1) / per_operand);
    let fixture = flow_image(ImageType::Unit, 1, |c| {
        let mut code = optional_local(c.int);
        for _ in 0..operands {
            let at = u32::try_from(code.len()).expect("small tape");
            code.extend([
                Instr::LocalGet(0),
                Instr::BranchPresent(at + 3),
                Instr::Jump(at + 4),
                Instr::ConstLoad(c.int),
            ]);
        }
        code.extend(std::iter::repeat_n(Instr::Pop, operands));
        code.push(Instr::Return);
        code
    });
    let (verdict, work) = verify_counted(&fixture.bytes);
    let image = verdict.expect("coalescing operands verify");
    assert_eq!(image.functions()[0].max_stack(), operands);
    assert!(
        work.stack_cells <= fixture.instrs,
        "{} stack cells for {} instructions",
        work.stack_cells,
        fixture.instrs
    );
}

#[test]
fn a_frozen_prefix_counts_toward_the_sealed_stack_depth() {
    let bytes = unit_image(0, |int, boolean| {
        let mut code = vec![Instr::ConstLoad(int); 200];
        code.push(Instr::ConstLoad(boolean));
        let join = u32::try_from(code.len() + 1).expect("small tape");
        code.push(Instr::JumpIfFalse(join));
        code.extend(std::iter::repeat_n(Instr::ConstLoad(int), 50));
        code.extend(std::iter::repeat_n(Instr::Pop, 250));
        code.push(Instr::Return);
        code
    });
    let image = crate::verify(&bytes).expect("a resumed deep stack verifies");
    assert_eq!(image.functions()[0].max_stack(), 250);
}

#[test]
fn a_boundary_is_never_queued_while_pending() {
    // One region branches to 5 twice with the same state, then jumps on.
    let repeated_edge = flow_image(ImageType::Unit, 0, |c| {
        vec![
            Instr::ConstLoad(c.boolean),
            Instr::JumpIfFalse(5),
            Instr::ConstLoad(c.boolean),
            Instr::JumpIfFalse(5),
            Instr::Jump(6),
            Instr::Return,
            Instr::Return,
        ]
    });
    // A loop whose arms write slot 0 as `int` and as `bool` before meeting at 9.
    let retyping_loop = flow_image(ImageType::Unit, 1, |c| {
        vec![
            Instr::ConstLoad(c.boolean),
            Instr::JumpIfFalse(10),
            Instr::ConstLoad(c.boolean),
            Instr::JumpIfFalse(7),
            Instr::ConstLoad(c.int),
            Instr::LocalSet(0),
            Instr::Jump(9),
            Instr::ConstLoad(c.boolean),
            Instr::LocalSet(0),
            Instr::Jump(0),
            Instr::Return,
        ]
    });
    for (fixture, local_count) in [(repeated_edge, 0), (retyping_loop, 1)] {
        let (verdict, work) = verify_counted(&fixture.bytes);
        verdict.expect("the fixture verifies");
        assert_eq!(work.queued_while_pending, 0);
        assert_run_bound(&work, &fixture, local_count);
    }
}

/// Region 0 queues 3 and then 5; the latest queued boundary runs first, so the empty
/// `Pop` at 5 rejects before the uninitialized read at 3 is reached.
#[test]
fn the_latest_queued_boundary_is_checked_first() {
    let bytes = unit_image(1, |_, boolean| {
        vec![
            Instr::ConstLoad(boolean),
            Instr::JumpIfFalse(3),
            Instr::Jump(5),
            Instr::LocalGet(0),
            Instr::Return,
            Instr::Pop,
            Instr::Return,
        ]
    });
    function_refusal(&bytes, RejectionKind::StackUnderflow);
}

#[test]
fn a_region_ending_in_a_marker_leaves_no_cells_for_the_next_region() {
    let untyped_pop = flow_image(ImageType::Unit, 0, |c| {
        vec![
            Instr::ConstLoad(c.boolean),
            Instr::JumpIfFalse(4),
            Instr::ConstLoad(c.int),
            Instr::Unreachable(c.text),
            Instr::Pop,
            Instr::Return,
        ]
    });
    let typed_pop = flow_image(ImageType::Unit, 0, |c| {
        vec![
            Instr::ConstLoad(c.boolean),
            Instr::JumpIfFalse(4),
            Instr::ConstLoad(c.int),
            Instr::Todo(c.text),
            Instr::IntNeg,
            Instr::Pop,
            Instr::Return,
        ]
    });
    for fixture in [untyped_pop, typed_pop] {
        function_refusal(&fixture.bytes, RejectionKind::StackUnderflow);
    }
}

#[test]
fn a_diamond_whose_arms_differ_below_the_top_rejects() {
    let bytes = unit_image(0, |int, boolean| {
        vec![
            Instr::ConstLoad(boolean),
            Instr::JumpIfFalse(5),
            Instr::ConstLoad(int),
            Instr::ConstLoad(boolean),
            Instr::Jump(7),
            Instr::ConstLoad(boolean),
            Instr::ConstLoad(boolean),
            Instr::Pop,
            Instr::Pop,
            Instr::Return,
        ]
    });
    function_refusal(&bytes, RejectionKind::StackMerge);
}

/// Every weakening of the ladder head re-runs 150 boundaries that each keep one more
/// cell. The re-runs push the cells their first run pushed, so they mint no new nodes.
#[test]
fn rerunning_cell_keeping_links_refinds_their_nodes() {
    let fixture = flow_image(ImageType::Unit, LADDER_STEPS, |c| {
        let mut code = Vec::new();
        push_weakening_ladder(&mut code, LADDER_STEPS, c);
        // Groups of three links laid out A, C, B so that no link falls through.
        for _ in 0..50 {
            let x = u32::try_from(code.len()).expect("small tape");
            code.extend([
                Instr::ConstLoad(c.int),
                Instr::Jump(x + 4),
                Instr::ConstLoad(c.int),
                Instr::Jump(x + 6),
                Instr::ConstLoad(c.int),
                Instr::Jump(x + 2),
            ]);
        }
        code.push(Instr::Unreachable(c.text));
        code
    });
    let (verdict, work) = verify_counted(&fixture.bytes);
    verdict.expect("cell-keeping links verify");
    assert!(
        work.nodes_minted <= fixture.instrs,
        "{} nodes for {} instructions",
        work.nodes_minted,
        fixture.instrs
    );
    assert!(
        work.stack_cells > fixture.instrs,
        "the ladder must re-run the links: {} cells",
        work.stack_cells
    );
    let bound = (usize::from(LADDER_STEPS) + 1) * fixture.instrs;
    assert!(
        work.stack_cells <= bound,
        "{} stack cells exceed {bound}",
        work.stack_cells
    );
    assert_eq!(work.queued_while_pending, 0);
}

/// A coincident branch retains `[int, int]`, so the return resumes with both cells in
/// the retained prefix: it pops one and must still see the other.
#[test]
fn a_return_sees_cells_in_the_retained_prefix() {
    let fixture = flow_image(ImageType::scalar(Scalar::Int), 0, |c| {
        vec![
            Instr::ConstLoad(c.int),
            Instr::ConstLoad(c.int),
            Instr::ConstLoad(c.boolean),
            Instr::JumpIfFalse(4),
            Instr::Return,
        ]
    });
    function_refusal(&fixture.bytes, RejectionKind::ReturnStack);
}

/// The `N(x ?? 0)` lowering: the `??` join is a boundary, so the guarded value is the
/// first cell of the resumed prefix.
#[test]
fn a_range_guard_reads_a_joined_operand_from_the_retained_prefix() {
    for scalar in [Scalar::Int, Scalar::Bool] {
        let fixture = flow_image(ImageType::scalar(scalar), 1, |c| {
            let value = if scalar == Scalar::Int {
                c.int
            } else {
                c.boolean
            };
            let mut code = optional_local(value);
            code.extend([
                Instr::LocalGet(0),
                Instr::BranchPresent(6),
                Instr::Jump(7),
                Instr::ConstLoad(value),
                Instr::RangeGuard { lo: 0, hi: 150 },
                Instr::Return,
            ]);
            code
        });
        if scalar == Scalar::Int {
            crate::verify(&fixture.bytes).expect("a guarded joined int verifies");
        } else {
            function_refusal(
                &fixture.bytes,
                RejectionKind::OperandType(crate::Operand::Scalar(Scalar::Int)),
            );
        }
    }
}

/// One step of a unit-test stack program.
#[derive(Clone, Copy)]
enum Step {
    Push(VType),
    Pop,
    Freeze,
}

/// Run `steps` from the empty stack and return the handle of the final stack.
fn stack_handle(stack: &mut OperandStack, steps: &[Step]) -> StackId {
    stack.resume(StackId::EMPTY);
    for step in steps {
        match step {
            Step::Push(cell) => stack.push(*cell),
            Step::Pop => {
                stack.pop().expect("a pushed cell");
            }
            Step::Freeze => {
                stack.freeze();
            }
        }
    }
    stack.freeze()
}

#[test]
fn equal_stacks_reached_along_different_paths_share_one_handle() {
    use Step::{Freeze, Pop, Push};
    let int = VType::bare_scalar(Scalar::Int);
    let boolean = VType::bare_scalar(Scalar::Bool);
    let mut stack = OperandStack::new();
    let int_bool = stack_handle(&mut stack, &[Push(int), Freeze, Push(boolean)]);
    // Pops a frozen cell, so the prefix steps back to its parent before the push.
    let int_bool_again = stack_handle(
        &mut stack,
        &[Push(int), Push(int), Freeze, Pop, Push(boolean)],
    );
    let int_int = stack_handle(&mut stack, &[Push(int), Push(int)]);
    let bool_only = stack_handle(&mut stack, &[Push(boolean)]);
    let bool_bool = stack_handle(&mut stack, &[Push(boolean), Freeze, Push(boolean)]);
    assert_eq!(int_bool, int_bool_again);
    let distinct = [int_bool, int_int, bool_only, bool_bool];
    for (i, left) in distinct.iter().enumerate() {
        for right in &distinct[i + 1..] {
            assert_ne!(left, right);
        }
    }
}

#[test]
fn the_stack_depth_check_counts_the_frozen_prefix() {
    let mut stack = OperandStack::new();
    let mut max_stack = 0;
    stack.push(VType::bare_scalar(Scalar::Int));
    stack.freeze();
    for _ in 1..MAX_STACK_DEPTH {
        stack.push(VType::bare_scalar(Scalar::Int));
    }
    check_stack_depth(&stack, &mut max_stack).expect("the bound itself is admitted");
    assert_eq!(max_stack, MAX_STACK_DEPTH);
    stack.push(VType::bare_scalar(Scalar::Int));
    let refusal = check_stack_depth(&stack, &mut max_stack).expect_err("one past the bound");
    assert_eq!(
        refusal.kind(),
        &RejectionKind::OverBound(crate::Bound::StackDepth)
    );
}
