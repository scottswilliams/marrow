use super::Entry;
use crate::{VerifiedImage, VerifyPhase};
use marrow_image::{
    ConstId, ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry,
};
use std::cell::Cell;
use std::panic::{catch_unwind, resume_unwind};

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    completed: usize,
    slots: usize,
    retained_frames: usize,
    local_capacity: usize,
    stack_capacity: usize,
}

thread_local! {
    static COUNTS: Cell<Option<Counts>> = const { Cell::new(None) };
}

pub(super) fn record_success(entry: &[Entry]) {
    COUNTS.with(|cell| {
        let Some(mut counts) = cell.get() else {
            return;
        };
        counts.completed += 1;
        counts.slots += entry.len();
        for state in entry {
            let Entry::Retained(frame) = state else {
                continue;
            };
            counts.retained_frames += 1;
            counts.local_capacity += frame.locals.capacity();
            counts.stack_capacity += frame.stack.capacity();
        }
        cell.set(Some(counts));
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
        Ok(result) => (result.expect("boolean tape verifies"), counts),
        Err(panic) => resume_unwind(panic),
    }
}

fn boolean_tape(width: u16, padding: usize, terminal: Instr) -> Vec<u8> {
    let mut owner = ImageDraft::new();
    let savepoint = owner.savepoint();
    let mut draft = owner.begin_transaction(savepoint).expect("fresh savepoint");
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

#[test]
fn ordinary_padding_retains_only_the_initial_type_frame() {
    // Complete every semantic check before asserting the ownership bound.
    let observations = [8u16, 32].map(|width| {
        [8, 32].map(|padding| {
            let bytes = boolean_tape(width, padding, Instr::Return);
            let (verified, counts) = observe(&bytes);
            assert_eq!(verified.functions().len(), 1);
            let function = &verified.functions()[0];
            assert_eq!(function.local_count(), width);
            assert_eq!(function.max_stack(), usize::from(width));
            assert_eq!(function.instrs().len(), 2 * usize::from(width) + padding);
            assert_eq!(function.span_at(0), Some((7, 3)));
            assert_eq!(function.span_at(function.instrs().len() - 1), Some((7, 3)));
            assert_eq!(counts.completed, 1);
            assert_eq!(counts.slots, function.instrs().len());
            eprintln!("type retention width={width} padding={padding}: {counts:?}");
            counts
        })
    });
    for (width, padded) in [8usize, 32].into_iter().zip(observations) {
        for counts in padded {
            assert_eq!(counts.retained_frames, 1);
            assert_eq!(counts.local_capacity, width);
            assert_eq!(counts.stack_capacity, 0);
        }
    }
}

#[test]
fn a_boolean_tape_that_falls_off_the_end_rejects() {
    let bytes = boolean_tape(32, 32, Instr::BoolNot);
    let refusal = crate::verify(&bytes).expect_err("missing terminal return");
    assert_eq!(refusal.phase(), VerifyPhase::Function);
    assert_eq!(refusal.code(), marrow_codes::Code::ImageFunction.as_str());
    assert_eq!(
        refusal.detail(),
        "execution falls off the end without returning"
    );
}

fn unit_image(local_count: u16, code: impl FnOnce(ConstId, ConstId) -> Vec<Instr>) -> Vec<u8> {
    let mut owner = ImageDraft::new();
    let savepoint = owner.savepoint();
    let mut draft = owner.begin_transaction(savepoint).expect("fresh savepoint");
    let name = draft.intern_string("inspect").expect("function name");
    let source = draft.intern_string("flow.mw").expect("source name");
    let int = draft.intern_int(1).expect("integer constant");
    let boolean = draft.intern_bool(true).expect("boolean constant");
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count,
            spans: vec![SpanEntry {
                instr_index: 0,
                line: 1,
                column: 1,
            }],
            code: code(int, boolean),
        })
        .expect("small unit function");
    draft.add_export(ExportId::of_local("", "inspect"), function);
    draft.encode().expect("unit image").bytes
}

fn function_refusal(bytes: &[u8], detail: &'static str) {
    let refusal = crate::verify(bytes).expect_err("invalid flow");
    assert_eq!(refusal.phase(), VerifyPhase::Function);
    assert_eq!(refusal.code(), marrow_codes::Code::ImageFunction.as_str());
    assert_eq!(refusal.detail(), detail);
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
            function_refusal(&bytes, "operand stack shapes disagree at a merge");
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
            function_refusal(&bytes, "local read before init");
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
            function_refusal(&bytes, "local read before init");
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
            function_refusal(&bytes, "operand stack shapes disagree at a merge");
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
                function_refusal(&bytes, "operand stack shapes disagree at a merge");
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
