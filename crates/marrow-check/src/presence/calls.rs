use marrow_syntax::{Argument, Expression};

use crate::facts::PresenceProofRead;

pub(super) fn is_exists_call(callee: &Expression) -> bool {
    matches!(callee, Expression::Name { segments, .. } if segments.as_slice() == ["exists"])
}

pub(super) fn is_attached_data_call(callee: &Expression) -> bool {
    let Expression::Name { segments, .. } = callee else {
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

pub(super) fn is_append_call(callee: &Expression) -> bool {
    matches!(callee, Expression::Name { segments, .. } if segments.as_slice() == ["append"])
}

pub(crate) fn append_call_args<'a>(
    callee: &Expression,
    args: &'a [Argument],
) -> Option<(&'a Argument, &'a [Argument])> {
    is_append_call(callee).then(|| args.split_first()).flatten()
}

pub(super) fn neighbor_read(callee: &Expression) -> Option<PresenceProofRead> {
    let Expression::Name { segments, .. } = callee else {
        return None;
    };
    match segments.as_slice() {
        [name] if name == "next" => Some(PresenceProofRead::Next),
        [name] if name == "prev" => Some(PresenceProofRead::Prev),
        _ => None,
    }
}

pub(super) fn is_neighbor_read(callee: &Expression) -> bool {
    neighbor_read(callee).is_some()
}

pub(super) fn wrapper_arg<'a>(expr: &'a Expression, wrapper: &str) -> Option<&'a Expression> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
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

pub(super) fn callee_name(callee: &Expression) -> Option<&str> {
    let Expression::Name { segments, .. } = callee else {
        return None;
    };
    segments.last().map(String::as_str)
}
