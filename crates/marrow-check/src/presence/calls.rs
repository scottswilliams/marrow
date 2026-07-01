use marrow_schema::stdlib::{self, ReturnType};

use crate::facts::ReadKind;
use crate::{CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedProgram, MarrowType};

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

/// The sole path argument of a collection-view call (`keys`/`values`/`entries`/
/// `reversed`), matched by its typed builtin target rather than the callee name,
/// or `None` for any other call.
pub(super) fn wrapper_arg(expr: &CheckedExpr, wrapper: CheckedBuiltinCall) -> Option<&CheckedExpr> {
    let CheckedExpr::Call { target, args, .. } = expr else {
        return None;
    };
    if *target != CheckedCallTarget::Builtin(wrapper) {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}
