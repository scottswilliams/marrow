use std::collections::HashMap;

use marrow_schema::ScalarType;
use marrow_syntax::{self as syntax, SourceSpan};

use crate::checks::catch_frame;
use crate::facts::EnumMemberId;
use crate::program::MarrowType;

use super::expr::lower_optional_expr;
use super::{CheckedBody, CheckedExecutableContext, CheckedExpr, checked_enum_member_ref_in};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedInterpolationPart {
    Text { text: String, span: SourceSpan },
    Expr(Box<CheckedExpr>),
}

impl CheckedInterpolationPart {
    pub(super) fn lower(
        part: &syntax::InterpolationPart,
        context: &CheckedExecutableContext<'_>,
        scope: &[HashMap<String, MarrowType>],
    ) -> Option<Self> {
        Some(match part {
            syntax::InterpolationPart::Text { text, span } => Self::Text {
                text: text.clone(),
                span: *span,
            },
            syntax::InterpolationPart::Expr(expr) => {
                Self::Expr(Box::new(CheckedExpr::lower(expr, context, scope)?))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedArg {
    pub name: Option<String>,
    pub value: CheckedExpr,
}

impl CheckedArg {
    pub(super) fn lower(
        arg: &syntax::Argument,
        context: &CheckedExecutableContext<'_>,
        scope: &[HashMap<String, MarrowType>],
    ) -> Option<Self> {
        Some(Self {
            name: arg.name.clone(),
            value: CheckedExpr::lower(&arg.value, context, scope)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedLiteralKind {
    Integer,
    Decimal,
    Duration,
    String,
    Bytes,
    Bool,
}

impl CheckedLiteralKind {
    pub(crate) fn lower(kind: syntax::LiteralKind) -> Self {
        match kind {
            syntax::LiteralKind::Integer => Self::Integer,
            syntax::LiteralKind::Decimal => Self::Decimal,
            syntax::LiteralKind::Duration => Self::Duration,
            syntax::LiteralKind::String => Self::String,
            syntax::LiteralKind::Bytes => Self::Bytes,
            syntax::LiteralKind::Bool => Self::Bool,
        }
    }

    /// The scalar type a literal of this kind carries. Sole owner of the
    /// literal-kind-to-type table for the checker.
    pub(crate) fn marrow_type(self) -> MarrowType {
        MarrowType::Primitive(match self {
            Self::Integer => ScalarType::Int,
            Self::Decimal => ScalarType::Decimal,
            Self::Duration => ScalarType::Duration,
            Self::String => ScalarType::Str,
            Self::Bytes => ScalarType::Bytes,
            Self::Bool => ScalarType::Bool,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedUnaryOp {
    Neg,
    Not,
}

impl CheckedUnaryOp {
    pub(super) fn lower(op: syntax::UnaryOp) -> Self {
        match op {
            syntax::UnaryOp::Neg => Self::Neg,
            syntax::UnaryOp::Not => Self::Not,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedBinaryOp {
    Multiply,
    Divide,
    Remainder,
    Add,
    Subtract,
    RangeExclusive,
    RangeInclusive,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    Coalesce,
    Is,
    And,
    Or,
}

impl CheckedBinaryOp {
    pub(super) fn lower(op: syntax::BinaryOp) -> Self {
        match op {
            syntax::BinaryOp::Multiply => Self::Multiply,
            syntax::BinaryOp::Divide => Self::Divide,
            syntax::BinaryOp::Remainder => Self::Remainder,
            syntax::BinaryOp::Add => Self::Add,
            syntax::BinaryOp::Subtract => Self::Subtract,
            syntax::BinaryOp::RangeExclusive => Self::RangeExclusive,
            syntax::BinaryOp::RangeInclusive => Self::RangeInclusive,
            syntax::BinaryOp::Less => Self::Less,
            syntax::BinaryOp::LessEqual => Self::LessEqual,
            syntax::BinaryOp::Greater => Self::Greater,
            syntax::BinaryOp::GreaterEqual => Self::GreaterEqual,
            syntax::BinaryOp::Equal => Self::Equal,
            syntax::BinaryOp::NotEqual => Self::NotEqual,
            syntax::BinaryOp::Coalesce => Self::Coalesce,
            syntax::BinaryOp::Is => Self::Is,
            syntax::BinaryOp::And => Self::And,
            syntax::BinaryOp::Or => Self::Or,
        }
    }
}

/// The lowered loop-head names, outermost-first: `names[0]` is the key a single
/// binding takes; additional names bind the remaining key columns and the leaf
/// value per the head arity. The parser guarantees the vector is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedForBinding {
    pub names: Vec<String>,
}

impl CheckedForBinding {
    pub(super) fn lower(binding: &syntax::ForBinding) -> Self {
        Self {
            names: binding.names.iter().map(|n| n.name.clone()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedMatchArm {
    pub path: Vec<String>,
    pub member_id: Option<EnumMemberId>,
    pub member_uses: Vec<(EnumMemberId, SourceSpan)>,
    pub block: CheckedBody,
    pub span: SourceSpan,
}

impl CheckedMatchArm {
    pub(super) fn lower(
        arm: &syntax::MatchArm,
        match_enum: Option<(&str, &str)>,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        let member_ref = match_enum.and_then(|(module, name)| {
            checked_enum_member_ref_in(context.program, module, name, &arm.path, &arm.path_spans)
        });
        Some(Self {
            path: arm.path.clone(),
            member_id: member_ref.as_ref().map(|member| member.member_id),
            member_uses: member_ref
                .map(|member| member.member_uses)
                .unwrap_or_default(),
            block: CheckedBody::lower_scoped(&arm.block, context, scope)?,
            span: arm.span,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedElseIf {
    pub condition: Option<CheckedExpr>,
    pub block: CheckedBody,
}

impl CheckedElseIf {
    pub(super) fn lower(
        else_if: &syntax::ElseIf,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Some(Self {
            condition: lower_optional_expr(Some(&else_if.condition), context, scope)?,
            block: CheckedBody::lower_scoped(&else_if.block, context, scope)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedCatchClause {
    pub name: String,
    pub block: CheckedBody,
}

impl CheckedCatchClause {
    pub(super) fn lower(
        catch: &syntax::CatchClause,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        scope.push(catch_frame(catch));
        let block = CheckedBody::lower_scoped(&catch.block, context, scope);
        scope.pop();
        Some(Self {
            name: catch.name.clone(),
            block: block?,
        })
    }
}
