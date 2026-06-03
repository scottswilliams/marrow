use marrow_syntax::SourceSpan;

use crate::error::{RuntimeError, divide_by_zero, overflow};

pub(crate) fn int_remainder(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    if b == 0 {
        return Err(divide_by_zero("integer remainder by zero", span));
    }
    a.checked_rem(b).ok_or_else(|| overflow(span))
}

pub(crate) fn int_modulo(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    let remainder = int_remainder(a, b, span)?;
    Ok(if remainder != 0 && (remainder < 0) != (b < 0) {
        remainder + b
    } else {
        remainder
    })
}
