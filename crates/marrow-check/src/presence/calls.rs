use marrow_schema::stdlib::{self, ParamType};

use crate::CheckedExpr;
use crate::facts::PresenceProofRead;

pub(super) fn is_exists_call(callee: &CheckedExpr) -> bool {
    matches!(callee, CheckedExpr::Name { segments, .. } if segments.as_slice() == ["exists"])
}

pub(super) fn is_attached_data_call(callee: &CheckedExpr) -> bool {
    let CheckedExpr::Name { segments, .. } = callee else {
        return false;
    };
    matches!(
        segments.as_slice(),
        [name]
            if matches!(
                name.as_str(),
                "keys" | "values" | "entries" | "count" | "next" | "prev" | "nextId" | "reversed"
            )
    )
}

pub(super) fn is_append_call(callee: &CheckedExpr) -> bool {
    matches!(callee, CheckedExpr::Name { segments, .. } if segments.as_slice() == ["append"])
}

pub(super) fn std_path_arg_mask(callee: &CheckedExpr) -> Option<Vec<bool>> {
    let CheckedExpr::Name { segments, .. } = callee else {
        return None;
    };
    let [std, module, op] = segments.as_slice() else {
        return None;
    };
    if std != "std" {
        return None;
    }
    Some(
        stdlib::lookup(module, op)?
            .params
            .iter()
            .map(|param| matches!(param, ParamType::Path))
            .collect(),
    )
}

pub(super) fn neighbor_read(callee: &CheckedExpr) -> Option<PresenceProofRead> {
    let CheckedExpr::Name { segments, .. } = callee else {
        return None;
    };
    match segments.as_slice() {
        [name] if name == "next" => Some(PresenceProofRead::Next),
        [name] if name == "prev" => Some(PresenceProofRead::Prev),
        _ => None,
    }
}

pub(super) fn is_neighbor_read(callee: &CheckedExpr) -> bool {
    neighbor_read(callee).is_some()
}

pub(super) fn wrapper_arg<'a>(expr: &'a CheckedExpr, wrapper: &str) -> Option<&'a CheckedExpr> {
    let CheckedExpr::Call { callee, args, .. } = expr else {
        return None;
    };
    let CheckedExpr::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.as_slice() != [wrapper] {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}
