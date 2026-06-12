use std::collections::HashMap;

use marrow_syntax::{self as syntax, SourceSpan};

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
        scope: &mut Vec<HashMap<String, MarrowType>>,
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
    pub mode: Option<CheckedArgMode>,
    pub name: Option<String>,
    pub value: CheckedExpr,
}

impl CheckedArg {
    pub(super) fn lower(
        arg: &syntax::Argument,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Some(Self {
            mode: arg.mode.map(CheckedArgMode::lower),
            name: arg.name.clone(),
            value: CheckedExpr::lower(&arg.value, context, scope)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedArgMode {
    InOut,
}

impl CheckedArgMode {
    fn lower(mode: syntax::ArgMode) -> Self {
        match mode {
            syntax::ArgMode::InOut => Self::InOut,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedParamMode {
    InOut,
}

impl CheckedParamMode {
    pub(crate) fn lower(mode: syntax::ParamMode) -> Self {
        match mode {
            syntax::ParamMode::InOut => Self::InOut,
        }
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
    pub(super) fn lower(kind: syntax::LiteralKind) -> Self {
        match kind {
            syntax::LiteralKind::Integer => Self::Integer,
            syntax::LiteralKind::Decimal => Self::Decimal,
            syntax::LiteralKind::Duration => Self::Duration,
            syntax::LiteralKind::String => Self::String,
            syntax::LiteralKind::Bytes => Self::Bytes,
            syntax::LiteralKind::Bool => Self::Bool,
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedForBinding {
    pub first: String,
    pub second: Option<String>,
}

impl CheckedForBinding {
    pub(super) fn lower(binding: &syntax::ForBinding) -> Self {
        Self {
            first: binding.first.clone(),
            second: binding.second.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedMatchArm {
    pub path: Vec<String>,
    pub member_id: Option<EnumMemberId>,
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
        Some(Self {
            path: arm.path.clone(),
            member_id: match_enum
                .and_then(|(module, name)| {
                    checked_enum_member_ref_in(context.program, module, name, &arm.path)
                })
                .map(|member| member.member_id),
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
            condition: lower_optional_expr(else_if.condition.as_ref(), context, scope)?,
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
        scope: &mut [HashMap<String, MarrowType>],
    ) -> Option<Self> {
        let mut catch_scope = scope.to_owned();
        catch_scope.push(HashMap::from([(catch.name.clone(), MarrowType::Error)]));
        Some(Self {
            name: catch.name.clone(),
            block: CheckedBody::lower_scoped(&catch.block, context, &mut catch_scope)?,
        })
    }
}
