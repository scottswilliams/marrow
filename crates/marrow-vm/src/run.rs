//! The stack VM execution loop.
//!
//! Runs one export over its sealed instruction tape. Because the image is
//! verified, the tape is well-typed and bounded: the loop can rely on operand
//! shapes the verifier already proved and only guards the *dynamic* faults the
//! verifier cannot rule out (overflow, budgets). The instruction set grows one
//! slice at a time alongside the verifier and compiler.

use marrow_codes::Code;
use marrow_verify::{SealedConst, SealedFunction, SealedInstr, VerifiedImage};

use crate::fault::RuntimeFault;
use crate::value::Value;

/// Per-invocation instruction budget (design §D). Bounds total work regardless of
/// loop structure.
const INSTRUCTION_BUDGET: u64 = 1 << 26;

/// Run the function at `func_index` with `args`, returning its value (or `None`
/// for a Unit return) or a source-mapped runtime fault.
pub fn run(
    image: &VerifiedImage,
    func_index: u16,
    args: Vec<Value>,
) -> Result<Option<Value>, RuntimeFault> {
    let function = image.function(func_index);
    let mut locals: Vec<Option<Value>> = Vec::with_capacity(function.local_count() as usize);
    for arg in args {
        locals.push(Some(arg));
    }
    locals.resize(function.local_count() as usize, None);

    let mut stack: Vec<Value> = Vec::with_capacity(function.max_stack());
    let mut pc = 0usize;
    let mut budget = INSTRUCTION_BUDGET;

    loop {
        if budget == 0 {
            return Err(fault(function, pc, Code::RunBudget.as_str()));
        }
        budget -= 1;

        match &function.instrs()[pc] {
            SealedInstr::ConstLoad(idx) => {
                stack.push(const_value(&image.consts()[*idx as usize]));
                pc += 1;
            }
            SealedInstr::Return => {
                return Ok(stack.pop());
            }
        }
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
