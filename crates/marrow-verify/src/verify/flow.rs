//! Phase 4+ type-flow checking: the abstract stack machine and durable-op predicates.

use super::context::Ctx;
use super::decode_code::Decoded;
use super::model::DecodedFunction;
use super::presence::push_on_fallthrough;
use super::reject;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{
    RetShape, SealedBranch, SealedCollectionType, SealedConst, SealedField, SealedIndexComponent,
    SealedInstr, SealedRoot, SealedSite, SealedSiteTarget,
};
use crate::vtype::VType;
use marrow_image::{ImageType, OperationClass, Scalar};

/// The control transfer an instruction performs, in tape indices. Successor
/// indices are derived from this by `check_flow`.
enum Control {
    /// Continue to the next instruction.
    Fallthrough,
    /// End the frame (no successor).
    Return,
    /// Unconditional transfer to one tape index.
    Jump(usize),
    /// Conditional: the branch target plus fallthrough (identical stack on both).
    Branch(usize),
    /// Optional-presence branch: the absent edge (`target`) carries the current
    /// stack; the present edge (fallthrough) additionally carries the unwrapped
    /// bare value.
    BranchPresent { target: usize, present: VType },
    /// Checked-arithmetic branch: the operands are already popped. The fault edge
    /// (`target`) carries the current stack; the success edge (fallthrough)
    /// additionally carries the operation's `result`.
    CheckedResult { target: usize, result: VType },
}

/// The abstract machine state at a program point: the typed operand stack and the
/// definite-init/type state of each local slot.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Frame {
    pub(super) stack: Vec<VType>,
    /// Per-slot type when definitely initialized on every path reaching this point,
    /// else `None`. Reading an uninitialized slot rejects.
    pub(super) locals: Vec<Option<VType>>,
}

/// Phase-3 structural, type, and local-init checks via a CFG worklist over the
/// typed operand stack and locals. Returns the sealed instruction tape and the true
/// max stack depth (computed here, never read from the image).
pub(super) fn check_flow(
    function: &DecodedFunction,
    ctx: &Ctx,
    code: &[Decoded],
    consts: &[SealedConst],
) -> Result<(Vec<SealedInstr>, usize), VerifyRejection> {
    if code.is_empty() {
        return Err(reject(VerifyPhase::Function, "function has no code"));
    }

    // Params occupy locals `0..param_count`, pre-initialized to their param type;
    // the rest start uninitialized. The entry operand stack is empty.
    let mut initial_locals: Vec<Option<VType>> = vec![None; function.local_count as usize];
    for (slot, param) in function.params.iter().enumerate() {
        initial_locals[slot] =
            Some(VType::from_image(*param).expect("a parameter type is never unit"));
    }
    let mut entry: Vec<Option<Frame>> = vec![None; code.len()];
    entry[0] = Some(Frame {
        stack: Vec::new(),
        locals: initial_locals,
    });
    let mut max_stack = 0usize;
    let mut worklist = vec![0usize];

    while let Some(index) = worklist.pop() {
        let mut frame = entry[index]
            .clone()
            .expect("worklist only enqueues reached instructions");
        let control = apply(function, ctx, &code[index].instr, consts, &mut frame)?;
        if frame.stack.len() > marrow_image::bounds::MAX_STACK_DEPTH {
            return Err(reject(
                VerifyPhase::Function,
                "operand stack exceeds depth bound",
            ));
        }
        max_stack = max_stack.max(frame.stack.len());
        // Each successor edge carries a frame; `BranchPresent` differs between edges.
        let edges: Vec<(usize, Frame)> = match control {
            Control::Return => Vec::new(),
            Control::Fallthrough => vec![(index + 1, frame.clone())],
            Control::Jump(target) => vec![(target, frame.clone())],
            Control::Branch(target) => vec![(target, frame.clone()), (index + 1, frame.clone())],
            // Both carry the current stack on the `target` edge and push one value on
            // the fallthrough edge; only which edge is the "taken" one differs in
            // meaning (present vs fault), not in the CFG edge shapes.
            Control::BranchPresent { target, present } => {
                push_on_fallthrough(&frame, target, index, present, &mut max_stack)?
            }
            Control::CheckedResult { target, result } => {
                push_on_fallthrough(&frame, target, index, result, &mut max_stack)?
            }
        };
        for (successor, edge_frame) in edges {
            if successor >= code.len() {
                return Err(reject(
                    VerifyPhase::Function,
                    "execution falls off the end without returning",
                ));
            }
            propagate(&mut entry, &mut worklist, successor, &edge_frame)?;
        }
    }

    if entry.iter().any(Option::is_none) {
        return Err(reject(VerifyPhase::Function, "unreachable instruction"));
    }

    let instrs = code.iter().map(|decoded| decoded.instr.clone()).collect();
    Ok((instrs, max_stack))
}

/// Merge `frame` into the entry state of `successor`, enqueueing it when its state
/// changes. Stacks must agree exactly; locals meet per slot (init on both paths
/// with the same type stays init, otherwise the slot becomes uninit).
fn propagate(
    entry: &mut [Option<Frame>],
    worklist: &mut Vec<usize>,
    successor: usize,
    frame: &Frame,
) -> Result<(), VerifyRejection> {
    match &entry[successor] {
        None => {
            entry[successor] = Some(frame.clone());
            worklist.push(successor);
            Ok(())
        }
        Some(existing) => {
            if existing.stack != frame.stack {
                return Err(reject(
                    VerifyPhase::Function,
                    "operand stack shapes disagree at a merge",
                ));
            }
            let mut merged = existing.locals.clone();
            for (slot, cell) in merged.iter_mut().enumerate() {
                let incoming = frame.locals[slot];
                *cell = match (*cell, incoming) {
                    (Some(a), Some(b)) if a == b => Some(a),
                    _ => None,
                };
            }
            if merged != existing.locals {
                entry[successor] = Some(Frame {
                    stack: existing.stack.clone(),
                    locals: merged,
                });
                worklist.push(successor);
            }
            Ok(())
        }
    }
}

/// Apply one instruction to the abstract frame and return its control transfer.
/// Grows one slice at a time with the opcode set.
fn apply(
    function: &DecodedFunction,
    ctx: &Ctx,
    instr: &SealedInstr,
    consts: &[SealedConst],
    frame: &mut Frame,
) -> Result<Control, VerifyRejection> {
    if is_durable(instr) {
        return apply_durable(ctx, instr, frame);
    }
    match instr {
        SealedInstr::ConstLoad(idx) => const_load(frame, consts, *idx),
        SealedInstr::LocalGet(slot) => local_get(frame, *slot),
        SealedInstr::LocalSet(slot) => local_set(frame, *slot),
        SealedInstr::Pop => discard(frame),
        SealedInstr::Return => check_return(function, frame),
        SealedInstr::Unreachable(idx) => unreachable_op(consts, *idx),
        SealedInstr::Todo(idx) => todo_op(consts, *idx),
        SealedInstr::Jump(target) => Ok(Control::Jump(*target)),
        SealedInstr::JumpIfFalse(target) => jump_if_false(frame, *target),
        SealedInstr::Assert => assert_op(frame),
        SealedInstr::Call(target) => call(ctx, frame, *target),
        SealedInstr::IntAdd
        | SealedInstr::IntSub
        | SealedInstr::IntMul
        | SealedInstr::IntRem
        | SealedInstr::IntDiv => binary(&mut frame.stack, Scalar::Int, Scalar::Int),
        SealedInstr::IntNeg => int_neg(frame),
        SealedInstr::IntAddChecked(target)
        | SealedInstr::IntSubChecked(target)
        | SealedInstr::IntMulChecked(target)
        | SealedInstr::IntDivChecked(target)
        | SealedInstr::IntRemChecked(target) => int_binary_checked(frame, *target),
        SealedInstr::IntNegChecked(target) => int_neg_checked(frame, *target),
        SealedInstr::RangeGuard { .. } => range_guard(frame),
        SealedInstr::BoolNot => bool_not(frame),
        SealedInstr::IntLt | SealedInstr::IntLe | SealedInstr::IntGt | SealedInstr::IntGe => {
            binary(&mut frame.stack, Scalar::Int, Scalar::Bool)
        }
        SealedInstr::EqInt => binary(&mut frame.stack, Scalar::Int, Scalar::Bool),
        SealedInstr::EqBool => binary(&mut frame.stack, Scalar::Bool, Scalar::Bool),
        SealedInstr::EqText => binary(&mut frame.stack, Scalar::Text, Scalar::Bool),
        SealedInstr::TextConcat => binary(&mut frame.stack, Scalar::Text, Scalar::Text),
        SealedInstr::TextLt | SealedInstr::TextLe | SealedInstr::TextGt | SealedInstr::TextGe => {
            binary(&mut frame.stack, Scalar::Text, Scalar::Bool)
        }
        SealedInstr::EqBytes
        | SealedInstr::BytesLt
        | SealedInstr::BytesLe
        | SealedInstr::BytesGt
        | SealedInstr::BytesGe => binary(&mut frame.stack, Scalar::Bytes, Scalar::Bool),
        SealedInstr::EqDate
        | SealedInstr::DateLt
        | SealedInstr::DateLe
        | SealedInstr::DateGt
        | SealedInstr::DateGe => binary(&mut frame.stack, Scalar::Date, Scalar::Bool),
        SealedInstr::EqInstant
        | SealedInstr::InstantLt
        | SealedInstr::InstantLe
        | SealedInstr::InstantGt
        | SealedInstr::InstantGe => binary(&mut frame.stack, Scalar::Instant, Scalar::Bool),
        SealedInstr::EqDuration
        | SealedInstr::DurationLt
        | SealedInstr::DurationLe
        | SealedInstr::DurationGt
        | SealedInstr::DurationGe => binary(&mut frame.stack, Scalar::Duration, Scalar::Bool),
        SealedInstr::DateAddDays => date_add_days(frame),
        SealedInstr::DateDaysBetween => binary(&mut frame.stack, Scalar::Date, Scalar::Int),
        SealedInstr::DurationAdd | SealedInstr::DurationSub => {
            binary(&mut frame.stack, Scalar::Duration, Scalar::Duration)
        }
        SealedInstr::InstantAddDuration | SealedInstr::InstantSubDuration => {
            instant_shift_duration(frame)
        }
        SealedInstr::ConvString => conv_string(frame),
        SealedInstr::ConvBytesText => conv_bytes_text(frame),
        SealedInstr::TextIsEmpty => text_is_empty(frame),
        SealedInstr::TextContains => text_contains(frame),
        SealedInstr::TextTrim => text_trim(frame),
        SealedInstr::TextSplit(idx) => text_split(ctx, frame, *idx),
        SealedInstr::TextLines(idx) => text_lines(ctx, frame, *idx),
        SealedInstr::TextJoin => text_join(ctx, frame),
        SealedInstr::RecordNew(ty) => record_new(ctx, frame, *ty),
        SealedInstr::FieldGet(field) => field_get(ctx, frame, *field),
        SealedInstr::FieldSet(field) => field_set(ctx, frame, *field),
        SealedInstr::FieldUnset(field) => field_unset(ctx, frame, *field),
        SealedInstr::SomeWrap => some_wrap(frame),
        SealedInstr::VacantLoad(ty) => vacant_load(ctx, frame, ty),
        SealedInstr::BranchPresent(target) => branch_present(frame, *target),
        SealedInstr::EnumConstruct { enum_idx, variant } => {
            enum_construct(ctx, frame, *enum_idx, *variant)
        }
        SealedInstr::EnumTag => enum_tag(frame),
        SealedInstr::EnumPayloadGet { variant, field } => {
            enum_payload_get(ctx, frame, *variant, *field)
        }
        SealedInstr::EqEnum => eq_enum(frame),
        SealedInstr::EqId => eq_id(frame),
        SealedInstr::MakeIdentity { root, cols } => make_identity(ctx, frame, *root, *cols),
        SealedInstr::IdentityKeyPath(cols) => identity_key_path(ctx, frame, *cols),
        SealedInstr::ListNew(idx) => list_new(ctx, frame, *idx),
        SealedInstr::ListAppend => list_append(ctx, frame),
        SealedInstr::ListLen => list_len(ctx, frame),
        SealedInstr::ListGet => list_get(ctx, frame),
        SealedInstr::ListIndex => list_index(ctx, frame),
        SealedInstr::MapNew(idx) => map_new(ctx, frame, *idx),
        SealedInstr::MapInsert => map_insert(ctx, frame),
        SealedInstr::MapRemove => map_remove(ctx, frame),
        SealedInstr::MapGet => map_get(ctx, frame),
        SealedInstr::MapLen => map_len(ctx, frame),
        SealedInstr::MapKeyAt => map_key_at(ctx, frame),
        SealedInstr::MapValueAt => map_value_at(ctx, frame),
        SealedInstr::DurExists(_)
        | SealedInstr::DurFamilyExists(_)
        | SealedInstr::DurReadField(_)
        | SealedInstr::DurReadEntry(_)
        | SealedInstr::DurSetRequired(_)
        | SealedInstr::DurSetSparse(_)
        | SealedInstr::DurSetSparsePresent { .. }
        | SealedInstr::DurCreateEntry(_)
        | SealedInstr::DurReplaceEntry(_)
        | SealedInstr::DurEraseField(_)
        | SealedInstr::DurEraseEntry(_)
        | SealedInstr::DurReadGroup(_)
        | SealedInstr::DurReplaceGroup(_)
        | SealedInstr::DurEraseGroup(_)
        | SealedInstr::DurIterateBounded { .. }
        | SealedInstr::DurIndexScan { .. }
        | SealedInstr::DurIndexLookup(_)
        | SealedInstr::DurIndexExists(_)
        | SealedInstr::TxnBegin
        | SealedInstr::TxnCommit => {
            unreachable!("a durable opcode or transaction marker returns from apply_durable")
        }
    }
}

fn const_load(
    frame: &mut Frame,
    consts: &[SealedConst],
    idx: u16,
) -> Result<Control, VerifyRejection> {
    let value = consts
        .get(idx as usize)
        .ok_or(reject(VerifyPhase::Function, "const index out of range"))?;
    frame.stack.push(VType::bare_scalar(const_scalar(value)));
    Ok(Control::Fallthrough)
}

fn local_get(frame: &mut Frame, slot: u16) -> Result<Control, VerifyRejection> {
    let ty = frame
        .locals
        .get(slot as usize)
        .ok_or(reject(VerifyPhase::Function, "local index out of range"))?
        .ok_or(reject(VerifyPhase::Function, "local read before init"))?;
    frame.stack.push(ty);
    Ok(Control::Fallthrough)
}

fn local_set(frame: &mut Frame, slot: u16) -> Result<Control, VerifyRejection> {
    let value = pop(&mut frame.stack)?;
    let cell = frame
        .locals
        .get_mut(slot as usize)
        .ok_or(reject(VerifyPhase::Function, "local index out of range"))?;
    match cell {
        Some(existing) if *existing != value => {
            return Err(reject(
                VerifyPhase::Function,
                "local slot reused at a different type",
            ));
        }
        _ => *cell = Some(value),
    }
    Ok(Control::Fallthrough)
}

fn discard(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    pop(&mut frame.stack)?;
    Ok(Control::Fallthrough)
}

fn check_return(function: &DecodedFunction, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    match (frame.stack.pop(), function.ret) {
        (Some(top), ret) if top.matches_ret(ret) => {}
        (None, RetShape::Unit) => {}
        _ => {
            return Err(reject(
                VerifyPhase::Function,
                "return stack shape does not match the return type",
            ));
        }
    }
    if !frame.stack.is_empty() {
        return Err(reject(
            VerifyPhase::Function,
            "operand stack not empty at return",
        ));
    }
    Ok(Control::Return)
}

/// The operand is the static invariant text; it must be a text const. The instruction
/// never falls through, so it ends the frame like `Return` without a return-value
/// check — it always faults.
fn unreachable_op(consts: &[SealedConst], idx: u16) -> Result<Control, VerifyRejection> {
    match consts.get(idx as usize) {
        Some(SealedConst::Text(_)) => Ok(Control::Return),
        Some(_) => Err(reject(
            VerifyPhase::Function,
            "unreachable operand must be a text const",
        )),
        None => Err(reject(VerifyPhase::Function, "const index out of range")),
    }
}

/// The operand is the static deferral text; like `Unreachable` it must be a text const
/// and ends the frame without a return-value check.
fn todo_op(consts: &[SealedConst], idx: u16) -> Result<Control, VerifyRejection> {
    match consts.get(idx as usize) {
        Some(SealedConst::Text(_)) => Ok(Control::Return),
        Some(_) => Err(reject(
            VerifyPhase::Function,
            "todo operand must be a text const",
        )),
        None => Err(reject(VerifyPhase::Function, "const index out of range")),
    }
}

fn jump_if_false(frame: &mut Frame, target: usize) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Bool)?;
    Ok(Control::Branch(target))
}

/// Pops the bool condition and pushes nothing; the test-entry phase separately proves
/// it appears only in a test-entry function.
fn assert_op(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Bool)?;
    Ok(Control::Fallthrough)
}

fn call(ctx: &Ctx, frame: &mut Frame, target: u16) -> Result<Control, VerifyRejection> {
    let sig = ctx.signatures.get(target as usize).ok_or(reject(
        VerifyPhase::Function,
        "call target index out of range",
    ))?;
    // a0 is pushed first, so pop arguments in reverse parameter order.
    for param in sig.params.iter().rev() {
        let got = pop(&mut frame.stack)?;
        let want = VType::from_image(*param).expect("a parameter type is never unit");
        if got != want {
            return Err(reject(VerifyPhase::Function, "call argument type mismatch"));
        }
    }
    match sig.ret {
        RetShape::Unit => {}
        RetShape::Scalar { scalar, optional } => {
            frame.stack.push(VType::Scalar { scalar, optional });
        }
        RetShape::Record { idx, optional } => {
            frame.stack.push(VType::Record { idx, optional });
        }
        RetShape::Enum { idx, optional } => {
            frame.stack.push(VType::Enum { idx, optional });
        }
        RetShape::Collection { idx, optional } => {
            frame.stack.push(VType::Collection { idx, optional });
        }
        RetShape::Identity { root, optional } => {
            frame.stack.push(VType::Identity { root, optional });
        }
    }
    Ok(Control::Fallthrough)
}

fn int_neg(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
    frame.stack.push(VType::bare_scalar(Scalar::Int));
    Ok(Control::Fallthrough)
}

fn int_binary_checked(frame: &mut Frame, target: usize) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
    expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
    Ok(Control::CheckedResult {
        target,
        result: VType::bare_scalar(Scalar::Int),
    })
}

fn int_neg_checked(frame: &mut Frame, target: usize) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
    Ok(Control::CheckedResult {
        target,
        result: VType::bare_scalar(Scalar::Int),
    })
}

/// Peeks the guarded value: the top of the stack must be a bare int, which the guard
/// leaves in place (fault or fall through).
fn range_guard(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let top = *frame.stack.last().ok_or(reject(
        VerifyPhase::Function,
        "range guard on an empty stack",
    ))?;
    expect_scalar(top, Scalar::Int)?;
    Ok(Control::Fallthrough)
}

fn bool_not(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Bool)?;
    frame.stack.push(VType::bare_scalar(Scalar::Bool));
    Ok(Control::Fallthrough)
}

/// `addDays(date, int) → date`: pop the int, then the date.
fn date_add_days(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
    expect_scalar(pop(&mut frame.stack)?, Scalar::Date)?;
    frame.stack.push(VType::bare_scalar(Scalar::Date));
    Ok(Control::Fallthrough)
}

/// `instant +/- duration → instant`: pop the duration, then the instant.
fn instant_shift_duration(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Duration)?;
    expect_scalar(pop(&mut frame.stack)?, Scalar::Instant)?;
    frame.stack.push(VType::bare_scalar(Scalar::Instant));
    Ok(Control::Fallthrough)
}

/// Render any interpolable value to text: a bare scalar, enum, or identity. A record,
/// collection, or optional is not renderable through this op.
fn conv_string(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let operand = pop(&mut frame.stack)?;
    let renderable = matches!(
        operand,
        VType::Scalar {
            optional: false,
            ..
        } | VType::Enum {
            optional: false,
            ..
        } | VType::Identity {
            optional: false,
            ..
        }
    );
    if !renderable {
        return Err(reject(
            VerifyPhase::Function,
            "conv-string operand must be a bare scalar, enum, or identity",
        ));
    }
    frame.stack.push(VType::bare_scalar(Scalar::Text));
    Ok(Control::Fallthrough)
}

fn conv_bytes_text(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
    frame.stack.push(VType::bare_scalar(Scalar::Bytes));
    Ok(Control::Fallthrough)
}

fn text_is_empty(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
    frame.stack.push(VType::bare_scalar(Scalar::Bool));
    Ok(Control::Fallthrough)
}

fn text_contains(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
    expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
    frame.stack.push(VType::bare_scalar(Scalar::Bool));
    Ok(Control::Fallthrough)
}

fn text_trim(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
    frame.stack.push(VType::bare_scalar(Scalar::Text));
    Ok(Control::Fallthrough)
}

/// `split(text, sep): List[string]`: separator then text on the stack.
fn text_split(ctx: &Ctx, frame: &mut Frame, idx: u16) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
    expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
    list_of_string(ctx, idx)?;
    frame.stack.push(VType::bare_collection(idx));
    Ok(Control::Fallthrough)
}

/// `lines(text): List[string]`.
fn text_lines(ctx: &Ctx, frame: &mut Frame, idx: u16) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
    list_of_string(ctx, idx)?;
    frame.stack.push(VType::bare_collection(idx));
    Ok(Control::Fallthrough)
}

/// `join(List[string], sep): string`: separator then list on the stack.
fn text_join(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
    let (idx, _) = list_elem(ctx, pop(&mut frame.stack)?)?;
    list_of_string(ctx, idx)?;
    frame.stack.push(VType::bare_scalar(Scalar::Text));
    Ok(Control::Fallthrough)
}

fn record_new(ctx: &Ctx, frame: &mut Frame, ty: u16) -> Result<Control, VerifyRejection> {
    let record = ctx.types.get(ty as usize).ok_or(reject(
        VerifyPhase::Function,
        "record type index out of range",
    ))?;
    // f0 is pushed first, so pop fields in reverse declaration order.
    for field in record.fields.iter().rev() {
        let bare = VType::from_image(field.ty).expect("a record field type is never unit");
        let want = if field.required {
            bare
        } else {
            bare.to_optional()
        };
        let got = pop(&mut frame.stack)?;
        if got != want {
            return Err(reject(
                VerifyPhase::Function,
                "record field operand type mismatch",
            ));
        }
    }
    frame.stack.push(VType::bare_record(ty));
    Ok(Control::Fallthrough)
}

fn field_get(ctx: &Ctx, frame: &mut Frame, field: u16) -> Result<Control, VerifyRejection> {
    let record = pop(&mut frame.stack)?;
    let VType::Record {
        idx,
        optional: false,
    } = record
    else {
        return Err(reject(
            VerifyPhase::Function,
            "field read requires a bare record",
        ));
    };
    let record_type = ctx.types.get(idx as usize).ok_or(reject(
        VerifyPhase::Function,
        "record type index out of range",
    ))?;
    let field_def = record_type
        .fields
        .get(field as usize)
        .ok_or(reject(VerifyPhase::Function, "field index out of range"))?;
    let bare = VType::from_image(field_def.ty).expect("a record field type is never unit");
    let result = if field_def.required {
        bare
    } else {
        bare.to_optional()
    };
    frame.stack.push(result);
    Ok(Control::Fallthrough)
}

/// `[record, value] → [record]`: store a bare field value present.
fn field_set(ctx: &Ctx, frame: &mut Frame, field: u16) -> Result<Control, VerifyRejection> {
    let value = pop(&mut frame.stack)?;
    let record = pop(&mut frame.stack)?;
    let VType::Record {
        idx,
        optional: false,
    } = record
    else {
        return Err(reject(
            VerifyPhase::Function,
            "field set requires a bare record",
        ));
    };
    let record_type = ctx.types.get(idx as usize).ok_or(reject(
        VerifyPhase::Function,
        "record type index out of range",
    ))?;
    let field_def = record_type
        .fields
        .get(field as usize)
        .ok_or(reject(VerifyPhase::Function, "field index out of range"))?;
    let want = VType::from_image(field_def.ty).expect("a record field type is never unit");
    if value != want {
        return Err(reject(
            VerifyPhase::Function,
            "field set operand type mismatch",
        ));
    }
    frame.stack.push(VType::bare_record(idx));
    Ok(Control::Fallthrough)
}

/// `[record] → [record]`: clear a sparse field to vacant.
fn field_unset(ctx: &Ctx, frame: &mut Frame, field: u16) -> Result<Control, VerifyRejection> {
    let record = pop(&mut frame.stack)?;
    let VType::Record {
        idx,
        optional: false,
    } = record
    else {
        return Err(reject(
            VerifyPhase::Function,
            "field unset requires a bare record",
        ));
    };
    let record_type = ctx.types.get(idx as usize).ok_or(reject(
        VerifyPhase::Function,
        "record type index out of range",
    ))?;
    let field_def = record_type
        .fields
        .get(field as usize)
        .ok_or(reject(VerifyPhase::Function, "field index out of range"))?;
    if field_def.required {
        return Err(reject(
            VerifyPhase::Function,
            "a required field cannot be unset",
        ));
    }
    frame.stack.push(VType::bare_record(idx));
    Ok(Control::Fallthrough)
}

fn some_wrap(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let value = pop(&mut frame.stack)?;
    if value.is_optional() {
        return Err(reject(
            VerifyPhase::Function,
            "some-wrap operand is already optional",
        ));
    }
    frame.stack.push(value.to_optional());
    Ok(Control::Fallthrough)
}

fn vacant_load(ctx: &Ctx, frame: &mut Frame, ty: &ImageType) -> Result<Control, VerifyRejection> {
    // A record/enum/collection operand names a value type; bounds-check it.
    match ty {
        ImageType::Record { idx, .. } if ctx.types.get(*idx as usize).is_none() => {
            return Err(reject(
                VerifyPhase::Function,
                "vacant-load record index out of range",
            ));
        }
        ImageType::Enum { idx, .. } if ctx.enums.get(*idx as usize).is_none() => {
            return Err(reject(
                VerifyPhase::Function,
                "vacant-load enum index out of range",
            ));
        }
        ImageType::Collection { idx, .. } if ctx.collections.get(*idx as usize).is_none() => {
            return Err(reject(
                VerifyPhase::Function,
                "vacant-load collection index out of range",
            ));
        }
        _ => {}
    }
    frame
        .stack
        .push(VType::from_image(*ty).expect("vacant-load operand is a value type, not unit"));
    Ok(Control::Fallthrough)
}

fn branch_present(frame: &mut Frame, target: usize) -> Result<Control, VerifyRejection> {
    let value = pop(&mut frame.stack)?;
    if !value.is_optional() {
        return Err(reject(
            VerifyPhase::Function,
            "branch-present requires an optional",
        ));
    }
    Ok(Control::BranchPresent {
        target,
        present: value.to_bare(),
    })
}

fn enum_construct(
    ctx: &Ctx,
    frame: &mut Frame,
    enum_idx: u16,
    variant: u16,
) -> Result<Control, VerifyRejection> {
    let enum_def = ctx.enums.get(enum_idx as usize).ok_or(reject(
        VerifyPhase::Function,
        "enum type index out of range",
    ))?;
    let variant_def = enum_def.variants().get(variant as usize).ok_or(reject(
        VerifyPhase::Function,
        "enum variant index out of range",
    ))?;
    // p0 is pushed first, so pop the payload in reverse declaration order.
    for ty in variant_def.payload.iter().rev() {
        let want = VType::from_image(*ty).expect("a payload leaf is never unit");
        let got = pop(&mut frame.stack)?;
        if got != want {
            return Err(reject(
                VerifyPhase::Function,
                "enum payload operand type mismatch",
            ));
        }
    }
    frame.stack.push(VType::bare_enum(enum_idx));
    Ok(Control::Fallthrough)
}

fn enum_tag(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let value = pop(&mut frame.stack)?;
    if !matches!(
        value,
        VType::Enum {
            optional: false,
            ..
        }
    ) {
        return Err(reject(
            VerifyPhase::Function,
            "enum-tag requires a bare enum",
        ));
    }
    frame.stack.push(VType::bare_scalar(Scalar::Int));
    Ok(Control::Fallthrough)
}

fn enum_payload_get(
    ctx: &Ctx,
    frame: &mut Frame,
    variant: u16,
    field: u16,
) -> Result<Control, VerifyRejection> {
    let value = pop(&mut frame.stack)?;
    let VType::Enum {
        idx,
        optional: false,
    } = value
    else {
        return Err(reject(
            VerifyPhase::Function,
            "enum-payload-get requires a bare enum",
        ));
    };
    let enum_def = ctx.enums.get(idx as usize).ok_or(reject(
        VerifyPhase::Function,
        "enum type index out of range",
    ))?;
    let variant_def = enum_def.variants().get(variant as usize).ok_or(reject(
        VerifyPhase::Function,
        "enum variant index out of range",
    ))?;
    // The variant operand types the payload leaf; the VM faults if the runtime value
    // carries a different variant, so the pushed type is never observed on a mismatch.
    let leaf = variant_def.payload.get(field as usize).ok_or(reject(
        VerifyPhase::Function,
        "enum payload field index out of range",
    ))?;
    frame
        .stack
        .push(VType::from_image(*leaf).expect("a payload leaf is never unit"));
    Ok(Control::Fallthrough)
}

fn eq_enum(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    let (
        VType::Enum {
            idx: r,
            optional: false,
        },
        VType::Enum {
            idx: l,
            optional: false,
        },
    ) = (right, left)
    else {
        return Err(reject(
            VerifyPhase::Function,
            "enum equality requires two bare enums",
        ));
    };
    if l != r {
        return Err(reject(
            VerifyPhase::Function,
            "enum equality operands are different enums",
        ));
    }
    frame.stack.push(VType::bare_scalar(Scalar::Bool));
    Ok(Control::Fallthrough)
}

fn eq_id(frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    let (
        VType::Identity {
            root: r,
            optional: false,
        },
        VType::Identity {
            root: l,
            optional: false,
        },
    ) = (right, left)
    else {
        return Err(reject(
            VerifyPhase::Function,
            "identity equality requires two bare entry identities",
        ));
    };
    if l != r {
        return Err(reject(
            VerifyPhase::Function,
            "identity equality operands name different store roots",
        ));
    }
    frame.stack.push(VType::bare_scalar(Scalar::Bool));
    Ok(Control::Fallthrough)
}

fn make_identity(
    ctx: &Ctx,
    frame: &mut Frame,
    root: u16,
    cols: u16,
) -> Result<Control, VerifyRejection> {
    let sealed_root = ctx.roots.get(root as usize).ok_or(reject(
        VerifyPhase::Function,
        "make-identity root index out of range",
    ))?;
    if sealed_root.keys.len() != cols as usize {
        return Err(reject(
            VerifyPhase::Function,
            "make-identity column count does not match the root's key columns",
        ));
    }
    // k0 is pushed first, so pop the key columns in reverse declaration order, each
    // matching the root's key-column scalar type.
    for scalar in sealed_root.keys.iter().rev() {
        let got = pop(&mut frame.stack)?;
        if got != VType::bare_scalar(*scalar) {
            return Err(reject(
                VerifyPhase::Function,
                "make-identity key operand type does not match the root's key column",
            ));
        }
    }
    frame.stack.push(VType::Identity {
        root,
        optional: false,
    });
    Ok(Control::Fallthrough)
}

fn identity_key_path(ctx: &Ctx, frame: &mut Frame, cols: u16) -> Result<Control, VerifyRejection> {
    let VType::Identity {
        root,
        optional: false,
    } = pop(&mut frame.stack)?
    else {
        return Err(reject(
            VerifyPhase::Function,
            "identity key-path requires a bare entry identity",
        ));
    };
    let sealed_root = ctx.roots.get(root as usize).ok_or(reject(
        VerifyPhase::Function,
        "identity key-path root index out of range",
    ))?;
    if sealed_root.keys.len() != cols as usize {
        return Err(reject(
            VerifyPhase::Function,
            "identity key-path column count does not match the root's key columns",
        ));
    }
    // Spread the key columns root-first (k0 pushed first) so the key-path sits exactly
    // as an inline `^root[k…]` access would leave it for the entry read. Each column is
    // tagged with the identity's root so a durable op can re-prove it addresses that
    // same root — an identity of one root can never key an operation on another, even
    // through a local slot.
    for scalar in &sealed_root.keys {
        frame.stack.push(VType::IdentityColumn {
            root,
            scalar: *scalar,
        });
    }
    Ok(Control::Fallthrough)
}

fn list_new(ctx: &Ctx, frame: &mut Frame, idx: u16) -> Result<Control, VerifyRejection> {
    match ctx.collections.get(idx as usize) {
        Some(SealedCollectionType::List { .. }) => {}
        _ => {
            return Err(reject(
                VerifyPhase::Function,
                "list-new operand does not name a list collection type",
            ));
        }
    }
    frame.stack.push(VType::bare_collection(idx));
    Ok(Control::Fallthrough)
}

fn map_new(ctx: &Ctx, frame: &mut Frame, idx: u16) -> Result<Control, VerifyRejection> {
    match ctx.collections.get(idx as usize) {
        Some(SealedCollectionType::Map { .. }) => {}
        _ => {
            return Err(reject(
                VerifyPhase::Function,
                "map-new operand does not name a map collection type",
            ));
        }
    }
    frame.stack.push(VType::bare_collection(idx));
    Ok(Control::Fallthrough)
}

fn list_append(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let value = pop(&mut frame.stack)?;
    let (idx, elem) = list_elem(ctx, pop(&mut frame.stack)?)?;
    if value != VType::from_image(elem).expect("a list element type is never unit") {
        return Err(reject(
            VerifyPhase::Function,
            "list-append value type does not match the element type",
        ));
    }
    frame.stack.push(VType::bare_collection(idx));
    Ok(Control::Fallthrough)
}

fn list_len(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    list_elem(ctx, pop(&mut frame.stack)?)?;
    frame.stack.push(VType::bare_scalar(Scalar::Int));
    Ok(Control::Fallthrough)
}

fn list_get(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
    let (_, elem) = list_elem(ctx, pop(&mut frame.stack)?)?;
    frame
        .stack
        .push(VType::from_image(elem).expect("a list element type is never unit"));
    Ok(Control::Fallthrough)
}

fn list_index(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
    let (_, elem) = list_elem(ctx, pop(&mut frame.stack)?)?;
    frame.stack.push(
        VType::from_image(elem)
            .expect("a list element type is never unit")
            .to_optional(),
    );
    Ok(Control::Fallthrough)
}

fn map_insert(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let value = pop(&mut frame.stack)?;
    let key = pop(&mut frame.stack)?;
    let (idx, key_ty, value_ty) = map_kv(ctx, pop(&mut frame.stack)?)?;
    if key != VType::from_image(key_ty).expect("a map key type is never unit") {
        return Err(reject(
            VerifyPhase::Function,
            "map-insert key type does not match the map key type",
        ));
    }
    if value != VType::from_image(value_ty).expect("a map value type is never unit") {
        return Err(reject(
            VerifyPhase::Function,
            "map-insert value type does not match the map value type",
        ));
    }
    frame.stack.push(VType::bare_collection(idx));
    Ok(Control::Fallthrough)
}

fn map_remove(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let key = pop(&mut frame.stack)?;
    let (idx, key_ty, _value_ty) = map_kv(ctx, pop(&mut frame.stack)?)?;
    if key != VType::from_image(key_ty).expect("a map key type is never unit") {
        return Err(reject(
            VerifyPhase::Function,
            "map-remove key type does not match the map key type",
        ));
    }
    frame.stack.push(VType::bare_collection(idx));
    Ok(Control::Fallthrough)
}

fn map_get(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    let key = pop(&mut frame.stack)?;
    let (_, key_ty, value_ty) = map_kv(ctx, pop(&mut frame.stack)?)?;
    if key != VType::from_image(key_ty).expect("a map key type is never unit") {
        return Err(reject(
            VerifyPhase::Function,
            "map-get key type does not match the map key type",
        ));
    }
    frame.stack.push(
        VType::from_image(value_ty)
            .expect("a map value type is never unit")
            .to_optional(),
    );
    Ok(Control::Fallthrough)
}

fn map_len(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    map_kv(ctx, pop(&mut frame.stack)?)?;
    frame.stack.push(VType::bare_scalar(Scalar::Int));
    Ok(Control::Fallthrough)
}

fn map_key_at(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
    let (_, key_ty, _) = map_kv(ctx, pop(&mut frame.stack)?)?;
    frame
        .stack
        .push(VType::from_image(key_ty).expect("a map key type is never unit"));
    Ok(Control::Fallthrough)
}

fn map_value_at(ctx: &Ctx, frame: &mut Frame) -> Result<Control, VerifyRejection> {
    expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
    let (_, _, value_ty) = map_kv(ctx, pop(&mut frame.stack)?)?;
    frame
        .stack
        .push(VType::from_image(value_ty).expect("a map value type is never unit"));
    Ok(Control::Fallthrough)
}

/// The COLLTYPES index and element type of a bare list `VType`, or a phase-3
/// rejection when the operand is not a bare list collection.
fn list_elem(ctx: &Ctx, value: VType) -> Result<(u16, ImageType), VerifyRejection> {
    let VType::Collection {
        idx,
        optional: false,
    } = value
    else {
        return Err(reject(VerifyPhase::Function, "operand is not a bare list"));
    };
    match ctx.collections.get(idx as usize) {
        Some(SealedCollectionType::List { elem }) => Ok((idx, *elem)),
        _ => Err(reject(
            VerifyPhase::Function,
            "collection index does not name a list type",
        )),
    }
}

/// Prove COLLTYPES index `idx` names a `List[string]`, the only collection the
/// text-floor `split`/`lines`/`join` opcodes produce or consume. A hand-built image
/// naming any other collection there is rejected.
fn list_of_string(ctx: &Ctx, idx: u16) -> Result<(), VerifyRejection> {
    match ctx.collections.get(idx as usize) {
        Some(SealedCollectionType::List { elem }) if *elem == ImageType::scalar(Scalar::Text) => {
            Ok(())
        }
        _ => Err(reject(
            VerifyPhase::Function,
            "text split/lines/join collection index does not name a list of string",
        )),
    }
}

/// The COLLTYPES index and `(key, value)` image types of a bare map `VType`, or a
/// phase-3 rejection when the operand is not a bare map collection.
fn map_kv(ctx: &Ctx, value: VType) -> Result<(u16, ImageType, ImageType), VerifyRejection> {
    let VType::Collection {
        idx,
        optional: false,
    } = value
    else {
        return Err(reject(VerifyPhase::Function, "operand is not a bare map"));
    };
    match ctx.collections.get(idx as usize) {
        Some(SealedCollectionType::Map { key, value }) => Ok((idx, *key, *value)),
        _ => Err(reject(
            VerifyPhase::Function,
            "collection index does not name a map type",
        )),
    }
}

/// Whether `instr` is handled by [`apply_durable`] (a durable op or a transaction
/// marker).
fn is_durable(instr: &SealedInstr) -> bool {
    is_mutation(instr)
        || is_durable_read(instr)
        || matches!(instr, SealedInstr::TxnBegin | SealedInstr::TxnCommit)
}

/// The site operand of a durable op, or `None` for a transaction marker.
pub(super) fn durable_site(instr: &SealedInstr) -> Option<u16> {
    match instr {
        SealedInstr::DurExists(site)
        | SealedInstr::DurFamilyExists(site)
        | SealedInstr::DurReadField(site)
        | SealedInstr::DurReadEntry(site)
        | SealedInstr::DurSetRequired(site)
        | SealedInstr::DurSetSparse(site)
        | SealedInstr::DurSetSparsePresent { site, .. }
        | SealedInstr::DurCreateEntry(site)
        | SealedInstr::DurReplaceEntry(site)
        | SealedInstr::DurEraseField(site)
        | SealedInstr::DurEraseEntry(site)
        | SealedInstr::DurReadGroup(site)
        | SealedInstr::DurReplaceGroup(site)
        | SealedInstr::DurEraseGroup(site)
        | SealedInstr::DurIterateBounded { site, .. }
        | SealedInstr::DurIndexScan { site, .. }
        | SealedInstr::DurIndexLookup(site)
        | SealedInstr::DurIndexExists(site) => Some(*site),
        _ => None,
    }
}

/// The single owner of the durable-opcode → [`OperationClass`] partition. The
/// closed projection of the durable operation algebra onto authority atoms:
/// `create`, `replace`, and required/sparse field sets are all writes; the two
/// erases are erases; presence is a probe; field/entry reads are reads; and the
/// bounded traversal is ordered traversal. Transaction markers and every pure opcode
/// make no atom.
///
/// The match is exhaustive with no `_` fallthrough, so a new durable opcode added
/// to [`SealedInstr`] without a class here fails to compile rather than silently
/// projecting to "no access". [`is_mutation`] and [`is_durable_read`] derive from
/// this partition and never restate it.
pub(super) fn durable_op_class(instr: &SealedInstr) -> Option<OperationClass> {
    match instr {
        SealedInstr::DurExists(_) | SealedInstr::DurFamilyExists(_) => {
            Some(OperationClass::Presence)
        }
        SealedInstr::DurReadField(_)
        | SealedInstr::DurReadEntry(_)
        | SealedInstr::DurReadGroup(_) => Some(OperationClass::Read),
        SealedInstr::DurSetRequired(_)
        | SealedInstr::DurSetSparse(_)
        | SealedInstr::DurSetSparsePresent { .. }
        | SealedInstr::DurCreateEntry(_)
        | SealedInstr::DurReplaceEntry(_)
        | SealedInstr::DurReplaceGroup(_) => Some(OperationClass::Write),
        SealedInstr::DurEraseField(_)
        | SealedInstr::DurEraseEntry(_)
        | SealedInstr::DurEraseGroup(_) => Some(OperationClass::Erase),
        // A unique-index presence probe reads the same index cell family as the lookup —
        // revealing strictly less (a bool, not the identity) — so it demands the same
        // index-read authority rather than a novel presence-over-an-index atom.
        SealedInstr::DurIterateBounded { .. }
        | SealedInstr::DurIndexScan { .. }
        | SealedInstr::DurIndexLookup(_)
        | SealedInstr::DurIndexExists(_) => Some(OperationClass::IndexRead),
        // Region markers open and close the transaction but stage no access.
        SealedInstr::TxnBegin | SealedInstr::TxnCommit => None,
        // The closed complement: every pure opcode stages no durable access.
        SealedInstr::ConstLoad(_)
        | SealedInstr::LocalGet(_)
        | SealedInstr::LocalSet(_)
        | SealedInstr::Pop
        | SealedInstr::Return
        | SealedInstr::Jump(_)
        | SealedInstr::JumpIfFalse(_)
        | SealedInstr::IntAdd
        | SealedInstr::IntSub
        | SealedInstr::IntMul
        | SealedInstr::IntRem
        | SealedInstr::IntDiv
        | SealedInstr::IntNeg
        | SealedInstr::BoolNot
        | SealedInstr::IntLt
        | SealedInstr::IntLe
        | SealedInstr::IntGt
        | SealedInstr::IntGe
        | SealedInstr::EqInt
        | SealedInstr::EqBool
        | SealedInstr::EqText
        | SealedInstr::TextConcat
        | SealedInstr::TextLt
        | SealedInstr::TextLe
        | SealedInstr::TextGt
        | SealedInstr::TextGe
        | SealedInstr::EqBytes
        | SealedInstr::BytesLt
        | SealedInstr::BytesLe
        | SealedInstr::BytesGt
        | SealedInstr::BytesGe
        | SealedInstr::ConvString
        | SealedInstr::ConvBytesText
        | SealedInstr::TextIsEmpty
        | SealedInstr::TextContains
        | SealedInstr::TextTrim
        | SealedInstr::TextSplit(_)
        | SealedInstr::TextLines(_)
        | SealedInstr::TextJoin
        | SealedInstr::EqDate
        | SealedInstr::DateLt
        | SealedInstr::DateLe
        | SealedInstr::DateGt
        | SealedInstr::DateGe
        | SealedInstr::EqInstant
        | SealedInstr::InstantLt
        | SealedInstr::InstantLe
        | SealedInstr::InstantGt
        | SealedInstr::InstantGe
        | SealedInstr::EqDuration
        | SealedInstr::DurationLt
        | SealedInstr::DurationLe
        | SealedInstr::DurationGt
        | SealedInstr::DurationGe
        | SealedInstr::DateAddDays
        | SealedInstr::DateDaysBetween
        | SealedInstr::DurationAdd
        | SealedInstr::DurationSub
        | SealedInstr::InstantAddDuration
        | SealedInstr::InstantSubDuration
        | SealedInstr::IntAddChecked(_)
        | SealedInstr::IntSubChecked(_)
        | SealedInstr::IntMulChecked(_)
        | SealedInstr::IntNegChecked(_)
        | SealedInstr::IntDivChecked(_)
        | SealedInstr::IntRemChecked(_)
        | SealedInstr::RangeGuard { .. }
        | SealedInstr::RecordNew(_)
        | SealedInstr::FieldGet(_)
        | SealedInstr::FieldSet(_)
        | SealedInstr::FieldUnset(_)
        | SealedInstr::SomeWrap
        | SealedInstr::VacantLoad(_)
        | SealedInstr::EnumConstruct { .. }
        | SealedInstr::EnumTag
        | SealedInstr::EnumPayloadGet { .. }
        | SealedInstr::EqEnum
        | SealedInstr::EqId
        | SealedInstr::MakeIdentity { .. }
        | SealedInstr::IdentityKeyPath(_)
        | SealedInstr::BranchPresent(_)
        | SealedInstr::Unreachable(_)
        | SealedInstr::Todo(_)
        | SealedInstr::Assert
        | SealedInstr::Call(_)
        | SealedInstr::ListNew(_)
        | SealedInstr::ListAppend
        | SealedInstr::ListLen
        | SealedInstr::ListGet
        | SealedInstr::ListIndex
        | SealedInstr::MapNew(_)
        | SealedInstr::MapInsert
        | SealedInstr::MapRemove
        | SealedInstr::MapGet
        | SealedInstr::MapLen
        | SealedInstr::MapKeyAt
        | SealedInstr::MapValueAt => None,
    }
}

/// Whether `instr` stages a durable mutation (a write or erase). Derived from the
/// [`durable_op_class`] partition so the mutation set never drifts from the atom it
/// projects.
pub(super) fn is_mutation(instr: &SealedInstr) -> bool {
    durable_op_class(instr).is_some_and(|class| class.mutates())
}

/// Whether `instr` reads durable data — a presence probe, a field/entry read, or an
/// ordered bounded traversal. The classified durable ops that are not mutations.
fn is_durable_read(instr: &SealedInstr) -> bool {
    durable_op_class(instr).is_some_and(|class| !class.mutates())
}

/// Mutably borrow two distinct elements of a slice. The demand fixpoint unions a
/// callee's closure into its caller; the call graph is acyclic, so `dst != src`.
pub(super) fn borrow_two<T>(slice: &mut [T], dst: usize, src: usize) -> (&mut T, &T) {
    assert_ne!(dst, src, "a call graph edge never self-loops");
    if dst < src {
        let (left, right) = slice.split_at_mut(src);
        (&mut left[dst], &right[0])
    } else {
        let (left, right) = slice.split_at_mut(dst);
        (&mut right[0], &left[src])
    }
}

/// Phase-3 type check for durable opcodes and transaction markers (design §D). The
/// transaction markers leave the stack unchanged; phase 5 checks their flow.
fn apply_durable(
    ctx: &Ctx,
    instr: &SealedInstr,
    frame: &mut Frame,
) -> Result<Control, VerifyRejection> {
    let Some(site_index) = durable_site(instr) else {
        // TxnBegin / TxnCommit: no stack effect here.
        return Ok(Control::Fallthrough);
    };
    let site = ctx.sites.get(site_index as usize).ok_or(reject(
        VerifyPhase::Function,
        "durable site index out of range",
    ))?;
    // A durable opcode may reference only a kernel-executable flat site. A parked
    // site (a nested placement, a group-scoped field, or a site on a singleton or
    // group/branch-bearing root) carries a complete identity but no executable
    // operation; an opcode over one is a forged or not-yet-executable image and is
    // refused here, independently of the compiler. A widened field site is executable.
    let (site_root, site_target) = match site {
        SealedSite::Flat { root, target } => (*root, target),
        SealedSite::Parked { .. } => {
            return Err(reject(
                VerifyPhase::Function,
                "a durable operation site is not yet executable",
            ));
        }
    };
    let root = ctx.roots.get(site_root as usize).ok_or(reject(
        VerifyPhase::Function,
        "durable site root out of range",
    ))?;
    // A managed-index read touches only the index cell family, not the entry tree, so it
    // is handled before the whole-entry key-path logic (and is admitted over a root whose
    // entry tree is not flat). Its projection is read from the sealed index the site names.
    if let SealedSiteTarget::IndexScan(index) | SealedSiteTarget::IndexLookup(index) = site_target {
        return apply_index_read(ctx, instr, frame, site_root, root, *index);
    }
    // A managed-index opcode is executable only over an index site. A forged image pairing
    // one with a whole-entry, field, or group site is refused here rather than falling
    // through to the whole-entry key-path logic (which has no arm for it), so the trust
    // boundary rejects the mismatch instead of reaching the closed-complement `unreachable`.
    if matches!(
        instr,
        SealedInstr::DurIndexScan { .. }
            | SealedInstr::DurIndexLookup(_)
            | SealedInstr::DurIndexExists(_)
    ) {
        return Err(reject(
            VerifyPhase::Function,
            "a managed-index opcode over a non-index site",
        ));
    }
    // Defense in depth: a flat site's root is free of groups and composite/nested
    // branches by construction (field-only branches with composite keys are executable;
    // a widened field is inline-framed and executable), but recheck at the opcode.
    if root.has_extras {
        return Err(reject(
            VerifyPhase::Function,
            "a durable operation site requires a flat-executable root",
        ));
    }
    if root.keys.is_empty() {
        return Err(reject(
            VerifyPhase::Function,
            "a durable operation site requires a keyed root",
        ));
    }
    // A site addresses a key-path of one column per key column of every node from the root
    // down to the addressed node: the root's key columns, then each branch hop's key
    // columns. The lowering pushes them root-first in column-declaration order, so the
    // stack-pop order is that whole column sequence reversed. `entry_record` is the record
    // a whole-entry op reads or writes (the addressed branch's own for a branch site);
    // `traversed_arity` is the addressed layer's own key column count, used to park
    // composite-keyed traversal.
    let mut columns: Vec<VType> = root
        .keys
        .iter()
        .map(|scalar| VType::bare_scalar(*scalar))
        .collect();
    let (entry_record, traversed_arity) = match site_target {
        SealedSiteTarget::BranchEntry(path)
        | SealedSiteTarget::BranchField { branch: path, .. } => {
            let branch = resolve_sealed_branch(root, path)?;
            for scalar in branch_key_columns(root, path)? {
                columns.push(VType::bare_scalar(scalar));
            }
            (branch.record, branch.keys().len())
        }
        SealedSiteTarget::WholePayload | SealedSiteTarget::FieldLeaf(_) => {
            (root.record, root.keys.len())
        }
        SealedSiteTarget::GroupEntry(group) => {
            // A group is addressed by the root's own key-path (it is a value unit of the
            // root entry, not a keyed child). Its whole-group op reads or writes the
            // group's own materialized record.
            let group = root.groups.get(*group as usize).ok_or(reject(
                VerifyPhase::Function,
                "durable group index out of range",
            ))?;
            (group.record, root.keys.len())
        }
        SealedSiteTarget::IndexScan(_) | SealedSiteTarget::IndexLookup(_) => {
            unreachable!("index read targets are handled before the entry key-path logic")
        }
    };
    // Stack-pop order is the root-first column sequence reversed (deepest last column on
    // top of the stack, popped first).
    let mut key_path = columns;
    key_path.reverse();
    let stack = &mut frame.stack;
    match instr {
        SealedInstr::DurExists(_) => {
            pop_key_path(stack, &key_path, site_root)?;
            stack.push(VType::bare_scalar(Scalar::Bool));
        }
        SealedInstr::DurFamilyExists(_) => {
            // The family-populated probe names a whole-entry family (a root or branch
            // entry site), never a field, and answers a bounded yes/no over its
            // immediate children. Like bounded traversal it iterates a single-column
            // family — the current language spells no composite-key family probe — so a
            // composite-keyed family parks with a typed rejection.
            require_entry(site_target)?;
            if traversed_arity != 1 {
                return Err(reject(
                    VerifyPhase::Function,
                    "family-populated probe over a composite-keyed family is not yet executable",
                ));
            }
            // The probe supplies no immediate child key: only the ancestor key-path
            // locating the family's parent entry sits on the stack (none for a root
            // family, the parent columns for a branch family). Drop the traversed
            // column — never pushed — and pop the ancestor path.
            let (_traversed_key, ancestor_path) = key_path
                .split_first()
                .expect("an entry site has a non-empty key-path");
            // The ancestor key-path locates the probed family's fixed parent entry; an
            // entry-identity parent column carries its root, re-proven exactly as the
            // whole key-path pop does.
            for ty in ancestor_path {
                pop_key_column(stack, *ty, site_root)?;
            }
            stack.push(VType::bare_scalar(Scalar::Bool));
        }
        SealedInstr::DurReadField(_) => {
            let field = field_of(ctx, site_target, root)?;
            let value = durable_field_vtype(field).to_optional();
            pop_key_path(stack, &key_path, site_root)?;
            stack.push(value);
        }
        SealedInstr::DurReadEntry(_) => {
            require_entry(site_target)?;
            pop_key_path(stack, &key_path, site_root)?;
            stack.push(VType::bare_record(entry_record).to_optional());
        }
        SealedInstr::DurReadGroup(_) => {
            require_group(site_target)?;
            pop_key_path(stack, &key_path, site_root)?;
            stack.push(VType::bare_record(entry_record).to_optional());
        }
        SealedInstr::DurReplaceGroup(_) => {
            require_group(site_target)?;
            expect(pop(stack)?, VType::bare_record(entry_record))?;
            pop_key_path(stack, &key_path, site_root)?;
        }
        SealedInstr::DurEraseGroup(_) => {
            require_group(site_target)?;
            pop_key_path(stack, &key_path, site_root)?;
        }
        SealedInstr::DurSetRequired(_) => {
            let field = field_of(ctx, site_target, root)?;
            if !field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-required targets a sparse field",
                ));
            }
            let value = durable_field_vtype(field);
            expect(pop(stack)?, value)?;
            pop_key_path(stack, &key_path, site_root)?;
        }
        SealedInstr::DurSetSparse(_) => {
            let field = field_of(ctx, site_target, root)?;
            if field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-sparse targets a required field",
                ));
            }
            let value = durable_field_vtype(field).to_optional();
            expect(pop(stack)?, value)?;
            pop_key_path(stack, &key_path, site_root)?;
        }
        SealedInstr::DurSetSparsePresent { key_slots, .. } => {
            // The strict present form reads its containing entry's whole key-path from
            // place slots rather than the stack. It addresses a stored field leaf — a
            // root field (`FieldLeaf`) or a branch field (`BranchField`); a whole-payload,
            // branch-entry, or index site is not a field set and is refused. The field's
            // containing entry is the root (root field) or the branch (branch field), and
            // either way the site's full key-path is `columns` (root-first).
            if !matches!(
                site_target,
                SealedSiteTarget::FieldLeaf(_) | SealedSiteTarget::BranchField { .. }
            ) {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-sparse-present requires a field-leaf site",
                ));
            }
            // The opcode's key-path must address the field's containing entry exactly:
            // one slot per key column of every node from the root down (the root-first
            // `columns` sequence). A forged image with too few or too many slots — e.g. a
            // single root key over a branch-field site, the slice-A write-safety concern —
            // is refused here so the kernel is never handed a mis-arity key-path.
            let columns_root_first: Vec<VType> = key_path.iter().rev().cloned().collect();
            if key_slots.len() != columns_root_first.len() {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-sparse-present key-path arity does not match its field site",
                ));
            }
            let field = field_of(ctx, site_target, root)?;
            if field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-sparse-present targets a required field",
                ));
            }
            // The strict form reads its key-path from the place's pre-evaluated local
            // slots rather than the stack, so only the value is popped. Each slot must be
            // definitely initialized with its column's key type, root-first. A slot captured
            // from an entry identity carries its root as an identity column, re-proven here
            // against the site root exactly as the stack key-path pop does.
            let value = durable_field_vtype(field).to_optional();
            expect(pop(stack)?, value)?;
            for (slot, column_ty) in key_slots.iter().zip(&columns_root_first) {
                match frame.locals.get(*slot as usize) {
                    Some(Some(slot_ty)) if slot_keys_column(*slot_ty, *column_ty, site_root) => {}
                    Some(Some(_)) => {
                        return Err(reject(
                            VerifyPhase::Function,
                            "set-sparse-present key slot has the wrong type",
                        ));
                    }
                    _ => {
                        return Err(reject(
                            VerifyPhase::Function,
                            "set-sparse-present key slot is uninitialized or out of range",
                        ));
                    }
                }
            }
        }
        SealedInstr::DurCreateEntry(_) | SealedInstr::DurReplaceEntry(_) => {
            require_entry(site_target)?;
            expect(pop(stack)?, VType::bare_record(entry_record))?;
            pop_key_path(stack, &key_path, site_root)?;
        }
        SealedInstr::DurEraseField(_) => {
            let field = field_of(ctx, site_target, root)?;
            if field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "erase targets a required field",
                ));
            }
            pop_key_path(stack, &key_path, site_root)?;
        }
        SealedInstr::DurEraseEntry(_) => {
            require_entry(site_target)?;
            pop_key_path(stack, &key_path, site_root)?;
        }
        SealedInstr::DurIterateBounded {
            limit,
            from,
            list_ty,
            ..
        } => {
            // Bounded traversal iterates the layer the site's placement belongs to: a
            // root site (WholePayload) the root's entry family, a branch site
            // (BranchEntry) that branch's children under a fixed root key. A field site
            // names no traversable layer.
            require_entry(site_target)?;
            // The `at most N` bound is a positive compile-time constant no larger than
            // the frozen-list ceiling.
            if *limit == 0 || *limit > marrow_image::bounds::MAX_TRAVERSAL_BOUND {
                return Err(reject(
                    VerifyPhase::Function,
                    "bounded traversal bound is out of range",
                ));
            }
            // Bounded traversal iterates a single key column: the loop binds one immediate
            // key and takes one inclusive `from`. A composite-keyed traversed layer has no
            // spelled single-column iteration in the current language, so it parks with a
            // typed rejection rather than inventing a last-column-under-prefix semantics.
            if traversed_arity != 1 {
                return Err(reject(
                    VerifyPhase::Function,
                    "bounded traversal over a composite-keyed layer is not yet executable",
                ));
            }
            // The traversed key is what iteration enumerates — the first element of the
            // site's whole-entry key-path (the single traversed column); the remainder is
            // the ancestor key-path locating the traversed layer's parent entry (empty for
            // a root site, the parent columns for a branch site).
            let (traversed_key, ancestor_path) = key_path
                .split_first()
                .expect("an entry site has a non-empty key-path");
            let VType::Scalar {
                scalar: key_scalar,
                optional: false,
            } = *traversed_key
            else {
                unreachable!("a durable key-path element is a bare scalar");
            };
            // The inclusive `from` key sits on top of the ancestor key-path and types
            // as the traversed key `K`. It is a source key expression, never an identity
            // column, so it pops as a bare scalar.
            if *from {
                expect(pop(stack)?, *traversed_key)?;
            }
            // The ancestor key-path locates the traversed layer's fixed parent entry. A
            // column spread from an entry-identity parent (`^root[Id(…)].branch`, or an
            // identity-keyed place base) carries its root, re-proven here exactly as a
            // whole key-path pop does.
            for ty in ancestor_path {
                pop_key_column(stack, *ty, site_root)?;
            }
            // `list_ty` must name exactly `List[K]`: the frozen keys materialize into
            // this one list value, so a hostile image naming a wider or wrong-element
            // list is refused before the runtime builds it.
            match ctx.collections.get(*list_ty as usize) {
                Some(SealedCollectionType::List { elem })
                    if *elem == ImageType::scalar(key_scalar) => {}
                _ => {
                    return Err(reject(
                        VerifyPhase::Function,
                        "bounded traversal list type does not name a list of the traversed key",
                    ));
                }
            }
            stack.push(VType::bare_collection(*list_ty));
            stack.push(VType::bare_scalar(Scalar::Bool));
        }
        _ => unreachable!("durable_site returned a site for this opcode"),
    }
    Ok(Control::Fallthrough)
}

/// The base scalar an index projection component holds: an identity key reads the
/// root's key-column scalar; a top-level field reads its own bare scalar type. A
/// component that resolves out of range or to a non-scalar is a forged image.
fn index_component_scalar(
    ctx: &Ctx,
    root: &SealedRoot,
    component: &SealedIndexComponent,
) -> Result<Scalar, VerifyRejection> {
    match component {
        SealedIndexComponent::Key(position) => root.keys.get(*position as usize).copied().ok_or(
            reject(VerifyPhase::Function, "index key component out of range"),
        ),
        SealedIndexComponent::Field(position) => {
            let record = ctx.types.get(root.record as usize).ok_or(reject(
                VerifyPhase::Function,
                "index root record out of range",
            ))?;
            let field = record.fields().get(*position as usize).ok_or(reject(
                VerifyPhase::Function,
                "index field component out of range",
            ))?;
            match field.ty {
                ImageType::Scalar {
                    scalar,
                    optional: false,
                } => Ok(scalar),
                _ => Err(reject(
                    VerifyPhase::Function,
                    "index field component is not a bare scalar",
                )),
            }
        }
    }
}

/// Phase-3 stack effect for the three managed-index reads. The projection scalar types
/// come from the sealed index the site names, re-resolved here against the root rather
/// than trusted from the instruction. A scan holds the index's leading field components
/// as a prefix and freezes the trailing identity component as `List[K]`; a lookup pops
/// the whole projection and yields the optional source identity; an `exists` probe pops
/// the whole projection and yields a bare `bool`. The read kind is rechecked against the
/// index's `unique` flag (defense in depth over the seal-time check), so a scan over a
/// unique index or a lookup/probe over a nonunique one is refused. A non-index durable
/// opcode routed here by a forged index-site operand is refused rather than reaching the
/// closed-complement `unreachable`.
fn apply_index_read(
    ctx: &Ctx,
    instr: &SealedInstr,
    frame: &mut Frame,
    site_root: u16,
    root: &SealedRoot,
    index_position: u16,
) -> Result<Control, VerifyRejection> {
    let index = ctx.indexes.get(index_position as usize).ok_or(reject(
        VerifyPhase::Function,
        "index read site names no sealed index",
    ))?;
    if index.root != site_root {
        return Err(reject(
            VerifyPhase::Function,
            "index read site names an index of a different root",
        ));
    }
    let projection: Vec<Scalar> = index
        .projection
        .iter()
        .map(|component| index_component_scalar(ctx, root, component))
        .collect::<Result<_, _>>()?;
    let stack = &mut frame.stack;
    match instr {
        SealedInstr::DurIndexScan {
            limit,
            from,
            list_ty,
            ..
        } => {
            if index.unique {
                return Err(reject(
                    VerifyPhase::Function,
                    "progressive scan of a unique index",
                ));
            }
            if *limit == 0 || *limit > marrow_image::bounds::MAX_TRAVERSAL_BOUND {
                return Err(reject(
                    VerifyPhase::Function,
                    "index scan bound is out of range",
                ));
            }
            // The scan yields the source identity, so the root's identity is a single key
            // column: the scanned (trailing) projection component is that one key. A
            // composite-identity root has no single-column identity to yield here.
            if root.keys.len() != 1 {
                return Err(reject(
                    VerifyPhase::Function,
                    "index scan over a composite-identity root is not yet executable",
                ));
            }
            let (scanned, prefix) = projection.split_last().ok_or(reject(
                VerifyPhase::Function,
                "index scan projection is empty",
            ))?;
            // The nonunique projection ends with the identity suffix; with a single-column
            // identity the trailing component is exactly the root key.
            if *scanned != root.keys[0] {
                return Err(reject(
                    VerifyPhase::Function,
                    "index scan trailing component is not the source identity key",
                ));
            }
            // The held prefix (the leading field components) sits under the inclusive
            // `from` key; pop `from` (the scanned key type) then the prefix in reverse.
            if *from {
                expect(pop(stack)?, VType::bare_scalar(*scanned))?;
            }
            for scalar in prefix.iter().rev() {
                expect(pop(stack)?, VType::bare_scalar(*scalar))?;
            }
            // `list_ty` names exactly `List[K]` of the scanned identity key: the frozen
            // raw keys materialize into this one list before the loop wraps each as an
            // identity, so a hostile image naming a wrong-element list is refused here.
            match ctx.collections.get(*list_ty as usize) {
                Some(SealedCollectionType::List { elem })
                    if *elem == ImageType::scalar(*scanned) => {}
                _ => {
                    return Err(reject(
                        VerifyPhase::Function,
                        "index scan list type does not name a list of the identity key",
                    ));
                }
            }
            stack.push(VType::bare_collection(*list_ty));
            stack.push(VType::bare_scalar(Scalar::Bool));
        }
        SealedInstr::DurIndexLookup(_) => {
            if !index.unique {
                return Err(reject(
                    VerifyPhase::Function,
                    "exact lookup of a nonunique index",
                ));
            }
            // Pop the whole projection (one key per component, projection order reversed),
            // then push the optional source identity of the root the index belongs to.
            for scalar in projection.iter().rev() {
                expect(pop(stack)?, VType::bare_scalar(*scalar))?;
            }
            stack.push(VType::Identity {
                root: site_root,
                optional: true,
            });
        }
        SealedInstr::DurIndexExists(_) => {
            if !index.unique {
                return Err(reject(
                    VerifyPhase::Function,
                    "presence probe of a nonunique index",
                ));
            }
            // Pop the whole projection (one key per component, projection order reversed),
            // then push the presence bool — the identity is never materialized.
            for scalar in projection.iter().rev() {
                expect(pop(stack)?, VType::bare_scalar(*scalar))?;
            }
            stack.push(VType::bare_scalar(Scalar::Bool));
        }
        // A whole-entry, field, or group opcode whose forged site operand names an index
        // site is dispatched here by site kind; it is not an index read, so refuse it at the
        // trust boundary rather than reaching the closed-complement of the three index reads.
        _ => {
            return Err(reject(
                VerifyPhase::Function,
                "a non-index opcode over a managed-index site",
            ));
        }
    }
    Ok(Control::Fallthrough)
}

/// Pop and type-check a durable operation's key-path against `key_path` (the key
/// types in stack-pop order — the branch key then the root key for a branch entry
/// site, the single root key otherwise).
fn pop_key_path(
    stack: &mut Vec<VType>,
    key_path: &[VType],
    site_root: u16,
) -> Result<(), VerifyRejection> {
    for ty in key_path {
        pop_key_column(stack, *ty, site_root)?;
    }
    Ok(())
}

/// Pop one key column and type-check it against `want`. A column spread from an entry
/// identity carries its root; it may key this operation only when the identity addresses
/// the *same* root as the site, and its scalar must still match the site's key column.
/// This re-proves the cross-root identity distinction through a local slot, independently
/// of the compiler. One owner for the per-column pop so the whole key-path pop, the
/// bounded-traversal ancestor pop, and the family-probe ancestor pop admit an
/// identity-keyed parent identically rather than through forked checks.
fn pop_key_column(
    stack: &mut Vec<VType>,
    want: VType,
    site_root: u16,
) -> Result<(), VerifyRejection> {
    match pop(stack)? {
        VType::IdentityColumn { root, scalar } => {
            if root != site_root {
                return Err(reject(
                    VerifyPhase::Function,
                    "an entry identity keys a durable operation on a different store root",
                ));
            }
            expect(VType::bare_scalar(scalar), want)
        }
        other => expect(other, want),
    }
}

/// Whether a definitely-initialized key slot of type `have` keys a column of type `want`
/// on `site_root`. A slot captured from an entry identity carries its root as an identity
/// column; it keys the column only when it addresses the same root and its scalar matches.
/// The slot-reading counterpart of [`pop_key_column`] for the strict present-entry set,
/// which reads its key-path from local slots rather than the stack.
fn slot_keys_column(have: VType, want: VType, site_root: u16) -> bool {
    match have {
        VType::IdentityColumn { root, scalar } => {
            root == site_root && VType::bare_scalar(scalar) == want
        }
        other => other == want,
    }
}

/// The field a field-target site addresses, or a rejection when the site is an
/// entry site. A top-level field resolves against the root's record; a branch field
/// against its branch's record, one level down.
fn field_of<'a>(
    ctx: &'a Ctx,
    target: &SealedSiteTarget,
    root: &SealedRoot,
) -> Result<&'a SealedField, VerifyRejection> {
    let (record, field) = match target {
        SealedSiteTarget::FieldLeaf(field) => (root.record, *field),
        SealedSiteTarget::BranchField { branch, field } => {
            (resolve_sealed_branch(root, branch)?.record, *field)
        }
        SealedSiteTarget::WholePayload
        | SealedSiteTarget::BranchEntry(_)
        | SealedSiteTarget::GroupEntry(_)
        | SealedSiteTarget::IndexScan(_)
        | SealedSiteTarget::IndexLookup(_) => {
            return Err(reject(
                VerifyPhase::Function,
                "operation requires a field site",
            ));
        }
    };
    ctx.types[record as usize]
        .fields()
        .get(field as usize)
        .ok_or(reject(
            VerifyPhase::Function,
            "site field index out of range",
        ))
}

/// The bare value type of a durable field: a scalar, a record (dense product), or an
/// enum/`Option`/`Result` (a sum) — the storable durable value set. A collection or unit
/// is never an admitted durable field type, so `from_image` always resolves here.
fn durable_field_vtype(field: &SealedField) -> VType {
    VType::from_image(field.ty).expect("a durable field type is scalar, record, or enum")
}

/// Require an entry-target site: the root's whole payload or a keyed branch entry. A
/// field-leaf, group, or index site is not an entry.
fn require_entry(target: &SealedSiteTarget) -> Result<(), VerifyRejection> {
    match target {
        SealedSiteTarget::WholePayload | SealedSiteTarget::BranchEntry(_) => Ok(()),
        SealedSiteTarget::FieldLeaf(_)
        | SealedSiteTarget::BranchField { .. }
        | SealedSiteTarget::GroupEntry(_)
        | SealedSiteTarget::IndexScan(_)
        | SealedSiteTarget::IndexLookup(_) => Err(reject(
            VerifyPhase::Function,
            "operation requires an entry site",
        )),
    }
}

/// Require a group-target site: the whole materialized value of one unkeyed group. A
/// whole-payload, field, branch, or index site is refused, so a group opcode can only
/// address a `GroupEntry` site (the trust backstop over the compiler's own boundary).
fn require_group(target: &SealedSiteTarget) -> Result<(), VerifyRejection> {
    match target {
        SealedSiteTarget::GroupEntry(_) => Ok(()),
        SealedSiteTarget::WholePayload
        | SealedSiteTarget::FieldLeaf(_)
        | SealedSiteTarget::BranchEntry(_)
        | SealedSiteTarget::BranchField { .. }
        | SealedSiteTarget::IndexScan(_)
        | SealedSiteTarget::IndexLookup(_) => Err(reject(
            VerifyPhase::Function,
            "operation requires a group site",
        )),
    }
}

/// Walk a branch path through the recursive sealed branch tree, returning the deepest
/// addressed branch. Refuses a path element out of range at any level, so a forged image
/// naming a branch index past the sealed tree is rejected here — the trust backstop over
/// the verifier's own site resolution. An empty path is not a branch site.
fn resolve_sealed_branch<'a>(
    root: &'a SealedRoot,
    path: &[u16],
) -> Result<&'a SealedBranch, VerifyRejection> {
    let mut branches: &'a [SealedBranch] = &root.branches;
    let mut deepest: Option<&'a SealedBranch> = None;
    for &index in path {
        let branch = branches.get(index as usize).ok_or(reject(
            VerifyPhase::Function,
            "durable branch index out of range",
        ))?;
        deepest = Some(branch);
        branches = &branch.branches;
    }
    deepest.ok_or(reject(
        VerifyPhase::Function,
        "a branch site requires a non-empty branch path",
    ))
}

/// The branch chain's key columns, root-first and flattened across hops (each hop
/// contributes its whole ordered key tuple), along a branch path. Refuses a path element
/// out of range at any level, the same forged-image backstop as [`resolve_sealed_branch`].
pub(super) fn branch_key_columns(
    root: &SealedRoot,
    path: &[u16],
) -> Result<Vec<Scalar>, VerifyRejection> {
    let mut branches = &root.branches;
    let mut columns = Vec::with_capacity(path.len());
    for &index in path {
        let branch = branches.get(index as usize).ok_or(reject(
            VerifyPhase::Function,
            "durable branch index out of range",
        ))?;
        columns.extend_from_slice(&branch.keys);
        branches = &branch.branches;
    }
    Ok(columns)
}

/// Require `value` to be exactly `want`.
pub(super) fn expect(value: VType, want: VType) -> Result<(), VerifyRejection> {
    if value == want {
        Ok(())
    } else {
        Err(reject(
            VerifyPhase::Function,
            "durable operand type mismatch",
        ))
    }
}

/// Pop the top operand, rejecting an empty stack (a verifier-internal shape error).
pub(super) fn pop(stack: &mut Vec<VType>) -> Result<VType, VerifyRejection> {
    stack
        .pop()
        .ok_or(reject(VerifyPhase::Function, "operand stack underflow"))
}

/// Require `value` to be a bare scalar of `scalar`.
fn expect_scalar(value: VType, scalar: Scalar) -> Result<(), VerifyRejection> {
    if value == VType::bare_scalar(scalar) {
        Ok(())
    } else {
        Err(reject(
            VerifyPhase::Function,
            "operand type mismatch for opcode",
        ))
    }
}

/// Pop two bare `operand`-typed scalars (right then left), push a bare `result`, and
/// fall through.
fn binary(
    stack: &mut Vec<VType>,
    operand: Scalar,
    result: Scalar,
) -> Result<Control, VerifyRejection> {
    let right = pop(stack)?;
    let left = pop(stack)?;
    expect_scalar(right, operand)?;
    expect_scalar(left, operand)?;
    stack.push(VType::bare_scalar(result));
    Ok(Control::Fallthrough)
}

fn const_scalar(value: &SealedConst) -> Scalar {
    match value {
        SealedConst::Int(_) => Scalar::Int,
        SealedConst::Bool(_) => Scalar::Bool,
        SealedConst::Text(_) => Scalar::Text,
        SealedConst::Date(_) => Scalar::Date,
        SealedConst::Instant(_) => Scalar::Instant,
        SealedConst::Duration(_) => Scalar::Duration,
    }
}
