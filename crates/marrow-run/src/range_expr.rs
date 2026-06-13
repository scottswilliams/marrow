use marrow_check::{CheckedBinaryOp, CheckedExpr};

pub(crate) struct CheckedRange<'a> {
    pub(crate) start: Option<&'a CheckedExpr>,
    pub(crate) end: Option<&'a CheckedExpr>,
    pub(crate) inclusive_end: bool,
}

pub(crate) fn checked_range(expr: &CheckedExpr) -> Option<CheckedRange<'_>> {
    match expr {
        CheckedExpr::Range {
            start,
            end,
            inclusive_end,
            ..
        } => Some(CheckedRange {
            start: start.as_deref(),
            end: end.as_deref(),
            inclusive_end: *inclusive_end,
        }),
        CheckedExpr::Binary {
            op, left, right, ..
        } if matches!(
            op,
            CheckedBinaryOp::RangeExclusive | CheckedBinaryOp::RangeInclusive
        ) =>
        {
            Some(CheckedRange {
                start: Some(left.as_ref()),
                end: Some(right.as_ref()),
                inclusive_end: matches!(op, CheckedBinaryOp::RangeInclusive),
            })
        }
        _ => None,
    }
}
