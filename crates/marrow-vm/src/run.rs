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
use marrow_kernel::durable::{BoundedLimit, CommitResult, Durable, EntryValue, Presence};
use marrow_kernel::equality::ValueDomain;
use marrow_verify::{
    SealedConst, SealedFunction, SealedInstr, SealedSite, SealedSiteTarget, VerifiedImage,
};

use crate::fault::RuntimeFault;
use crate::value::{Value, key_bytes, list_bytes};

// The VM is the sole owner of one invocation's dynamic limits. They are private
// constants: `run`/`run_durable` take no budget or limit parameter, so no runner,
// CLI, environment variable, or caller can raise or disable them. The verifier
// owns the complementary static bounds (stack depth, locals, code size).

/// Per-invocation instruction budget (design §D). Bounds total work across the
/// whole call tree regardless of loop or call structure, so a non-terminating loop
/// faults with `run.budget` rather than running forever.
const INSTRUCTION_BUDGET: u64 = 1 << 26;

/// Maximum dynamic call depth (design §D). Static recursion is already rejected at
/// verify, so this guards a pathologically deep non-recursive chain.
const MAX_CALL_DEPTH: u32 = 64;

/// Text-concatenation result ceiling (design §D). A private VM runtime bound: the
/// VM has no edge to the image crate, so it owns this limit itself.
const MAX_TEXT_BYTES: usize = 64 * 1024;

/// Law-9 collection bounds (design §D collections). A `List`/`Map` may hold at most
/// `MAX_COLLECTION_LEN` elements and measure at most `MAX_AGGREGATE_BYTES` in total
/// value size; an append or insert that would exceed either faults
/// `run.collection_limit` rather than allocating unboundedly. Private VM constants:
/// no runner, CLI, or caller can raise them.
const MAX_COLLECTION_LEN: usize = 65_536;
const MAX_AGGREGATE_BYTES: usize = 1 << 20;

/// Run a storeless function at `func_index` with `args`, returning its value (or
/// `None` for a Unit return) or a source-mapped runtime fault. Rejected for a
/// durable export (its demand is nonempty); use [`run_durable`].
pub fn run(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
) -> Result<Option<Value>, RuntimeFault> {
    let mut budget = INSTRUCTION_BUDGET;
    execute(image, func_index, args, 0, &mut budget, None, None)
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
    execute(image, func_index, args, 0, &mut budget, Some(session), None)
}

/// A test-body driver: it turns each durable-touching call the driver frame makes
/// into its own invocation boundary — opening the session that call's demand
/// requires, running it, committing or rolling it back, and closing it — exactly as
/// a terminal invocation does. The driver frame itself performs no direct durable
/// operation (the test-entry phase refuses a body that mixes the two), so it drives
/// only through calls.
pub(crate) trait DriverDispatch {
    fn invoke(
        &mut self,
        func: u16,
        args: Vec<Value>,
        depth: u32,
        budget: &mut u64,
    ) -> Result<Option<Value>, RuntimeFault>;
}

/// Run `func_index` as a test-body driver frame: it holds no session of its own and
/// dispatches each call through `driver`, which opens one session per invocation.
pub(crate) fn run_driver(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
    driver: &mut dyn DriverDispatch,
) -> Result<Option<Value>, RuntimeFault> {
    let mut budget = INSTRUCTION_BUDGET;
    execute(image, func_index, args, 0, &mut budget, None, Some(driver))
}

/// Run `func_index` at `depth` sharing `budget`, driving `session` for its durable
/// operations (or `None` for a storeless callee). The driver uses this to run one
/// dispatched invocation inside the session it opened, without a nested driver.
pub(crate) fn run_in_session(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
    depth: u32,
    budget: &mut u64,
    session: Option<&mut dyn Durable>,
) -> Result<Option<Value>, RuntimeFault> {
    execute(image, func_index, args, depth, budget, session, None)
}

/// Execute one frame. `depth` is the current call depth and `budget` is shared
/// across the whole call tree so total work stays bounded. `session` drives durable
/// operations; it is `None` for a storeless call tree. `driver` is present only for a
/// test-body driver frame, where each call becomes its own session-scoped invocation.
fn execute<'s>(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
    depth: u32,
    budget: &mut u64,
    mut session: Option<&mut (dyn Durable + 's)>,
    mut driver: Option<&mut dyn DriverDispatch>,
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
            SealedInstr::IntDiv => {
                let (a, b) = pop_ints(&mut stack);
                if b == 0 {
                    return Err(fault(function, pc, Code::RunDivideByZero.as_str()));
                }
                // Truncating division toward zero, paired with the truncating `%`
                // remainder so `a == (a / b) * b + a % b`. `i64::MIN / -1` has an
                // unrepresentable quotient, so it faults as overflow.
                match a.checked_div(b) {
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
            SealedInstr::TextLt
            | SealedInstr::TextLe
            | SealedInstr::TextGt
            | SealedInstr::TextGe => {
                let b = as_text(pop(&mut stack));
                let a = as_text(pop(&mut stack));
                // Strings order lexicographically by their UTF-8 bytes.
                let ordering = a.as_bytes().cmp(b.as_bytes());
                let result = match &function.instrs()[pc] {
                    SealedInstr::TextLt => ordering.is_lt(),
                    SealedInstr::TextLe => ordering.is_le(),
                    SealedInstr::TextGt => ordering.is_gt(),
                    _ => ordering.is_ge(),
                };
                stack.push(Value::Bool(result));
                pc += 1;
            }
            SealedInstr::EqBytes => {
                let b = as_bytes(pop(&mut stack));
                let a = as_bytes(pop(&mut stack));
                stack.push(Value::Bool(a == b));
                pc += 1;
            }
            SealedInstr::BytesLt
            | SealedInstr::BytesLe
            | SealedInstr::BytesGt
            | SealedInstr::BytesGe => {
                let b = as_bytes(pop(&mut stack));
                let a = as_bytes(pop(&mut stack));
                // Bytes order lexicographically, like the durable byte-key order.
                let ordering = a.as_ref().cmp(b.as_ref());
                let result = match &function.instrs()[pc] {
                    SealedInstr::BytesLt => ordering.is_lt(),
                    SealedInstr::BytesLe => ordering.is_le(),
                    SealedInstr::BytesGt => ordering.is_gt(),
                    _ => ordering.is_ge(),
                };
                stack.push(Value::Bool(result));
                pc += 1;
            }
            // Temporal comparison and equality. Each temporal type is a signed
            // integer domain (days for a date, nanoseconds for an instant or
            // duration), so the language order is the integer order — which agrees
            // with the kernel key-codec byte order (pinned in
            // `tests/temporal_order_agreement.rs`).
            SealedInstr::EqDate
            | SealedInstr::DateLt
            | SealedInstr::DateLe
            | SealedInstr::DateGt
            | SealedInstr::DateGe => {
                let b = pop_date(&mut stack);
                let a = pop_date(&mut stack);
                stack.push(Value::Bool(temporal_compare(
                    &function.instrs()[pc],
                    a.cmp(&b),
                )));
                pc += 1;
            }
            SealedInstr::EqInstant
            | SealedInstr::InstantLt
            | SealedInstr::InstantLe
            | SealedInstr::InstantGt
            | SealedInstr::InstantGe => {
                let b = pop_instant(&mut stack);
                let a = pop_instant(&mut stack);
                stack.push(Value::Bool(temporal_compare(
                    &function.instrs()[pc],
                    a.cmp(&b),
                )));
                pc += 1;
            }
            SealedInstr::EqDuration
            | SealedInstr::DurationLt
            | SealedInstr::DurationLe
            | SealedInstr::DurationGt
            | SealedInstr::DurationGe => {
                let b = pop_duration(&mut stack);
                let a = pop_duration(&mut stack);
                stack.push(Value::Bool(temporal_compare(
                    &function.instrs()[pc],
                    a.cmp(&b),
                )));
                pc += 1;
            }
            // The closed temporal arithmetic floor. Each faults
            // `run.temporal_overflow` when its result leaves the supported domain;
            // `marrow-temporal` owns the checked operations.
            SealedInstr::DateAddDays => {
                let days = pop_int(&mut stack);
                let date = pop_date(&mut stack);
                match marrow_temporal::add_days(date, days) {
                    Some(result) => stack.push(Value::Date(result)),
                    None => return Err(fault(function, pc, Code::RunTemporalOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::DateDaysBetween => {
                let to = pop_date(&mut stack);
                let from = pop_date(&mut stack);
                stack.push(Value::Int(marrow_temporal::days_between(from, to)));
                pc += 1;
            }
            SealedInstr::DurationAdd => {
                let b = pop_duration(&mut stack);
                let a = pop_duration(&mut stack);
                match marrow_temporal::duration_add(a, b) {
                    Some(result) => stack.push(Value::Duration(result)),
                    None => return Err(fault(function, pc, Code::RunTemporalOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::DurationSub => {
                let b = pop_duration(&mut stack);
                let a = pop_duration(&mut stack);
                match marrow_temporal::duration_sub(a, b) {
                    Some(result) => stack.push(Value::Duration(result)),
                    None => return Err(fault(function, pc, Code::RunTemporalOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::InstantAddDuration => {
                let duration = pop_duration(&mut stack);
                let instant = pop_instant(&mut stack);
                match marrow_temporal::instant_add_duration(instant, duration) {
                    Some(result) => stack.push(Value::Instant(result)),
                    None => return Err(fault(function, pc, Code::RunTemporalOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::InstantSubDuration => {
                let duration = pop_duration(&mut stack);
                let instant = pop_instant(&mut stack);
                match marrow_temporal::instant_sub_duration(instant, duration) {
                    Some(result) => stack.push(Value::Instant(result)),
                    None => return Err(fault(function, pc, Code::RunTemporalOverflow.as_str())),
                }
                pc += 1;
            }
            SealedInstr::ConvString => {
                let value = pop(&mut stack);
                let text = crate::render::value_text(&value, image.record_types(), image.enums());
                stack.push(Value::Text(text.into()));
                pc += 1;
            }
            SealedInstr::ConvBytesText => {
                let text = as_text(pop(&mut stack));
                stack.push(Value::Bytes(Rc::from(text.as_bytes())));
                pc += 1;
            }
            // Checked arithmetic: on overflow, transfer to the fault handler instead
            // of raising `run.*`. A zero divisor is handled by a compiler-emitted
            // branch before the op, so these guard only the representable-result case.
            SealedInstr::IntAddChecked(target) => {
                let (a, b) = pop_ints(&mut stack);
                checked_or_branch(&mut stack, a.checked_add(b), *target, &mut pc);
            }
            SealedInstr::IntSubChecked(target) => {
                let (a, b) = pop_ints(&mut stack);
                checked_or_branch(&mut stack, a.checked_sub(b), *target, &mut pc);
            }
            SealedInstr::IntMulChecked(target) => {
                let (a, b) = pop_ints(&mut stack);
                checked_or_branch(&mut stack, a.checked_mul(b), *target, &mut pc);
            }
            SealedInstr::IntNegChecked(target) => {
                let a = pop_int(&mut stack);
                checked_or_branch(&mut stack, a.checked_neg(), *target, &mut pc);
            }
            SealedInstr::IntDivChecked(target) => {
                let (a, b) = pop_ints(&mut stack);
                // Compiler output routes a zero divisor to the `zero_divisor` arm
                // before this op, so here `checked_div` fails only on i64::MIN / -1.
                // `checked_div` (not `/`) is load-bearing regardless: it returns `None`
                // for a zero or overflowing divisor, so it never panics even on a
                // hand-built image that omits the branch.
                checked_or_branch(&mut stack, a.checked_div(b), *target, &mut pc);
            }
            SealedInstr::IntRemChecked(target) => {
                // `checked_rem` returns `None` for a zero or i64::MIN / -1 divisor, so
                // it never panics; keep it rather than `%`.
                let (a, b) = pop_ints(&mut stack);
                checked_or_branch(&mut stack, a.checked_rem(b), *target, &mut pc);
            }
            SealedInstr::RangeGuard { lo, hi } => {
                // Peek the guarded value: the verifier proved a bare int on top.
                let value = match stack.last() {
                    Some(Value::Int(value)) => *value,
                    _ => unreachable!("verifier proved an int under a range guard"),
                };
                if value < *lo || value > *hi {
                    return Err(fault(function, pc, Code::RunRange.as_str()));
                }
                pc += 1;
            }
            SealedInstr::TextIsEmpty => {
                let s = as_text(pop(&mut stack));
                stack.push(Value::Bool(s.is_empty()));
                pc += 1;
            }
            SealedInstr::TextContains => {
                let needle = as_text(pop(&mut stack));
                let haystack = as_text(pop(&mut stack));
                stack.push(Value::Bool(haystack.contains(needle.as_ref())));
                pc += 1;
            }
            SealedInstr::TextTrim => {
                // Trim leading/trailing Unicode whitespace; the result never grows,
                // so it needs no length bound.
                let s = as_text(pop(&mut stack));
                stack.push(Value::Text(Rc::from(s.trim())));
                pc += 1;
            }
            SealedInstr::TextSplit(idx) => {
                // `split(text, sep)`: the substrings of `text` separated by each
                // non-overlapping occurrence of `sep`, in order. An empty separator
                // yields the single-element list `[text]` rather than splitting
                // between every character.
                let sep = as_text(pop(&mut stack));
                let text = as_text(pop(&mut stack));
                let pieces: Vec<Value> = if sep.is_empty() {
                    vec![Value::Text(text)]
                } else {
                    text.split(sep.as_ref())
                        .map(|piece| Value::Text(Rc::from(piece)))
                        .collect()
                };
                let items = bounded_list(pieces)
                    .ok_or_else(|| fault(function, pc, Code::RunCollectionLimit.as_str()))?;
                stack.push(Value::list(*idx, Rc::new(items)));
                pc += 1;
            }
            SealedInstr::TextLines(idx) => {
                // `lines(text)`: the lines of `text`, split on line feeds with a
                // carriage return before a line feed removed; a final line
                // terminator does not produce a trailing empty line (Rust
                // `str::lines` semantics).
                let text = as_text(pop(&mut stack));
                let pieces: Vec<Value> = text
                    .lines()
                    .map(|line| Value::Text(Rc::from(line)))
                    .collect();
                let items = bounded_list(pieces)
                    .ok_or_else(|| fault(function, pc, Code::RunCollectionLimit.as_str()))?;
                stack.push(Value::list(*idx, Rc::new(items)));
                pc += 1;
            }
            SealedInstr::TextJoin => {
                // `join(list, sep)`: concatenate the list's text elements in order,
                // inserting `sep` between adjacent elements, faulting `run.text_limit`
                // on a concatenation-ceiling excess.
                let sep = as_text(pop(&mut stack));
                let (_, items) = as_list(pop(&mut stack));
                let mut joined = String::new();
                for (position, item) in items.iter().enumerate() {
                    let piece = match item {
                        Value::Text(text) => text,
                        _ => unreachable!("verifier proved a list of string"),
                    };
                    if position > 0 {
                        if joined.len() + sep.len() > MAX_TEXT_BYTES {
                            return Err(fault(function, pc, Code::RunTextLimit.as_str()));
                        }
                        joined.push_str(&sep);
                    }
                    if joined.len() + piece.len() > MAX_TEXT_BYTES {
                        return Err(fault(function, pc, Code::RunTextLimit.as_str()));
                    }
                    joined.push_str(piece);
                }
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
            SealedInstr::FieldSet(field) => {
                // `[record, value] → [record']`: store the bare value present.
                let value = pop(&mut stack);
                let (ty, mut slots) = as_record(pop(&mut stack));
                slots[*field as usize] = Some(value);
                stack.push(Value::Record(ty, slots));
                pc += 1;
            }
            SealedInstr::FieldUnset(field) => {
                // `[record] → [record']`: clear the sparse field to vacant.
                let (ty, mut slots) = as_record(pop(&mut stack));
                slots[*field as usize] = None;
                stack.push(Value::Record(ty, slots));
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
            SealedInstr::EnumConstruct { enum_idx, variant } => {
                let arity = image.enums()[*enum_idx as usize].variants()[*variant as usize]
                    .payload
                    .len();
                // p0 was pushed first, so the popped values fill slots in reverse.
                let mut payload: Vec<Value> = vec![Value::Bool(false); arity];
                for slot in payload.iter_mut().rev() {
                    *slot = pop(&mut stack);
                }
                stack.push(Value::Enum(*enum_idx, *variant, payload.into_boxed_slice()));
                pc += 1;
            }
            SealedInstr::EnumTag => {
                let (_, variant, _) = as_enum(pop(&mut stack));
                stack.push(Value::Int(i64::from(variant)));
                pc += 1;
            }
            SealedInstr::EnumPayloadGet { variant, field } => {
                let (_, actual_variant, payload) = as_enum(pop(&mut stack));
                // The verifier typed the pushed leaf against the operand variant but
                // could not prove the runtime variant, so guard it: a hostile image
                // extracting the wrong variant's payload faults rather than pushing a
                // wrongly-typed value. Compiler output dispatches on the tag first,
                // so this never fires in practice.
                if actual_variant != *variant {
                    return Err(fault(function, pc, Code::RunEnumVariant.as_str()));
                }
                stack.push(payload[*field as usize].clone());
                pc += 1;
            }
            SealedInstr::EqEnum => {
                let b = pop(&mut stack);
                let a = pop(&mut stack);
                stack.push(Value::Bool(a == b));
                pc += 1;
            }
            SealedInstr::EqId => {
                let b = pop(&mut stack);
                let a = pop(&mut stack);
                stack.push(Value::Bool(a == b));
                pc += 1;
            }
            SealedInstr::MakeIdentity { root, cols } => {
                // k0 was pushed first, so the popped keys fill the tuple in reverse.
                let mut keys: Vec<KeyScalar> = vec![KeyScalar::Bool(false); *cols as usize];
                for slot in keys.iter_mut().rev() {
                    *slot = value_to_key(pop(&mut stack));
                }
                stack.push(Value::Id(*root, Rc::from(keys.as_slice())));
                pc += 1;
            }
            SealedInstr::IdentityKeyPath(cols) => {
                let (_, keys) = as_identity(pop(&mut stack));
                // The verifier proved the identity's root and pinned `cols` to that
                // root's key-column count, and `MakeIdentity` builds the tuple from the
                // same count, so the arity always agrees on a verified path.
                debug_assert_eq!(keys.len(), *cols as usize);
                // Spread root-first (k0 first) so the key-path sits exactly as an inline
                // `^root[k…]` access would leave it for the following entry read.
                for key in keys.iter() {
                    stack.push(key_to_value(key.clone()));
                }
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
            SealedInstr::Unreachable(idx) => {
                let text = match &image.consts()[*idx as usize] {
                    SealedConst::Text(text) => text.clone(),
                    _ => unreachable!("verifier proved a text const operand"),
                };
                let (line, column) = function.span_at(pc).unwrap_or((1, 1));
                return Err(RuntimeFault::with_detail(
                    Code::RunUnreachable.as_str(),
                    line,
                    column,
                    text,
                ));
            }
            SealedInstr::Todo(idx) => {
                let text = match &image.consts()[*idx as usize] {
                    SealedConst::Text(text) => text.clone(),
                    _ => unreachable!("verifier proved a text const operand"),
                };
                let (line, column) = function.span_at(pc).unwrap_or((1, 1));
                return Err(RuntimeFault::with_detail(
                    Code::RunTodo.as_str(),
                    line,
                    column,
                    text,
                ));
            }
            SealedInstr::Assert => {
                // A test assertion: on a false condition the running test fails with a
                // source-mapped `run.assert` fault; the verifier proved this appears
                // only in a test-entry function.
                if as_bool(pop(&mut stack)) {
                    pc += 1;
                } else {
                    return Err(fault(function, pc, Code::RunAssert.as_str()));
                }
            }
            SealedInstr::Call(target) => {
                if depth + 1 > MAX_CALL_DEPTH {
                    return Err(fault(function, pc, Code::RunCallDepth.as_str()));
                }
                let arg_count = image.function(*target).params().len();
                let start = stack.len() - arg_count;
                // a0 was pushed first, so the tail of the stack is a0..an-1 in order.
                let call_args = stack.split_off(start);
                // In a driver frame each call is its own invocation boundary: the
                // driver opens the session the callee demands, runs it, and closes it.
                // Otherwise the call joins this frame's session (a helper inside its
                // owner's transaction) or runs storeless.
                let returned = match driver.as_deref_mut() {
                    Some(driver) => driver.invoke(*target, call_args, depth + 1, budget)?,
                    None => execute(
                        image,
                        *target,
                        call_args,
                        depth + 1,
                        budget,
                        session.as_deref_mut(),
                        None,
                    )?,
                };
                if let Some(value) = returned {
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
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                let present = durable
                    .presence(&authorized, &keys)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                stack.push(Value::Bool(present == Presence::Present));
                pc += 1;
            }
            SealedInstr::DurFamilyExists(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                // The probe supplies only the ancestor key-path — one fewer than the
                // whole-entry key arity, since it names the family rather than one
                // immediate child.
                let ancestor_keys = pop_key_path(&mut stack, authorized.key_arity() - 1);
                let present = durable
                    .family_populated(&authorized, &ancestor_keys)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                stack.push(Value::Bool(present == Presence::Present));
                pc += 1;
            }
            SealedInstr::DurReadField(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                let value = durable
                    .read_field(&authorized, &keys)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                stack.push(Value::Optional(value.map(|s| Box::new(domain_to_value(s)))));
                pc += 1;
            }
            SealedInstr::DurReadEntry(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                let entry = durable
                    .read_entry(&authorized, &keys)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                let ty = entry_record_type(image, *site);
                let group_records = site_group_records(image, *site);
                stack.push(Value::Optional(
                    entry.map(|entry| Box::new(entry_to_record(ty, entry, &group_records))),
                ));
                pc += 1;
            }
            SealedInstr::DurSetRequired(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let value = value_to_domain(pop(&mut stack));
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                durable
                    .set_required(&authorized, &keys, value)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurSetSparse(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let value = as_optional(pop(&mut stack)).map(value_to_domain);
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                durable
                    .set_sparse(&authorized, &keys, value)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurSetSparsePresent { site, key_slots } => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let value = as_optional(pop(&mut stack)).map(value_to_domain);
                // The strict form reads its containing entry's whole key-path from the
                // place's pre-evaluated slots (root-first); the verifier proved each slot
                // definitely initialized with its column type here.
                let keys: Vec<_> = key_slots
                    .iter()
                    .map(|slot| {
                        value_to_key(
                            locals[*slot as usize]
                                .clone()
                                .expect("verifier proved definite init of the place key slot"),
                        )
                    })
                    .collect();
                durable
                    .set_sparse_present(&authorized, &keys, value)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurCreateEntry(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let entry =
                    record_to_entry(pop(&mut stack), site_group_records(image, *site).len());
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                durable
                    .create_entry(&authorized, &keys, entry)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurReplaceEntry(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let entry =
                    record_to_entry(pop(&mut stack), site_group_records(image, *site).len());
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                durable
                    .replace_entry(&authorized, &keys, entry)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurEraseField(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                durable
                    .erase_field(&authorized, &keys)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurEraseEntry(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                durable
                    .erase_entry(&authorized, &keys)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurReadGroup(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                let group = durable
                    .read_group(&authorized, &keys)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                // A group value is a plain record of its own leaves (nested groups park),
                // so it joins with no further group sub-records.
                let ty = entry_record_type(image, *site);
                stack.push(Value::Optional(
                    group.map(|group| Box::new(entry_to_record(ty, group, &[]))),
                ));
                pc += 1;
            }
            SealedInstr::DurReplaceGroup(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let group = record_to_entry(pop(&mut stack), 0);
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                durable
                    .replace_group(&authorized, &keys, group)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::DurEraseGroup(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let keys = pop_key_path(&mut stack, authorized.key_arity());
                durable
                    .erase_group(&authorized, &keys)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                pc += 1;
            }
            SealedInstr::ListNew(idx) => {
                stack.push(Value::list(*idx, Rc::new(Vec::new())));
                pc += 1;
            }
            SealedInstr::ListAppend => {
                let value = pop(&mut stack);
                let Value::List(idx, old_bytes, mut items) = pop(&mut stack) else {
                    unreachable!("verifier proved a list operand");
                };
                // Delta against the cached aggregate: the pre-append list already
                // satisfied both bounds, so one element grows the size by exactly its
                // own structural bytes. Both faults match the whole-list re-measure.
                if items.len() + 1 > MAX_COLLECTION_LEN {
                    return Err(fault(function, pc, Code::RunCollectionLimit.as_str()));
                }
                let new_bytes = old_bytes + value.structural_bytes();
                if new_bytes > MAX_AGGREGATE_BYTES {
                    return Err(fault(function, pc, Code::RunCollectionLimit.as_str()));
                }
                Rc::make_mut(&mut items).push(value);
                stack.push(Value::List(idx, new_bytes, items));
                pc += 1;
            }
            SealedInstr::ListLen => {
                let (_, items) = as_list(pop(&mut stack));
                stack.push(Value::Int(items.len() as i64));
                pc += 1;
            }
            SealedInstr::ListGet => {
                let index = pop_int(&mut stack);
                let (_, items) = as_list(pop(&mut stack));
                // Positional `for` lowering drives the index over `0..length`, so a
                // verified compiler image is always in bounds. The verifier proves the
                // operand types but not the index value, so a forged image can present
                // an out-of-range index; it fails closed with `run.corruption` rather
                // than reading past the collection.
                let Some(value) = usize::try_from(index).ok().and_then(|i| items.get(i)) else {
                    return Err(fault(function, pc, Code::RunCorruption.as_str()));
                };
                stack.push(value.clone());
                pc += 1;
            }
            SealedInstr::ListIndex => {
                // The source-level `xs[i]` read: 1-based, yielding the optional element.
                // An index outside `1..=length` is absent — no out-of-bounds fault.
                let index = pop_int(&mut stack);
                let (_, items) = as_list(pop(&mut stack));
                let element = index
                    .checked_sub(1)
                    .and_then(|zero_based| usize::try_from(zero_based).ok())
                    .and_then(|i| items.get(i))
                    .map(|value| Box::new(value.clone()));
                stack.push(Value::Optional(element));
                pc += 1;
            }
            SealedInstr::MapNew(idx) => {
                stack.push(Value::map(*idx, Rc::new(Vec::new())));
                pc += 1;
            }
            SealedInstr::MapInsert => {
                let value = pop(&mut stack);
                let key = pop_key(&mut stack);
                let Value::Map(idx, old_bytes, mut entries) = pop(&mut stack) else {
                    unreachable!("verifier proved a map operand");
                };
                let value_bytes = value.structural_bytes();
                // Delta against the cached aggregate. A replace swaps one value's
                // bytes; an insert adds a key and a value. The fault matches the
                // whole-map re-measure at the same boundary.
                match entries.binary_search_by(|(k, _)| k.cmp(&key)) {
                    Ok(position) => {
                        let new_bytes =
                            old_bytes - entries[position].1.structural_bytes() + value_bytes;
                        if new_bytes > MAX_AGGREGATE_BYTES {
                            return Err(fault(function, pc, Code::RunCollectionLimit.as_str()));
                        }
                        Rc::make_mut(&mut entries)[position].1 = value;
                        stack.push(Value::Map(idx, new_bytes, entries));
                    }
                    Err(position) => {
                        if entries.len() + 1 > MAX_COLLECTION_LEN {
                            return Err(fault(function, pc, Code::RunCollectionLimit.as_str()));
                        }
                        let new_bytes = old_bytes + key_bytes(&key) + value_bytes;
                        if new_bytes > MAX_AGGREGATE_BYTES {
                            return Err(fault(function, pc, Code::RunCollectionLimit.as_str()));
                        }
                        Rc::make_mut(&mut entries).insert(position, (key, value));
                        stack.push(Value::Map(idx, new_bytes, entries));
                    }
                }
                pc += 1;
            }
            SealedInstr::MapRemove => {
                let key = pop_key(&mut stack);
                let Value::Map(idx, old_bytes, mut entries) = pop(&mut stack) else {
                    unreachable!("verifier proved a map operand");
                };
                // Remove the entry if present; an absent key is an idempotent no-op.
                // The delta mirrors `MapInsert`: drop the removed key and value bytes
                // from the cached aggregate. Removal never exceeds a bound, so it
                // cannot fault.
                if let Ok(position) = entries.binary_search_by(|(k, _)| k.cmp(&key)) {
                    let new_bytes =
                        old_bytes - key_bytes(&key) - entries[position].1.structural_bytes();
                    Rc::make_mut(&mut entries).remove(position);
                    stack.push(Value::Map(idx, new_bytes, entries));
                } else {
                    stack.push(Value::Map(idx, old_bytes, entries));
                }
                pc += 1;
            }
            SealedInstr::MapGet => {
                let key = pop_key(&mut stack);
                let (_, entries) = as_map(pop(&mut stack));
                let found = entries
                    .binary_search_by(|(k, _)| k.cmp(&key))
                    .ok()
                    .map(|position| Box::new(entries[position].1.clone()));
                stack.push(Value::Optional(found));
                pc += 1;
            }
            SealedInstr::MapLen => {
                let (_, entries) = as_map(pop(&mut stack));
                stack.push(Value::Int(entries.len() as i64));
                pc += 1;
            }
            SealedInstr::MapKeyAt => {
                let index = pop_int(&mut stack);
                let (_, entries) = as_map(pop(&mut stack));
                // Positional map `for` lowering drives the index over `0..length`; a
                // forged image with an out-of-range index fails closed with
                // `run.corruption` rather than reading past the map (see `ListGet`).
                let Some((key, _)) = usize::try_from(index).ok().and_then(|i| entries.get(i))
                else {
                    return Err(fault(function, pc, Code::RunCorruption.as_str()));
                };
                stack.push(key_to_value(key.clone()));
                pc += 1;
            }
            SealedInstr::MapValueAt => {
                let index = pop_int(&mut stack);
                let (_, entries) = as_map(pop(&mut stack));
                // Positional map `for` lowering drives the index over `0..length`; a
                // forged image with an out-of-range index fails closed with
                // `run.corruption` rather than reading past the map (see `ListGet`).
                let Some((_, value)) = usize::try_from(index).ok().and_then(|i| entries.get(i))
                else {
                    return Err(fault(function, pc, Code::RunCorruption.as_str()));
                };
                stack.push(value.clone());
                pc += 1;
            }
            SealedInstr::DurIterateBounded {
                site,
                limit,
                from,
                list_ty,
            } => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                // The stack holds the ancestor key-path (root-first) with the inclusive
                // `from` key on top when present. Pop `from` first, then the ancestor
                // key-path — one fewer than the whole-entry key arity, since the
                // traversed key is what iteration enumerates rather than an operand.
                let from = from.then(|| pop_key(&mut stack));
                let ancestor_keys = pop_key_path(&mut stack, authorized.key_arity() - 1);
                let bound =
                    BoundedLimit::new(*limit).expect("verifier proved a positive traversal bound");
                let bounded = durable
                    .iterate_bounded(&authorized, &ancestor_keys, from, bound)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                // The frozen keys materialize into one ordinary bounded `List[K]`,
                // obeying the single collection aggregate ceiling (a wide-key traversal
                // faults `run.collection_limit` here, not through a second bound).
                let items: Vec<Value> = bounded.keys.into_iter().map(key_to_value).collect();
                let items = bounded_list(items)
                    .ok_or_else(|| fault(function, pc, Code::RunCollectionLimit.as_str()))?;
                stack.push(Value::list(*list_ty, Rc::new(items)));
                stack.push(Value::Bool(bounded.more));
                pc += 1;
            }
            SealedInstr::DurIndexScan {
                site,
                limit,
                from,
                list_ty,
            } => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                // The stack holds the held prefix (the index's leading field components,
                // root-first) with the inclusive `from` key of the scanned component on top
                // when present. The prefix is one fewer than the whole projection — the
                // trailing component is what the scan enumerates.
                let prefix_arity = authorized
                    .index_projection_len()
                    .expect("verifier proved an index scan site")
                    - 1;
                let from = from.then(|| pop_key(&mut stack));
                let prefix = pop_key_path(&mut stack, prefix_arity);
                let bound =
                    BoundedLimit::new(*limit).expect("verifier proved a positive scan bound");
                let bounded = durable
                    .index_scan(&authorized, &prefix, from, bound)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                // The frozen scanned keys are the raw identity key scalars; they materialize
                // into one bounded `List[K]` (the compiler wraps each into `Id(^root)` at the
                // loop binding), obeying the single collection aggregate ceiling.
                let items: Vec<Value> = bounded.keys.into_iter().map(key_to_value).collect();
                let items = bounded_list(items)
                    .ok_or_else(|| fault(function, pc, Code::RunCollectionLimit.as_str()))?;
                stack.push(Value::list(*list_ty, Rc::new(items)));
                stack.push(Value::Bool(bounded.more));
                pc += 1;
            }
            SealedInstr::DurIndexLookup(site) => {
                let durable = session
                    .as_deref_mut()
                    .expect("verifier proved a durable opcode runs with a session");
                let authorized = durable.site(*site);
                let arity = authorized
                    .index_projection_len()
                    .expect("verifier proved an index lookup site");
                let key = pop_key_path(&mut stack, arity);
                let found = durable
                    .index_lookup(&authorized, &key)
                    .map_err(|kf| kernel_fault(function, pc, &kf))?;
                // The lookup yields the source key tuple of the matching entry, wrapped as
                // the optional source identity `Id(^root)` — present with the found key
                // tuple, or vacant.
                let value = found.map(|keys| Box::new(Value::Id(0, Rc::from(keys.as_slice()))));
                stack.push(Value::Optional(value));
                pc += 1;
            }
        }
    }
}

/// Push a checked-arithmetic result and advance, or transfer to the fault-handler
/// tape index `target` when the operation had no representable result.
fn checked_or_branch(stack: &mut Vec<Value>, result: Option<i64>, target: usize, pc: &mut usize) {
    match result {
        Some(v) => {
            stack.push(Value::Int(v));
            *pc += 1;
        }
        None => *pc = target,
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

fn pop_date(stack: &mut Vec<Value>) -> i32 {
    match pop(stack) {
        Value::Date(v) => v,
        _ => unreachable!("verifier proved a date operand"),
    }
}

fn pop_instant(stack: &mut Vec<Value>) -> i128 {
    match pop(stack) {
        Value::Instant(v) => v,
        _ => unreachable!("verifier proved an instant operand"),
    }
}

fn pop_duration(stack: &mut Vec<Value>) -> i128 {
    match pop(stack) {
        Value::Duration(v) => v,
        _ => unreachable!("verifier proved a duration operand"),
    }
}

/// Resolve a temporal comparison opcode against the operands' ordering. Equality
/// opcodes test `is_eq`; the four order opcodes test their relation.
fn temporal_compare(instr: &SealedInstr, ordering: std::cmp::Ordering) -> bool {
    match instr {
        SealedInstr::EqDate | SealedInstr::EqInstant | SealedInstr::EqDuration => ordering.is_eq(),
        SealedInstr::DateLt | SealedInstr::InstantLt | SealedInstr::DurationLt => ordering.is_lt(),
        SealedInstr::DateLe | SealedInstr::InstantLe | SealedInstr::DurationLe => ordering.is_le(),
        SealedInstr::DateGt | SealedInstr::InstantGt | SealedInstr::DurationGt => ordering.is_gt(),
        _ => ordering.is_ge(),
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

fn as_bytes(value: Value) -> Rc<[u8]> {
    match value {
        Value::Bytes(v) => v,
        _ => unreachable!("verifier proved a bytes operand"),
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

fn as_enum(value: Value) -> (u16, u16, Box<[Value]>) {
    match value {
        Value::Enum(ty, variant, payload) => (ty, variant, payload),
        _ => unreachable!("verifier proved an enum operand"),
    }
}

fn as_identity(value: Value) -> (u16, Rc<[KeyScalar]>) {
    match value {
        Value::Id(root, keys) => (root, keys),
        _ => unreachable!("verifier proved an entry-identity operand"),
    }
}

fn as_list(value: Value) -> (u16, Rc<Vec<Value>>) {
    match value {
        Value::List(idx, _, items) => (idx, items),
        _ => unreachable!("verifier proved a list operand"),
    }
}

fn as_map(value: Value) -> (u16, Rc<Vec<(KeyScalar, Value)>>) {
    match value {
        Value::Map(idx, _, entries) => (idx, entries),
        _ => unreachable!("verifier proved a map operand"),
    }
}

/// Admit a batch-built list value only within the same law-9 collection bounds
/// `append` enforces incrementally: at most `MAX_COLLECTION_LEN` elements and
/// `MAX_AGGREGATE_BYTES` of aggregate value size. The one aggregate-bound owner for a
/// list materialized all at once — the text-floor `split`/`lines` results and the
/// frozen `List[K]` of a bounded durable traversal alike — so no list value bypasses
/// the single collection ceiling. Returns `None` on a bound excess so the caller
/// faults `run.collection_limit` rather than materializing an unbounded list.
fn bounded_list(items: Vec<Value>) -> Option<Vec<Value>> {
    if items.len() > MAX_COLLECTION_LEN || list_bytes(&items) > MAX_AGGREGATE_BYTES {
        return None;
    }
    Some(items)
}

fn const_value(value: &SealedConst) -> Value {
    match value {
        SealedConst::Int(v) => Value::Int(*v),
        SealedConst::Bool(v) => Value::Bool(*v),
        SealedConst::Text(v) => Value::Text(v.clone()),
        SealedConst::Date(v) => Value::Date(*v),
        SealedConst::Instant(v) => Value::Instant(*v),
        SealedConst::Duration(v) => Value::Duration(*v),
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
        Value::Bytes(v) => KeyScalar::Bytes(v.to_vec()),
        Value::Date(v) => KeyScalar::Date(v),
        Value::Instant(v) => KeyScalar::Instant(v),
        Value::Duration(v) => KeyScalar::Duration(v),
        _ => unreachable!("verifier proved a scalar key operand"),
    }
}

/// Convert a key scalar back to a runtime value.
fn key_to_value(key: KeyScalar) -> Value {
    match key {
        KeyScalar::Int(v) => Value::Int(v),
        KeyScalar::Bool(v) => Value::Bool(v),
        KeyScalar::Str(v) => Value::Text(v.into()),
        KeyScalar::Bytes(v) => Value::Bytes(Rc::from(v.as_slice())),
        KeyScalar::Date(v) => Value::Date(v),
        KeyScalar::Instant(v) => Value::Instant(v),
        KeyScalar::Duration(v) => Value::Duration(v),
    }
}

/// Convert a scalar runtime value to a typed runtime scalar. Total over the closed
/// scalar domain the verifier proves a durable scalar field or composite leaf holds.
fn value_to_scalar(value: Value) -> RuntimeScalar {
    match value {
        Value::Int(v) => RuntimeScalar::Int(v),
        Value::Bool(v) => RuntimeScalar::Bool(v),
        Value::Text(v) => RuntimeScalar::Str(v.to_string()),
        Value::Bytes(v) => RuntimeScalar::Bytes(v.to_vec()),
        Value::Date(v) => RuntimeScalar::Date(v),
        Value::Instant(v) => RuntimeScalar::Instant(v),
        Value::Duration(v) => RuntimeScalar::Duration(v),
        _ => unreachable!("verifier proved a scalar value operand"),
    }
}

/// Convert a typed runtime scalar back to a runtime value. Total over the closed
/// scalar domain: every scalar a durable field (or a widened composite leaf) may hold.
fn scalar_to_value(scalar: RuntimeScalar) -> Value {
    match scalar {
        RuntimeScalar::Int(v) => Value::Int(v),
        RuntimeScalar::Bool(v) => Value::Bool(v),
        RuntimeScalar::Str(v) => Value::Text(v.into()),
        RuntimeScalar::Bytes(v) => Value::Bytes(Rc::from(v.as_slice())),
        RuntimeScalar::Date(v) => Value::Date(v),
        RuntimeScalar::Instant(v) => Value::Instant(v),
        RuntimeScalar::Duration(v) => Value::Duration(v),
    }
}

/// Convert a runtime value crossing into the durable kernel to its storable value
/// domain: a scalar, a record (dense product), or an enum/`Option`/`Result` value
/// (a sum). A record slot recurses; an enum payload leaf recurses. `Value::Optional`
/// is the presence axis (stripped by `as_optional` before this boundary) and never a
/// stored value; a collection is never an inline field value (the checker/verifier
/// refuse it), so no list/map arm exists.
fn value_to_domain(value: Value) -> ValueDomain {
    match value {
        Value::Record(ty, slots) => ValueDomain::Product {
            ty,
            fields: slots
                .into_vec()
                .into_iter()
                .map(|slot| slot.map(value_to_domain))
                .collect(),
        },
        Value::Enum(ty, variant, payload) => ValueDomain::Sum {
            ty,
            variant,
            payload: payload
                .into_vec()
                .into_iter()
                .map(value_to_domain)
                .collect(),
        },
        // An entry identity is not a durable value on this line: the checker and
        // verifier keep it out of every durable field, entry, and key position, so it
        // never crosses the store-write boundary. Naming it explicitly keeps the
        // no-identity-at-the-encoder contract visible rather than folding it into the
        // scalar catch-all's panic.
        Value::Id(..) => unreachable!("an entry identity is never a durable value"),
        scalar => ValueDomain::Scalar(value_to_scalar(scalar)),
    }
}

/// Convert a storable value domain read from the durable kernel back to a runtime value,
/// the inverse of [`value_to_domain`]: a scalar, a product to a `Value::Record`, or a sum
/// to a `Value::Enum` (an `Option`/`Result` is an ordinary enum value). A `unit`,
/// collection, or product slot the verifier never stores in a field would be a kernel
/// invariant breach.
fn domain_to_value(domain: ValueDomain) -> Value {
    match domain {
        ValueDomain::Scalar(scalar) => scalar_to_value(scalar),
        ValueDomain::Product { ty, fields } => Value::Record(
            ty,
            fields
                .into_iter()
                .map(|slot| slot.map(domain_to_value))
                .collect(),
        ),
        ValueDomain::Sum {
            ty,
            variant,
            payload,
        } => Value::Enum(
            ty,
            variant,
            payload.into_iter().map(domain_to_value).collect(),
        ),
        ValueDomain::Unit
        | ValueDomain::List { .. }
        | ValueDomain::Map { .. }
        | ValueDomain::Identity { .. } => {
            unreachable!("a durable field value is a scalar, dense product, or sum")
        }
    }
}

/// Convert a record value into a whole-entry value, splitting its trailing `group_count`
/// Record slots into nested group [`EntryValue`]s. The unified materialized record is
/// `[value fields..][one Record slot per root-level group..]`, in declaration order; the
/// leading slots are the entry's flat fields and each trailing slot is a group's own
/// record value (which has no further groups on this line — nested groups park). The
/// verifier typed the value against this record, so the slot count and the present group
/// slots are structurally guaranteed here.
fn record_to_entry(value: Value, group_count: usize) -> EntryValue {
    let (_, slots) = as_record(value);
    let mut slots = slots.into_vec();
    let groups_start = slots
        .len()
        .checked_sub(group_count)
        .expect("the verified record carries one slot per group after its fields");
    let group_slots = slots.split_off(groups_start);
    let groups = group_slots
        .into_iter()
        .map(|slot| {
            let sub = slot.expect("a group is a present value unit of its containing entry");
            record_to_entry(sub, 0)
        })
        .collect();
    EntryValue {
        fields: slots
            .into_iter()
            .map(|slot| slot.map(value_to_domain))
            .collect(),
        groups,
    }
}

/// Convert a whole-entry value into a record value of type `ty`, joining its group
/// sub-records back as the record's trailing slots. `group_records` names each group's
/// materialized record type, in declaration order; it is empty for a group-less entry (or
/// a branch entry, whose executable shape carries no group). The kernel builds
/// `entry.groups` from the store schema, so its count matches `group_records`.
fn entry_to_record(ty: u16, entry: EntryValue, group_records: &[u16]) -> Value {
    let mut slots: Vec<Option<Value>> = entry
        .fields
        .into_iter()
        .map(|slot| slot.map(domain_to_value))
        .collect();
    assert_eq!(
        entry.groups.len(),
        group_records.len(),
        "the kernel builds one group value per schema group"
    );
    for (group, &record) in entry.groups.into_iter().zip(group_records) {
        slots.push(Some(entry_to_record(record, group, &[])));
    }
    Value::Record(ty, slots.into_boxed_slice())
}

/// The materialized record types of a root-entry site's root-level groups, in declaration
/// order — the `group_records` a whole-entry [`entry_to_record`] joins. A branch-entry
/// site (or any non-root-entry target) has no group on this line, so this is empty.
fn site_group_records(image: &VerifiedImage, site: u16) -> Vec<u16> {
    let SealedSite::Flat { root, target } = &image.sites()[site as usize] else {
        unreachable!("the verifier admits a durable opcode only over a flat site")
    };
    match target {
        SealedSiteTarget::WholePayload => image.roots()[*root as usize]
            .groups()
            .iter()
            .map(marrow_verify::SealedGroup::record)
            .collect(),
        _ => Vec::new(),
    }
}

/// The record type index of the entry a site addresses: the branch's record for a
/// branch entry site, the root's otherwise. The verifier admits a durable opcode only
/// over a flat executable site, so the referenced site is `Flat`.
fn entry_record_type(image: &VerifiedImage, site: u16) -> u16 {
    let (root, target) = match &image.sites()[site as usize] {
        SealedSite::Flat { root, target } => (*root, target),
        SealedSite::Parked { .. } => {
            unreachable!("the verifier admits a durable opcode only over a flat site")
        }
    };
    let root = &image.roots()[root as usize];
    match target {
        SealedSiteTarget::BranchEntry(path) => {
            // Walk the branch path level by level through the recursive sealed branch tree;
            // the deepest branch's own record is the whole-entry record. The verifier
            // resolved this path against the same tree, so every index is in range.
            let mut branches = root.branches();
            let mut record = root.record();
            for &hop in path.as_ref() {
                let branch = &branches[hop as usize];
                record = branch.record();
                branches = branch.branches();
            }
            record
        }
        // A field-leaf site (top-level or branch) never reaches a whole-entry read
        // (the verifier rejects `DurReadEntry` over a field site), so this arm is
        // never observed; it keeps the match total.
        SealedSiteTarget::WholePayload
        | SealedSiteTarget::FieldLeaf(_)
        | SealedSiteTarget::BranchField { .. } => root.record(),
        // A whole-group op reads or writes the group's own materialized record, named by
        // the group's index into the root's declaration-ordered groups.
        SealedSiteTarget::GroupEntry(group) => root.groups()[*group as usize].record(),
        // An index-read site names no source entry record; a whole-entry read never
        // resolves to one.
        SealedSiteTarget::IndexScan(_) | SealedSiteTarget::IndexLookup(_) => {
            unreachable!("a whole-entry read never targets an index site")
        }
    }
}

/// Pop a durable operation's key-path: `arity` key operands assembled root-first. The
/// lowering pushes the root key then each branch key, so the innermost (branch) key is
/// on top; pop `arity` keys and reverse to the root-first order the kernel expects.
fn pop_key_path(stack: &mut Vec<Value>, arity: usize) -> Vec<KeyScalar> {
    let mut keys: Vec<KeyScalar> = (0..arity).map(|_| pop_key(stack)).collect();
    keys.reverse();
    keys
}

#[cfg(test)]
mod bound_ties {
    use super::MAX_COLLECTION_LEN;

    /// The traversal ceiling and the collection ceiling are one bound: a bounded
    /// traversal freezes at most `at most N` keys into a single `List[K]`, and the
    /// verifier caps `N` at `MAX_TRAVERSAL_BOUND`. If that ceiling exceeded this
    /// collection limit, a verified image could name a bound the frozen list can never
    /// hold; if it were smaller, the runtime would fault a bound the verifier accepted.
    #[test]
    fn the_traversal_bound_equals_the_collection_limit() {
        assert_eq!(
            MAX_COLLECTION_LEN,
            marrow_image::bounds::MAX_TRAVERSAL_BOUND as usize
        );
    }
}
