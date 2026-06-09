//! Exact base-10 decimal arithmetic.
//!
//! A [`Decimal`] is `coefficient * 10^(-scale)`, kept value-canonical (no
//! trailing-zero scale, so each value has one representation) within a
//! 34-significant-digit / 34-fractional-place envelope. This module provides
//! parsing, canonical formatting, exact add/sub/mul, half-to-even division, and
//! value comparison. The same canonical form backs
//! [`SavedValue::Decimal`](crate::value::SavedValue), so a decimal round-trips
//! through storage unchanged.

use std::cmp::Ordering;

/// The decimal envelope: at most 34 significant digits and 34 fractional places.
const MAX_DIGITS: u32 = 34;

/// Why a decimal spelling did not parse to a stored value. The runtime maps these
/// to distinct faults — an envelope overflow is a recoverable arithmetic limit,
/// while malformed input is a type error — so the classification lives here with
/// the parser rather than being re-derived by callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecimalParseError {
    /// A canonical decimal spelling whose value exceeds the 34-digit / 34-place
    /// envelope.
    Overflow,
    /// Text that is not a canonical decimal: malformed structure, a non-digit, a
    /// non-canonical spelling (leading zero, trailing-zero fraction), or `-0`.
    Malformed,
}

/// An exact base-10 decimal, value `coefficient * 10^(-scale)`, in canonical form.
///
/// Canonical means the scale carries no trailing zero (`1.50` and `1.5` are the
/// same `Decimal`), so equal values share one representation and `Eq` is the
/// derived field equality. Ordering compares by numeric value, not by parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decimal {
    coefficient: i128,
    scale: u32,
}

impl Decimal {
    /// The value zero (`coefficient 0`, `scale 0`).
    pub const ZERO: Decimal = Decimal {
        coefficient: 0,
        scale: 0,
    };

    /// Build a decimal from `coefficient * 10^(-scale)`, normalizing to canonical
    /// form. `None` if the normalized value falls outside the 34-digit envelope.
    pub fn from_parts(coefficient: i128, scale: u32) -> Option<Decimal> {
        let (coefficient, scale) = normalize(coefficient, scale);
        if significant_digits(coefficient) > MAX_DIGITS || scale > MAX_DIGITS {
            return None;
        }
        Some(Decimal { coefficient, scale })
    }

    /// The canonical coefficient (`value * 10^scale`).
    pub fn coefficient(self) -> i128 {
        self.coefficient
    }

    /// The canonical scale (number of fractional places, no trailing zero).
    pub fn scale(self) -> u32 {
        self.scale
    }

    /// Whether this is the value zero.
    pub fn is_zero(self) -> bool {
        self.coefficient == 0
    }

    /// Parse a decimal: an optional `-`, one or more integer digits, and an
    /// optional `.` with one or more fraction digits. Trailing-zero fractions are
    /// accepted and normalized away. `None` for malformed text, `-0`, or a value
    /// outside the envelope.
    pub fn parse(text: &str) -> Option<Decimal> {
        let shape = DecimalShape::parse(text)?;
        shape.to_decimal().ok()
    }

    /// Parse a decimal whose spelling must already be canonical (no leading zeros,
    /// no trailing-zero fraction, no `-0`), distinguishing an envelope
    /// [`Overflow`](DecimalParseError::Overflow) from
    /// [`Malformed`](DecimalParseError::Malformed) input.
    ///
    /// Only a canonical spelling that exceeds the envelope is an overflow; a
    /// non-canonical but in-range spelling such as `1.50` or `01` is malformed,
    /// since the canonical form (`1.5`, `1`) is the one stored value. This is the
    /// same canonical contract [`decode_value`](crate::value::decode_value) reads
    /// back, lifted to a typed result so the runtime need not re-derive it.
    pub fn parse_canonical(text: &str) -> Result<Decimal, DecimalParseError> {
        let shape = DecimalShape::parse(text).ok_or(DecimalParseError::Malformed)?;
        // The canonical-spelling check is syntactic, so a non-canonical spelling is
        // malformed whether or not its magnitude fits — the envelope check below
        // applies only to a spelling that is already the one canonical form.
        if !shape.is_canonical() {
            return Err(DecimalParseError::Malformed);
        }
        shape.to_decimal()
    }

    /// Canonical decimal text: no leading zeros, no trailing-zero fraction, no
    /// exponent.
    pub fn to_text(self) -> String {
        if self.coefficient == 0 {
            return "0".to_string();
        }
        let sign = if self.coefficient < 0 { "-" } else { "" };
        let digits = self.coefficient.unsigned_abs().to_string();
        if self.scale == 0 {
            return format!("{sign}{digits}");
        }
        let scale = self.scale as usize;
        let padded = format!("{digits:0>width$}", width = scale + 1);
        let point = padded.len() - scale;
        format!("{sign}{}.{}", &padded[..point], &padded[point..])
    }

    /// Exact sum, or `None` if it overflows the envelope.
    pub fn checked_add(self, other: Decimal) -> Option<Decimal> {
        self.combine(other, i128::checked_add)
    }

    /// Exact difference, or `None` if it overflows the envelope.
    pub fn checked_sub(self, other: Decimal) -> Option<Decimal> {
        self.combine(other, i128::checked_sub)
    }

    /// Align both coefficients to the wider scale, combine them with `op`, and
    /// normalize back into the envelope. `None` if aligning or `op` overflows
    /// `i128` or the result leaves the envelope.
    fn combine(self, other: Decimal, op: fn(i128, i128) -> Option<i128>) -> Option<Decimal> {
        let scale = self.scale.max(other.scale);
        let left = scaled_coefficient(self.coefficient, scale - self.scale)?;
        let right = scaled_coefficient(other.coefficient, scale - other.scale)?;
        Decimal::from_parts(op(left, right)?, scale)
    }

    /// Exact product, or `None` if it overflows the envelope.
    pub fn checked_mul(self, other: Decimal) -> Option<Decimal> {
        if self.coefficient == 0 || other.coefficient == 0 {
            return Some(Decimal::ZERO);
        }
        let negative = (self.coefficient < 0) != (other.coefficient < 0);
        let digits = multiply_digits(
            &abs_coefficient_digits(self.coefficient),
            &abs_coefficient_digits(other.coefficient),
        );
        exact_digits_to_decimal(digits, self.scale + other.scale, negative)
    }

    /// Quotient rounded half-to-even into the decimal envelope. `None` if the
    /// divisor is zero or the rounded result falls outside the envelope; a caller
    /// that needs to tell those apart checks [`Decimal::is_zero`] on the divisor
    /// first.
    pub fn checked_div(self, divisor: Decimal) -> Option<Decimal> {
        if divisor.coefficient == 0 {
            return None;
        }
        if self.coefficient == 0 {
            return Some(Decimal::ZERO);
        }
        let negative = (self.coefficient < 0) != (divisor.coefficient < 0);
        let dividend = self.coefficient.unsigned_abs();
        let by = divisor.coefficient.unsigned_abs();

        let DividedDigits {
            mut digits,
            leading_power,
            inexact,
        } = divide_to_digits(dividend, by, MAX_DIGITS as usize + 1);

        // value = coefficient * 10^(power), shifted by the operands' scales.
        let power =
            leading_power - (digits.len() as i32 - 1) + divisor.scale as i32 - self.scale as i32;
        if power >= 0 {
            digits.extend(std::iter::repeat_n(0, power as usize));
            round_digits_to_envelope(digits, 0, negative, inexact)
        } else {
            round_digits_to_envelope(digits, power.unsigned_abs(), negative, inexact)
        }
    }

    /// The absolute value. Always representable (magnitude has the same digits).
    pub fn abs(self) -> Decimal {
        Decimal {
            coefficient: self.coefficient.abs(),
            scale: self.scale,
        }
    }

    /// The greatest integer less than or equal to this value (floor), as an
    /// `i128`. The caller narrows to the language's `int` and reports overflow.
    pub fn floor(self) -> i128 {
        if self.scale == 0 {
            return self.coefficient;
        }
        // Euclidean division floors toward negative infinity for a positive
        // divisor: (-27).div_euclid(10) == -3, i.e. floor(-2.7).
        self.coefficient.div_euclid(10i128.pow(self.scale))
    }
}

/// A structurally valid decimal spelling split into its sign and digit groups,
/// before any envelope or canonical-form judgement. Borrows the input.
struct DecimalShape<'a> {
    negative: bool,
    integer: &'a str,
    fraction: Option<&'a str>,
}

impl<'a> DecimalShape<'a> {
    /// Split `text` into a sign, integer digits, and optional fraction digits,
    /// requiring at least one integer digit and, when a point is present, at least
    /// one fraction digit, all ASCII. `None` if the structure is malformed; this
    /// makes no canonical-form or envelope judgement.
    fn parse(text: &'a str) -> Option<DecimalShape<'a>> {
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (integer, fraction) = match rest.split_once('.') {
            Some((integer, fraction)) => (integer, Some(fraction)),
            None => (rest, None),
        };
        if integer.is_empty() || !integer.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if let Some(fraction) = fraction
            && (fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        Some(DecimalShape {
            negative,
            integer,
            fraction,
        })
    }

    /// Whether this spelling is already the one canonical form: no leading zero in
    /// a multi-digit integer, no trailing zero in the fraction, and not `-0`.
    fn is_canonical(&self) -> bool {
        let integer_canonical = self.integer == "0" || !self.integer.starts_with('0');
        let fraction_canonical = self.fraction.is_none_or(|f| !f.ends_with('0'));
        let is_negative_zero =
            self.negative && self.integer.bytes().all(|b| b == b'0') && self.fraction.is_none();
        integer_canonical && fraction_canonical && !is_negative_zero
    }

    /// The decimal value, normalized into the envelope.
    /// [`Overflow`](DecimalParseError::Overflow) if the magnitude exceeds the
    /// 34-digit / 34-place envelope or `i128` range;
    /// [`Malformed`](DecimalParseError::Malformed) for `-0`, which is no value.
    fn to_decimal(&self) -> Result<Decimal, DecimalParseError> {
        let scale = self.fraction.map_or(0, str::len) as u32;
        let digits = match self.fraction {
            Some(fraction) => format!("{}{fraction}", self.integer),
            None => self.integer.to_string(),
        };
        let magnitude: i128 = digits.parse().map_err(|_| DecimalParseError::Overflow)?;
        if self.negative && magnitude == 0 {
            return Err(DecimalParseError::Malformed); // `-0` is not a value
        }
        let coefficient = if self.negative { -magnitude } else { magnitude };
        Decimal::from_parts(coefficient, scale).ok_or(DecimalParseError::Overflow)
    }
}

impl Ord for Decimal {
    /// Compare by numeric value. Comparing aligned coefficients directly could
    /// overflow, so this splits each value into its integer and fractional parts:
    /// both fit `i128` (the integer part is no wider than the coefficient, and a
    /// fractional part aligned to the common scale `s` is below `10^s <= 10^34`).
    fn cmp(&self, other: &Decimal) -> Ordering {
        let sign = self.coefficient.signum().cmp(&other.coefficient.signum());
        if sign != Ordering::Equal {
            return sign;
        }
        // Same sign (or both zero): compare magnitudes, then apply the sign.
        let magnitude = cmp_magnitude(
            self.coefficient.unsigned_abs(),
            self.scale,
            other.coefficient.unsigned_abs(),
            other.scale,
        );
        if self.coefficient < 0 {
            magnitude.reverse()
        } else {
            magnitude
        }
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Decimal) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compare non-negative magnitudes `a * 10^(-sa)` and `b * 10^(-sb)` without overflow.
fn cmp_magnitude(a: u128, sa: u32, b: u128, sb: u32) -> Ordering {
    let (a_int, a_frac) = split(a, sa);
    let (b_int, b_frac) = split(b, sb);
    match a_int.cmp(&b_int) {
        Ordering::Equal => {}
        ordering => return ordering,
    }
    // Aligning the fractional parts to the common scale keeps each below
    // `10^(common scale) <= 10^34`, so each aligned value fits `u128`.
    let common = sa.max(sb);
    let a_frac = a_frac * 10u128.pow(common - sa);
    let b_frac = b_frac * 10u128.pow(common - sb);
    a_frac.cmp(&b_frac)
}

/// Split a magnitude into its integer and fractional parts at `scale`.
fn split(magnitude: u128, scale: u32) -> (u128, u128) {
    let divisor = 10u128.pow(scale);
    (magnitude / divisor, magnitude % divisor)
}

/// `coefficient * 10^power`, or `None` on overflow.
fn scaled_coefficient(coefficient: i128, power: u32) -> Option<i128> {
    coefficient.checked_mul(10i128.checked_pow(power)?)
}

/// The significant digits of a quotient produced by long division.
struct DividedDigits {
    /// Quotient digits, most significant first, with one guard digit past precision.
    digits: Vec<u8>,
    /// Power of ten of the leading digit (`0` for units, negative below one).
    leading_power: i32,
    /// Sticky bit: a nonzero remainder survives past `digits`, used for half-even ties.
    inexact: bool,
}

/// Divide `dividend` by `by` (both nonzero) into up to `precision` significant
/// digits by long division, generating one guard digit. The remainder stays below
/// `by`, so nothing overflows.
fn divide_to_digits(dividend: u128, by: u128, precision: usize) -> DividedDigits {
    let mut digits: Vec<u8> = Vec::new();
    let mut leading_power: i32;
    let integer = dividend / by; // <= dividend, so at most 34 digits
    let mut rem = dividend % by;
    if integer > 0 {
        let text = integer.to_string();
        leading_power = text.len() as i32 - 1;
        digits.extend(text.bytes().map(|b| b - b'0'));
    } else {
        // Value below 1: skip leading fractional zeros to the first nonzero digit.
        // At most 34 of them, since `dividend >= 1` and `by <= 10^34`, so the loop
        // terminates.
        leading_power = -1;
        loop {
            rem *= 10;
            let digit = (rem / by) as u8;
            rem %= by;
            if digit != 0 {
                digits.push(digit);
                break;
            }
            leading_power -= 1;
        }
    }
    while digits.len() < precision && rem != 0 {
        rem *= 10;
        digits.push((rem / by) as u8);
        rem %= by;
    }
    DividedDigits {
        digits,
        leading_power,
        inexact: rem != 0,
    }
}

fn exact_digits_to_decimal(mut digits: Vec<u8>, mut scale: u32, negative: bool) -> Option<Decimal> {
    digits = trim_leading_zeros(digits);
    if digits == [0] {
        return Some(Decimal::ZERO);
    }
    while scale > 0 && digits.last() == Some(&0) {
        digits.pop();
        scale -= 1;
    }
    if digits.len() as u32 > MAX_DIGITS || scale > MAX_DIGITS {
        return None;
    }
    let mut coefficient = digits_to_i128(&digits)?;
    if negative {
        coefficient = coefficient.checked_neg()?;
    }
    Some(Decimal { coefficient, scale })
}

/// Round a finite coefficient digit string into the 34-digit / 34-place envelope.
///
/// `digits * 10^-scale` is the exact magnitude. `sticky` says there are further
/// nonzero digits beyond `digits`, used only when the dropped part is exactly half.
fn round_digits_to_envelope(
    digits: Vec<u8>,
    scale: u32,
    negative: bool,
    sticky: bool,
) -> Option<Decimal> {
    let digits = trim_leading_zeros(digits);
    if digits == [0] {
        return Some(Decimal::ZERO);
    }
    let integer_digits = (digits.len() as i64 - i64::from(scale)).max(0) as u32;
    if integer_digits > MAX_DIGITS {
        return None;
    }
    let significant_excess = (digits.len() as u32).saturating_sub(MAX_DIGITS);
    let scale_excess = scale.saturating_sub(MAX_DIGITS);
    let drop = significant_excess.max(scale_excess);
    let rounded = round_least_significant_digits(digits, drop as usize, sticky);
    let mut coefficient = digits_to_i128(&rounded)?;
    if negative {
        coefficient = coefficient.checked_neg()?;
    }
    Decimal::from_parts(coefficient, scale.checked_sub(drop)?)
}

fn round_least_significant_digits(digits: Vec<u8>, drop: usize, sticky: bool) -> Vec<u8> {
    if drop == 0 {
        return trim_leading_zeros(digits);
    }
    let split = digits.len().saturating_sub(drop);
    let mut kept = digits[..split].to_vec();
    let dropped = &digits[split..];
    let last_kept_is_odd = kept.last().is_some_and(|digit| digit % 2 == 1);
    let round_up = match cmp_dropped_to_half(dropped, drop) {
        Ordering::Greater => true,
        Ordering::Equal => sticky || last_kept_is_odd,
        Ordering::Less => false,
    };
    if round_up {
        increment_digits(&mut kept);
    }
    trim_leading_zeros(kept)
}

fn cmp_dropped_to_half(dropped: &[u8], drop: usize) -> Ordering {
    if dropped.len() < drop {
        return Ordering::Less;
    }
    match dropped[0].cmp(&5) {
        Ordering::Equal => {
            if dropped[1..].iter().any(|&digit| digit != 0) {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        ordering => ordering,
    }
}

/// Add one to the least significant digit, propagating the carry.
fn increment_digits(digits: &mut Vec<u8>) {
    if digits.is_empty() {
        digits.push(1);
        return;
    }
    for digit in digits.iter_mut().rev() {
        if *digit == 9 {
            *digit = 0;
        } else {
            *digit += 1;
            return;
        }
    }
    digits.insert(0, 1);
}

fn multiply_digits(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut product = vec![0u16; left.len() + right.len()];
    for (left_index, left_digit) in left.iter().rev().enumerate() {
        for (right_index, right_digit) in right.iter().rev().enumerate() {
            product[left_index + right_index] += u16::from(*left_digit) * u16::from(*right_digit);
        }
    }
    let mut carry = 0u16;
    for value in &mut product {
        let total = *value + carry;
        *value = total % 10;
        carry = total / 10;
    }
    while carry > 0 {
        product.push(carry % 10);
        carry /= 10;
    }
    let digits = product.into_iter().rev().map(|digit| digit as u8).collect();
    trim_leading_zeros(digits)
}

fn abs_coefficient_digits(coefficient: i128) -> Vec<u8> {
    coefficient
        .unsigned_abs()
        .to_string()
        .bytes()
        .map(|byte| byte - b'0')
        .collect()
}

fn digits_to_i128(digits: &[u8]) -> Option<i128> {
    digits.iter().try_fold(0i128, |value, &digit| {
        value.checked_mul(10)?.checked_add(i128::from(digit))
    })
}

fn trim_leading_zeros(digits: Vec<u8>) -> Vec<u8> {
    match digits.iter().position(|&digit| digit != 0) {
        Some(index) => digits[index..].to_vec(),
        None => vec![0],
    }
}

/// Strip a trailing-zero scale to reach canonical form; zero normalizes to scale 0.
fn normalize(mut coefficient: i128, mut scale: u32) -> (i128, u32) {
    if coefficient == 0 {
        return (0, 0);
    }
    while scale > 0 && coefficient % 10 == 0 {
        coefficient /= 10;
        scale -= 1;
    }
    (coefficient, scale)
}

/// The number of significant digits in a coefficient; zero has none.
fn significant_digits(coefficient: i128) -> u32 {
    if coefficient == 0 {
        0
    } else {
        coefficient.unsigned_abs().to_string().len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::Decimal;

    fn dec(text: &str) -> Decimal {
        Decimal::parse(text).expect("valid decimal")
    }

    #[test]
    fn parses_and_formats_canonically() {
        for (input, canonical) in [
            ("1.5", "1.5"),
            ("1.0", "1"),
            ("0.50", "0.5"),
            ("123.456", "123.456"),
            ("-2.5", "-2.5"),
            ("0", "0"),
            ("0.0", "0"),
            ("100", "100"),
        ] {
            assert_eq!(dec(input).to_text(), canonical, "input {input}");
        }
    }

    #[test]
    fn parse_canonical_distinguishes_overflow_from_malformed() {
        use super::DecimalParseError;

        // Canonical, in-envelope spellings parse to the same value as `parse`.
        for text in ["0", "1.5", "123.456", "-2.5", &"9".repeat(34)] {
            assert_eq!(
                Decimal::parse_canonical(text),
                Ok(Decimal::parse(text).unwrap()),
                "{text}",
            );
        }

        // A canonical decimal spelling whose value exceeds the envelope is an
        // overflow, distinct from malformed input.
        for text in [
            "99999999999999999999999999999999999",
            "0.11111111111111111111111111111111111",
        ] {
            assert_eq!(
                Decimal::parse_canonical(text),
                Err(DecimalParseError::Overflow),
                "{text}",
            );
        }

        // Malformed structure and non-canonical spellings are malformed, never
        // overflow: a non-decimal, a doubled point, a trailing-zero fraction, a
        // leading zero, `-0`, and an out-of-envelope but non-canonical magnitude.
        for text in [
            "",
            "abc",
            "1.2.3",
            "1.",
            ".5",
            "1.50",
            "01",
            "-0",
            "+1",
            &format!("0.{}", "0".repeat(35)),
        ] {
            assert_eq!(
                Decimal::parse_canonical(text),
                Err(DecimalParseError::Malformed),
                "{text}",
            );
        }
    }

    #[test]
    fn rejects_malformed_or_out_of_envelope_text() {
        // `-0` is rejected: zero has no sign (`to_text` never emits it, and source
        // literals are non-negative since `-` is a separate unary operator).
        for bad in ["", "abc", "1.2.3", "1.", ".5", "1.x", "- 1", "+1", "-0"] {
            assert!(Decimal::parse(bad).is_none(), "should reject {bad:?}");
        }
        // 35 significant digits and a 35-place scale both exceed the envelope.
        assert!(Decimal::parse(&"9".repeat(35)).is_none());
        assert!(Decimal::parse(&format!("0.{}", "1".repeat(35))).is_none());
        // 34 of each is allowed.
        assert!(Decimal::parse(&"9".repeat(34)).is_some());
        assert!(Decimal::parse(&format!("0.{}", "1".repeat(34))).is_some());
    }

    #[test]
    fn adds_and_subtracts_aligning_scales() {
        assert_eq!(dec("1.5").checked_add(dec("2.5")).unwrap(), dec("4"));
        assert_eq!(dec("0.1").checked_add(dec("0.2")).unwrap(), dec("0.3"));
        assert_eq!(dec("1.05").checked_add(dec("2")).unwrap(), dec("3.05"));
        assert_eq!(dec("-1.5").checked_add(dec("1.5")).unwrap(), Decimal::ZERO);
        assert_eq!(dec("4").checked_sub(dec("1.5")).unwrap(), dec("2.5"));
        assert_eq!(dec("0.3").checked_sub(dec("0.1")).unwrap(), dec("0.2"));
        assert_eq!(dec("1").checked_sub(dec("1")).unwrap(), Decimal::ZERO);
        assert_eq!(dec("1").checked_sub(dec("2")).unwrap(), dec("-1"));
    }

    #[test]
    fn multiplies_and_normalizes() {
        assert_eq!(dec("1.5").checked_mul(dec("2")).unwrap(), dec("3"));
        assert_eq!(dec("0.2").checked_mul(dec("0.5")).unwrap(), dec("0.1"));
        assert_eq!(dec("1.5").checked_mul(dec("1.5")).unwrap(), dec("2.25"));
        assert_eq!(dec("-2").checked_mul(dec("3")).unwrap(), dec("-6"));
        assert_eq!(
            dec("123.4").checked_mul(Decimal::ZERO).unwrap(),
            Decimal::ZERO
        );
    }

    #[test]
    fn multiplication_rejects_products_that_do_not_fit_exactly() {
        let a = dec("0.123456789012345678");
        assert!(
            a.checked_mul(a).is_none(),
            "product needs 36 fractional places"
        );

        let tiny = dec("0.0000000000000000001");
        assert!(
            tiny.checked_mul(tiny).is_none(),
            "product needs 38 fractional places"
        );

        let third = dec("1").checked_div(dec("3")).unwrap();
        assert!(
            third.checked_mul(third).is_none(),
            "product of rounded quotient still must fit exactly"
        );
    }

    #[test]
    fn arithmetic_outside_the_envelope_is_none() {
        let big = dec(&"9".repeat(34));
        assert!(big.checked_add(dec("1")).is_none(), "sum exceeds 34 digits");
        assert!(big.checked_mul(big).is_none(), "product exceeds 34 digits");
        // A product that normalizes back within the envelope is fine.
        assert_eq!(dec("0.5").checked_mul(dec("0.2")).unwrap(), dec("0.1"));
    }

    #[test]
    fn compares_by_value_across_scales() {
        assert!(dec("1.5") < dec("2"));
        assert!(dec("2") > dec("1.5"));
        assert_eq!(dec("1.5"), dec("1.50"));
        assert_eq!(dec("0.1").checked_add(dec("0.2")).unwrap(), dec("0.3"));
        assert!(dec("-1") < Decimal::ZERO);
        assert!(Decimal::ZERO < dec("0.0001"));
        assert!(dec("1.5") < dec("1.55"));
        assert!(dec("10") > dec("9.999"));
        assert!(dec("-2.5") < dec("-2"));
        assert!(dec("-2") > dec("-2.5"));
        // Same integer part, differing fractions, large scale gap (no overflow).
        assert!(dec("1.00000000000000000000000000000001") > dec("1"));
    }

    #[test]
    fn zero_is_recognized() {
        assert!(Decimal::ZERO.is_zero());
        assert!(dec("0").is_zero());
        assert!(dec("1").checked_sub(dec("1")).unwrap().is_zero());
        assert!(!dec("0.0001").is_zero());
    }

    #[test]
    fn divides_exactly() {
        for (a, b, q) in [
            ("1", "2", "0.5"),
            ("1", "4", "0.25"),
            ("1", "8", "0.125"),
            ("6", "2", "3"),
            ("7", "2", "3.5"),
            ("9", "4", "2.25"),
            ("1", "5", "0.2"),
            ("3", "3", "1"),
            ("123.45", "1", "123.45"),
            ("1.5", "0.5", "3"),
            ("0.1", "0.1", "1"),
        ] {
            assert_eq!(dec(a).checked_div(dec(b)).unwrap(), dec(q), "{a} / {b}");
        }
        assert_eq!(Decimal::ZERO.checked_div(dec("5")).unwrap(), Decimal::ZERO);
    }

    #[test]
    fn division_by_zero_is_none() {
        assert!(dec("1").checked_div(Decimal::ZERO).is_none());
        assert!(Decimal::ZERO.checked_div(Decimal::ZERO).is_none());
    }

    #[test]
    fn repeating_division_rounds_half_even_to_34_digits() {
        // 1/3 truncates (the 35th digit, 3, is below the halfway point); 2/3 rounds
        // up (the 35th digit, 6, is above it).
        assert_eq!(
            dec("1").checked_div(dec("3")).unwrap().to_text(),
            format!("0.{}", "3".repeat(34))
        );
        assert_eq!(
            dec("2").checked_div(dec("3")).unwrap().to_text(),
            format!("0.{}7", "6".repeat(33))
        );
    }

    #[test]
    fn sub_one_division_rounds_to_the_fractional_envelope() {
        assert_eq!(
            dec("1").checked_div(dec("62")).unwrap().to_text(),
            "0.0161290322580645161290322580645161"
        );
    }

    #[test]
    fn half_even_ties_round_to_even() {
        // (10^34 - 1) / 2 = 4999...9.5 exactly; the last kept digit is odd, so the
        // tie rounds up to the even 5000...0.
        assert_eq!(
            dec(&"9".repeat(34))
                .checked_div(dec("2"))
                .unwrap()
                .to_text(),
            format!("5{}", "0".repeat(33))
        );
        // (8*10^33 + 1) / 2 = 4000...0.5 exactly; the last kept digit is even, so the
        // tie stays at 4000...0.
        assert_eq!(
            dec(&format!("8{}1", "0".repeat(32)))
                .checked_div(dec("2"))
                .unwrap()
                .to_text(),
            format!("4{}", "0".repeat(33))
        );
    }

    #[test]
    fn division_carries_sign() {
        let third = format!("0.{}", "3".repeat(34));
        assert_eq!(
            dec("-1").checked_div(dec("3")).unwrap().to_text(),
            format!("-{third}")
        );
        assert_eq!(
            dec("1").checked_div(dec("-3")).unwrap().to_text(),
            format!("-{third}")
        );
        assert_eq!(dec("-1").checked_div(dec("-3")).unwrap().to_text(), third);
    }

    #[test]
    fn division_outside_the_envelope_is_none() {
        // 10^34 / 10^-34 = 10^68, far beyond 34 significant digits.
        let tiny = dec(&format!("0.{}1", "0".repeat(33)));
        assert!(dec(&"9".repeat(34)).checked_div(tiny).is_none());
    }

    #[test]
    fn absolute_value() {
        assert_eq!(dec("-2.5").abs(), dec("2.5"));
        assert_eq!(dec("2.5").abs(), dec("2.5"));
        assert_eq!(Decimal::ZERO.abs(), Decimal::ZERO);
    }

    #[test]
    fn floor_rounds_toward_negative_infinity() {
        for (value, floored) in [
            ("2.7", 2),
            ("2.0", 2),
            ("0.4", 0),
            ("0", 0),
            ("-0.4", -1),
            ("-2.7", -3),
            ("-5", -5),
            ("5", 5),
        ] {
            assert_eq!(dec(value).floor(), floored, "floor({value})");
        }
    }
}
