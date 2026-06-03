//! Evaluate an `evolve default` value to its typed, encoded constant.
//!
//! The v0.1 contract is that an `evolve default <member> = <expr>` value is a
//! constant the checker can evaluate at discharge time. A per-record-varying fill is
//! a transform, not a default, so only constant literals (optionally negated) are
//! accepted. This module is the single interpreter of a default literal's text:
//! apply writes the encoded bytes verbatim and never re-reads the source, so there
//! is exactly one place the literal is given meaning.

use marrow_store::Decimal;
use marrow_store::value::{Scalar, ScalarType, encode_value};
use marrow_syntax::{Expression, LiteralKind, UnaryOp};

use super::witness::DefaultValue;

/// Why a default value cannot be carried as a typed constant.
pub(crate) enum ConstDefaultError {
    /// The expression is not a constant literal the checker evaluates.
    NotConstant,
    /// The literal's type does not match the member's leaf type.
    TypeMismatch,
    /// The literal is a valid shape but its value cannot be encoded (out of range).
    NotEncodable,
}

impl ConstDefaultError {
    /// The diagnostic a rejected default reports. A non-constant value steers the
    /// developer to a transform; a type or range failure names the conflict.
    pub(crate) fn message(&self) -> String {
        match self {
            ConstDefaultError::NotConstant => {
                "evolve default must be a constant value; use a transform for computed values"
            }
            ConstDefaultError::TypeMismatch => {
                "evolve default value type does not match the member"
            }
            ConstDefaultError::NotEncodable => {
                "evolve default value is out of range for the member"
            }
        }
        .to_string()
    }
}

/// Evaluate `value` to the encoded constant a defaulting obligation backfills, typed
/// by the member's leaf scalar type. A non-constant expression, a type mismatch, or
/// an unencodable value is reported so discharge can fail the default closed.
pub(crate) fn eval_const_default(
    value: &Expression,
    leaf: ScalarType,
) -> Result<DefaultValue, ConstDefaultError> {
    let scalar = const_scalar(value)?;
    if scalar.ty() != leaf {
        return Err(ConstDefaultError::TypeMismatch);
    }
    let encoded = encode_value(&scalar).map_err(|_| ConstDefaultError::NotEncodable)?;
    Ok(DefaultValue {
        scalar_type: leaf,
        encoded,
    })
}

/// The constant scalar a default expression denotes. Supports a scalar literal and
/// the negation of a numeric literal; every other shape (a call such as `date(...)`,
/// an interpolation, a name, a duration or bytes literal) is a non-constant fill the
/// developer must express as a transform.
fn const_scalar(value: &Expression) -> Result<Scalar, ConstDefaultError> {
    match value {
        Expression::Literal { kind, text, .. } => literal_scalar(*kind, text),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => negate(const_scalar(operand)?),
        _ => Err(ConstDefaultError::NotConstant),
    }
}

fn literal_scalar(kind: LiteralKind, text: &str) -> Result<Scalar, ConstDefaultError> {
    match kind {
        LiteralKind::Integer => text
            .parse::<i64>()
            .map(Scalar::Int)
            .map_err(|_| ConstDefaultError::NotEncodable),
        LiteralKind::Bool => Ok(Scalar::Bool(text == "true")),
        LiteralKind::Decimal => Decimal::parse(text)
            .map(Scalar::Decimal)
            .ok_or(ConstDefaultError::NotEncodable),
        LiteralKind::String => decode_string_literal(text).map(Scalar::Str),
        // A duration or bytes literal's value needs the runtime codec, and the
        // checker does not evaluate it; treat it as a non-constant default.
        LiteralKind::Duration | LiteralKind::Bytes => Err(ConstDefaultError::NotConstant),
    }
}

fn negate(scalar: Scalar) -> Result<Scalar, ConstDefaultError> {
    match scalar {
        Scalar::Int(value) => value
            .checked_neg()
            .map(Scalar::Int)
            .ok_or(ConstDefaultError::NotEncodable),
        Scalar::Decimal(value) => value
            .coefficient()
            .checked_neg()
            .and_then(|coefficient| Decimal::from_parts(coefficient, value.scale()))
            .map(Scalar::Decimal)
            .ok_or(ConstDefaultError::NotEncodable),
        _ => Err(ConstDefaultError::NotConstant),
    }
}

/// Decode a string literal's value: strip the surrounding quotes and resolve the
/// escape set Marrow recognizes. This is the only interpreter of a default string,
/// so apply never re-reads the source text.
fn decode_string_literal(text: &str) -> Result<String, ConstDefaultError> {
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or(ConstDefaultError::NotConstant)?;
    let mut decoded = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => decoded.push('\\'),
            Some('"') => decoded.push('"'),
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('t') => decoded.push('\t'),
            _ => return Err(ConstDefaultError::NotConstant),
        }
    }
    Ok(decoded)
}
