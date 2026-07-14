//! The stack VM execution loop.
//!
//! Runs one export over its sealed instruction tape. Because the image is
//! verified, the tape is well-typed and bounded: the loop can rely on operand
//! shapes the verifier already proved and only guards the *dynamic* faults the
//! verifier cannot rule out (overflow, budgets). The instruction set grows one
//! slice at a time alongside the verifier and compiler.

use std::rc::Rc;

use marrow_codes::Code;
use marrow_verify::{SealedConst, SealedFunction, SealedInstr, VerifiedImage};

use crate::fault::RuntimeFault;
use crate::value::Value;

/// Per-invocation instruction budget (design §D). Bounds total work across the
/// whole call tree regardless of loop or call structure.
const INSTRUCTION_BUDGET: u64 = 1 << 26;

/// Maximum dynamic call depth (design §D). Static recursion is already rejected at
/// verify, so this guards a pathologically deep non-recursive chain.
const MAX_CALL_DEPTH: u32 = 64;

/// Text-concatenation result ceiling (design §D). A private VM runtime bound: the
/// VM has no edge to the image crate, so it owns this limit itself.
const MAX_TEXT_BYTES: usize = 64 * 1024;

/// Run the function at `func_index` with `args`, returning its value (or `None`
/// for a Unit return) or a source-mapped runtime fault.
pub fn run(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
) -> Result<Option<Value>, RuntimeFault> {
    let mut budget = INSTRUCTION_BUDGET;
    execute(image, func_index, args, 0, &mut budget)
}

/// Execute one frame. `depth` is the current call depth and `budget` is shared
/// across the whole call tree so total work stays bounded.
fn execute(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
    depth: u32,
    budget: &mut u64,
) -> Result<Option<Value>, RuntimeFault> {
    let function = image.function(func_index);
    let mut locals: Vec<Option<Value>> = Vec::with_capacity(function.local_count() as usize);
    for arg in args {
        locals.push(Some(arg));
    }
    locals.resize(function.local_count() as usize, None);

    let mut stack: Vec<Value> = Vec::with_capacity(function.max_stack());
    let mut pc = 0usize;

    loop {
        if *budget == 0 {
            return Err(fault(function, pc, Code::RunBudget.as_str()));
        }
        *budget -= 1;

        match &function.instrs()[pc] {
            SealedInstr::ConstLoad(idx) => {
                stack.push(const_value(&image.consts()[*idx as usize]));
                pc += 1;
            }
            SealedInstr::LocalGet(slot) => {
                stack.push(
                    locals[*slot as usize]
                        .clone()
                        .expect("verifier proved definite init"),
                );
                pc += 1;
            }
            SealedInstr::LocalSet(slot) => {
                locals[*slot as usize] = Some(pop(&mut stack));
                pc += 1;
            }
            SealedInstr::Pop => {
                pop(&mut stack);
                pc += 1;
            }
            SealedInstr::Return => {
                return Ok(stack.pop());
            }
            SealedInstr::Jump(target) => {
                pc = *target;
            }
            SealedInstr::JumpIfFalse(target) => {
                if as_bool(pop(&mut stack)) {
                    pc += 1;
                } else {
                    pc = *target;
                }
            }
            SealedInstr::IntAdd => {
                let (a, b) = pop_ints(&mut stack);
                match a.checked_add(b) {
                    Some(v) => stack.push(Value::Int(v)),
                    None => return Err(fault(function, pc, Code::RunOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::IntSub => {
                let (a, b) = pop_ints(&mut stack);
                match a.checked_sub(b) {
                    Some(v) => stack.push(Value::Int(v)),
                    None => return Err(fault(function, pc, Code::RunOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::IntMul => {
                let (a, b) = pop_ints(&mut stack);
                match a.checked_mul(b) {
                    Some(v) => stack.push(Value::Int(v)),
                    None => return Err(fault(function, pc, Code::RunOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::IntRem => {
                let (a, b) = pop_ints(&mut stack);
                if b == 0 {
                    return Err(fault(function, pc, Code::RunDivideByZero.as_str()));
                }
                // `i64::MIN % -1` overflows the checked remainder (the quotient is
                // unrepresentable), so it faults as overflow rather than panicking.
                match a.checked_rem(b) {
                    Some(v) => stack.push(Value::Int(v)),
                    None => return Err(fault(function, pc, Code::RunOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::IntNeg => {
                let a = pop_int(&mut stack);
                match a.checked_neg() {
                    Some(v) => stack.push(Value::Int(v)),
                    None => return Err(fault(function, pc, Code::RunOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::BoolNot => {
                let a = as_bool(pop(&mut stack));
                stack.push(Value::Bool(!a));
                pc += 1;
            }
            SealedInstr::IntLt => {
                let (a, b) = pop_ints(&mut stack);
                stack.push(Value::Bool(a < b));
                pc += 1;
            }
            SealedInstr::IntLe => {
                let (a, b) = pop_ints(&mut stack);
                stack.push(Value::Bool(a <= b));
                pc += 1;
            }
            SealedInstr::IntGt => {
                let (a, b) = pop_ints(&mut stack);
                stack.push(Value::Bool(a > b));
                pc += 1;
            }
            SealedInstr::IntGe => {
                let (a, b) = pop_ints(&mut stack);
                stack.push(Value::Bool(a >= b));
                pc += 1;
            }
            SealedInstr::EqInt => {
                let (a, b) = pop_ints(&mut stack);
                stack.push(Value::Bool(a == b));
                pc += 1;
            }
            SealedInstr::EqBool => {
                let b = as_bool(pop(&mut stack));
                let a = as_bool(pop(&mut stack));
                stack.push(Value::Bool(a == b));
                pc += 1;
            }
            SealedInstr::EqText => {
                let b = as_text(pop(&mut stack));
                let a = as_text(pop(&mut stack));
                stack.push(Value::Bool(a == b));
                pc += 1;
            }
            SealedInstr::TextConcat => {
                let b = as_text(pop(&mut stack));
                let a = as_text(pop(&mut stack));
                if a.len() + b.len() > MAX_TEXT_BYTES {
                    return Err(fault(function, pc, Code::RunTextLimit.as_str()));
                }
                let mut joined = String::with_capacity(a.len() + b.len());
                joined.push_str(&a);
                joined.push_str(&b);
                stack.push(Value::Text(joined.into()));
                pc += 1;
            }
            SealedInstr::RecordNew(ty) => {
                let fields = image.record_type(*ty).fields();
                // f0 was pushed first, so the popped values fill slots in reverse.
                let mut slots: Vec<Option<Value>> = vec![None; fields.len()];
                for (index, field) in fields.iter().enumerate().rev() {
                    let value = pop(&mut stack);
                    slots[index] = if field.required {
                        Some(value)
                    } else {
                        as_optional(value)
                    };
                }
                stack.push(Value::Record(*ty, slots.into_boxed_slice()));
                pc += 1;
            }
            SealedInstr::FieldGet(field) => {
                let (ty, slots) = as_record(pop(&mut stack));
                let cell = slots[*field as usize].clone();
                let required = image.record_type(ty).fields()[*field as usize].required;
                if required {
                    stack.push(cell.expect("verifier proved a required field is present"));
                } else {
                    stack.push(Value::Optional(cell.map(Box::new)));
                }
                pc += 1;
            }
            SealedInstr::SomeWrap => {
                let value = pop(&mut stack);
                stack.push(Value::Optional(Some(Box::new(value))));
                pc += 1;
            }
            SealedInstr::VacantLoad(_) => {
                stack.push(Value::Optional(None));
                pc += 1;
            }
            SealedInstr::BranchPresent(target) => match as_optional(pop(&mut stack)) {
                Some(inner) => {
                    stack.push(inner);
                    pc += 1;
                }
                None => {
                    pc = *target;
                }
            },
            SealedInstr::Call(target) => {
                if depth + 1 > MAX_CALL_DEPTH {
                    return Err(fault(function, pc, Code::RunCallDepth.as_str()));
                }
                let arg_count = image.function(*target).params().len();
                let start = stack.len() - arg_count;
                // a0 was pushed first, so the tail of the stack is a0..an-1 in order.
                let call_args = stack.split_off(start);
                if let Some(value) = execute(image, *target, call_args, depth + 1, budget)? {
                    stack.push(value);
                }
                pc += 1;
            }
        }
    }
}

/// Pop the top value. The verifier proved the stack is non-empty here.
fn pop(stack: &mut Vec<Value>) -> Value {
    stack.pop().expect("verifier proved operand present")
}

/// Pop two ints (right operand first, then left): `…, a, b →`.
fn pop_ints(stack: &mut Vec<Value>) -> (i64, i64) {
    let b = pop_int(stack);
    let a = pop_int(stack);
    (a, b)
}

fn pop_int(stack: &mut Vec<Value>) -> i64 {
    match pop(stack) {
        Value::Int(v) => v,
        _ => unreachable!("verifier proved an int operand"),
    }
}

fn as_bool(value: Value) -> bool {
    match value {
        Value::Bool(v) => v,
        _ => unreachable!("verifier proved a bool operand"),
    }
}

fn as_text(value: Value) -> Rc<str> {
    match value {
        Value::Text(v) => v,
        _ => unreachable!("verifier proved a text operand"),
    }
}

/// Unwrap an optional value to its inner `Option`.
fn as_optional(value: Value) -> Option<Value> {
    match value {
        Value::Optional(inner) => inner.map(|boxed| *boxed),
        _ => unreachable!("verifier proved an optional operand"),
    }
}

fn as_record(value: Value) -> (u16, Box<[Option<Value>]>) {
    match value {
        Value::Record(ty, slots) => (ty, slots),
        _ => unreachable!("verifier proved a record operand"),
    }
}

fn const_value(value: &SealedConst) -> Value {
    match value {
        SealedConst::Int(v) => Value::Int(*v),
        SealedConst::Bool(v) => Value::Bool(*v),
        SealedConst::Text(v) => Value::Text(v.clone()),
    }
}

/// Build a runtime fault at the source position mapped to `pc`. Every instruction
/// has a span mapping, so a location always exists; a missing one is a verifier
/// invariant breach and defaults to the function's own start.
fn fault(function: &SealedFunction, pc: usize, code: &'static str) -> RuntimeFault {
    let (line, column) = function.span_at(pc).unwrap_or((1, 1));
    RuntimeFault::new(code, line, column)
}
