//! Evaluate an `evolve default` value to its typed, encoded constant.
//!
//! The v0.1 contract is that an `evolve default <member> = <expr>` value is a
//! constant the checker can evaluate at discharge time. A per-record-varying fill is
//! a transform, not a default, so only constant literals (optionally negated) are
//! accepted. This module is the single interpreter of a default literal's text:
//! apply writes the encoded bytes verbatim and never re-reads the source, so there
//! is exactly one place the literal is given meaning.

use marrow_store::Decimal;
use marrow_store::value::{Scalar, ScalarType, decode_value, encode_value};
use marrow_syntax::{Argument, Expression, LiteralKind, UnaryOp, decode_string_literal};

use super::witness::{DefaultValue, RejectedDefault};
use crate::StoreLeafKind;

/// The single owner of the const-default rule applied to a member leaf: a non-scalar leaf
/// (an enum, an identity, or a non-tokenizable position with no leaf kind) cannot take a
/// constant default, because a computed fill is a transform, not a default; a scalar leaf
/// evaluates its value through [`eval_const_default`]. Both the discharge accumulator and
/// the resume verifier route through here so the gate and the eval never drift. A rejected
/// default returns its typed cause so the verdict names which way the default failed.
pub(crate) fn default_value_for_leaf(
    value: &Expression,
    leaf: Option<&StoreLeafKind>,
) -> Result<DefaultValue, RejectedDefault> {
    // A non-scalar member cannot take a constant default; a computed fill over an enum,
    // identity, or non-tokenizable leaf is a transform, so this is the not-constant cause.
    let Some(StoreLeafKind::Scalar(scalar)) = leaf else {
        return Err(RejectedDefault::NotConstant);
    };
    eval_const_default(value, *scalar)
}

/// Evaluate `value` to the encoded constant a defaulting obligation backfills, typed
/// by the member's leaf scalar type. A non-constant expression, a type mismatch, or
/// an unencodable value is reported so discharge can fail the default closed.
fn eval_const_default(
    value: &Expression,
    leaf: ScalarType,
) -> Result<DefaultValue, RejectedDefault> {
    let scalar = const_scalar(value)?;
    if scalar.ty() != leaf {
        return Err(RejectedDefault::TypeMismatch);
    }
    let encoded = encode_value(&scalar).map_err(|_| RejectedDefault::NotEncodable)?;
    Ok(DefaultValue {
        scalar_type: leaf,
        encoded,
    })
}

/// The constant scalar a default expression denotes. Supports a scalar literal, the
/// negation of a numeric literal, and a validating-constructor call over a constant
/// string (`date("...")`, `instant("...")`, `duration("...")`, `bytes("...")`); every
/// other shape (an interpolation, a name, a per-record-varying call) is a non-constant
/// fill the developer must express as a transform.
fn const_scalar(value: &Expression) -> Result<Scalar, RejectedDefault> {
    match value {
        Expression::Literal { kind, text, .. } => literal_scalar(*kind, text),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => negate(const_scalar(operand)?),
        Expression::Call { callee, args, .. } => constructor_scalar(callee, args),
        _ => Err(RejectedDefault::NotConstant),
    }
}

fn literal_scalar(kind: LiteralKind, text: &str) -> Result<Scalar, RejectedDefault> {
    match kind {
        LiteralKind::Integer => text
            .parse::<i64>()
            .map(Scalar::Int)
            .map_err(|_| RejectedDefault::NotEncodable),
        LiteralKind::Bool => Ok(Scalar::Bool(text == "true")),
        LiteralKind::Decimal => Decimal::parse(text)
            .map(Scalar::Decimal)
            .ok_or(RejectedDefault::NotEncodable),
        LiteralKind::String => decode_string_literal(text)
            .map(Scalar::Str)
            .map_err(|_| RejectedDefault::NotConstant),
        // A bare duration literal (`1.day`) and a bytes literal (`b"..."`) decode
        // through the runtime codec the checker does not host. The constant
        // temporal/bytes default the checker does evaluate is the validating
        // constructor over a string, handled in `constructor_scalar`.
        LiteralKind::Duration | LiteralKind::Bytes => Err(RejectedDefault::NotConstant),
    }
}

/// The constant scalar a `date`/`instant`/`duration`/`bytes` constructor call over a
/// single string literal denotes. The string is validated against the same canonical
/// saved form a stored value of that type must satisfy — the boundary `decode_value`
/// enforces everywhere — so an ill-formed temporal value is a `NotEncodable` default
/// rather than a value the store could never read back. Any other callee or argument
/// shape is a non-constant fill the developer must express as a transform.
fn constructor_scalar(callee: &Expression, args: &[Argument]) -> Result<Scalar, RejectedDefault> {
    let Expression::Name { segments, .. } = callee else {
        return Err(RejectedDefault::NotConstant);
    };
    let [name] = segments.as_slice() else {
        return Err(RejectedDefault::NotConstant);
    };
    let Some(ty) = string_constructor_type(name) else {
        return Err(RejectedDefault::NotConstant);
    };
    let [arg] = args else {
        return Err(RejectedDefault::NotConstant);
    };
    if arg.mode.is_some() || arg.name.is_some() {
        return Err(RejectedDefault::NotConstant);
    }
    let Expression::Literal {
        kind: LiteralKind::String,
        text,
        ..
    } = &arg.value
    else {
        return Err(RejectedDefault::NotConstant);
    };
    let inner = decode_string_literal(text).map_err(|_| RejectedDefault::NotConstant)?;
    decode_value(inner.as_bytes(), ty).ok_or(RejectedDefault::NotEncodable)
}

/// The scalar type a validating constructor names when it takes a single string the
/// checker can validate against canonical form. The numeric and string constructors
/// (`int`, `decimal`, `bool`, `string`) are spelled as bare literals instead, so they
/// are not constructor-form defaults.
///
/// `bytes` belongs here because `bytes(string)` is the constructor form, but unlike the
/// temporal types it has no narrower canonical form: a `bytes` value is the argument
/// string's raw UTF-8 bytes, so every string is valid, matching the runtime `bytes(...)`
/// conversion.
fn string_constructor_type(name: &str) -> Option<ScalarType> {
    let ty = ScalarType::from_scalar_name(name)?;
    matches!(
        ty,
        ScalarType::Date | ScalarType::Instant | ScalarType::Duration | ScalarType::Bytes
    )
    .then_some(ty)
}

fn negate(scalar: Scalar) -> Result<Scalar, RejectedDefault> {
    match scalar {
        Scalar::Int(value) => value
            .checked_neg()
            .map(Scalar::Int)
            .ok_or(RejectedDefault::NotEncodable),
        Scalar::Decimal(value) => value
            .coefficient()
            .checked_neg()
            .and_then(|coefficient| Decimal::from_parts(coefficient, value.scale()))
            .map(Scalar::Decimal)
            .ok_or(RejectedDefault::NotEncodable),
        _ => Err(RejectedDefault::NotConstant),
    }
}
