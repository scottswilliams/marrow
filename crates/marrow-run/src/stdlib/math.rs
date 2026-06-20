use marrow_syntax::SourceSpan;

use crate::error::{RuntimeError, divide_by_zero};

pub(crate) fn int_remainder(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    if b == 0 {
        return Err(divide_by_zero("integer remainder by zero", span));
    }
    // The remainder always fits, even where the corresponding quotient would not:
    // `i64::MIN % -1` is 0, while `i64::MIN / -1` overflows. `wrapping_rem` yields
    // the true remainder without the trapping division `checked_rem` performs.
    Ok(a.wrapping_rem(b))
}

pub(crate) fn int_modulo(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    let remainder = int_remainder(a, b, span)?;
    Ok(if remainder != 0 && (remainder < 0) != (b < 0) {
        remainder + b
    } else {
        remainder
    })
}
