use marrow_syntax::SourceSpan;

use crate::error::{RuntimeError, divide_by_zero, overflow};

/// Integer quotient truncated toward zero, the pairing partner of [`int_remainder`]:
/// for nonzero `b`, `a == int_quotient(a, b) * b + int_remainder(a, b)`. The lone
/// non-representable result is `i64::MIN / -1`, whose magnitude overflows `int`.
pub(crate) fn int_quotient(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    if b == 0 {
        return Err(divide_by_zero("integer quotient by zero", span));
    }
    a.checked_div(b).ok_or_else(|| overflow(span))
}

/// Integer quotient floored toward minus infinity, the pairing partner of
/// [`int_modulo`]: for nonzero `b`, `a == int_div_floor(a, b) * b + int_modulo(a, b)`.
/// It equals the truncated quotient except when the operands have opposite signs and do
/// not divide evenly, where flooring rounds one further toward minus infinity.
pub(crate) fn int_div_floor(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    let quotient = int_quotient(a, b, span)?;
    let remainder = a.wrapping_rem(b);
    Ok(if remainder != 0 && (remainder < 0) != (b < 0) {
        quotient - 1
    } else {
        quotient
    })
}

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

#[cfg(test)]
mod tests {
    use super::{int_div_floor, int_quotient};
    use crate::error::{RUN_DIVIDE_BY_ZERO, RUN_OVERFLOW};
    use marrow_syntax::SourceSpan;

    const SPAN: SourceSpan = SourceSpan {
        start_byte: 0,
        end_byte: 0,
        line: 0,
        column: 0,
    };

    #[test]
    fn the_single_non_representable_quotient_faults_rather_than_panicking() {
        // `i64::MIN / -1` overflows `int`; both division helpers must report it as a
        // typed overflow fault rather than panic on the trapping native division.
        assert_eq!(
            int_quotient(i64::MIN, -1, SPAN).unwrap_err().code(),
            RUN_OVERFLOW
        );
        assert_eq!(
            int_div_floor(i64::MIN, -1, SPAN).unwrap_err().code(),
            RUN_OVERFLOW
        );
    }

    #[test]
    fn dividing_by_zero_faults_for_both_helpers() {
        assert_eq!(
            int_quotient(7, 0, SPAN).unwrap_err().code(),
            RUN_DIVIDE_BY_ZERO
        );
        assert_eq!(
            int_div_floor(7, 0, SPAN).unwrap_err().code(),
            RUN_DIVIDE_BY_ZERO
        );
    }
}
