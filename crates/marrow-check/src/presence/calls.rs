use marrow_schema::stdlib::{self, ParamType};

use crate::facts::PresenceProofRead;
use crate::{CheckedBuiltinCall, CheckedCallTarget, CheckedExpr};

/// The positional path-argument mask of a resolved `std::module::op` call: which
/// of its arguments name saved data rather than plain values. Derived from the
/// typed [`CheckedCallTarget::Std`] the checker already resolved, not from the
/// callee's name segments.
pub(super) fn std_path_arg_mask(target: &CheckedCallTarget) -> Option<Vec<bool>> {
    let CheckedCallTarget::Std(std) = target else {
        return None;
    };
    Some(
        stdlib::lookup(std.module, std.op)?
            .params
            .iter()
            .map(|param| matches!(param, ParamType::Path))
            .collect(),
    )
}

/// The neighbor read a `next`/`prev` call records, resolved from the call's typed
/// builtin target.
pub(super) fn neighbor_read(target: &CheckedCallTarget) -> Option<PresenceProofRead> {
    match target {
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Next) => Some(PresenceProofRead::Next),
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Prev) => Some(PresenceProofRead::Prev),
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
