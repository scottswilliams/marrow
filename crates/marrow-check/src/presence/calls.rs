use marrow_schema::stdlib::{self, ReturnType};

use crate::facts::ReadKind;
use crate::{CheckedBuiltinCall, CheckedCallTarget, CheckedProgram, MarrowType};

/// Whether a call's result is maybe-present (`T?`), read off the return type
/// rather than a parallel presence flag: a std op declared `OptionalScalar`, or a
/// user function whose declared return type is [`MarrowType::Optional`].
pub(crate) fn maybe_present_result(program: &CheckedProgram, target: &CheckedCallTarget) -> bool {
    match target {
        CheckedCallTarget::Std(std) => stdlib::lookup(std.module, std.op)
            .is_some_and(|op| matches!(op.ret, ReturnType::OptionalScalar(_))),
        CheckedCallTarget::Function(function) => program
            .modules
            .get(function.module as usize)
            .and_then(|module| module.functions.get(function.function as usize))
            .and_then(|function| function.return_type.as_ref())
            .is_some_and(|ty| matches!(ty, MarrowType::Optional(_))),
        _ => false,
    }
}

/// The neighbor read a `next`/`prev` call records, resolved from the call's typed
/// builtin target.
pub(super) fn neighbor_read(target: &CheckedCallTarget) -> Option<ReadKind> {
    match target {
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Next) => Some(ReadKind::Next),
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Prev) => Some(ReadKind::Prev),
        _ => None,
    }
}
