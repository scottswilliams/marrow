//! The stack VM execution loop.
//!
//! Runs one export over its sealed instruction tape. Because the image is
//! verified, the tape is well-typed and bounded: the loop can rely on operand
//! shapes the verifier already proved and only guards the *dynamic* faults the
//! verifier cannot rule out (overflow, budgets). The instruction set grows one
//! slice at a time alongside the verifier and compiler.

use std::rc::Rc;

use marrow_codes::Code;
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::RuntimeScalar;
use marrow_kernel::durable::{CommitResult, Durable, EntryValue, NextKey, Presence};
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

/// Run a storeless function at `func_index` with `args`, returning its value (or
/// `None` for a Unit return) or a source-mapped runtime fault. Rejected for a
/// durable export (its demand is nonempty); use [`run_durable`].
pub fn run(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
) -> Result<Option<Value>, RuntimeFault> {
    let mut budget = INSTRUCTION_BUDGET;
    execute(image, func_index, args, 0, &mut budget, None)
}

/// Run a durable function at `func_index`, driving `session` for every durable
/// operation. The session is a read session for a read-only export and a
/// transaction session for a mutating one.
pub fn run_durable(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
    session: &mut dyn Durable,
) -> Result<Option<Value>, RuntimeFault> {
    let mut budget = INSTRUCTION_BUDGET;
    execute(image, func_index, args, 0, &mut budget, Some(session))
}

/// Execute one frame. `depth` is the current call depth and `budget` is shared
/// across the whole call tree so total work stays bounded. `session` drives durable
/// operations; it is `None` for a storeless call tree.
fn execute<'s>(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
    depth: u32,
    budget: &mut u64,
    mut session: Option<&mut (dyn Durable + 's)>,
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
                if let Some(value) = execute(
                    image,
                    *target,
                    call_args,
                    depth + 1,
                    budget,
                    session.as_deref_mut(),
                )? {
                    stack.push(value);
                }
                pc += 1;
            }
            SealedInstr::TxnBegin => {
                // The session's engine transaction is already open; Begin is the
                // verifier's flow marker, a runtime no-op.
                pc += 1;
            }
            SealedInstr::TxnCommit => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a mutating export runs with a session");
                match durable.commit() {
                    CommitResult::Committed => pc += 1,
                    CommitResult::RequiredMissing { .. } => {
                        return Err(fault(function, pc, Code::RunRequiredMissing.as_str()));
                    }
                    CommitResult::CommitFault => {
                        return Err(fault(function, pc, Code::RunCommit.as_str()));
                    }
                }
            }
            SealedInstr::DurExists(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let key = pop_key(&mut stack);
                let present = durable
                    .presence(&authorized, key)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                stack.push(Value::Bool(present == Presence::Present));
                pc += 1;
            }
            SealedInstr::DurReadField(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let key = pop_key(&mut stack);
                let value = durable
                    .read_field(&authorized, key)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                stack.push(Value::Optional(value.map(|s| Box::new(scalar_to_value(s)))));
                pc += 1;
            }
            SealedInstr::DurReadEntry(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let key = pop_key(&mut stack);
                let entry = durable
                    .read_entry(&authorized, key)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                let ty = entry_record_type(image, *site);
                stack.push(Value::Optional(
                    entry.map(|entry| Box::new(entry_to_record(ty, entry))),
                ));
                pc += 1;
            }
            SealedInstr::DurSetRequired(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let value = value_to_scalar(pop(&mut stack));
                let key = pop_key(&mut stack);
                durable
                    .set_required(&authorized, key, value)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurSetSparse(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let value = as_optional(pop(&mut stack)).map(value_to_scalar);
                let key = pop_key(&mut stack);
                durable
                    .set_sparse(&authorized, key, value)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurCreateEntry(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let entry = record_to_entry(pop(&mut stack));
                let key = pop_key(&mut stack);
                durable
                    .create_entry(&authorized, key, entry)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurReplaceEntry(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let entry = record_to_entry(pop(&mut stack));
                let key = pop_key(&mut stack);
                durable
                    .replace_entry(&authorized, key, entry)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurEraseField(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let key = pop_key(&mut stack);
                durable
                    .erase_field(&authorized, key)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurEraseEntry(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let key = pop_key(&mut stack);
                durable
                    .erase_entry(&authorized, key)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurNextKey(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let after = as_optional(pop(&mut stack)).map(value_to_key);
                let next = durable
                    .next_key(&authorized, after)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                stack.push(Value::Optional(match next {
                    NextKey::Next(key) => Some(Box::new(key_to_value(key))),
                    NextKey::End => None,
                }));
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

/// Map a kernel fault to a source-mapped runtime fault at `pc`.
fn kernel_fault(
    function: &SealedFunction,
    pc: usize,
    kernel: &marrow_kernel::durable::KernelFault,
) -> RuntimeFault {
    fault(function, pc, kernel.code())
}

/// Pop a key operand and convert it to a typed key scalar.
fn pop_key(stack: &mut Vec<Value>) -> KeyScalar {
    value_to_key(pop(stack))
}

/// Convert a scalar runtime value to a typed key scalar (verifier-proved scalar).
fn value_to_key(value: Value) -> KeyScalar {
    match value {
        Value::Int(v) => KeyScalar::Int(v),
        Value::Bool(v) => KeyScalar::Bool(v),
        Value::Text(v) => KeyScalar::Str(v.to_string()),
        _ => unreachable!("verifier proved a scalar key operand"),
    }
}

/// Convert a key scalar back to a runtime value.
fn key_to_value(key: KeyScalar) -> Value {
    match key {
        KeyScalar::Int(v) => Value::Int(v),
        KeyScalar::Bool(v) => Value::Bool(v),
        KeyScalar::Str(v) => Value::Text(v.into()),
        _ => unreachable!("T01 keys are int, string, or bool"),
    }
}

/// Convert a scalar runtime value to a typed runtime scalar.
fn value_to_scalar(value: Value) -> RuntimeScalar {
    match value {
        Value::Int(v) => RuntimeScalar::Int(v),
        Value::Bool(v) => RuntimeScalar::Bool(v),
        Value::Text(v) => RuntimeScalar::Str(v.to_string()),
        _ => unreachable!("verifier proved a scalar value operand"),
    }
}

/// Convert a typed runtime scalar back to a runtime value.
fn scalar_to_value(scalar: RuntimeScalar) -> Value {
    match scalar {
        RuntimeScalar::Int(v) => Value::Int(v),
        RuntimeScalar::Bool(v) => Value::Bool(v),
        RuntimeScalar::Str(v) => Value::Text(v.into()),
        _ => unreachable!("T01 durable values are int, bool, or string"),
    }
}

/// Convert a record value into a whole-entry value: one slot per field in order.
fn record_to_entry(value: Value) -> EntryValue {
    let (_, slots) = as_record(value);
    EntryValue {
        fields: slots
            .into_vec()
            .into_iter()
            .map(|slot| slot.map(value_to_scalar))
            .collect(),
    }
}

/// Convert a whole-entry value into a record value of type `ty`.
fn entry_to_record(ty: u16, entry: EntryValue) -> Value {
    let slots: Vec<Option<Value>> = entry
        .fields
        .into_iter()
        .map(|slot| slot.map(scalar_to_value))
        .collect();
    Value::Record(ty, slots.into_boxed_slice())
}

/// The record type index of the entry a site's root addresses.
fn entry_record_type(image: &VerifiedImage, site: u16) -> u16 {
    let root = image.sites()[site as usize].root;
    image.roots()[root as usize].record()
}
